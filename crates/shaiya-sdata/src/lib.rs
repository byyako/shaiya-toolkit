//! SData tables as rows and columns, read-only. The tables (Item, Skill, Monster, NpcQuest) are described by layout files
//! (`layouts/*.json`, one per format variant) and read by one engine; a layout applies to a file only when it consumes the whole
//! plaintext to the last byte with plausible counts. [`Hint::Auto`] tries every layout and takes the only one that fits; no fit, or
//! more than one, is [`NoFit`] and the caller shows the file raw.
//!
//! Layout facts were derived from the loaders of the clients listed as verified in `docs/version-matrix.md` and checked on real
//! files; which clients each variant was checked on is in the same file. Field names are `f<offset>` unless the meaning is known.
//! Every layout file says, in its `evidence` object, which clients it was read from the loader of and which it only fits; a layout
//! with no loader client is FIT-ONLY (positional names, [`FIT_ONLY_NOTE`]).

mod csv;
mod engine;
pub mod layout;

pub use csv::{csv, csv_rows, csv_text};
pub use layout::{ColKind, Column, Evidence, Layout, Status, TableKind, FIT_ONLY_NOTE};

/// What every table view says about how its layout was chosen: by fit, not by the client's own loader, except on the verified clients.
pub const FIT_NOTE: &str = "chosen because it reads the whole file. It was checked against the game's own loader only on the clients listed as verified in the version matrix.";

use shaiya_core::text::Encoding;
use shaiya_core::Error;
use std::fmt;
use std::sync::OnceLock;

/// The layout files, in the order `auto` tries them. Each is embedded in the program.
const LAYOUT_FILES: [&str; 11] = [
    include_str!("../layouts/item-81.json"),
    include_str!("../layouts/item-132.json"),
    include_str!("../layouts/item-134.json"),
    include_str!("../layouts/skill-3.json"),
    include_str!("../layouts/skill-9.json"),
    include_str!("../layouts/skill-15.json"),
    include_str!("../layouts/monster-31.json"),
    include_str!("../layouts/monster-36.json"),
    include_str!("../layouts/npcquest-165.json"),
    include_str!("../layouts/npcquest-87.json"),
    include_str!("../layouts/npcquest-287.json"),
];

/// Every layout (parsed once). The embedded files are tested to parse; a broken one is a bug that stops the program at first use.
pub fn layouts() -> &'static [Layout] {
    static ALL: OnceLock<Vec<Layout>> = OnceLock::new();
    ALL.get_or_init(|| LAYOUT_FILES.iter().map(|t| layout::parse_layout(t).unwrap_or_else(|e| panic!("embedded layout file is broken: {e}"))).collect())
}

/// Parses a layout file that is not embedded (tests, and a future way to add a layout without a build).
pub fn parse_layout(text: &str) -> Result<Layout, String> {
    layout::parse_layout(text)
}

/// One cell.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f32),
    Text(String),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::Text(s) => f.write_str(s),
        }
    }
}

/// One table of a file (most files have one; NpcQuest has five).
#[derive(Clone, Debug)]
pub struct Sheet {
    pub name: String,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
}

#[derive(Clone, Debug)]
pub struct Table {
    pub layout: &'static Layout,
    pub sheets: Vec<Sheet>,
}

impl Table {
    /// The sheet shown first: the layout's primary section.
    pub fn primary(&self) -> &Sheet {
        &self.sheets[self.layout.primary()]
    }

    pub fn sheet(&self, name: &str) -> Option<&Sheet> {
        self.sheets.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// The whole table as one JSON document: layout, table, and per sheet the columns, the row count and the rows.
    pub fn to_json(&self, only: Option<&Sheet>) -> serde_json::Value {
        use serde_json::{json, Value as J};
        let cell = |v: &Value| match v {
            Value::Int(i) => json!(i),
            Value::Float(x) => format!("{x}").parse::<f64>().ok().and_then(serde_json::Number::from_f64).map_or(J::Null, J::Number),
            Value::Text(s) => json!(s),
        };
        let sheets: Vec<J> = self
            .sheets
            .iter()
            .filter(|s| only.is_none_or(|o| std::ptr::eq(o, *s)))
            .map(|s| {
                json!({
                    "name": s.name,
                    "columns": s.columns.iter().map(|c| json!({"name": c.name, "kind": c.kind.label()})).collect::<Vec<_>>(),
                    "row_count": s.rows.len(),
                    "rows": s.rows.iter().map(|r| r.iter().map(cell).collect::<Vec<_>>()).collect::<Vec<_>>(),
                })
            })
            .collect();
        let mut doc = json!({"layout": self.layout.variant, "table": self.layout.table.name(), "chosen_by": "fit"});
        if let J::Object(m) = &mut doc {
            m.extend(self.layout.provenance_json());
            m.insert("sheets".into(), J::Array(sheets));
        }
        doc
    }
}

/// Which layouts to try.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hint {
    Auto,
    Table(TableKind),
}

impl Hint {
    /// `auto`, `item`, `skill`, `monster`, `npcquest`.
    pub fn from_name(s: &str) -> Option<Hint> {
        if s.eq_ignore_ascii_case("auto") {
            Some(Hint::Auto)
        } else {
            TableKind::from_name(s).map(Hint::Table)
        }
    }
}

/// Why no table could be made from a file's plaintext.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoFit {
    /// The plaintext size.
    pub len: usize,
    /// For each layout that was tried and did not fit: the variant, the byte where it stopped, and why.
    pub tried: Vec<(String, usize, String)>,
    /// More than one layout read the whole file: their variants (a table is not shown then).
    pub ambiguous: Vec<String>,
}

impl fmt::Display for NoFit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "No table layout fits this file ({} bytes)", self.len)?;
        if !self.ambiguous.is_empty() {
            write!(f, ": more than one layout reads it to the end ({})", self.ambiguous.join(", "))?;
        }
        Ok(())
    }
}

impl std::error::Error for NoFit {}

/// Reads the plaintext of an SData file as a table. The layouts of `hint` are tried; the one that fits is used. `enc` decodes the
/// names and descriptions (code page 949 is what the original client uses).
pub fn read(plain: &[u8], hint: Hint, enc: Encoding) -> Result<Table, NoFit> {
    let mut tried = Vec::new();
    let mut fits: Vec<&'static Layout> = Vec::new();
    for l in layouts().iter().filter(|l| match hint {
        Hint::Auto => true,
        Hint::Table(k) => l.table == k,
    }) {
        match engine::run(l, plain, enc, false) {
            Ok(_) => fits.push(l),
            Err(s) => tried.push((l.variant.clone(), s.offset, s.reason)),
        }
    }
    match fits.as_slice() {
        [one] => match engine::run(one, plain, enc, true) {
            Ok(rows) => Ok(Table { layout: one, sheets: one.sections.iter().zip(rows).map(|(s, rows)| Sheet { name: s.name.clone(), columns: s.columns.clone(), rows }).collect() }),
            Err(s) => {
                // the fit check and the build walk the same bytes; this cannot differ, but never panic over it
                tried.push((one.variant.clone(), s.offset, s.reason));
                Err(NoFit { len: plain.len(), tried, ambiguous: Vec::new() })
            }
        },
        _ => Err(NoFit { len: plain.len(), tried, ambiguous: fits.iter().map(|l| l.variant.clone()).collect() }),
    }
}

/// Why [`read_file`] gave nothing.
#[derive(Debug)]
pub enum ReadError {
    /// The SEED wrapper is damaged (checksum or size).
    Unreadable(Error),
    NoFit(NoFit),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::Unreadable(e) => write!(f, "{e}"),
            ReadError::NoFit(n) => write!(f, "{n}"),
        }
    }
}

impl std::error::Error for ReadError {}

impl ReadError {
    /// As the toolkit's error type, naming the file.
    pub fn into_error(self, name: &str) -> Error {
        match self {
            ReadError::Unreadable(e) => e,
            ReadError::NoFit(n) => Error::format(name, None, n.to_string()),
        }
    }
}

/// [`read`] over a file as stored: SEED-wrapped files are unwrapped first (the checksum must pass), plain files are used as they are.
pub fn read_file(raw: &[u8], name: &str, hint: Hint, enc: Encoding) -> Result<Table, ReadError> {
    let (plain, _) = shaiya_seed::unwrap(raw, name).map_err(ReadError::Unreadable)?;
    read(&plain, hint, enc).map_err(ReadError::NoFit)
}

/// `cp949`, `cp1252`, `cp1251`, `cp936`, `utf-8`, `utf-16le` (any case).
pub fn encoding_by_name(name: &str) -> Option<Encoding> {
    Some(match name.to_ascii_lowercase().as_str() {
        "cp949" => Encoding::Cp949,
        "cp1252" => Encoding::Cp1252,
        "cp1251" => Encoding::Cp1251,
        "cp936" => Encoding::Cp936,
        "utf-8" | "utf8" => Encoding::Utf8,
        "utf-16le" | "utf16le" => Encoding::Utf16Le,
        _ => return None,
    })
}

pub fn encoding_name(e: Encoding) -> &'static str {
    match e {
        Encoding::Cp949 => "cp949",
        Encoding::Cp1252 => "cp1252",
        Encoding::Cp1251 => "cp1251",
        Encoding::Cp936 => "cp936",
        Encoding::Utf8 => "utf-8",
        Encoding::Utf16Le => "utf-16le",
    }
}

/// The code pages the viewer offers.
pub const ENCODINGS: [Encoding; 6] = [Encoding::Cp949, Encoding::Cp1252, Encoding::Cp1251, Encoding::Cp936, Encoding::Utf8, Encoding::Utf16Le];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_layout_parses_and_names_are_unique() {
        let all = layouts();
        assert_eq!(all.len(), LAYOUT_FILES.len());
        let mut names: Vec<&str> = all.iter().map(|l| l.variant.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.len(), "variant names are unique");
        for t in TableKind::ALL {
            assert!(all.iter().any(|l| l.table == t), "{t:?} has a layout");
        }
    }
}
