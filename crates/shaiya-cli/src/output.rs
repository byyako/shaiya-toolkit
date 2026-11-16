//! Result output: human text on stdout, or one JSON document with `--json`.

use crate::Ctx;
use serde::Serialize;
use std::io::Write;

/// A command's result: human text plus `serde::Serialize` for `--json`.
pub trait Report: Serialize {
    fn human(&self, w: &mut dyn Write) -> std::io::Result<()>;
}

/// Object-safe form of [`Report`] used by [`emit`].
pub trait ReportDyn {
    fn human(&self, w: &mut dyn Write) -> std::io::Result<()>;
    fn json(&self) -> serde_json::Value;
}

impl<T: Report> ReportDyn for T {
    fn human(&self, w: &mut dyn Write) -> std::io::Result<()> {
        Report::human(self, w)
    }
    fn json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

/// Writes the result to stdout. A closed pipe is not an error worth reporting.
pub fn emit(ctx: &Ctx, r: &dyn ReportDyn) {
    let stdout = std::io::stdout();
    let mut w = stdout.lock();
    let _ = if ctx.json {
        serde_json::to_writer_pretty(&mut w, &r.json()).map_err(std::io::Error::from).and_then(|_| writeln!(w))
    } else {
        r.human(&mut w)
    };
    let _ = w.flush();
}

/// The JSON error object for a failure: `{"error": {"kind", "message", "file", "offset"}}`.
pub fn error_json(e: &shaiya_core::Error) -> serde_json::Value {
    let (kind, file, offset) = match e {
        shaiya_core::Error::Io { path, .. } => ("io", path.as_ref().map(|p| p.display().to_string()), None),
        shaiya_core::Error::Format { file, offset, .. } => ("format", Some(file.clone()), *offset),
        shaiya_core::Error::Unsupported(_) => ("unsupported", None, None),
        shaiya_core::Error::Refused(_) => ("refused", None, None),
        shaiya_core::Error::Cancelled => ("cancelled", None, None),
    };
    serde_json::json!({ "error": { "kind": kind, "message": e.to_string(), "file": file, "offset": offset } })
}

/// `1,234,567`
pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_groups() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1000), "1,000");
        assert_eq!(group(1_991_954_802), "1,991,954,802");
    }

    #[test]
    fn error_objects() {
        let e = shaiya_core::Error::format("Item.SData", Some(40), "SEED checksum failed");
        let v = error_json(&e);
        assert_eq!(v["error"]["kind"], "format");
        assert_eq!(v["error"]["file"], "Item.SData");
        assert_eq!(v["error"]["offset"], 40);
        assert_eq!(v["error"]["message"], "Item.SData: SEED checksum failed (at byte 40)");
    }
}
