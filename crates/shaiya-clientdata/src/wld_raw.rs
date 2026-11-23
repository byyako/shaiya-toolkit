//! Lossless `.wld` model. Where [`crate::wld::Wld`] keeps only
//! the decoded text of each name, [`WldRaw`] keeps every name as the raw 256-byte buffer the map tool wrote, garbage after the
//! NUL included, and every other value as the bytes it was read from (floats as `f32`, so even a NaN comes back as it was).
//! `parse` then `to_bytes` gives the input back byte for byte for every file whose counts are plain lengths.
//!
//! Two counts are not plain lengths and are **recomputed on write** from the vectors (see the World format page): the NPC list's
//! `n` is the number of records plus all their extra points, and the zone groups' `total` is the number of boxes of all
//! groups. A file whose declared value differs from that (the reader stops early or late) does not write back identically:
//! `to_bytes` then differs from the input, which is how the editor finds out that a file must stay read-only.
//!
//! The client reads a `.wld` with no length check (an observed behaviour of the client): a file that is too short inside the archive runs on into the next
//! file. So the writer never guesses; [`WldRaw::validate`] lists everything that would give a file the reader cannot take
//! back, and the editor refuses to write a model that fails it.

use crate::error::{Cur, Error, Result};
use crate::wld::{
    BgmZone, EffectPlacement, Mani, Obj, OrientedBox, Placement, SoundPoint, WldKind, WldNpc, ZoneBox, ZoneGroup, HEIGHT_OFFSET,
    HEIGHT_SCALE,
};

/// A name buffer of the file: 256 bytes, NUL-terminated, garbage after the NUL.
pub type Name256 = [u8; 256];
/// The portal's destination name: 248 bytes.
pub type Name248 = [u8; 248];

// ---------------------------------------------------------------------------------------------------------------- code pages

/// The code page of the names. The files of the EP4 client are cp949 (EUC-KR family); other clients use other pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Codepage {
    #[default]
    Cp949,
    ShiftJis,
    Gbk,
    /// Western European (Windows-1252).
    Latin1,
}

impl Codepage {
    pub const ALL: [Codepage; 4] = [Codepage::Cp949, Codepage::ShiftJis, Codepage::Gbk, Codepage::Latin1];

    /// The name stored in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            Codepage::Cp949 => "cp949",
            Codepage::ShiftJis => "shift_jis",
            Codepage::Gbk => "gbk",
            Codepage::Latin1 => "latin1",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Codepage::Cp949 => "cp949 (Korean, the EP4 client)",
            Codepage::ShiftJis => "Shift-JIS (Japanese)",
            Codepage::Gbk => "GBK (Chinese)",
            Codepage::Latin1 => "Windows-1252 (Western)",
        }
    }

    /// The page for a settings key; an unknown key gives `None`.
    pub fn from_key(key: &str) -> Option<Codepage> {
        Codepage::ALL.into_iter().find(|c| c.key().eq_ignore_ascii_case(key.trim()))
    }

    fn encoding(self) -> &'static encoding_rs::Encoding {
        match self {
            Codepage::Cp949 => encoding_rs::EUC_KR,
            Codepage::ShiftJis => encoding_rs::SHIFT_JIS,
            Codepage::Gbk => encoding_rs::GBK,
            Codepage::Latin1 => encoding_rs::WINDOWS_1252,
        }
    }

    /// The text of a name buffer: the bytes before the first NUL.
    pub fn decode(self, buf: &[u8]) -> String {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        self.encoding().decode_without_bom_handling(&buf[..end]).0.into_owned()
    }

    /// The bytes of a text in this page, or why it cannot be written.
    pub fn encode(self, text: &str) -> std::result::Result<Vec<u8>, String> {
        if text.contains('\0') {
            return Err("the text contains a NUL character".to_string());
        }
        let (bytes, _, had_errors) = self.encoding().encode(text);
        if had_errors {
            return Err(format!("the text has a character that {} cannot write", self.key()));
        }
        Ok(bytes.into_owned())
    }

    /// Writes `text` into a fixed buffer: NUL-terminated, the rest zeroed. Fails (and leaves the buffer alone) when the text
    /// does not fit with its NUL or has a character the page cannot write.
    pub fn set_fixed(self, buf: &mut [u8], text: &str) -> std::result::Result<(), String> {
        let bytes = self.encode(text)?;
        if bytes.len() >= buf.len() {
            return Err(format!("the text is {} bytes; at most {} fit", bytes.len(), buf.len() - 1));
        }
        buf.fill(0);
        buf[..bytes.len()].copy_from_slice(&bytes);
        Ok(())
    }
}

/// The text of a name buffer in the default page (cp949).
pub fn name_text(buf: &[u8]) -> String {
    Codepage::Cp949.decode(buf)
}

// ---------------------------------------------------------------------------------------------------------------- the model

/// The terrain grid of a field map (`n = size / 2 + 1` corners per side, index `cz * n + cx`).
#[derive(Clone, Debug, PartialEq)]
pub struct RawTerrain {
    pub size: i32,
    pub heights: Vec<i16>,
    pub tex_index: Vec<u8>,
}

impl RawTerrain {
    /// Corners per side for this size.
    pub fn corners(&self) -> usize {
        (self.size.max(0) as usize) / 2 + 1
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RawTexture {
    pub name: Name256,
    pub tiling: f32,
    pub footstep: Name256,
}

/// A list of raw names and the placements that point at them by index.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawPlaced {
    pub names: Vec<Name256>,
    pub items: Vec<Placement>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawMani {
    pub names: Vec<Name256>,
    pub items: Vec<Mani>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawObjects {
    pub names: Vec<Name256>,
    pub items: Vec<Obj>,
    pub pairs: Vec<[Obj; 2]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RawWavZone {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub wavs: Vec<i32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawSound {
    pub bgm_names: Vec<Name256>,
    pub bgm_zones: Vec<BgmZone>,
    pub wav_names: Vec<Name256>,
    pub wav_zones: Vec<RawWavZone>,
    pub points: Vec<SoundPoint>,
}

/// A portal (556 bytes) with its raw names.
#[derive(Clone, Debug, PartialEq)]
pub struct RawPortal {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub rot_y: f32,
    pub name: Name256,
    pub dest_name: Name248,
    pub country: i32,
    pub level_min: i16,
    pub level_max: i16,
    pub dest_map: i32,
    pub dest_pos: [f32; 3],
}

/// A named area (548 bytes) with its raw names.
#[derive(Clone, Debug, PartialEq)]
pub struct RawArea {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub rot_y: f32,
    pub name: Name256,
    pub tga: Name256,
    pub mode: i32,
    pub extra: i32,
}

/// A whole `.wld`, every byte accounted for.
#[derive(Clone, Debug, PartialEq)]
pub struct WldRaw {
    pub kind: WldKind,
    /// Fields only.
    pub terrain: Option<RawTerrain>,
    pub textures: Vec<RawTexture>,
    /// `.wtr` name (fields only).
    pub water: Name256,
    /// `.dg` name (dungeons only).
    pub dg: Name256,
    pub building: RawPlaced,
    pub shape: RawPlaced,
    pub tree: RawPlaced,
    pub grass: RawPlaced,
    pub vani: RawPlaced,
    pub vani2: RawPlaced,
    pub dungeon: RawPlaced,
    pub mani: RawMani,
    pub effect_file: Name256,
    pub effects: Vec<EffectPlacement>,
    pub object: RawObjects,
    pub object2: RawPlaced,
    pub sound: RawSound,
    pub boxes: Vec<OrientedBox>,
    pub portals: Vec<RawPortal>,
    pub zone_groups: Vec<ZoneGroup>,
    pub areas: Vec<RawArea>,
    pub npcs: Vec<WldNpc>,
    /// Sky dome and two cloud names (fields only: three names; dungeons: none).
    pub sky: Vec<Name256>,
    pub colours: [[f32; 3]; 3],
    pub fog_near: f32,
    pub fog_far: f32,
}

// ---------------------------------------------------------------------------------------------------------------- reading

fn raw_name(r: &mut Cur, what: &str) -> Result<Name256> {
    Ok(r.bytes(256, what)?.try_into().expect("256 bytes"))
}

fn raw_names(r: &mut Cur, what: &str) -> Result<Vec<Name256>> {
    let n = r.count(256, 100_000, what)?;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(raw_name(r, what)?);
    }
    Ok(v)
}

fn raw_placed(r: &mut Cur, what: &str) -> Result<RawPlaced> {
    let names = raw_names(r, what)?;
    let n = r.count(40, 10_000_000, what)?;
    let mut items = Vec::with_capacity(n);
    for _ in 0..n {
        items.push(Placement { index: r.i32(what)?, pos: r.f3(what)?, dir: r.f3(what)?, up: r.f3(what)? });
    }
    Ok(RawPlaced { names, items })
}

fn raw_obj(r: &mut Cur) -> Result<Obj> {
    let index = r.i32("object index")?;
    let pos = r.f3("object")?;
    let dir = r.f3("object")?;
    let up = r.f3("object")?;
    let mut params = [0u32; 9];
    for p in params.iter_mut() {
        *p = r.u32("object parameter")?;
    }
    Ok(Obj { index, pos, dir, up, params })
}

impl WldRaw {
    /// Reads a `.wld` with the same rules and limits as [`crate::wld::Wld::parse`]: the file must end exactly at the last byte.
    pub fn parse(data: &[u8], file: &str) -> Result<WldRaw> {
        let mut r = Cur::new(data, file);
        let kind = match r.bytes(4, "map type")? {
            b"FLD\0" => WldKind::Field,
            b"DUN\0" => WldKind::Dungeon,
            other => return Err(Error::Format { file: file.into(), what: format!("map type {other:?} is neither FLD nor DUN"), offset: 0 }),
        };
        let mut terrain = None;
        let mut textures = Vec::new();
        let mut water = [0u8; 256];
        let mut dg = [0u8; 256];
        if kind == WldKind::Field {
            let at = r.p;
            let size = r.i32("map size")?;
            if size <= 0 || size % 2 != 0 || size > 8192 {
                r.p = at;
                return Err(r.fail(format!("map size {size} is not an even number up to 8192")));
            }
            let n = size as usize / 2 + 1;
            let cells = n * n;
            if cells * 3 > r.left() {
                return Err(r.fail(format!("terrain grid {n}x{n} does not fit in the file")));
            }
            let heights = r.bytes(cells * 2, "terrain heights")?.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
            let tex_index = r.bytes(cells, "terrain texture indices")?.to_vec();
            terrain = Some(RawTerrain { size, heights, tex_index });
            let nt = r.count(516, 256, "terrain texture count")?;
            for _ in 0..nt {
                let name = raw_name(&mut r, "terrain texture name")?;
                let tiling = r.f32("terrain texture tiling")?;
                let footstep = raw_name(&mut r, "terrain footstep name")?;
                textures.push(RawTexture { name, tiling, footstep });
            }
            water = raw_name(&mut r, "water name")?;
        } else {
            dg = raw_name(&mut r, "dungeon name")?;
        }
        let building = raw_placed(&mut r, "building")?;
        let shape = raw_placed(&mut r, "shape")?;
        let tree = raw_placed(&mut r, "tree")?;
        let grass = raw_placed(&mut r, "grass")?;
        let vani = raw_placed(&mut r, "vani")?;
        let vani2 = raw_placed(&mut r, "vani2")?;
        let dungeon = raw_placed(&mut r, "dungeon")?;

        let mut mani = RawMani { names: raw_names(&mut r, "mani names")?, items: Vec::new() };
        let n = r.count(44, 10_000_000, "mani count")?;
        for _ in 0..n {
            mani.items.push(Mani {
                index1: r.i32("mani index")?,
                index2: r.i32("mani index")?,
                pos: r.f3("mani")?,
                dir: r.f3("mani")?,
                up: r.f3("mani")?,
            });
        }

        let effect_file = raw_name(&mut r, "effect file")?;
        let n = r.count(40, 10_000_000, "effect count")?;
        let mut effects = Vec::with_capacity(n);
        for _ in 0..n {
            effects.push(EffectPlacement { pos: r.f3("effect")?, dir: r.f3("effect")?, up: r.f3("effect")?, sequence: r.i32("effect sequence")? });
        }

        let mut object = RawObjects { names: raw_names(&mut r, "object names")?, ..Default::default() };
        let n = r.count(76, 10_000_000, "object count")?;
        for _ in 0..n {
            object.items.push(raw_obj(&mut r)?);
        }
        let n = r.count(152, 10_000_000, "object pair count")?;
        for _ in 0..n {
            object.pairs.push([raw_obj(&mut r)?, raw_obj(&mut r)?]);
        }
        let object2 = raw_placed(&mut r, "object2")?;

        let mut sound = RawSound { bgm_names: raw_names(&mut r, "bgm names")?, ..Default::default() };
        let n = r.count(36, 10_000_000, "bgm zone count")?;
        for _ in 0..n {
            sound.bgm_zones.push(BgmZone {
                min: r.f3("bgm zone")?,
                max: r.f3("bgm zone")?,
                rot_y: r.f32("bgm zone")?,
                bgm_index: r.i32("bgm zone")?,
                flag: r.i32("bgm zone")?,
            });
        }
        sound.wav_names = raw_names(&mut r, "wav names")?;
        let n = r.count(28, 10_000_000, "wav zone count")?;
        for _ in 0..n {
            let min = r.f3("wav zone")?;
            let max = r.f3("wav zone")?;
            let k = r.count(4, 1000, "wav zone entries")?;
            let mut wavs = Vec::with_capacity(k);
            for _ in 0..k {
                wavs.push(r.i32("wav index")?);
            }
            sound.wav_zones.push(RawWavZone { min, max, wavs });
        }
        let n = r.count(20, 10_000_000, "sound point count")?;
        for _ in 0..n {
            sound.points.push(SoundPoint { wav: r.i32("sound point")?, pos: r.f3("sound point")?, radius: r.f32("sound point")? });
        }

        let n = r.count(28, 10_000_000, "oriented box count")?;
        let mut boxes = Vec::with_capacity(n);
        for _ in 0..n {
            boxes.push(OrientedBox { min: r.f3("box")?, max: r.f3("box")?, rot_y: r.f32("box")? });
        }
        let n = r.count(556, 1_000_000, "portal count")?;
        let mut portals = Vec::with_capacity(n);
        for _ in 0..n {
            portals.push(RawPortal {
                min: r.f3("portal")?,
                max: r.f3("portal")?,
                rot_y: r.f32("portal")?,
                name: raw_name(&mut r, "portal name")?,
                dest_name: r.bytes(248, "portal destination name")?.try_into().expect("248 bytes"),
                country: r.i32("portal country")?,
                level_min: r.i16("portal level")?,
                level_max: r.i16("portal level")?,
                dest_map: r.i32("portal destination map")?,
                dest_pos: r.f3("portal destination")?,
            });
        }
        // zone groups: the client reads groups until the boxes read reach `total`; the writer recomputes `total`
        let total = r.count(0, 10_000_000, "zone record count")?;
        let mut zone_groups = Vec::new();
        let mut seen = 0usize;
        while seen < total {
            let k = r.count(36, 100_000, "zone group size")?;
            if k == 0 {
                return Err(r.fail("empty zone group (the client would loop forever)"));
            }
            let mut g = ZoneGroup::default();
            for _ in 0..k {
                g.boxes.push(ZoneBox {
                    min: r.f3("zone box")?,
                    max: r.f3("zone box")?,
                    rot_y: r.f32("zone box")?,
                    country: r.i32("zone country")?,
                    attr: r.i32("zone attr")?,
                });
            }
            seen += k;
            zone_groups.push(g);
        }
        let n = r.count(548, 1_000_000, "area count")?;
        let mut areas = Vec::with_capacity(n);
        for _ in 0..n {
            areas.push(RawArea {
                min: r.f3("area")?,
                max: r.f3("area")?,
                rot_y: r.f32("area")?,
                name: raw_name(&mut r, "area name")?,
                tga: raw_name(&mut r, "area caption")?,
                mode: r.i32("area mode")?,
                extra: r.i32("area extra")?,
            });
        }
        // NPC list: the client subtracts each record's extra point count from the declared n
        let declared = r.count(0, 10_000_000, "npc count")?;
        let mut left = declared as i64;
        let mut npcs = Vec::new();
        while (npcs.len() as i64) < left {
            let kind = r.i32("npc type")?;
            let type_id = r.i32("npc type id")?;
            let pos = r.f3("npc position")?;
            let angle = r.f32("npc angle")?;
            let k = r.count(12, 100_000, "npc extra points")?;
            let mut extra = Vec::with_capacity(k);
            for _ in 0..k {
                extra.push(r.f3("npc path point")?);
            }
            left -= k as i64;
            npcs.push(WldNpc { kind, type_id, pos, angle, extra });
        }

        let mut sky = Vec::new();
        if kind == WldKind::Field {
            for _ in 0..3 {
                sky.push(raw_name(&mut r, "sky name")?);
            }
        }
        let mut colours = [[0.0f32; 3]; 3];
        for c in colours.iter_mut() {
            *c = r.f3("colour")?;
        }
        let fog_near = r.f32("fog near")?;
        let fog_far = r.f32("fog far")?;
        r.end("map")?;

        Ok(WldRaw {
            kind,
            terrain,
            textures,
            water,
            dg,
            building,
            shape,
            tree,
            grass,
            vani,
            vani2,
            dungeon,
            mani,
            effect_file,
            effects,
            object,
            object2,
            sound,
            boxes,
            portals,
            zone_groups,
            areas,
            npcs,
            sky,
            colours,
            fog_near,
            fog_far,
        })
    }
}

// ---------------------------------------------------------------------------------------------------------------- writing

struct Out(Vec<u8>);

impl Out {
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i16(&mut self, v: i16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_bits().to_le_bytes());
    }
    fn f3(&mut self, v: [f32; 3]) {
        for x in v {
            self.f32(x);
        }
    }
    fn len(&mut self, n: usize) {
        self.i32(n as i32);
    }
    fn names(&mut self, v: &[Name256]) {
        self.len(v.len());
        for n in v {
            self.0.extend_from_slice(n);
        }
    }
    fn placed(&mut self, p: &RawPlaced) {
        self.names(&p.names);
        self.len(p.items.len());
        for it in &p.items {
            self.i32(it.index);
            self.f3(it.pos);
            self.f3(it.dir);
            self.f3(it.up);
        }
    }
    fn obj(&mut self, o: &Obj) {
        self.i32(o.index);
        self.f3(o.pos);
        self.f3(o.dir);
        self.f3(o.up);
        for p in o.params {
            self.u32(p);
        }
    }
}

impl WldRaw {
    /// The file's bytes. Counts come from the vectors; the NPC list's `n` is the records plus all extra points and the zone
    /// groups' `total` is all boxes. Call [`validate`](Self::validate) first: a model that fails it gives bytes the reader
    /// would not take back.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Out(Vec::with_capacity(self.size_hint()));
        w.0.extend_from_slice(match self.kind {
            WldKind::Field => b"FLD\0",
            WldKind::Dungeon => b"DUN\0",
        });
        match self.kind {
            WldKind::Field => {
                if let Some(t) = &self.terrain {
                    w.i32(t.size);
                    for h in &t.heights {
                        w.i16(*h);
                    }
                    w.0.extend_from_slice(&t.tex_index);
                }
                w.len(self.textures.len());
                for t in &self.textures {
                    w.0.extend_from_slice(&t.name);
                    w.f32(t.tiling);
                    w.0.extend_from_slice(&t.footstep);
                }
                w.0.extend_from_slice(&self.water);
            }
            WldKind::Dungeon => w.0.extend_from_slice(&self.dg),
        }
        for p in [&self.building, &self.shape, &self.tree, &self.grass, &self.vani, &self.vani2, &self.dungeon] {
            w.placed(p);
        }
        w.names(&self.mani.names);
        w.len(self.mani.items.len());
        for m in &self.mani.items {
            w.i32(m.index1);
            w.i32(m.index2);
            w.f3(m.pos);
            w.f3(m.dir);
            w.f3(m.up);
        }
        w.0.extend_from_slice(&self.effect_file);
        w.len(self.effects.len());
        for e in &self.effects {
            w.f3(e.pos);
            w.f3(e.dir);
            w.f3(e.up);
            w.i32(e.sequence);
        }
        w.names(&self.object.names);
        w.len(self.object.items.len());
        for o in &self.object.items {
            w.obj(o);
        }
        w.len(self.object.pairs.len());
        for [a, b] in &self.object.pairs {
            w.obj(a);
            w.obj(b);
        }
        w.placed(&self.object2);
        w.names(&self.sound.bgm_names);
        w.len(self.sound.bgm_zones.len());
        for z in &self.sound.bgm_zones {
            w.f3(z.min);
            w.f3(z.max);
            w.f32(z.rot_y);
            w.i32(z.bgm_index);
            w.i32(z.flag);
        }
        w.names(&self.sound.wav_names);
        w.len(self.sound.wav_zones.len());
        for z in &self.sound.wav_zones {
            w.f3(z.min);
            w.f3(z.max);
            w.len(z.wavs.len());
            for v in &z.wavs {
                w.i32(*v);
            }
        }
        w.len(self.sound.points.len());
        for p in &self.sound.points {
            w.i32(p.wav);
            w.f3(p.pos);
            w.f32(p.radius);
        }
        w.len(self.boxes.len());
        for b in &self.boxes {
            w.f3(b.min);
            w.f3(b.max);
            w.f32(b.rot_y);
        }
        w.len(self.portals.len());
        for p in &self.portals {
            w.f3(p.min);
            w.f3(p.max);
            w.f32(p.rot_y);
            w.0.extend_from_slice(&p.name);
            w.0.extend_from_slice(&p.dest_name);
            w.i32(p.country);
            w.i16(p.level_min);
            w.i16(p.level_max);
            w.i32(p.dest_map);
            w.f3(p.dest_pos);
        }
        w.len(self.zone_total());
        for g in &self.zone_groups {
            w.len(g.boxes.len());
            for b in &g.boxes {
                w.f3(b.min);
                w.f3(b.max);
                w.f32(b.rot_y);
                w.i32(b.country);
                w.i32(b.attr);
            }
        }
        w.len(self.areas.len());
        for a in &self.areas {
            w.f3(a.min);
            w.f3(a.max);
            w.f32(a.rot_y);
            w.0.extend_from_slice(&a.name);
            w.0.extend_from_slice(&a.tga);
            w.i32(a.mode);
            w.i32(a.extra);
        }
        w.len(self.npc_declared());
        for n in &self.npcs {
            w.i32(n.kind);
            w.i32(n.type_id);
            w.f3(n.pos);
            w.f32(n.angle);
            w.len(n.extra.len());
            for p in &n.extra {
                w.f3(*p);
            }
        }
        if self.kind == WldKind::Field {
            for s in &self.sky {
                w.0.extend_from_slice(s);
            }
        }
        for c in self.colours {
            w.f3(c);
        }
        w.f32(self.fog_near);
        w.f32(self.fog_far);
        w.0
    }

    /// The NPC list's declared `n`: records plus every extra point.
    pub fn npc_declared(&self) -> usize {
        self.npcs.iter().map(|n| 1 + n.extra.len()).sum()
    }

    /// The zone groups' declared `total`: every box of every group.
    pub fn zone_total(&self) -> usize {
        self.zone_groups.iter().map(|g| g.boxes.len()).sum()
    }

    fn size_hint(&self) -> usize {
        let t = self.terrain.as_ref().map_or(0, |t| t.heights.len() * 3);
        t + 64 * 1024
    }

    /// Everything that would give bytes the client's reader (or this one) cannot take back. Empty means the model is writable.
    pub fn validate(&self) -> Vec<String> {
        let mut bad = Vec::new();
        match (&self.kind, &self.terrain) {
            (WldKind::Field, None) => bad.push("a field map has no terrain".to_string()),
            (WldKind::Dungeon, Some(_)) => bad.push("a dungeon has terrain".to_string()),
            (WldKind::Field, Some(t)) => {
                if t.size <= 0 || t.size % 2 != 0 || t.size > 8192 {
                    bad.push(format!("map size {} is not an even number up to 8192", t.size));
                } else {
                    let cells = t.corners() * t.corners();
                    if t.heights.len() != cells || t.tex_index.len() != cells {
                        bad.push(format!("the terrain grid needs {cells} heights and texture indices, has {} and {}", t.heights.len(), t.tex_index.len()));
                    }
                }
            }
            (WldKind::Dungeon, None) => {}
        }
        let sky_want = if self.kind == WldKind::Field { 3 } else { 0 };
        if self.sky.len() != sky_want {
            bad.push(format!("{} sky names, the file needs {sky_want}", self.sky.len()));
        }
        if self.textures.len() > 256 {
            bad.push(format!("{} terrain textures; the reader takes at most 256", self.textures.len()));
        }
        for (what, l) in [
            ("building", &self.building),
            ("shape", &self.shape),
            ("tree", &self.tree),
            ("grass", &self.grass),
            ("vani", &self.vani),
            ("vani2", &self.vani2),
            ("dungeon", &self.dungeon),
            ("object2", &self.object2),
        ] {
            if l.names.len() > 100_000 {
                bad.push(format!("{what}: {} names; the reader takes at most 100000", l.names.len()));
            }
        }
        for (what, l) in [("mani", &self.mani.names), ("object", &self.object.names), ("music", &self.sound.bgm_names), ("ambient sound", &self.sound.wav_names)] {
            if l.len() > 100_000 {
                bad.push(format!("{what}: {} names; the reader takes at most 100000", l.len()));
            }
        }
        for (i, g) in self.zone_groups.iter().enumerate() {
            if g.boxes.is_empty() {
                bad.push(format!("bind group {i} has no box (the client would loop forever)"));
            } else if g.boxes.len() > 100_000 {
                bad.push(format!("bind group {i} has {} boxes; the reader takes at most 100000", g.boxes.len()));
            }
        }
        for (i, n) in self.npcs.iter().enumerate() {
            if n.extra.len() > 100_000 {
                bad.push(format!("NPC {i} has {} extra points; the reader takes at most 100000", n.extra.len()));
            }
        }
        for (i, z) in self.sound.wav_zones.iter().enumerate() {
            if z.wavs.len() > 1000 {
                bad.push(format!("ambient zone {i} lists {} sounds; the reader takes at most 1000", z.wavs.len()));
            }
        }
        bad
    }

    /// The decoded form the rest of the editor works with, names read in `cp`.
    pub fn to_wld(&self, cp: Codepage) -> crate::wld::Wld {
        use crate::wld::{ManiList, ObjectList, PlacedList, Sound, Terrain, TerrainTexture, Wld, WavZone, WldArea, WldPortal};
        let text = |b: &[u8]| cp.decode(b);
        let placed = |p: &RawPlaced| PlacedList { names: p.names.iter().map(|n| text(n)).collect(), items: p.items.clone() };
        Wld {
            kind: self.kind.clone(),
            terrain: self.terrain.as_ref().map(|t| Terrain { size: t.size as u32, n: t.corners(), heights: t.heights.clone(), tex_index: t.tex_index.clone() }),
            textures: self.textures.iter().map(|t| TerrainTexture { name: text(&t.name), tiling: t.tiling, footstep: text(&t.footstep) }).collect(),
            water: if self.kind == WldKind::Field { text(&self.water) } else { String::new() },
            dg: if self.kind == WldKind::Dungeon { text(&self.dg) } else { String::new() },
            building: placed(&self.building),
            shape: placed(&self.shape),
            tree: placed(&self.tree),
            grass: placed(&self.grass),
            vani: placed(&self.vani),
            vani2: placed(&self.vani2),
            dungeon: placed(&self.dungeon),
            mani: ManiList { names: self.mani.names.iter().map(|n| text(n)).collect(), items: self.mani.items.clone() },
            effect_file: text(&self.effect_file),
            effects: self.effects.clone(),
            object: ObjectList { names: self.object.names.iter().map(|n| text(n)).collect(), items: self.object.items.clone(), pairs: self.object.pairs.clone() },
            object2: placed(&self.object2),
            sound: Sound {
                bgm_names: self.sound.bgm_names.iter().map(|n| text(n)).collect(),
                bgm_zones: self.sound.bgm_zones.clone(),
                wav_names: self.sound.wav_names.iter().map(|n| text(n)).collect(),
                wav_zones: self.sound.wav_zones.iter().map(|z| WavZone { min: z.min, max: z.max, wavs: z.wavs.clone() }).collect(),
                points: self.sound.points.clone(),
            },
            boxes: self.boxes.clone(),
            portals: self
                .portals
                .iter()
                .map(|p| WldPortal {
                    min: p.min,
                    max: p.max,
                    rot_y: p.rot_y,
                    name: text(&p.name),
                    dest_name: text(&p.dest_name),
                    country: p.country,
                    level_min: p.level_min,
                    level_max: p.level_max,
                    dest_map: p.dest_map,
                    dest_pos: p.dest_pos,
                })
                .collect(),
            zone_groups: self.zone_groups.clone(),
            areas: self.areas.iter().map(|a| WldArea { min: a.min, max: a.max, rot_y: a.rot_y, name: text(&a.name), tga: text(&a.tga), mode: a.mode, extra: a.extra }).collect(),
            npc_declared: self.npc_declared() as u32,
            npcs: self.npcs.clone(),
            sky: self.sky.iter().map(|n| text(n)).collect(),
            colours: self.colours,
            fog_near: self.fog_near,
            fog_far: self.fog_far,
        }
    }

    /// Height of a terrain corner in world units (a convenience for tools that edit heights).
    pub fn corner_height(&self, cx: usize, cz: usize) -> Option<f32> {
        let t = self.terrain.as_ref()?;
        let n = t.corners();
        (cx < n && cz < n).then(|| t.heights[cz * n + cx] as f32 * HEIGHT_SCALE + HEIGHT_OFFSET)
    }
}

/// The offset of the first byte where two byte strings differ (the length of the shorter one when one is a prefix of the other),
/// or `None` when they are equal.
pub fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    if a == b {
        return None;
    }
    Some(a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len())))
}

/// Whether `data` parses and writes back to exactly the same bytes. `Err` carries the reason a person can read.
pub fn lossless_check(data: &[u8], file: &str) -> std::result::Result<(), String> {
    let raw = WldRaw::parse(data, file).map_err(|e| e.to_string())?;
    let back = raw.to_bytes();
    match first_difference(data, &back) {
        None => Ok(()),
        Some(at) => Err(format!("first difference at byte {at} (file {} bytes, written {} bytes)", data.len(), back.len())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wld::Wld;

    fn name(s: &str) -> Name256 {
        let mut b = [0u8; 256];
        b[..s.len()].copy_from_slice(s.as_bytes());
        b
    }

    /// A name with stack garbage after its NUL, as the map tool left it.
    fn dirty(s: &str, junk: u8) -> Name256 {
        let mut b = [junk; 256];
        b[..s.len()].copy_from_slice(s.as_bytes());
        b[s.len()] = 0;
        b
    }

    fn pl(i: i32) -> Placement {
        Placement { index: i, pos: [1.0, 2.0, 3.0], dir: [0.0, 0.0, 1.0], up: [0.0, 1.0, 0.0] }
    }

    /// A small field map with something in every list, dirty names and two zone groups.
    pub(super) fn sample() -> WldRaw {
        let size = 8;
        let n = size / 2 + 1;
        let mut portal_dest = [0xAAu8; 248];
        portal_dest[..4].copy_from_slice(b"Home");
        portal_dest[4] = 0;
        WldRaw {
            kind: WldKind::Field,
            terrain: Some(RawTerrain { size, heights: (0..n * n).map(|i| 9000 + i as i16).collect(), tex_index: (0..n * n).map(|i| (i % 2) as u8).collect() }),
            textures: vec![
                RawTexture { name: dirty("grass.dds", 0x11), tiling: 2.5, footstep: dirty("ch_walkg", 0x22) },
                RawTexture { name: name("stone.dds"), tiling: 1.0, footstep: name("ch_walks") },
            ],
            water: dirty("lake.wtr", 0x33),
            dg: [0; 256],
            building: RawPlaced { names: vec![dirty("house.smod", 0x44), name("tower.smod")], items: vec![pl(0), pl(1), pl(-1)] },
            shape: RawPlaced { names: vec![name("rock.smod")], items: vec![pl(0)] },
            tree: RawPlaced::default(),
            grass: RawPlaced { names: vec![name("g.smod")], items: vec![] },
            vani: RawPlaced::default(),
            vani2: RawPlaced::default(),
            dungeon: RawPlaced::default(),
            mani: RawMani { names: vec![name("m.mani")], items: vec![Mani { index1: 0, index2: 1, pos: [1.0; 3], dir: [0.0, 0.0, 1.0], up: [0.0, 1.0, 0.0] }] },
            effect_file: dirty("world_a1.eft", 0x55),
            effects: vec![EffectPlacement { pos: [4.0; 3], dir: [0.0, 0.0, 1.0], up: [0.0, 1.0, 0.0], sequence: 7 }],
            object: RawObjects { names: vec![name("o.obj")], items: vec![], pairs: vec![] },
            object2: RawPlaced::default(),
            sound: RawSound {
                bgm_names: vec![name("bgm_a")],
                bgm_zones: vec![BgmZone { min: [0.0; 3], max: [8.0, 4.0, 8.0], rot_y: 0.0, bgm_index: 0, flag: 1 }],
                wav_names: vec![name("w1.wav"), name("w2.wav")],
                wav_zones: vec![RawWavZone { min: [0.0; 3], max: [4.0; 3], wavs: vec![0, 1] }],
                points: vec![SoundPoint { wav: 1, pos: [2.0; 3], radius: 5.0 }],
            },
            boxes: vec![OrientedBox { min: [0.0; 3], max: [1.0; 3], rot_y: 0.5 }],
            portals: vec![RawPortal {
                min: [10.0, 1.0, 10.0],
                max: [14.0, 5.0, 14.0],
                rot_y: 0.0,
                name: dirty("portal0", 0x66),
                dest_name: portal_dest,
                country: 1,
                level_min: 0,
                level_max: 999,
                dest_map: 2,
                dest_pos: [100.0, 3.0, 100.0],
            }],
            zone_groups: vec![
                ZoneGroup { boxes: vec![ZoneBox { min: [0.0; 3], max: [2.0; 3], rot_y: 0.0, country: 1, attr: 0 }, ZoneBox { min: [3.0; 3], max: [5.0; 3], rot_y: 0.0, country: 1, attr: 0 }] },
                ZoneGroup { boxes: vec![ZoneBox { min: [6.0; 3], max: [7.0; 3], rot_y: 0.0, country: 2, attr: 1 }] },
            ],
            areas: vec![RawArea { min: [0.0; 3], max: [8.0, 4.0, 8.0], rot_y: 0.0, name: dirty("Meadow", 0x77), tga: name(""), mode: 1, extra: 0 }],
            npcs: vec![
                WldNpc { kind: 1, type_id: 3, pos: [5.0, 1.0, 5.0], angle: 1.5, extra: vec![] },
                WldNpc { kind: 2, type_id: 1, pos: [6.0, 1.0, 6.0], angle: 0.0, extra: vec![[7.0, 1.0, 7.0], [8.0, 1.0, 8.0]] },
                WldNpc { kind: 1, type_id: 4, pos: [1.0, 1.0, 1.0], angle: 3.0, extra: vec![[2.0, 1.0, 2.0]] },
            ],
            sky: vec![dirty("sky.bmp", 0x88), name("cloud1.tga"), name("cloud2.tga")],
            colours: [[255.0, 255.0, 255.0], [10.0, 20.0, 30.0], [0.0, 0.0, 0.0]],
            fog_near: 300.0,
            fog_far: 600.0,
        }
    }

    pub(super) fn sample_dungeon() -> WldRaw {
        let mut w = sample();
        w.kind = WldKind::Dungeon;
        w.terrain = None;
        w.textures.clear();
        w.water = [0; 256];
        w.dg = dirty("cave.dg", 0x99);
        w.sky.clear();
        w
    }

    #[test]
    fn hand_made_maps_roundtrip_with_their_garbage() {
        for w in [sample(), sample_dungeon()] {
            assert!(w.validate().is_empty(), "{:?}", w.validate());
            let bytes = w.to_bytes();
            let back = WldRaw::parse(&bytes, "t.wld").unwrap();
            assert_eq!(back, w);
            assert_eq!(back.to_bytes(), bytes, "bytes are identical");
            assert_eq!(back.building.names[0][12], 0x44, "the garbage after the NUL is kept");
            assert!(lossless_check(&bytes, "t.wld").is_ok());
            // the decoded reader agrees with the raw one
            let rd = Wld::parse(&bytes, "t.wld").unwrap();
            assert_eq!(back.to_wld(Codepage::Cp949), rd);
        }
    }

    /// The NPC list's `n` counts records plus extra points; the zone `total` counts all boxes. A wrong value in a file makes the
    /// writer produce a different file, which the editor then refuses to edit.
    #[test]
    fn wld_counts_recomputed() {
        let w = sample();
        assert_eq!((w.npcs.len(), w.npc_declared()), (3, 6), "3 records + 3 extra points");
        assert_eq!(w.zone_total(), 3);
        let bytes = w.to_bytes();
        let rd = Wld::parse(&bytes, "t.wld").unwrap();
        assert_eq!(rd.npc_declared, 6);
        assert_eq!(rd.npcs.len(), 3);
        assert_eq!(rd.zone_groups.len(), 2);
        // adding a point to an NPC and a box to a group moves both counts
        let mut e = w.clone();
        e.npcs[0].extra.push([9.0, 1.0, 9.0]);
        e.zone_groups[1].boxes.push(ZoneBox { min: [0.0; 3], max: [1.0; 3], rot_y: 0.0, country: 2, attr: 1 });
        let rd = Wld::parse(&e.to_bytes(), "t.wld").unwrap();
        assert_eq!((rd.npc_declared, rd.npcs.len(), rd.npcs[0].extra.len()), (7, 3, 1));
        assert_eq!(rd.zone_groups[1].boxes.len(), 2);
        assert_eq!(WldRaw::parse(&e.to_bytes(), "t.wld").unwrap(), e);
        let pos = zone_total_offset(&w);
        assert_eq!(i32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()), 3, "the total is every box of every group");
    }

    /// Offset of the zone `total` field in `w.to_bytes()`: the sum of everything before it.
    fn zone_total_offset(w: &WldRaw) -> usize {
        let mut cut = w.clone();
        cut.zone_groups.clear();
        cut.areas.clear();
        cut.npcs.clear();
        // bytes before the total = all bytes minus (total + groups + areas + npcs + the tail)
        let full = cut.to_bytes().len();
        let tail = 4 + 4 + 4 /* total, area count, npc count */ + w.sky.len() * 256 + 9 * 4 + 8;
        full - tail
    }

    #[test]
    fn a_zone_total_that_stops_early_is_not_lossless() {
        // one group of 2 boxes, declared total 1: the reader takes the whole group, the writer says 2
        let mut w = sample();
        w.zone_groups.truncate(1);
        let mut bytes = w.to_bytes();
        let pos = zone_total_offset(&w);
        assert_eq!(i32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()), 2);
        bytes[pos..pos + 4].copy_from_slice(&1i32.to_le_bytes());
        let why = lossless_check(&bytes, "t.wld").unwrap_err();
        assert!(why.contains("first difference at byte"), "{why}");
        assert_eq!(first_difference(&bytes, &w.to_bytes()), Some(pos));
    }

    #[test]
    fn wld_write_edit_reparse() {
        let mut w = sample();
        let cp = Codepage::Cp949;
        let before = w.to_bytes();
        // an edited name is NUL-terminated with the rest zeroed; untouched names keep their bytes
        cp.set_fixed(&mut w.building.names[0], "castle.smod").unwrap();
        assert_eq!(w.building.names[0][11], 0);
        assert!(w.building.names[0][11..].iter().all(|b| *b == 0), "the old garbage is gone");
        assert_eq!(w.water[8], 0);
        assert_eq!(w.water[9], 0x33, "an untouched name keeps its garbage");
        // a portal box moved 5 units in x, a destination name edited, an NPC point added, a height changed
        w.portals[0].min[0] += 5.0;
        w.portals[0].max[0] += 5.0;
        cp.set_fixed(&mut w.portals[0].dest_name, "Capital").unwrap();
        w.npcs[1].extra.push([9.0, 1.0, 9.0]);
        w.terrain.as_mut().unwrap().heights[3] = -5;
        assert!(w.validate().is_empty());
        let bytes = w.to_bytes();
        assert_ne!(bytes, before);
        assert_eq!(bytes.len(), before.len() + 12, "one NPC point is 12 bytes");
        let raw = WldRaw::parse(&bytes, "t.wld").unwrap();
        assert_eq!(raw, w);
        let rd = Wld::parse(&bytes, "t.wld").unwrap();
        assert_eq!(rd.portals[0].min[0], 15.0);
        assert_eq!(rd.portals[0].dest_name, "Capital");
        assert_eq!(rd.building.names[0], "castle.smod");
        assert_eq!(rd.npcs[1].extra.len(), 3);
        assert_eq!(rd.npc_declared, 7);
        assert_eq!(rd.terrain.unwrap().heights[3], -5);
        // an unchanged re-write is byte-identical
        assert_eq!(raw.to_bytes(), bytes);
    }

    #[test]
    fn text_helpers() {
        let cp = Codepage::Cp949;
        let mut b = [0xFFu8; 8];
        assert!(cp.set_fixed(&mut b, "1234567").is_ok());
        assert_eq!(&b, b"1234567\0");
        assert!(cp.set_fixed(&mut b, "12345678").is_err(), "no room for the NUL");
        assert_eq!(&b, b"1234567\0", "a refused text leaves the buffer alone");
        assert!(cp.set_fixed(&mut b, "a\0b").is_err());
        // Korean text in cp949 and in a page that cannot write it
        let mut k = [0u8; 16];
        cp.set_fixed(&mut k, "\u{d3ec}\u{d0c8}").unwrap();
        assert_eq!(cp.decode(&k), "\u{d3ec}\u{d0c8}");
        assert_eq!(k[4], 0);
        assert!(Codepage::Latin1.set_fixed(&mut k, "\u{d3ec}").is_err());
        assert_eq!(Codepage::from_key("SHIFT_JIS"), Some(Codepage::ShiftJis));
        assert_eq!(Codepage::from_key("nonsense"), None);
        for c in Codepage::ALL {
            assert_eq!(Codepage::from_key(c.key()), Some(c));
        }
        assert_eq!(name_text(&dirty("abc", 0x41)), "abc");
    }

    #[test]
    fn validate_names_what_would_break_the_file() {
        let mut w = sample();
        w.terrain.as_mut().unwrap().heights.pop();
        w.zone_groups.push(ZoneGroup::default());
        w.sky.pop();
        let v = w.validate().join(" | ");
        assert!(v.contains("terrain grid") && v.contains("bind group 2 has no box") && v.contains("2 sky names"), "{v}");
        let mut d = sample_dungeon();
        d.terrain = Some(RawTerrain { size: 8, heights: vec![], tex_index: vec![] });
        assert!(d.validate().join(" ").contains("dungeon has terrain"));
    }

    #[test]
    fn garbage_never_panics() {
        let good = sample().to_bytes();
        for cut in [0, 3, 4, 7, 100, good.len() - 1] {
            assert!(WldRaw::parse(&good[..cut], "t.wld").is_err(), "cut at {cut}");
        }
        let mut more = good.clone();
        more.push(0);
        assert!(WldRaw::parse(&more, "t.wld").is_err(), "trailing byte");
        assert!(WldRaw::parse(b"XXXX", "t.wld").is_err());
        // every cut of both hand-made maps is refused (a truncated file never parses as a shorter map)
        for w in [sample(), sample_dungeon()] {
            let b = w.to_bytes();
            for cut in 0..b.len() {
                assert!(WldRaw::parse(&b[..cut], "t.wld").is_err(), "cut at {cut} of {}", b.len());
            }
        }
        // pseudo-random damage: flipped bytes, counts set to huge, negative or off-by-one values; never a panic, and whatever
        // parses writes back to bytes that parse to the same model
        let mut x = 0x2545_f491u32;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        for round in 0..3000 {
            let mut b = if round % 2 == 0 { good.clone() } else { sample_dungeon().to_bytes() };
            for _ in 0..1 + next() % 4 {
                let at = next() as usize % b.len();
                match next() % 4 {
                    0 => b[at] ^= 1 << (next() % 8),
                    1 if at + 4 <= b.len() => b[at..at + 4].copy_from_slice(&[0xff, 0xff, 0xff, 0x7f]),
                    2 if at + 4 <= b.len() => b[at..at + 4].copy_from_slice(&(-1i32).to_le_bytes()),
                    _ => {
                        if at + 4 <= b.len() {
                            let v = i32::from_le_bytes(b[at..at + 4].try_into().unwrap()).wrapping_add(1);
                            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
                        }
                    }
                }
            }
            if let Ok(w) = WldRaw::parse(&b, "t.wld") {
                let back = w.to_bytes();
                assert_eq!(WldRaw::parse(&back, "t.wld").ok().as_ref().map(|m| m.to_bytes()), Some(back.clone()), "round {round}");
                let _ = lossless_check(&b, "t.wld");
            }
        }
    }

    // ------------------------------------------------------------------ the real client

    fn client() -> Option<crate::source::ClientSource> {
        let dir = shaiya_testkit::env_client()?;
        dir.is_dir().then(|| crate::source::ClientSource::open(&dir).expect("open the client"))
    }

    /// Every `.wld` of the client writes back byte for byte, raw names (garbage after the NUL) included.
    #[test]
    fn wld_roundtrip_all() {
        let Some(c) = client() else { return };
        let paths = c.wld_paths();
        assert!(!paths.is_empty());
        let mut garbage_names = 0usize;
        for p in &paths {
            let bytes = c.read(p).unwrap();
            let raw = WldRaw::parse(&bytes, p).unwrap_or_else(|e| panic!("{e}"));
            assert!(raw.validate().is_empty(), "{p}: {:?}", raw.validate());
            let back = raw.to_bytes();
            if let Some(at) = first_difference(&bytes, &back) {
                panic!("{p}: first difference at byte {at} (file {} bytes, written {} bytes)", bytes.len(), back.len());
            }
            // the garbage is really there: some name has non-zero bytes after its NUL
            for n in raw.building.names.iter().chain(&raw.shape.names).chain(&raw.tree.names) {
                let nul = n.iter().position(|&b| b == 0).unwrap();
                if n[nul..].iter().any(|&b| b != 0) {
                    garbage_names += 1;
                }
            }
            // and a parse of the written file gives the same model
            assert_eq!(WldRaw::parse(&back, p).unwrap(), raw);
            // a truncated or lengthened copy is refused, never read as a shorter map
            for cut in [bytes.len() - 1, bytes.len() - 4, bytes.len() - 9 * 4 - 8, bytes.len() / 2, bytes.len() / 3, 4] {
                assert!(WldRaw::parse(&bytes[..cut], p).is_err(), "{p}: cut at {cut}");
            }
            let mut longer = bytes.clone();
            longer.extend_from_slice(&[0; 4]);
            assert!(WldRaw::parse(&longer, p).is_err(), "{p}: 4 more bytes");
        }
        eprintln!("wld_roundtrip_all: {} files byte-identical, {garbage_names} placement names with garbage after the NUL", paths.len());
        assert!(garbage_names > 0, "the originals do carry garbage after the NUL, so the test proves it is kept");
    }

    /// The raw model decodes to exactly what the existing reader gives, on every file.
    #[test]
    fn wld_raw_matches_reader() {
        let Some(c) = client() else { return };
        for p in c.wld_paths() {
            let bytes = c.read(&p).unwrap();
            let rd = Wld::parse(&bytes, &p).unwrap();
            let raw = WldRaw::parse(&bytes, &p).unwrap();
            assert_eq!(raw.to_wld(Codepage::Cp949), rd, "{p}");
        }
    }
}
