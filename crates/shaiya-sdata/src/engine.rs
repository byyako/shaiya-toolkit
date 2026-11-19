//! The one reader. It walks a [`Layout`] over the plaintext. A walk without `build` only does the arithmetic (the fit check, no
//! allocation); a walk with `build` also makes the rows. Both stop at the first byte that does not fit, and a walk that finishes must
//! have consumed the whole file.

use crate::layout::{Count, Field, Kind, Layout, Node, Section};
use crate::Value;
use shaiya_core::text::Encoding;

/// Where and why a layout stopped fitting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stop {
    pub offset: usize,
    pub reason: String,
}

struct Run<'a> {
    d: &'a [u8],
    pos: usize,
    enc: Encoding,
    build: bool,
}

type Step<T> = Result<T, Stop>;

fn num(kind: Kind, b: &[u8]) -> Value {
    match kind {
        Kind::U8 => Value::Int(i64::from(b[0])),
        Kind::I8 => Value::Int(i64::from(b[0] as i8)),
        Kind::U16 => Value::Int(i64::from(u16::from_le_bytes([b[0], b[1]]))),
        Kind::I16 => Value::Int(i64::from(i16::from_le_bytes([b[0], b[1]]))),
        Kind::U32 => Value::Int(i64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))),
        Kind::I32 => Value::Int(i64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))),
        Kind::F32 => Value::Float(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
    }
}

fn text_of(enc: Encoding, raw: &[u8]) -> Value {
    let end = raw.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    Value::Text(enc.decode(&raw[..end]).0)
}

impl<'a> Run<'a> {
    fn left(&self) -> usize {
        self.d.len() - self.pos
    }

    fn stop(&self, reason: impl Into<String>) -> Stop {
        Stop { offset: self.pos, reason: reason.into() }
    }

    fn take(&mut self, n: usize, what: &str) -> Step<&'a [u8]> {
        if n > self.left() {
            return Err(self.stop(format!("{what}: {n} bytes needed, {} left", self.left())));
        }
        let s = &self.d[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u32(&mut self, what: &str) -> Step<u32> {
        let b = self.take(4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn fields(&self, fields: &[Field], b: &[u8], row: &mut Vec<Value>) {
        let mut at = 0;
        for f in fields {
            row.push(num(f.kind, &b[at..at + f.kind.size()]));
            at += f.kind.size();
        }
    }

    fn node(&mut self, n: &Node, row: &mut Vec<Value>) -> Step<()> {
        match n {
            Node::Scalar { kind, count, .. } => {
                let b = self.take(kind.size() * *count as usize, "a value")?;
                if self.build {
                    if *count == 1 {
                        row.push(num(*kind, b));
                    } else {
                        let parts: Vec<String> = b.chunks(kind.size()).map(|c| num(*kind, c).to_string()).collect();
                        row.push(Value::Text(parts.join(" ")));
                    }
                }
            }
            Node::Str { .. } => {
                let len = self.u32("a text length")? as usize;
                if len > self.left() {
                    return Err(self.stop(format!("a text of {len} bytes does not fit in the {} bytes left", self.left())));
                }
                let b = self.take(len, "a text")?;
                if self.build {
                    row.push(text_of(self.enc, b));
                }
            }
            Node::Fixed { size, fields } => {
                let b = self.take(*size, "a fixed block")?;
                if self.build {
                    self.fields(fields, b, row);
                }
            }
            Node::List { elem, .. } => {
                let n = self.u32("a list length")? as usize;
                let each: usize = elem.iter().map(|f| f.kind.size()).sum();
                if n.saturating_mul(each) > self.left() {
                    return Err(self.stop(format!("a list of {n} elements does not fit in the {} bytes left", self.left())));
                }
                let b = self.take(n * each, "a list")?;
                if self.build {
                    let parts: Vec<String> = b
                        .chunks(each)
                        .map(|c| {
                            let mut one = Vec::with_capacity(elem.len());
                            self.fields(elem, c, &mut one);
                            one.iter().map(Value::to_string).collect::<Vec<_>>().join(":")
                        })
                        .collect();
                    row.push(Value::Text(parts.join(" ")));
                }
            }
            Node::Repeat { count, items, .. } => {
                for _ in 0..*count {
                    for it in items {
                        self.node(it, row)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn walk(&mut self, sec: &Section, depth: usize, idx: &mut Vec<i64>, rep: Option<i64>, rows: &mut Vec<Vec<Value>>) -> Step<()> {
        if depth == sec.loops.len() {
            let mut row = Vec::new();
            if self.build {
                row.reserve(sec.columns.len());
                row.extend(rep.map(Value::Int));
                row.extend(idx.iter().map(|&i| Value::Int(i)));
            }
            for n in &sec.record {
                self.node(n, &mut row)?;
            }
            if self.build {
                rows.push(row);
            }
            return Ok(());
        }
        let lp = &sec.loops[depth];
        let n = match lp.count {
            Count::Fixed(n) => u64::from(n),
            Count::Read => {
                let n = u64::from(self.u32("a count")?);
                if let Some(max) = lp.max {
                    if n > max {
                        return Err(Stop { offset: self.pos - 4, reason: format!("a count of {n} is not plausible (the most is {max})") });
                    }
                }
                n
            }
        };
        if n.saturating_mul(sec.min_bytes[depth + 1]) > self.left() as u64 {
            return Err(self.stop(format!("a count of {n} does not fit in the {} bytes left", self.left())));
        }
        let base = lp.index.as_ref().map_or(0, |i| i.1);
        for i in 0..n {
            if lp.index.is_some() {
                idx.push(base + i as i64);
            }
            let r = self.walk(sec, depth + 1, idx, rep, rows);
            if lp.index.is_some() {
                idx.pop();
            }
            r?;
        }
        Ok(())
    }
}

/// Reads `data` with `layout`. `Ok` holds one row list per section (empty without `build`) only when the layout consumed every byte.
pub fn run(layout: &Layout, data: &[u8], enc: Encoding, build: bool) -> Result<Vec<Vec<Vec<Value>>>, Stop> {
    let mut r = Run { d: data, pos: 0, enc, build };
    let mut out = Vec::with_capacity(layout.sections.len());
    for sec in &layout.sections {
        let mut rows = Vec::new();
        let reps = sec.repeat.as_ref().map_or(1, |x| x.count);
        for i in 0..reps {
            let rep = sec.repeat.as_ref().map(|x| x.base + i64::from(i));
            r.walk(sec, 0, &mut Vec::new(), rep, &mut rows)?;
        }
        out.push(rows);
    }
    if r.pos != data.len() {
        return Err(r.stop(format!("{} bytes are left after the table", data.len() - r.pos)));
    }
    Ok(out)
}
