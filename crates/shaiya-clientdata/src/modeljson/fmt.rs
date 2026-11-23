//! The text layout of the model JSON files (`serde_json` formatter).
//!
//! Layout, in the words of the specification:
//!
//! - Objects: `{`, then each member on its own line (`\n` + 2 spaces per depth) as `"key": value`, members separated by `,`, then `\n` +
//!   indent + `}`. An empty object is `{}`.
//! - Arrays: an empty array is `[]`. An array inside an array is written on one line. Any other array is written on one line when its first
//!   element is a number; otherwise each element goes on its own line (`\n` + indent of depth + 1), separated by `,`, closed by `\n` + indent
//!   + `]`. Inside a one-line array everything is on that line, elements separated by `, ` (comma, space).
//! - A top-level `[f32; 3]` whose first value is written as `"bits:..."` (a string) goes one element per line: still valid and deterministic.
//!
//! How it works: when an array that is not inside another array opens, its mode is undecided; the first value written into it decides
//! (a number: one line; anything else: one element per line). Every method that starts a value resolves that pending decision first.

use serde_json::ser::{CompactFormatter, Formatter};
use std::io::{self, Write};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Undecided,
    Inline,
    Lines,
}

enum Frame {
    Object { members: usize },
    Array { mode: Mode, items: usize },
}

#[derive(Default)]
pub(crate) struct LineFormatter {
    stack: Vec<Frame>,
}

impl LineFormatter {
    pub fn new() -> Self {
        Self::default()
    }

    fn indent<W: ?Sized + Write>(w: &mut W, depth: usize) -> io::Result<()> {
        w.write_all(b"\n")?;
        for _ in 0..depth {
            w.write_all(b"  ")?;
        }
        Ok(())
    }

    /// A value is about to start: settle an undecided array (`is_number`: the value is a number).
    fn start<W: ?Sized + Write>(&mut self, w: &mut W, is_number: bool) -> io::Result<()> {
        let depth = self.stack.len();
        if let Some(Frame::Array { mode, .. }) = self.stack.last_mut() {
            if *mode == Mode::Undecided {
                if is_number {
                    *mode = Mode::Inline;
                } else {
                    *mode = Mode::Lines;
                    Self::indent(w, depth)?;
                }
            }
        }
        Ok(())
    }
}

macro_rules! number {
    ($name:ident, $ty:ty) => {
        fn $name<W: ?Sized + Write>(&mut self, w: &mut W, value: $ty) -> io::Result<()> {
            self.start(w, true)?;
            write!(w, "{}", value)
        }
    };
}

impl Formatter for LineFormatter {
    fn write_null<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.start(w, false)?;
        w.write_all(b"null")
    }

    fn write_bool<W: ?Sized + Write>(&mut self, w: &mut W, value: bool) -> io::Result<()> {
        self.start(w, false)?;
        w.write_all(if value { b"true" } else { b"false" })
    }

    number!(write_i8, i8);
    number!(write_i16, i16);
    number!(write_i32, i32);
    number!(write_i64, i64);
    number!(write_i128, i128);
    number!(write_u8, u8);
    number!(write_u16, u16);
    number!(write_u32, u32);
    number!(write_u64, u64);
    number!(write_u128, u128);

    fn write_f32<W: ?Sized + Write>(&mut self, w: &mut W, value: f32) -> io::Result<()> {
        self.start(w, true)?;
        CompactFormatter.write_f32(w, value)
    }

    fn write_f64<W: ?Sized + Write>(&mut self, w: &mut W, value: f64) -> io::Result<()> {
        self.start(w, true)?;
        CompactFormatter.write_f64(w, value)
    }

    fn write_number_str<W: ?Sized + Write>(&mut self, w: &mut W, value: &str) -> io::Result<()> {
        self.start(w, true)?;
        w.write_all(value.as_bytes())
    }

    fn begin_string<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.start(w, false)?;
        w.write_all(b"\"")
    }

    fn begin_array<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.start(w, false)?;
        let inside_array = matches!(self.stack.last(), Some(Frame::Array { .. }));
        self.stack.push(Frame::Array { mode: if inside_array { Mode::Inline } else { Mode::Undecided }, items: 0 });
        w.write_all(b"[")
    }

    fn end_array<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        let Some(Frame::Array { mode, items }) = self.stack.pop() else { return w.write_all(b"]") };
        if mode == Mode::Lines && items > 0 {
            Self::indent(w, self.stack.len())?;
        }
        w.write_all(b"]")
    }

    fn begin_array_value<W: ?Sized + Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        let depth = self.stack.len();
        if let Some(Frame::Array { mode, items }) = self.stack.last_mut() {
            *items += 1;
            if !first {
                match mode {
                    Mode::Lines => {
                        w.write_all(b",")?;
                        Self::indent(w, depth)?;
                    }
                    _ => w.write_all(b", ")?,
                }
            }
        }
        Ok(())
    }

    fn end_array_value<W: ?Sized + Write>(&mut self, _w: &mut W) -> io::Result<()> {
        Ok(())
    }

    fn begin_object<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.start(w, false)?;
        self.stack.push(Frame::Object { members: 0 });
        w.write_all(b"{")
    }

    fn end_object<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        let Some(Frame::Object { members }) = self.stack.pop() else { return w.write_all(b"}") };
        if members > 0 {
            Self::indent(w, self.stack.len())?;
        }
        w.write_all(b"}")
    }

    fn begin_object_key<W: ?Sized + Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        let depth = self.stack.len();
        if let Some(Frame::Object { members }) = self.stack.last_mut() {
            *members += 1;
        }
        if !first {
            w.write_all(b",")?;
        }
        Self::indent(w, depth)
    }

    fn begin_object_value<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        w.write_all(b": ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Probe {
        empty_object: Empty,
        empty_array: Vec<u8>,
        numbers: Vec<u8>,
        floats: Vec<f32>,
        pairs: Vec<(i32, f32)>,
        objects: Vec<Item>,
        names: Vec<&'static str>,
        bits_first: Vec<&'static str>,
        flag: Option<bool>,
    }

    #[derive(Serialize)]
    struct Empty {}

    #[derive(Serialize)]
    struct Item {
        a: u8,
        b: Vec<u8>,
    }

    fn text<T: Serialize>(v: &T) -> String {
        let mut out = Vec::new();
        let mut ser = serde_json::Serializer::with_formatter(&mut out, LineFormatter::new());
        v.serialize(&mut ser).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn objects_and_arrays_follow_the_layout_rules() {
        let p = Probe {
            empty_object: Empty {},
            empty_array: vec![],
            numbers: vec![1, 2, 3],
            floats: vec![0.5, 1.0],
            pairs: vec![(0, 0.25), (60, 1.5)],
            objects: vec![Item { a: 1, b: vec![] }, Item { a: 2, b: vec![7, 8] }],
            names: vec!["x", "y"],
            bits_first: vec!["bits:7fc00000", "bits:7fc00001"],
            flag: None,
        };
        let expected = r#"{
  "empty_object": {},
  "empty_array": [],
  "numbers": [1, 2, 3],
  "floats": [0.5, 1.0],
  "pairs": [
    [0, 0.25],
    [60, 1.5]
  ],
  "objects": [
    {
      "a": 1,
      "b": []
    },
    {
      "a": 2,
      "b": [7, 8]
    }
  ],
  "names": [
    "x",
    "y"
  ],
  "bits_first": [
    "bits:7fc00000",
    "bits:7fc00001"
  ],
  "flag": null
}"#;
        assert_eq!(text(&p), expected);
    }
}
