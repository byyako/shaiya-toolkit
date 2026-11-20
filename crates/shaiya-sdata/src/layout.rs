//! Layout files: what a table looks like, as data. One engine ([`crate::engine`]) reads every layout; adding a table or an episode
//! is adding a file under `layouts/` and listing it in `LAYOUT_FILES` (lib.rs).
//!
//! A layout file is JSON:
//!
//! ```text
//! { "format": 1, "table": "Item", "variant": "item-81", "file": "Item/Item.SData",
//!   "evidence": { "loader": ["us-108"], "fit_only": ["cn-ep4"] },
//!   "sections": [ { "name": "items", "primary": true,
//!       "loops":  [ { "index": "bank", "base": 0, "count": "u32", "max": 255 }, { "count": "u32" } ],
//!       "record": [ { "kind": "lenstr", "name": "name" },
//!                   { "kind": "fixed", "size": 81, "fields": [ { "offset": 0, "name": "type", "kind": "u8" } ] } ] } ] }
//! ```
//!
//! `evidence` is required: `loader` lists the clients whose own table loader the layout was read from (and checked on that client's
//! files), `fit_only` the clients whose files it reads to the last byte without any loader read. A layout with an empty `loader` is
//! FIT-ONLY ([`Layout::status`]). Client ids are the column names of `docs/version-matrix.md`.
//!
//! A section is read as nested loops (each loop's count is a `u32` in the file or a constant) and one record per innermost
//! iteration; a record is a list of nodes: `u8 i8 u16 i16 u32 i32 f32` (with an optional `count`), `lenstr` (a `u32` byte length and
//! that many bytes of text), `fixed` (a block of known size with named fields at their offsets), `list` (a `u32` count and that many
//! elements) and `repeat` (items read `count` times, their columns numbered).

use serde_json::Value as J;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    F32,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "u8" => Kind::U8,
            "i8" => Kind::I8,
            "u16" => Kind::U16,
            "i16" => Kind::I16,
            "u32" => Kind::U32,
            "i32" => Kind::I32,
            "f32" => Kind::F32,
            _ => return None,
        })
    }

    pub fn size(self) -> usize {
        match self {
            Kind::U8 | Kind::I8 => 1,
            Kind::U16 | Kind::I16 => 2,
            Kind::U32 | Kind::I32 | Kind::F32 => 4,
        }
    }
}

/// The tables the viewer knows; `--layout` and [`crate::Hint`] name them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableKind {
    Item,
    Skill,
    Monster,
    NpcQuest,
}

impl TableKind {
    pub const ALL: [TableKind; 4] = [TableKind::Item, TableKind::Skill, TableKind::Monster, TableKind::NpcQuest];

    pub fn name(self) -> &'static str {
        match self {
            TableKind::Item => "Item",
            TableKind::Skill => "Skill",
            TableKind::Monster => "Monster",
            TableKind::NpcQuest => "NpcQuest",
        }
    }

    /// Case-insensitive; `npcquest` and `npc-quest` both work.
    pub fn from_name(s: &str) -> Option<TableKind> {
        let t = s.to_ascii_lowercase().replace(['-', '_'], "");
        TableKind::ALL.into_iter().find(|k| k.name().to_ascii_lowercase() == t)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Scalar { name: String, kind: Kind, count: u32 },
    Str { name: String },
    Fixed { size: usize, fields: Vec<Field> },
    List { name: String, elem: Vec<Field> },
    Repeat { name: String, count: u32, items: Vec<Node> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Count {
    Fixed(u32),
    /// A `u32` in the file.
    Read,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loop {
    /// A column holding the iteration number (`base` for the first).
    pub index: Option<(String, i64)>,
    pub count: Count,
    /// A count above this is not plausible (the loader keeps only that many bits).
    pub max: Option<u64>,
}

/// The whole section is read `count` times; a column holds `base + n`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repeat {
    pub count: u32,
    pub column: String,
    pub base: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColKind {
    Int,
    Float,
    Text,
}

impl ColKind {
    pub fn label(self) -> &'static str {
        match self {
            ColKind::Int => "int",
            ColKind::Float => "float",
            ColKind::Text => "text",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub kind: ColKind,
}

#[derive(Clone, Debug)]
pub struct Section {
    pub name: String,
    pub primary: bool,
    pub repeat: Option<Repeat>,
    pub loops: Vec<Loop>,
    pub record: Vec<Node>,
    pub columns: Vec<Column>,
    /// `min_bytes[d]`: the fewest bytes one iteration of loop `d - 1` takes (`min_bytes[loops.len()]` is one record).
    pub min_bytes: Vec<u64>,
}

/// What a table view says about a layout that no client's own loader was read for.
pub const FIT_ONLY_NOTE: &str = "FIT-ONLY: this layout reads the whole file, but it was not read from any client's own loader. Columns are named by position; values may be split wrongly.";

/// Where a layout's facts come from (the layout file's `evidence` object). Client ids are the version matrix's column names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evidence {
    /// Clients whose own table loader the layout was read from and checked against.
    pub loader: Vec<String>,
    /// Clients whose file the layout reads to the last byte, with no loader read.
    pub fit_only: Vec<String>,
}

/// Whether a layout was read from any client's own loader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    LoaderVerified,
    FitOnly,
}

impl Status {
    /// The word used in JSON: `loader-verified` or `fit-only`.
    pub fn label(self) -> &'static str {
        match self {
            Status::LoaderVerified => "loader-verified",
            Status::FitOnly => "fit-only",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub variant: String,
    pub table: TableKind,
    /// The usual archive path of the file (a hint for people, not used to pick a layout).
    pub file: String,
    pub evidence: Evidence,
    pub sections: Vec<Section>,
}

impl Layout {
    /// FIT-ONLY when no client's loader was read for this layout.
    pub fn status(&self) -> Status {
        if self.evidence.loader.is_empty() {
            Status::FitOnly
        } else {
            Status::LoaderVerified
        }
    }

    /// The sentence the viewers show about where this layout comes from. FIT-ONLY: [`FIT_ONLY_NOTE`]. Loader-verified: "Loader-verified
    /// on: <clients>." and, when the layout also fits clients without a loader read, which ones.
    pub fn provenance(&self) -> String {
        match self.status() {
            Status::FitOnly => FIT_ONLY_NOTE.to_string(),
            Status::LoaderVerified => {
                let mut s = format!("Loader-verified on: {}.", self.evidence.loader.join(", "));
                if !self.evidence.fit_only.is_empty() {
                    s.push_str(&format!(" On {} it only fits (FIT-ONLY: no loader read).", self.evidence.fit_only.join(", ")));
                }
                s
            }
        }
    }

    /// The additive JSON keys: `status`, `loader_verified_on`, `fit_only_on`.
    pub fn provenance_json(&self) -> serde_json::Map<String, J> {
        let mut m = serde_json::Map::new();
        m.insert("status".into(), J::String(self.status().label().into()));
        m.insert("loader_verified_on".into(), self.evidence.loader.iter().cloned().map(J::String).collect());
        m.insert("fit_only_on".into(), self.evidence.fit_only.iter().cloned().map(J::String).collect());
        m
    }

    pub fn primary(&self) -> usize {
        self.sections.iter().position(|s| s.primary).unwrap_or(0)
    }
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else if name.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}_{name}")
    }
}

fn col_kind(k: Kind) -> ColKind {
    if k == Kind::F32 {
        ColKind::Float
    } else {
        ColKind::Int
    }
}

impl Node {
    pub fn push_columns(&self, prefix: &str, out: &mut Vec<Column>) {
        match self {
            Node::Scalar { name, kind, count } => out.push(Column { name: join(prefix, name), kind: if *count > 1 { ColKind::Text } else { col_kind(*kind) } }),
            Node::Str { name } | Node::List { name, .. } => out.push(Column { name: join(prefix, name), kind: ColKind::Text }),
            Node::Fixed { fields, .. } => {
                for f in fields {
                    out.push(Column { name: join(prefix, &f.name), kind: col_kind(f.kind) });
                }
            }
            Node::Repeat { name, count, items } => {
                for i in 1..=*count {
                    let p = join(prefix, &format!("{name}{i}"));
                    for it in items {
                        it.push_columns(&p, out);
                    }
                }
            }
        }
    }

    /// The fewest bytes the node takes.
    pub fn min_bytes(&self) -> u64 {
        match self {
            Node::Scalar { kind, count, .. } => kind.size() as u64 * u64::from(*count),
            Node::Str { .. } | Node::List { .. } => 4,
            Node::Fixed { size, .. } => *size as u64,
            Node::Repeat { count, items, .. } => u64::from(*count) * items.iter().map(Node::min_bytes).sum::<u64>(),
        }
    }
}

fn clients_of(ev: &J, key: &str, variant: &str) -> Result<Vec<String>, String> {
    let arr = ev.get(key).and_then(J::as_array).ok_or_else(|| bad(variant, format!("evidence has no \"{key}\" list")))?;
    let mut out: Vec<String> = Vec::new();
    for c in arr {
        let s = c.as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| bad(variant, format!("evidence \"{key}\" holds something that is not a client id")))?;
        if out.iter().any(|o| o == s) {
            return Err(bad(variant, format!("evidence lists {s} twice")));
        }
        out.push(s.to_string());
    }
    Ok(out)
}

fn evidence_of(v: &J, variant: &str) -> Result<Evidence, String> {
    let ev = v.get("evidence").filter(|e| e.is_object()).ok_or_else(|| bad(variant, "\"evidence\" is missing (it names the clients the layout was read from or only fitted)"))?;
    let (loader, fit_only) = (clients_of(ev, "loader", variant)?, clients_of(ev, "fit_only", variant)?);
    if let Some(c) = loader.iter().find(|c| fit_only.contains(c)) {
        return Err(bad(variant, format!("evidence lists {c} as both loader and fit_only")));
    }
    if loader.is_empty() && fit_only.is_empty() {
        return Err(bad(variant, "evidence lists no client"));
    }
    Ok(Evidence { loader, fit_only })
}

fn bad(variant: &str, what: impl std::fmt::Display) -> String {
    format!("layout {variant}: {what}")
}

fn str_of<'a>(v: &'a J, key: &str, variant: &str) -> Result<&'a str, String> {
    v.get(key).and_then(J::as_str).ok_or_else(|| bad(variant, format!("\"{key}\" is missing")))
}

fn kind_of(v: &J, variant: &str) -> Result<Kind, String> {
    let s = str_of(v, "kind", variant)?;
    Kind::parse(s).ok_or_else(|| bad(variant, format!("unknown kind {s:?}")))
}

type OffsetField = (Option<u64>, Field);

fn fields_of(arr: &J, variant: &str) -> Result<Vec<OffsetField>, String> {
    let a = arr.as_array().ok_or_else(|| bad(variant, "\"fields\" is not a list"))?;
    a.iter().map(|f| Ok((f.get("offset").and_then(J::as_u64), Field { name: str_of(f, "name", variant)?.to_string(), kind: kind_of(f, variant)? }))).collect()
}

fn node_of(v: &J, variant: &str) -> Result<Node, String> {
    let kind = str_of(v, "kind", variant)?;
    let name = v.get("name").and_then(J::as_str).unwrap_or("").to_string();
    Ok(match kind {
        "lenstr" => Node::Str { name },
        "fixed" => {
            let size = v.get("size").and_then(J::as_u64).ok_or_else(|| bad(variant, "a fixed block has no size"))? as usize;
            let fields = fields_of(v.get("fields").unwrap_or(&J::Null), variant)?;
            let mut pos = 0usize;
            for (off, f) in &fields {
                if off.is_some_and(|o| o as usize != pos) {
                    return Err(bad(variant, format!("field {} is at offset {:?}, the fields before it end at {pos}", f.name, off)));
                }
                pos += f.kind.size();
            }
            if pos != size {
                return Err(bad(variant, format!("the fields of a {size}-byte block add up to {pos} bytes")));
            }
            Node::Fixed { size, fields: fields.into_iter().map(|f| f.1).collect() }
        }
        "list" => {
            let elem = fields_of(v.get("elem").unwrap_or(&J::Null), variant)?.into_iter().map(|f| f.1).collect::<Vec<_>>();
            if elem.is_empty() {
                return Err(bad(variant, "a list has no element fields"));
            }
            Node::List { name, elem }
        }
        "repeat" => {
            let count = v.get("count").and_then(J::as_u64).ok_or_else(|| bad(variant, "a repeat has no count"))? as u32;
            let items = v.get("items").and_then(J::as_array).ok_or_else(|| bad(variant, "a repeat has no items"))?.iter().map(|i| node_of(i, variant)).collect::<Result<Vec<_>, _>>()?;
            Node::Repeat { name, count, items }
        }
        other => match Kind::parse(other) {
            Some(k) => Node::Scalar { name, kind: k, count: v.get("count").and_then(J::as_u64).unwrap_or(1).max(1) as u32 },
            None => return Err(bad(variant, format!("unknown node kind {other:?}"))),
        },
    })
}

fn loop_of(v: &J, variant: &str) -> Result<Loop, String> {
    let count = match v.get("count") {
        Some(J::String(s)) if s == "u32" => Count::Read,
        Some(J::Number(n)) => Count::Fixed(n.as_u64().ok_or_else(|| bad(variant, "a loop count is not a whole number"))? as u32),
        _ => return Err(bad(variant, "a loop count is neither \"u32\" nor a number")),
    };
    let index = v.get("index").and_then(J::as_str).map(|n| (n.to_string(), v.get("base").and_then(J::as_i64).unwrap_or(0)));
    Ok(Loop { index, count, max: v.get("max").and_then(J::as_u64) })
}

fn section_of(v: &J, variant: &str) -> Result<Section, String> {
    let name = str_of(v, "name", variant)?.to_string();
    let loops = v.get("loops").and_then(J::as_array).ok_or_else(|| bad(variant, "a section has no loops"))?.iter().map(|l| loop_of(l, variant)).collect::<Result<Vec<_>, _>>()?;
    let record = v.get("record").and_then(J::as_array).ok_or_else(|| bad(variant, "a section has no record"))?.iter().map(|n| node_of(n, variant)).collect::<Result<Vec<_>, _>>()?;
    let repeat = match v.get("repeat") {
        Some(r) => Some(Repeat {
            count: r.get("count").and_then(J::as_u64).ok_or_else(|| bad(variant, "a section repeat has no count"))? as u32,
            column: str_of(r, "column", variant)?.to_string(),
            base: r.get("base").and_then(J::as_i64).unwrap_or(0),
        }),
        None => None,
    };
    let mut columns = Vec::new();
    if let Some(r) = &repeat {
        columns.push(Column { name: r.column.clone(), kind: ColKind::Int });
    }
    for l in &loops {
        if let Some((n, _)) = &l.index {
            columns.push(Column { name: n.clone(), kind: ColKind::Int });
        }
    }
    for n in &record {
        n.push_columns("", &mut columns);
    }
    let mut seen = HashSet::new();
    for c in &columns {
        if !seen.insert(c.name.as_str()) {
            return Err(bad(variant, format!("section {name}: the column name {:?} is used twice", c.name)));
        }
    }
    let mut min_bytes = vec![0u64; loops.len() + 1];
    min_bytes[loops.len()] = record.iter().map(Node::min_bytes).sum();
    for d in (0..loops.len()).rev() {
        min_bytes[d] = match loops[d].count {
            Count::Fixed(n) => u64::from(n).saturating_mul(min_bytes[d + 1]),
            Count::Read => 4,
        };
    }
    Ok(Section { name, primary: v.get("primary").and_then(J::as_bool).unwrap_or(false), repeat, loops, record, columns, min_bytes })
}

/// Parses one layout file.
pub fn parse_layout(text: &str) -> Result<Layout, String> {
    let v: J = serde_json::from_str(text).map_err(|e| format!("layout file: {e}"))?;
    let variant = str_of(&v, "variant", "?")?.to_string();
    if v.get("format").and_then(J::as_u64) != Some(1) {
        return Err(bad(&variant, "format must be 1"));
    }
    let table_name = str_of(&v, "table", &variant)?;
    let table = TableKind::from_name(table_name).ok_or_else(|| bad(&variant, format!("unknown table {table_name:?}")))?;
    let sections = v.get("sections").and_then(J::as_array).ok_or_else(|| bad(&variant, "no sections"))?.iter().map(|s| section_of(s, &variant)).collect::<Result<Vec<_>, _>>()?;
    if sections.is_empty() {
        return Err(bad(&variant, "no sections"));
    }
    let evidence = evidence_of(&v, &variant)?;
    Ok(Layout { file: str_of(&v, "file", &variant)?.to_string(), variant, table, evidence, sections })
}
