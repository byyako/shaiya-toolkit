//! The JSON form of the animation and model formats: [`export`] turns a model file into a documented, versioned JSON text and [`import`]
//! turns that text back into the file's bytes. An unedited file comes back byte for byte; export checks that itself (the self-check) before
//! it returns anything. Everything is built in memory: nothing here writes a file.
//!
//! Schema `v1` of each format is documented on its format page; the layout of the text is in [`fmt`]; floats are JSON numbers in their
//! shortest form when that reads back to the same bits, otherwise `"bits:XXXXXXXX"` (see [`types`]). Byte strings and bit-exact matrices
//! are hex in file order. Unknown keys are refused. This module covers `.ANI`; the other formats follow in the same shape.

mod ani;
mod fmt;
mod types;

use crate::error::Error;
use serde::Deserialize;
use std::fmt as stdfmt;
use std::io;

/// The six formats that have a JSON form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFormat {
    Ani,
    C3dc,
    Vani,
    Mani,
    Mesh3do,
    Mesh3de,
}

impl ModelFormat {
    pub const ALL: [ModelFormat; 6] = [ModelFormat::Ani, ModelFormat::C3dc, ModelFormat::Vani, ModelFormat::Mani, ModelFormat::Mesh3do, ModelFormat::Mesh3de];

    /// The format a file name belongs to by its last extension, in any letter case: `ani`, `3dc`, `vani`, `mani`, `3do`, `3de`.
    pub fn from_file_name(name: &str) -> Option<ModelFormat> {
        let (_, ext) = name.rsplit_once('.')?;
        ModelFormat::ALL.into_iter().find(|f| f.extension().eq_ignore_ascii_case(ext))
    }

    /// The extension without the dot, lower case.
    pub fn extension(self) -> &'static str {
        match self {
            ModelFormat::Ani => "ani",
            ModelFormat::C3dc => "3dc",
            ModelFormat::Vani => "vani",
            ModelFormat::Mani => "mani",
            ModelFormat::Mesh3do => "3do",
            ModelFormat::Mesh3de => "3de",
        }
    }

    /// The extension as people write it: `.ANI`, `.3DC`, `.VANI`, `.MANI`, `.3DO`, `.3DE`.
    pub fn label(self) -> &'static str {
        match self {
            ModelFormat::Ani => ".ANI",
            ModelFormat::C3dc => ".3DC",
            ModelFormat::Vani => ".VANI",
            ModelFormat::Mani => ".MANI",
            ModelFormat::Mesh3do => ".3DO",
            ModelFormat::Mesh3de => ".3DE",
        }
    }

    /// The `format` value of the version 1 schema: `shaiya-toolkit-ani-v1` and so on.
    pub fn schema(self) -> &'static str {
        match self {
            ModelFormat::Ani => "shaiya-toolkit-ani-v1",
            ModelFormat::C3dc => "shaiya-toolkit-3dc-v1",
            ModelFormat::Vani => "shaiya-toolkit-vani-v1",
            ModelFormat::Mani => "shaiya-toolkit-mani-v1",
            ModelFormat::Mesh3do => "shaiya-toolkit-3do-v1",
            ModelFormat::Mesh3de => "shaiya-toolkit-3de-v1",
        }
    }
}

/// A model file over this size is not read.
pub const MAX_MODEL_BYTES: u64 = 128 << 20;
/// A JSON over this size is not read, and an export that would be longer is refused.
pub const MAX_JSON_BYTES: u64 = 256 << 20;

/// Why a model could not be turned into JSON or back.
#[derive(Debug)]
pub enum JsonError {
    /// The model file does not read (the reader's message).
    Unreadable(Error),
    /// A key value that must be finite is not, or a number is too large for a 32-bit float (`at` names the place).
    NotFinite { at: String },
    /// Malformed JSON, a wrong type, a missing or unknown key.
    Json { line: usize, column: usize, message: String },
    /// An unknown or newer format, other columns, a bone out of order, counts that do not fit, layout and frame block that disagree.
    Schema(String),
    /// A size cap (`MAX_MODEL_BYTES`, `MAX_JSON_BYTES`) or a count over its limit.
    TooLarge(String),
    /// The bytes built from the JSON were refused by the writer (they would not read back).
    NotWritable(String),
    /// Export's own read-back differs from the file: a bug in the toolkit.
    SelfCheck(String),
}

impl stdfmt::Display for JsonError {
    fn fmt(&self, f: &mut stdfmt::Formatter<'_>) -> stdfmt::Result {
        match self {
            JsonError::Unreadable(e) => write!(f, "{e}"),
            JsonError::NotFinite { at } => write!(f, "{at}: a key value must be a finite number that fits a 32-bit float (NaN and infinity are refused)"),
            JsonError::Json { line, column, message } => write!(f, "line {line}, column {column}: {message}"),
            JsonError::Schema(s) | JsonError::TooLarge(s) | JsonError::NotWritable(s) => f.write_str(s),
            JsonError::SelfCheck(s) => write!(f, "the export did not read back to the same bytes ({s})"),
        }
    }
}

impl std::error::Error for JsonError {}

impl JsonError {
    /// True for the one error that is a bug of the toolkit and not a problem with the input.
    pub fn is_bug(&self) -> bool {
        matches!(self, JsonError::SelfCheck(_))
    }
}

impl From<Error> for JsonError {
    fn from(e: Error) -> Self {
        JsonError::Unreadable(e)
    }
}

/// Turns a `serde_json` error into a [`JsonError`]; a number that does not fit a 32-bit float becomes `NotFinite`.
pub(crate) fn json_error(e: serde_json::Error) -> JsonError {
    let (line, column) = (e.line(), e.column());
    let mut message = e.to_string();
    if let Some(at) = message.rfind(" at line ") {
        message.truncate(at);
    }
    if message.contains(types::OUT_OF_RANGE) {
        return JsonError::NotFinite { at: format!("line {line}, column {column}") };
    }
    JsonError::Json { line, column, message }
}

/// `Ok` when a model of `len` bytes may be read under `cap`.
pub(crate) fn check_model_len(len: u64, cap: u64) -> Result<(), JsonError> {
    if len > cap {
        return Err(JsonError::TooLarge(format!("{len} bytes is over the {cap}-byte limit for a model file")));
    }
    Ok(())
}

/// `Ok` when a JSON of `len` bytes may be read under `cap`.
pub(crate) fn check_json_len(len: u64, cap: u64) -> Result<(), JsonError> {
    if len > cap {
        return Err(JsonError::TooLarge(format!("{len} bytes is over the {cap}-byte limit for a model JSON")));
    }
    Ok(())
}

/// A `Vec<u8>` that refuses to grow past `cap`: the serializer stops there, so no text over the cap is ever held in memory.
pub(crate) struct Capped {
    pub buf: Vec<u8>,
    cap: usize,
}

impl Capped {
    pub fn new(cap: u64) -> Self {
        Capped { buf: Vec::new(), cap: usize::try_from(cap).unwrap_or(usize::MAX) }
    }
}

impl io::Write for Capped {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.buf.len().saturating_add(data.len()) > self.cap {
            return Err(io::Error::new(io::ErrorKind::OutOfMemory, "over the JSON size cap"));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serializes `value` with the model layout into at most `cap` bytes, ending with a newline.
pub(crate) fn render<T: serde::Serialize>(value: &T, cap: u64) -> Result<String, JsonError> {
    let mut out = Capped::new(cap.saturating_sub(1));
    let mut ser = serde_json::Serializer::with_formatter(&mut out, fmt::LineFormatter::new());
    if let Err(e) = serde::Serialize::serialize(value, &mut ser) {
        if e.is_io() {
            return Err(JsonError::TooLarge(format!("the JSON would be over the {cap}-byte limit")));
        }
        return Err(json_error(e));
    }
    out.buf.push(b'\n');
    String::from_utf8(out.buf).map_err(|_| JsonError::SelfCheck("the JSON text is not UTF-8".into()))
}

#[derive(Deserialize)]
struct FormatOnly {
    format: String,
}

/// Only reads the `format` key (every other key is skipped without being kept): for a caller that wants to compare the format with a file name before the full import.
pub fn format_of(json: &[u8]) -> Result<ModelFormat, JsonError> {
    check_json_len(json.len() as u64, MAX_JSON_BYTES)?;
    format_of_unchecked(json)
}

fn format_of_unchecked(json: &[u8]) -> Result<ModelFormat, JsonError> {
    let probe: FormatOnly = serde_json::from_slice(json).map_err(json_error)?;
    let name = probe.format;
    if let Some(f) = ModelFormat::ALL.into_iter().find(|f| f.schema() == name) {
        return Ok(f);
    }
    for f in ModelFormat::ALL {
        let stem = f.schema().trim_end_matches("v1");
        if let Some(version) = name.strip_prefix(stem).filter(|v| v.starts_with('v') && v.len() > 1) {
            return Err(JsonError::Schema(format!("made by a newer toolkit ({version}); this toolkit reads v1")));
        }
    }
    Err(JsonError::Schema(format!("not a toolkit model JSON (format \"{name}\")")))
}

/// The JSON text of a model file. `source` is the archive path or the file name (never a folder path). Runs the self-check: importing the
/// text must give back `(format, bytes)` exactly.
pub fn export(format: ModelFormat, bytes: &[u8], source: &str) -> Result<String, JsonError> {
    export_capped(format, bytes, source, MAX_MODEL_BYTES, MAX_JSON_BYTES)
}

pub(crate) fn export_capped(format: ModelFormat, bytes: &[u8], source: &str, model_cap: u64, json_cap: u64) -> Result<String, JsonError> {
    check_model_len(bytes.len() as u64, model_cap)?;
    let text = match format {
        ModelFormat::Ani => ani::export(bytes, source, json_cap)?,
        other => return Err(JsonError::Schema(format!("the JSON form of {} files is not available in this build", other.label()))),
    };
    match import_capped(text.as_bytes(), MAX_MODEL_BYTES, json_cap) {
        Ok((f, back)) if f == format && back == bytes => Ok(text),
        Ok((f, back)) => {
            let at = bytes.iter().zip(&back).position(|(a, b)| a != b).unwrap_or(bytes.len().min(back.len()));
            Err(JsonError::SelfCheck(format!("format {f:?}, {} bytes read back for {}; first difference at byte {at}", back.len(), bytes.len())))
        }
        Err(e) => Err(JsonError::SelfCheck(format!("the text does not import: {e}"))),
    }
}

/// The bytes a JSON text describes (the format comes from its `format` key), after every check of the schema; the writer re-parses its bytes.
pub fn import(json: &[u8]) -> Result<(ModelFormat, Vec<u8>), JsonError> {
    import_capped(json, MAX_MODEL_BYTES, MAX_JSON_BYTES)
}

pub(crate) fn import_capped(json: &[u8], model_cap: u64, json_cap: u64) -> Result<(ModelFormat, Vec<u8>), JsonError> {
    check_json_len(json.len() as u64, json_cap)?;
    let format = format_of_unchecked(json)?;
    let bytes = match format {
        ModelFormat::Ani => ani::import(json, model_cap)?,
        other => return Err(JsonError::Schema(format!("the JSON form of {} files is not available in this build", other.label()))),
    };
    Ok((format, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn formats_know_their_names() {
        for (name, f) in [("a.ani", ModelFormat::Ani), ("A.3DC", ModelFormat::C3dc), ("x.Vani", ModelFormat::Vani), ("y.MANI", ModelFormat::Mani), ("z.3do", ModelFormat::Mesh3do), ("q.3De", ModelFormat::Mesh3de)] {
            assert_eq!(ModelFormat::from_file_name(name), Some(f), "{name}");
            assert!(f.schema().starts_with("shaiya-toolkit-") && f.schema().ends_with("-v1"));
            assert!(f.label().eq_ignore_ascii_case(&format!(".{}", f.extension())));
        }
        assert_eq!(ModelFormat::from_file_name("noext"), None);
        assert_eq!(ModelFormat::from_file_name("a.ani.json"), None);
        assert_eq!(ModelFormat::from_file_name("a.sdata"), None);
    }

    #[test]
    fn the_json_cap_is_checked_before_parsing() {
        assert!(check_json_len(10, 10).is_ok());
        assert!(matches!(check_json_len(11, 10), Err(JsonError::TooLarge(_))));
        // the same check guards the import: a 20-byte text against a cap of 10 is refused without being read as JSON
        assert!(matches!(import_capped(b"this is not json!!!!", MAX_MODEL_BYTES, 10), Err(JsonError::TooLarge(_))));
    }

    #[test]
    fn the_model_cap_is_checked_before_parsing() {
        assert!(check_model_len(10, 10).is_ok());
        assert!(matches!(check_model_len(11, 10), Err(JsonError::TooLarge(_))));
        assert!(matches!(export_capped(ModelFormat::Ani, &[0u8; 11], "a.ani", 10, MAX_JSON_BYTES), Err(JsonError::TooLarge(_))));
    }

    #[test]
    fn export_stops_writing_at_the_json_cap() {
        let mut c = Capped::new(100);
        assert!(c.write_all(&[b'x'; 100]).is_ok());
        assert!(c.write_all(b"y").is_err());
        assert_eq!(c.buf.len(), 100, "the buffer never grows past the cap");
        let arm = shaiya_testkit::anim::arm_ani(60);
        let e = export_capped(ModelFormat::Ani, &arm, "arm.ANI", MAX_MODEL_BYTES, 300).unwrap_err();
        assert!(matches!(e, JsonError::TooLarge(_)), "{e}");
        assert!(export_capped(ModelFormat::Ani, &arm, "arm.ANI", MAX_MODEL_BYTES, MAX_JSON_BYTES).is_ok());
    }

    #[test]
    fn unknown_newer_and_missing_format_are_refused() {
        let newer = import(br#"{"format": "shaiya-toolkit-ani-v2"}"#).unwrap_err();
        assert!(matches!(newer, JsonError::Schema(_)) && newer.to_string().contains("newer toolkit (v2)"), "{newer}");
        let foreign = import(br#"{"format": "foo"}"#).unwrap_err();
        assert!(matches!(foreign, JsonError::Schema(_)) && foreign.to_string().contains("not a toolkit model JSON (format \"foo\")"), "{foreign}");
        let none = import(br#"{"first_field": 0}"#).unwrap_err();
        assert!(matches!(none, JsonError::Json { .. }), "{none}");
        assert!(matches!(import(b"not json"), Err(JsonError::Json { .. })));
        assert_eq!(format_of(br#"{"format": "shaiya-toolkit-3do-v1", "x": [1, 2]}"#).unwrap(), ModelFormat::Mesh3do);
    }
}
