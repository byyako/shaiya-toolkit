//! `.MANI` building-motion files: exactly 108 bytes, no counts and no header. The sizes and the order of the 20 fields were read from
//! client files (every file reads, and every `.MANI` the nine matrix clients' indexes list is 108 bytes); **every meaning below is
//! inferred** and was not checked in the game.
//!
//! The file is kept as its 108 raw bytes with typed views, so [`Mani::to_bytes`] gives back the bytes [`Mani::parse`] read, whatever the
//! float bits are. Two writer generations exist: some files hold the integer 500 (bytes `f4 01 00 00`, a denormal as a float) at offset
//! 104 and zero bytes at 80 to 83; the others hold two `u16` at 80 and a float at 104. The toolkit shows both and interprets neither.

use crate::error::{Error, Result};

/// The size of a `.MANI`.
pub const MANI_SIZE: usize = 108;

/// How a field is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManiType {
    I32,
    U16,
    F32,
    F32x3,
}

impl ManiType {
    /// Bytes the type takes in the file.
    pub fn size(self) -> usize {
        match self {
            ManiType::U16 => 2,
            ManiType::I32 | ManiType::F32 => 4,
            ManiType::F32x3 => 12,
        }
    }
}

/// One field of the table: byte offset, the toolkit's name, the stored type and the inferred meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ManiField {
    pub offset: usize,
    pub name: &'static str,
    pub kind: ManiType,
    /// Always an inference (nothing here was checked in the game).
    pub meaning: &'static str,
}

const fn f(offset: usize, name: &'static str, kind: ManiType, meaning: &'static str) -> ManiField {
    ManiField { offset, name, kind, meaning }
}

use ManiType::{F32, F32x3, I32, U16};

/// The 20 fields in file order; together they cover the 108 bytes with no gap. The names are the toolkit's; the meanings are inferred.
pub const MANI_FIELDS: [ManiField; 20] = [
    f(0, "tick_ms", I32, "the motion steps once per this many milliseconds"),
    f(4, "swing_on", I32, "rocking about swing_axis (0 or 1)"),
    f(8, "swing_axis", F32x3, "axis of the rocking"),
    f(20, "swing_limit", F32, "radians; the direction flips at this angle"),
    f(24, "swing_step", F32, "radians per step"),
    f(28, "swing_unused", F32, "no use known"),
    f(32, "swing_pause_ms", I32, "wait at each end of the rocking, in milliseconds"),
    f(36, "orbit_on", I32, "circular offset about orbit_axis (0 or 1)"),
    f(40, "orbit_axis", F32x3, "axis of the circular offset"),
    f(52, "orbit_step", F32, "radians per step"),
    f(56, "orbit_radius", F32, "world units"),
    f(60, "spin_on", I32, "continuous rotation about spin_axis (0 or 1)"),
    f(64, "spin_axis", F32x3, "axis of the rotation"),
    f(76, "spin_step", F32, "radians per step"),
    f(80, "move_on", U16, "back-and-forth movement along move_dir (0 or 1)"),
    f(82, "move_pause_ms", U16, "wait at each end of the movement, in milliseconds"),
    f(84, "move_dir", F32x3, "direction of the movement"),
    f(96, "move_speed", F32, "start distance per step"),
    f(100, "move_accel", F32, "speed factor per step"),
    f(104, "move_distance", F32, "end point = move_dir times this"),
];

/// A field's value, read from the raw bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ManiValue {
    I32(i32),
    U16(u16),
    F32(f32),
    F32x3([f32; 3]),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mani {
    /// The file as read.
    pub raw: [u8; MANI_SIZE],
}

impl Mani {
    /// Reads a `.MANI`: exactly 108 bytes, anything else is refused.
    pub fn parse(data: &[u8], file: &str) -> Result<Mani> {
        if data.len() != MANI_SIZE {
            return Err(Error::Format { file: file.to_string(), what: format!("a .MANI is {MANI_SIZE} bytes, this file has {}", data.len()), offset: 0 });
        }
        let mut raw = [0u8; MANI_SIZE];
        raw.copy_from_slice(data);
        Ok(Mani { raw })
    }

    /// The 108 bytes, as read.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.raw.to_vec()
    }

    fn u32_at(&self, at: usize) -> u32 {
        u32::from_le_bytes([self.raw[at], self.raw[at + 1], self.raw[at + 2], self.raw[at + 3]])
    }

    /// The value of a table field.
    pub fn value(&self, field: &ManiField) -> ManiValue {
        let at = field.offset;
        match field.kind {
            ManiType::I32 => ManiValue::I32(self.u32_at(at) as i32),
            ManiType::U16 => ManiValue::U16(u16::from_le_bytes([self.raw[at], self.raw[at + 1]])),
            ManiType::F32 => ManiValue::F32(f32::from_bits(self.u32_at(at))),
            ManiType::F32x3 => ManiValue::F32x3([f32::from_bits(self.u32_at(at)), f32::from_bits(self.u32_at(at + 4)), f32::from_bits(self.u32_at(at + 8))]),
        }
    }

    /// Every field with its value, in file order.
    pub fn fields(&self) -> Vec<(ManiField, ManiValue)> {
        MANI_FIELDS.iter().map(|f| (*f, self.value(f))).collect()
    }

    /// Builds a `.MANI` from one value per field, in [`MANI_FIELDS`] order, each written at its offset (floats by their bits). A value
    /// whose variant is not the field's type is refused.
    pub fn from_values(values: &[ManiValue; 20]) -> Result<Mani> {
        let mut raw = [0u8; MANI_SIZE];
        for (field, value) in MANI_FIELDS.iter().zip(values) {
            let at = field.offset;
            let bytes: Vec<u8> = match (field.kind, value) {
                (ManiType::I32, ManiValue::I32(v)) => v.to_le_bytes().to_vec(),
                (ManiType::U16, ManiValue::U16(v)) => v.to_le_bytes().to_vec(),
                (ManiType::F32, ManiValue::F32(v)) => v.to_bits().to_le_bytes().to_vec(),
                (ManiType::F32x3, ManiValue::F32x3(v)) => v.iter().flat_map(|c| c.to_bits().to_le_bytes()).collect(),
                _ => {
                    return Err(Error::Format { file: "mani".into(), what: format!("{}: the value is not a {:?}", field.name, field.kind), offset: at });
                }
            };
            raw[at..at + bytes.len()].copy_from_slice(&bytes);
        }
        Ok(Mani { raw })
    }

    /// The integer stored at offset 104 when its bits read as a float are a denormal (some files hold the integer 500 there); otherwise `None`.
    pub fn distance_as_integer(&self) -> Option<i32> {
        let bits = self.u32_at(104);
        (bits & 0x7FFF_FFFF != 0 && bits & 0x7F80_0000 == 0).then_some(bits as i32)
    }

    /// Whether the file holds an integer at offset 104 and zero bytes at offsets 80 to 83 (one of the two writer generations; not interpreted).
    pub fn is_integer_generation(&self) -> bool {
        self.distance_as_integer().is_some() && self.raw[80..84] == [0, 0, 0, 0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::anim::mani_bytes;

    fn parse(d: &[u8]) -> Result<Mani> {
        Mani::parse(d, "t.mani")
    }

    #[test]
    fn the_field_table_covers_the_108_bytes_without_a_gap() {
        let mut at = 0;
        for f in &MANI_FIELDS {
            assert_eq!(f.offset, at, "{}", f.name);
            at += f.kind.size();
        }
        assert_eq!(at, MANI_SIZE);
    }

    #[test]
    fn a_mani_reads_and_writes_back() {
        let d = mani_bytes(33, 1, 500, 10.0f32.to_bits());
        let m = parse(&d).unwrap();
        assert_eq!(m.to_bytes(), d);
        let v: std::collections::BTreeMap<&str, ManiValue> = m.fields().into_iter().map(|(f, v)| (f.name, v)).collect();
        assert_eq!(v["tick_ms"], ManiValue::I32(33));
        assert_eq!(v["move_on"], ManiValue::U16(1));
        assert_eq!(v["move_pause_ms"], ManiValue::U16(500));
        assert_eq!(v["move_distance"], ManiValue::F32(10.0));
        assert_eq!(v["spin_axis"], ManiValue::F32x3([0.0, 0.0, 1.0]));
        assert_eq!(v["move_dir"], ManiValue::F32x3([1.0, 0.0, 0.0]));
        assert_eq!(m.fields().len(), 20);
        assert_eq!(m.distance_as_integer(), None);
        assert!(!m.is_integer_generation());
    }

    #[test]
    fn the_integer_in_the_last_field_is_shown_as_an_integer_too() {
        let m = parse(&mani_bytes(33, 0, 0, 500)).unwrap();
        assert_eq!(m.distance_as_integer(), Some(500));
        assert!(m.is_integer_generation());
        let float = m.value(&MANI_FIELDS[19]);
        assert!(matches!(float, ManiValue::F32(x) if x != 0.0 && !x.is_normal()), "{float:?}");
        // a u16 pair at 80 with the integer still reads; it is the other generation
        assert!(!parse(&mani_bytes(33, 1, 0, 500)).unwrap().is_integer_generation());
        // zero is not a denormal, and neither is negative zero (sign bit only)
        assert_eq!(parse(&mani_bytes(33, 0, 0, 0x8000_0000)).unwrap().distance_as_integer(), None);
        assert_eq!(parse(&mani_bytes(33, 0, 0, 0)).unwrap().distance_as_integer(), None);
    }

    #[test]
    fn a_mani_of_107_or_109_bytes_is_refused() {
        let d = mani_bytes(33, 1, 500, 10.0f32.to_bits());
        assert_eq!(d.len(), MANI_SIZE);
        for n in [0, 1, 4, 80, 107] {
            let e = parse(&d[..n]).unwrap_err().to_string();
            assert!(e.contains("a .MANI is 108 bytes"), "{e}");
        }
        let mut longer = d.clone();
        longer.push(0);
        assert!(parse(&longer).unwrap_err().to_string().contains("a .MANI is 108 bytes"));
        longer.extend_from_slice(&[0; 100]);
        assert!(parse(&longer).is_err());
    }

    #[test]
    fn odd_float_bits_survive() {
        let mut d = mani_bytes(33, 1, 500, 10.0f32.to_bits());
        d[8..12].copy_from_slice(&0xFFC0_0000u32.to_le_bytes());
        d[20..24].copy_from_slice(&0x7FA0_0001u32.to_le_bytes());
        let m = parse(&d).unwrap();
        assert_eq!(m.to_bytes(), d);
        let ManiValue::F32x3(axis) = m.value(&MANI_FIELDS[2]) else { panic!() };
        assert_eq!(axis[0].to_bits(), 0xFFC0_0000);
    }

    #[test]
    fn from_values_writes_each_field_at_its_offset() {
        for d in [mani_bytes(33, 1, 500, 10.0f32.to_bits()), mani_bytes(33, 0, 0, 500)] {
            let m = parse(&d).unwrap();
            let values: Vec<ManiValue> = m.fields().into_iter().map(|(_, v)| v).collect();
            let values: [ManiValue; 20] = values.try_into().unwrap();
            assert_eq!(Mani::from_values(&values).unwrap().to_bytes(), d);
        }
        // every field gets its own distinct bytes: one value changed changes only that field's bytes
        let base = parse(&mani_bytes(33, 1, 500, 10.0f32.to_bits())).unwrap();
        let values: Vec<ManiValue> = base.fields().into_iter().map(|(_, v)| v).collect();
        for (i, field) in MANI_FIELDS.iter().enumerate() {
            let mut changed: [ManiValue; 20] = values.clone().try_into().unwrap();
            changed[i] = match field.kind {
                ManiType::I32 => ManiValue::I32(0x0102_0304),
                ManiType::U16 => ManiValue::U16(0x0A0B),
                ManiType::F32 => ManiValue::F32(f32::from_bits(0x0000_0002)),
                ManiType::F32x3 => ManiValue::F32x3([f32::from_bits(0x1111_1111), f32::from_bits(0x2222_2222), f32::from_bits(0x3333_3333)]),
            };
            let out = Mani::from_values(&changed).unwrap();
            for at in 0..MANI_SIZE {
                let inside = at >= field.offset && at < field.offset + field.kind.size();
                if !inside {
                    assert_eq!(out.raw[at], base.raw[at], "{}: byte {at} changed", field.name);
                }
            }
            assert_eq!(out.value(field), changed[i], "{}", field.name);
        }
    }

    #[test]
    fn from_values_refuses_a_value_of_another_type() {
        let base = parse(&mani_bytes(33, 1, 500, 10.0f32.to_bits())).unwrap();
        let values: Vec<ManiValue> = base.fields().into_iter().map(|(_, v)| v).collect();
        let mut wrong: [ManiValue; 20] = values.try_into().unwrap();
        wrong[0] = ManiValue::F32(1.0);
        assert!(Mani::from_values(&wrong).unwrap_err().to_string().contains("tick_ms"));
        wrong[0] = ManiValue::I32(33);
        wrong[14] = ManiValue::I32(1);
        assert!(Mani::from_values(&wrong).unwrap_err().to_string().contains("move_on"));
        wrong[14] = ManiValue::U16(1);
        wrong[2] = ManiValue::F32(0.0);
        assert!(Mani::from_values(&wrong).is_err());
    }
}
