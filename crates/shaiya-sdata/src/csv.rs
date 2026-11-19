//! CSV: UTF-8 with a byte-order mark, CRLF line ends, comma separated. A text cell that starts with `=`, `+`, `-`, `@`, a tab or a
//! carriage return gets a leading `'` so a spreadsheet does not run it as a formula; numbers are written as they are.

use crate::{Sheet, Value};

const BOM: &str = "\u{feff}";

/// One text cell: formula-safe, quoted when it holds a comma, a quote or a line break.
pub fn csv_text(s: &str) -> String {
    let mut t = s.to_string();
    if t.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        t.insert(0, '\'');
    }
    if t.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", t.replace('"', "\"\""))
    } else {
        t
    }
}

fn cell(v: &Value) -> String {
    match v {
        Value::Text(s) => csv_text(s),
        other => other.to_string(),
    }
}

/// The sheet as CSV: a header row with the column names, then the rows.
pub fn csv(sheet: &Sheet) -> String {
    csv_rows(sheet, sheet.rows.iter())
}

/// The header and the given rows (the viewer exports what its filter shows).
pub fn csv_rows<'a>(sheet: &Sheet, rows: impl Iterator<Item = &'a Vec<Value>>) -> String {
    let mut out = String::from(BOM);
    out.push_str(&sheet.columns.iter().map(|c| csv_text(&c.name)).collect::<Vec<_>>().join(","));
    out.push_str("\r\n");
    for r in rows {
        out.push_str(&r.iter().map(cell).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    out
}
