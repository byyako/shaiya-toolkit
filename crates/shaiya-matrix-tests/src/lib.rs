//! The version matrix: what the toolkit parses on which Shaiya client. `EXPECTED` mirrors `docs/version-matrix.md`;
//! `observe` measures one client folder; the tests in `tests/matrix.rs` compare the two on the clients the user has (opt-in through
//! `SHAIYA_CLIENT` and `SHAIYA_TEST_CLIENTS`). No game data lives in the repository.
//!
//! A client folder holds `data.sah` and `game.exe`, and either `data.saf` (a full client: SData files are read from the archive) or
//! `files/<archive path with "/" replaced by "__">` (SData files and `sysmsg-uni.txt`, fetched by HTTP range).

mod t1;
mod t5;
mod t10;
mod t7;

use shaiya_archive::{Archive, Index};
use shaiya_core::Job;
use shaiya_inspect::{inspect, Kind, Options};
use shaiya_seed::Wrapper;
use std::path::{Path, PathBuf};

/// One cell of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    Ok,
    /// Parsing returns an error (the right failure mode for an unsupported layout).
    Fails,
    /// Every SData file is plain (not encrypted).
    Plain,
    /// A table layout reads the whole file, but no loader of this client was read for it (the layout's `evidence`): never called OK.
    FitOnly,
    NotAvailable,
}

impl Cell {
    pub fn label(self) -> &'static str {
        match self {
            Cell::Ok => "OK",
            Cell::Fails => "fails",
            Cell::Plain => "Plain",
            Cell::FitOnly => "FIT-ONLY",
            Cell::NotAvailable => "n/a",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cap {
    IndexParse,
    ToBytesIdentical,
    InspectSah,
    SeedUnwrap,
    WrapIdentity,
    Monsters,
    NpcQuest,
    InspectPe,
    // archive, patch and SData batch capabilities
    ArchiveVisibleCount,
    ArchiveCheckIndex,
    ArchiveFreeList,
    ArchiveDataEnd,
    ArchiveDiffSelf,
    ArchiveEditRollbackStub,
    PatchApplyRollbackStub,
    SdataBatchCheck,
    SdataLikeRoundtrip,
    // world and map capabilities
    WldParse,
    WldLossless,
    WldPreview,
    ModelPreview,
    AreaTxtRoundtrip,
    NpcQuestGates,
    MapNameIds,
    // SData tables
    SdataItemFits,
    SdataSkillFits,
    SdataMonsterFits,
    SdataNpcQuestFits,
    SdataAutoPicks,
    // animations
    AniIndexCount,
    AniRoundTrip,
    C3dcIndexCount,
    C3dcRoundTrip,
    AniPreview,
    VaniIndexCount,
    VaniRoundTrip,
    ManiIndexCount,
    ManiIndexSize,
    ManiRoundTrip,
}

pub const CAPS: [Cap; 39] = [
    Cap::IndexParse,
    Cap::ToBytesIdentical,
    Cap::InspectSah,
    Cap::SeedUnwrap,
    Cap::WrapIdentity,
    Cap::Monsters,
    Cap::NpcQuest,
    Cap::InspectPe,
    Cap::ArchiveVisibleCount,
    Cap::ArchiveCheckIndex,
    Cap::ArchiveFreeList,
    Cap::ArchiveDataEnd,
    Cap::ArchiveDiffSelf,
    Cap::ArchiveEditRollbackStub,
    Cap::PatchApplyRollbackStub,
    Cap::SdataBatchCheck,
    Cap::SdataLikeRoundtrip,
    Cap::WldParse,
    Cap::WldLossless,
    Cap::WldPreview,
    Cap::ModelPreview,
    Cap::AreaTxtRoundtrip,
    Cap::NpcQuestGates,
    Cap::MapNameIds,
    Cap::SdataItemFits,
    Cap::SdataSkillFits,
    Cap::SdataMonsterFits,
    Cap::SdataNpcQuestFits,
    Cap::SdataAutoPicks,
    Cap::AniIndexCount,
    Cap::AniRoundTrip,
    Cap::C3dcIndexCount,
    Cap::C3dcRoundTrip,
    Cap::AniPreview,
    Cap::VaniIndexCount,
    Cap::VaniRoundTrip,
    Cap::ManiIndexCount,
    Cap::ManiIndexSize,
    Cap::ManiRoundTrip,
];

/// The archive and patch capabilities, which need a readable `data.sah` (they all fail with it).
const SAH_CAPS: [Cap; 7] = [
    Cap::ArchiveVisibleCount,
    Cap::ArchiveCheckIndex,
    Cap::ArchiveFreeList,
    Cap::ArchiveDataEnd,
    Cap::ArchiveDiffSelf,
    Cap::ArchiveEditRollbackStub,
    Cap::PatchApplyRollbackStub,
];

impl Cap {
    pub fn label(self) -> &'static str {
        match self {
            Cap::IndexParse => "`Index::parse`",
            Cap::ToBytesIdentical => "`to_bytes` byte-identical",
            Cap::InspectSah => "`inspect` on data.sah",
            Cap::SeedUnwrap => "`seed::unwrap` + CRC, every SData",
            Cap::WrapIdentity => "`wrap(unwrap(x)) == x`, every SData",
            Cap::Monsters => "clientdata `Monsters::parse`",
            Cap::NpcQuest => "clientdata `NpcQuest::parse`",
            Cap::InspectPe => "`inspect` on game.exe (PE)",
            Cap::ArchiveVisibleCount => "`archive_visible_count`",
            Cap::ArchiveCheckIndex => "`archive_check_index`",
            Cap::ArchiveFreeList => "`archive_free_list`",
            Cap::ArchiveDataEnd => "`archive_data_end`",
            Cap::ArchiveDiffSelf => "`archive_diff_self`",
            Cap::ArchiveEditRollbackStub => "`archive_edit_rollback_stub`",
            Cap::PatchApplyRollbackStub => "`patch_apply_rollback_stub`",
            Cap::SdataBatchCheck => "`sdata_batch_check`",
            Cap::SdataLikeRoundtrip => "`sdata_like_roundtrip`",
            Cap::WldParse => "clientdata `Wld::parse`, every `world/*.wld`",
            Cap::WldLossless => "`wld_raw` writes every `.wld` back byte-identical",
            Cap::WldPreview => "map preview data of every `world/*.wld` (`render::hillshade_grey` for a field, no terrain for a dungeon)",
            Cap::ModelPreview => "model preview data of every `.smod` (`render::model_picture`: modules, triangles, textures looked up under `Entity/Texture`)",
            Cap::AreaTxtRoundtrip => "`area_txt` writes every `world/*.txt` back byte-identical",
            Cap::NpcQuestGates => "gatekeepers: 3 gates each, map ids 0..=255, finite positions",
            Cap::MapNameIds => "sysmsg has text for the distinct map-name message ids of the EP4.5 table",
            Cap::SdataItemFits => "sdata `Item` layout fits",
            Cap::SdataSkillFits => "sdata `Skill` layout fits",
            Cap::SdataMonsterFits => "sdata `Monster` layout fits",
            Cap::SdataNpcQuestFits => "sdata `NpcQuest` layout fits",
            Cap::SdataAutoPicks => "`auto` picks exactly one layout or none, every SData",
            Cap::AniIndexCount => "`.ANI` files the index lists (index only)",
            Cap::AniRoundTrip => "`Ani::parse` reads every `.ANI` to the last byte and `to_bytes` gives it back byte for byte; `validate` finds no error",
            Cap::C3dcIndexCount => "`.3DC` files the index lists (index only)",
            Cap::C3dcRoundTrip => "`C3dc::parse` reads every `.3DC` to the last byte with exactly one layout and `to_bytes` gives it back byte for byte",
            Cap::AniPreview => "animation preview: `.ANI` files with at least one compatible `.3DC` (validated offline; by names and bone counts, `meshpick`)",
            Cap::VaniIndexCount => "`.VANI` files the index lists (index only)",
            Cap::VaniRoundTrip => "`Vani::parse` reads every `.VANI` to the last byte and `to_bytes` gives it back byte for byte",
            Cap::ManiIndexCount => "`.MANI` files the index lists (index only)",
            Cap::ManiIndexSize => "every `.MANI` the index lists is 108 bytes (index only)",
            Cap::ManiRoundTrip => "`Mani::parse` reads every `.MANI` (108 bytes) and `to_bytes` gives it back byte for byte",
        }
    }
}

/// The client ids, the author's client first.
pub const CLIENTS: [&str; 9] = ["us-108", "ru-ep3", "ru-ep45", "ru-ep54", "fr-ep54", "phoenix-ep63", "kr-ep8", "fawkes-v68", "cn-ep4"];

use Cell::{Fails as F, FitOnly as FO, NotAvailable as NA, Ok as O, Plain as P};

/// Expected cells, one row per capability in `CAPS` order, one column per client in `CLIENTS` order. `ru-ep54` has an
/// index only (its data.saf is not available) and no game.exe. The two "fails" rows for kr-ep8, fawkes-v68 and cn-ep4 assert that parsing returns
/// an error; a future schema fix shows up as a test asking for this table to be updated.
///
/// Note: the 9 EP8 `DB*Data` binary-header files of kr-ep8 and fawkes-v68 do not survive `seed::unwrap` + CRC
/// (the SEED rows of those two clients may need `Fails` once a run shows it); a run that disagrees must be reported, not forced to pass.
pub const EXPECTED: [[Cell; 9]; 39] = [
    /* IndexParse       */ [O, O, O, O, O, O, O, O, O],
    /* ToBytesIdentical */ [O, O, O, O, O, O, O, O, O],
    /* InspectSah       */ [O, O, O, O, O, O, O, O, O],
    /* SeedUnwrap       */ [O, O, O, NA, O, O, O, O, P],
    /* WrapIdentity     */ [O, O, O, NA, O, O, O, O, O],
    /* Monsters         */ [O, O, O, NA, O, O, F, F, F],
    /* NpcQuest         */ [O, O, O, NA, O, O, F, F, O],
    /* InspectPe        */ [O, O, O, NA, O, O, O, O, O],
    /* ArchiveVisibleCount     */ [O, O, O, O, O, O, O, O, O],
    /* ArchiveCheckIndex       */ [O, O, O, O, O, O, O, O, O],
    /* ArchiveFreeList         */ [O, O, O, O, O, O, O, O, O],
    /* ArchiveDataEnd          */ [O, O, O, O, O, O, O, O, O],
    /* ArchiveDiffSelf         */ [O, O, O, O, O, O, O, O, O],
    /* ArchiveEditRollbackStub */ [O, O, O, O, O, O, O, O, O],
    /* PatchApplyRollbackStub  */ [O, O, O, O, O, O, O, O, O],
    /* SdataBatchCheck         */ [O, O, O, NA, O, O, O, O, P],
    /* SdataLikeRoundtrip      */ [O, O, O, NA, O, O, O, O, O],
    /* WldParse               */ [O, NA, NA, NA, NA, NA, NA, NA, NA],
    /* WldLossless            */ [O, NA, NA, NA, NA, NA, NA, NA, NA],
    /* WldPreview             */ [O, NA, NA, NA, NA, NA, NA, NA, NA],
    /* ModelPreview           */ [O, NA, NA, NA, NA, NA, NA, NA, NA],
    /* AreaTxtRoundtrip       */ [O, NA, NA, NA, NA, NA, NA, NA, NA],
    /* NpcQuestGates          */ [O, O, O, NA, O, O, F, F, O],
    /* MapNameIds             */ [O, O, O, NA, O, O, O, O, O],
    /* SdataItemFits          */ [O, O, O, NA, O, O, FO, FO, FO],
    /* SdataSkillFits         */ [O, O, O, NA, O, O, FO, FO, FO],
    /* SdataMonsterFits       */ [O, O, O, NA, O, O, FO, FO, F],
    /* SdataNpcQuestFits      */ [O, O, O, NA, O, O, FO, FO, FO],
    /* SdataAutoPicks         */ [O, O, O, NA, O, O, O, O, O],
    /* AniIndexCount          */ [O, O, O, O, O, O, O, O, O],
    /* AniRoundTrip           */ [O, NA, NA, NA, NA, NA, NA, NA, NA],
    /* C3dcIndexCount         */ [O, O, O, O, O, O, O, O, O],
    /* C3dcRoundTrip          */ [O, NA, NA, NA, NA, NA, NA, NA, NA],
    /* AniPreview             */ [O, NA, NA, NA, NA, NA, NA, NA, NA],
    /* VaniIndexCount         */ [O, O, O, O, O, O, O, O, O],
    /* VaniRoundTrip          */ [O, NA, NA, NA, NA, NA, NA, NA, NA],
    /* ManiIndexCount         */ [O, O, O, O, O, O, O, O, O],
    /* ManiIndexSize          */ [O, O, O, O, O, O, O, O, O],
    /* ManiRoundTrip          */ [O, NA, NA, NA, NA, NA, NA, NA, NA],
];

/// The numbers the T1 rows assert (the "OK 19,939" style cells of docs/version-matrix.md), in `CLIENTS` order; `None` = a plain "OK" cell.
/// `ArchiveVisibleCount`: files minus shadowed. `ArchiveCheckIndex`: the `shadowed` warnings of the index-only check. `ArchiveFreeList`:
/// free-list entries (preserved by an in-place edit). `ArchiveDataEnd`: `max(offset + length)`, asserted where the table gives it.
/// `WldParse`/`WldLossless`/`WldPreview`: `.wld` files parsed / written back identically / given a map preview (a grey picture for a field, none for a dungeon). `AreaTxtRoundtrip`: `world/*.txt` identical. `NpcQuestGates`: gatekeepers. `ModelPreview`: `.smod` files whose preview data is built (every module and triangle counted, textures looked up by the editor's rule).
/// `MapNameIds`: ids of the map-name table that have text in sysmsg. `SdataItemFits`/`SdataSkillFits`/`SdataMonsterFits`/`SdataNpcQuestFits`:
/// records of `Item/Item.SData`, `Character/Skill.SData`, `Monster/Monster.SData` and `Npc/NpcQuest.SData` (its quests) read by their layout;
/// a `FIT-ONLY` cell is a layout that reads the file but was not read from that client's loader. `SdataAutoPicks`: SData files that exactly one layout reads (the rest are raw).
/// `AniIndexCount`: `.ANI` files the index lists (every client; index only). `AniRoundTrip`: `.ANI` files that read to the last byte, write back byte for byte and have no validation error (only where `data.saf` is present).
/// `C3dcIndexCount`: `.3DC` files the index lists (every client; index only). `C3dcRoundTrip`: `.3DC` files that read to the last byte with exactly one layout and write back byte for byte (only where `data.saf` is present).
/// `AniPreview`: `.ANI` files of the US client that have at least one compatible `.3DC` (by name and bone count); `n/a` without a `data.saf`.
/// `VaniIndexCount`/`ManiIndexCount`: `.VANI`/`.MANI` files the index lists (every client; index only). `ManiIndexSize`: `.MANI` files the index lists with a length of 108 bytes (index only; the cell is `OK` only when it is every one). `VaniRoundTrip`/`ManiRoundTrip`: files that read to the last byte and write back byte for byte (only where `data.saf` is present).
pub const NUMBERS: [(Cap, [Option<u64>; 9]); 26] = [
    (Cap::ArchiveVisibleCount, [Some(19_939), Some(21_523), Some(23_533), Some(24_745), Some(24_599), Some(30_340), Some(34_418), Some(48_118), Some(20_418)]),
    (Cap::ArchiveCheckIndex, [Some(0), Some(0), Some(0), Some(0), Some(0), Some(0), Some(1), Some(1), Some(0)]),
    (Cap::ArchiveFreeList, [Some(0), Some(0), Some(507), Some(500), Some(500), Some(0), Some(0), Some(630), Some(500)]),
    (Cap::ArchiveDataEnd, [None, None, None, None, None, None, None, Some(6_645_531_195), None]),
    (Cap::WldParse, [Some(83), None, None, None, None, None, None, None, None]),
    (Cap::WldLossless, [Some(83), None, None, None, None, None, None, None, None]),
    (Cap::WldPreview, [Some(83), None, None, None, None, None, None, None, None]),
    (Cap::ModelPreview, [Some(1355), None, None, None, None, None, None, None, None]),
    (Cap::AreaTxtRoundtrip, [Some(33), None, None, None, None, None, None, None, None]),
    (Cap::NpcQuestGates, [Some(61), Some(61), Some(72), None, Some(72), Some(106), None, None, Some(61)]),
    (Cap::MapNameIds, [Some(47), Some(47), Some(47), None, Some(47), Some(47), Some(47), Some(47), Some(44)]),
    (Cap::SdataItemFits, [Some(16_045), Some(15_612), Some(16_559), None, Some(17_090), Some(22_697), Some(25_245), Some(24_990), Some(16_047)]),
    (Cap::SdataSkillFits, [Some(978), Some(882), Some(978), None, Some(3_348), Some(6_765), Some(8_775), Some(8_460), Some(978)]),
    (Cap::SdataMonsterFits, [Some(2_474), Some(2_372), Some(2_474), None, Some(2_992), Some(4_511), Some(4_776), Some(4_774), None]),
    (Cap::SdataNpcQuestFits, [Some(1_720), Some(1_531), Some(2_109), None, Some(2_262), Some(3_051), Some(3_820), Some(4_182), Some(1_531)]),
    (Cap::SdataAutoPicks, [Some(5), Some(5), Some(5), None, Some(5), Some(5), Some(6), Some(5), Some(6)]),
    (Cap::AniIndexCount, [Some(3_509), Some(3_537), Some(3_624), Some(3_724), Some(3_723), Some(4_099), Some(4_255), Some(5_951), Some(3_429)]),
    (Cap::AniRoundTrip, [Some(3_509), None, None, None, None, None, None, None, None]),
    (Cap::C3dcIndexCount, [Some(2_431), Some(2_635), Some(3_049), Some(3_164), Some(3_126), Some(4_988), Some(4_220), Some(6_534), Some(2_317)]),
    (Cap::C3dcRoundTrip, [Some(2_431), None, None, None, None, None, None, None, None]),
    (Cap::AniPreview, [Some(3_365), None, None, None, None, None, None, None, None]),
    (Cap::VaniIndexCount, [Some(66), Some(103), Some(116), Some(116), Some(116), Some(129), Some(129), Some(141), Some(74)]),
    (Cap::VaniRoundTrip, [Some(66), None, None, None, None, None, None, None, None]),
    (Cap::ManiIndexCount, [Some(23), Some(39), Some(39), Some(39), Some(39), Some(54), Some(48), Some(64), Some(35)]),
    (Cap::ManiIndexSize, [Some(23), Some(39), Some(39), Some(39), Some(39), Some(54), Some(48), Some(64), Some(35)]),
    (Cap::ManiRoundTrip, [Some(23), None, None, None, None, None, None, None, None]),
];

pub fn expected_number(client: &str, cap: Cap) -> Option<u64> {
    let col = CLIENTS.iter().position(|c| *c == client)?;
    NUMBERS.iter().find(|n| n.0 == cap).and_then(|n| n.1[col])
}

pub fn expected(client: &str, cap: Cap) -> Option<Cell> {
    let col = CLIENTS.iter().position(|c| *c == client)?;
    let row = CAPS.iter().position(|c| *c == cap)?;
    Some(EXPECTED[row][col])
}

/// What one client folder gave.
#[derive(Clone, Debug)]
pub struct Observed {
    pub client: String,
    pub cells: [Cell; 39],
    /// Measured numbers of the T1 rows (see [`NUMBERS`]).
    pub numbers: Vec<(Cap, u64)>,
    /// Counts and other facts printed with the table (not asserted).
    pub facts: Vec<String>,
    pub sah_files: Option<usize>,
    pub sah_millis: Option<f64>,
    pub sdata_files: usize,
    pub sdata_bytes: usize,
    /// Notes on cells that are not OK (file names and error texts).
    pub notes: Vec<String>,
    /// SData files that a layout reads although the layout's `evidence` lists neither `loader` nor `fit_only` for this client
    /// ("<file> = <variant>"). Must be empty: it is the matrix consistency check.
    pub unlisted: Vec<String>,
}

impl Observed {
    pub fn cell(&self, cap: Cap) -> Cell {
        self.cells[CAPS.iter().position(|c| *c == cap).expect("every Cap is in CAPS")]
    }

    pub fn number(&self, cap: Cap) -> Option<u64> {
        self.numbers.iter().find(|n| n.0 == cap).map(|n| n.1)
    }

    pub(crate) fn set(&mut self, cap: Cap, c: Cell) {
        self.cells[CAPS.iter().position(|x| *x == cap).expect("every Cap is in CAPS")] = c;
    }
}

fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.is_file() && p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)))
}

/// The SData files of a client: (archive path with `/`, bytes).
pub fn sdata_files(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out = Vec::new();
    if find_ci(dir, "data.saf").is_some() {
        let a = Archive::open(dir).map_err(|e| e.to_string())?;
        for (path, e) in a.index().files() {
            if path.to_ascii_lowercase().ends_with(".sdata") {
                out.push((path, a.read_entry(e).map_err(|e| e.to_string())?));
            }
        }
    } else if let Ok(rd) = std::fs::read_dir(dir.join("files")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.to_ascii_lowercase().ends_with(".sdata") {
                out.push((name.replace("__", "/"), std::fs::read(e.path()).map_err(|e| e.to_string())?));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn by_name<'a>(files: &'a [(String, Vec<u8>)], tail: &str) -> Option<&'a (String, Vec<u8>)> {
    files.iter().find(|(p, _)| p.to_ascii_lowercase().ends_with(&tail.to_ascii_lowercase()))
}

/// Measures one client folder.
pub fn observe(client: &str, dir: &Path) -> Observed {
    let mut o = Observed { client: client.to_string(), cells: [NA; 39], numbers: Vec::new(), facts: Vec::new(), sah_files: None, sah_millis: None, sdata_files: 0, sdata_bytes: 0, notes: Vec::new(), unlisted: Vec::new() };
    let set = |o: &mut Observed, cap: Cap, c: Cell| o.cells[CAPS.iter().position(|x| *x == cap).expect("cap")] = c;

    // the index
    match find_ci(dir, "data.sah").map(|p| (std::fs::read(&p), p)) {
        Some((Ok(raw), path)) => {
            let t = std::time::Instant::now();
            match Index::parse(&raw) {
                Ok(idx) => {
                    o.sah_millis = Some(t.elapsed().as_secs_f64() * 1000.0);
                    o.sah_files = Some(idx.file_count());
                    set(&mut o, Cap::IndexParse, O);
                    set(&mut o, Cap::ToBytesIdentical, if idx.to_bytes() == raw { O } else { F });
                    t1::index_caps(&mut o, &idx, &raw);
                    t10::ani_caps(&mut o, dir, &idx);
                    t10::c3dc_caps(&mut o, dir, &idx);
                    t10::ani_preview_caps(&mut o, dir, &idx);
                    t10::vani_caps(&mut o, dir, &idx);
                    t10::mani_caps(&mut o, dir, &idx);
                }
                Err(e) => {
                    o.notes.push(format!("Index::parse: {e}"));
                    set(&mut o, Cap::IndexParse, F);
                    set(&mut o, Cap::ToBytesIdentical, F);
                    for cap in SAH_CAPS {
                        set(&mut o, cap, F);
                    }
                }
            }
            let c = match inspect(&path, &Options::default(), &Job::silent()) {
                Ok(i) if i.kind == Kind::Sah && i.problems.is_empty() => O,
                Ok(i) => {
                    o.notes.push(format!("inspect data.sah: {:?} {:?}", i.kind, i.problems));
                    F
                }
                Err(e) => {
                    o.notes.push(format!("inspect data.sah: {e}"));
                    F
                }
            };
            set(&mut o, Cap::InspectSah, c);
        }
        _ => {
            set(&mut o, Cap::IndexParse, F);
            set(&mut o, Cap::ToBytesIdentical, F);
            set(&mut o, Cap::InspectSah, F);
            for cap in SAH_CAPS {
                set(&mut o, cap, F);
            }
            o.notes.push("no readable data.sah".into());
        }
    }

    // the SData batch capabilities
    t1::sdata_caps_for(&mut o, dir, find_ci(dir, "data.saf").is_some());

    // the world and map capabilities
    t5::world_caps(&mut o, dir);

    // the SData files
    match sdata_files(dir) {
        Ok(files) if !files.is_empty() => {
            o.sdata_files = files.len();
            o.sdata_bytes = files.iter().map(|f| f.1.len()).sum();
            let (mut plain, mut encrypted, mut bad, mut mismatch) = (0, 0, 0, 0);
            for (name, raw) in &files {
                match shaiya_seed::unwrap(raw, name) {
                    Ok((p, w)) => {
                        match w {
                            Wrapper::Plain => plain += 1,
                            Wrapper::Seed { .. } => encrypted += 1,
                        }
                        if shaiya_seed::wrap(&p, &w) != *raw {
                            mismatch += 1;
                            o.notes.push(format!("wrap(unwrap({name})) differs"));
                        }
                    }
                    Err(e) => {
                        bad += 1;
                        o.notes.push(format!("seed::unwrap: {e}"));
                    }
                }
            }
            set(&mut o, Cap::SeedUnwrap, if bad > 0 { F } else if encrypted == 0 && plain > 0 { P } else { O });
            set(&mut o, Cap::WrapIdentity, if bad == 0 && mismatch == 0 { O } else { F });
            for (cap, tail) in [(Cap::Monsters, "Monster/Monster.SData"), (Cap::NpcQuest, "Npc/NpcQuest.SData")] {
                let c = match by_name(&files, tail) {
                    None => NA,
                    Some((name, raw)) => match shaiya_seed::unwrap(raw, name) {
                        Ok((plain, _)) => {
                            let r = if cap == Cap::Monsters { shaiya_clientdata::Monsters::parse(&plain, name).map(|_| ()) } else { shaiya_clientdata::NpcQuest::parse(&plain, name).map(|_| ()) };
                            match r {
                                Ok(()) => O,
                                Err(e) => {
                                    o.notes.push(format!("{tail}: {e}"));
                                    F
                                }
                            }
                        }
                        Err(_) => F,
                    },
                };
                set(&mut o, cap, c);
            }
            t7::sdata_table_caps(&mut o, &files);
        }
        Ok(_) => {}
        Err(e) => o.notes.push(format!("SData files: {e}")),
    }

    // the program
    if let Some(exe) = find_ci(dir, "game.exe") {
        let c = match inspect(&exe, &Options::default(), &Job::silent()) {
            Ok(i) if i.kind == Kind::Pe => O,
            Ok(i) => {
                o.notes.push(format!("inspect game.exe: {:?}", i.kind));
                F
            }
            Err(e) => {
                o.notes.push(format!("inspect game.exe: {e}"));
                F
            }
        };
        set(&mut o, Cap::InspectPe, c);
    }
    o
}

/// The folders to test: the author's client and the matrix folder's clients that exist.
pub fn present_clients(us: Option<&Path>, root: Option<&Path>) -> Vec<(String, PathBuf)> {
    let mut v = Vec::new();
    if let Some(us) = us {
        v.push(("us-108".to_string(), us.to_path_buf()));
    }
    if let Some(root) = root {
        for id in &CLIENTS[1..] {
            let d = root.join(id);
            if d.is_dir() {
                v.push((id.to_string(), d));
            } else {
                eprintln!("skipped: {id} not found in the test clients folder");
            }
        }
    }
    v
}

/// 19939 -> "19,939".
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The observed table as Markdown (capabilities as rows, clients as columns), with file counts.
pub fn markdown(all: &[Observed]) -> String {
    let mut s = String::from("| Capability |");
    for o in all {
        s.push_str(&format!(" {} |", o.client));
    }
    s.push_str("\n|---|");
    for _ in all {
        s.push_str("---|");
    }
    s.push('\n');
    s.push_str("| index files |");
    for o in all {
        s.push_str(&format!(" {} |", o.sah_files.map_or("n/a".to_string(), |n| n.to_string())));
    }
    s.push('\n');
    for cap in CAPS {
        s.push_str(&format!("| {} |", cap.label()));
        for o in all {
            let n = expected_number(&o.client, cap).and_then(|_| o.number(cap)).filter(|_| matches!(o.cell(cap), O | FO));
            s.push_str(&format!(" {} |", n.map_or(o.cell(cap).label().to_string(), |n| format!("{} {}", o.cell(cap).label(), group(n)))));
        }
        s.push('\n');
    }
    s.push_str("| SData files |");
    for o in all {
        s.push_str(&format!(" {} |", o.sdata_files));
    }
    s.push('\n');
    s
}

/// Differences between what was observed and `EXPECTED`, one line each, with the instruction to update both tables.
pub fn differences(all: &[Observed]) -> Vec<String> {
    let mut out = Vec::new();
    for o in all {
        for cap in CAPS {
            let (want, got) = (expected(&o.client, cap), o.cell(cap));
            if want != Some(got) {
                out.push(format!(
                    "{} / {}: expected {}, observed {}; update EXPECTED and docs/version-matrix.md{}",
                    o.client,
                    cap.label(),
                    want.map_or("(not in the table)", Cell::label),
                    got.label(),
                    if o.notes.is_empty() { String::new() } else { format!(" (notes: {})", o.notes.join("; ")) }
                ));
            }
            if let Some(want) = expected_number(&o.client, cap) {
                if o.number(cap) != Some(want) {
                    out.push(format!(
                        "{} / {}: expected {want}, observed {}; update EXPECTED and docs/version-matrix.md",
                        o.client,
                        cap.label(),
                        o.number(cap).map_or("nothing".to_string(), |n| n.to_string())
                    ));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_sdata::TableKind;
    use shaiya_testkit::{pe_stub, SahBuilder, TempDir};

    fn synthetic_client(dir: &Path, encrypted: bool, with_exe: bool) {
        let item = if encrypted { shaiya_seed::encrypt(b"item table") } else { b"item table".to_vec() };
        SahBuilder::new(0).file(b"Item/Item.SData", &item).file(b"Monster/Monster.SData", &item).write_to(dir);
        if with_exe {
            std::fs::write(dir.join("game.exe"), pe_stub(0x014C, 1)).unwrap();
        }
    }

    #[test]
    fn the_sdata_table_rows_measure_fit_and_raw() {
        // one bank, one record of the 81-byte shape: Item fits; a Skill file that is only text does not
        let mut item = Vec::new();
        for n in [1u32, 1, 1] {
            item.extend_from_slice(&n.to_le_bytes());
        }
        item.push(b'A');
        item.extend_from_slice(&0u32.to_le_bytes());
        item.extend_from_slice(&[7u8; 81]);
        let d = TempDir::new("matrix-tables");
        SahBuilder::new(0)
            .file(b"Item/Item.SData", &shaiya_seed::encrypt(&item))
            .file(b"Character/Skill.SData", &shaiya_seed::encrypt(b"text, not a table"))
            .file(b"Other/Thing.SData", b"also nothing")
            .write_to(d.path());
        let o = observe("ru-ep3", d.path());
        assert_eq!(o.cell(Cap::SdataItemFits), Cell::Ok, "item-81 was read from the ru-ep3 loader: {:?}", o.notes);
        assert_eq!(o.number(Cap::SdataItemFits), Some(1));
        assert_eq!(o.cell(Cap::SdataSkillFits), Cell::Fails);
        assert_eq!(o.number(Cap::SdataSkillFits), None);
        assert_eq!((o.cell(Cap::SdataMonsterFits), o.cell(Cap::SdataNpcQuestFits)), (Cell::NotAvailable, Cell::NotAvailable), "no Monster or NpcQuest file in this client");
        assert_eq!((o.cell(Cap::SdataAutoPicks), o.number(Cap::SdataAutoPicks)), (Cell::Ok, Some(1)), "one file fits, two stay raw");
        assert!(o.facts.iter().any(|f| f.contains("1 fit one layout, 2 raw, 0 ambiguous")), "{:?}", o.facts);
        assert!(o.unlisted.is_empty(), "{:?}", o.unlisted);
        // the same file on cn-ep4 is a fit without a loader read: FIT-ONLY, and the cell says so with the number
        let cn = observe("cn-ep4", d.path());
        assert_eq!((cn.cell(Cap::SdataItemFits), cn.number(Cap::SdataItemFits)), (Cell::FitOnly, Some(1)));
        assert!(cn.unlisted.is_empty(), "cn-ep4 is listed in fit_only of item-81: {:?}", cn.unlisted);
    }

    #[test]
    fn a_fit_only_cell_prints_as_fit_only_with_its_number_and_a_layout_that_does_not_list_the_client_is_flagged() {
        // a 134-byte-block Item table: only the FIT-ONLY layout item-134 reads it (listed for kr-ep8 and fawkes-v68)
        let mut item = Vec::new();
        for n in [1u32, 1, 1] {
            item.extend_from_slice(&n.to_le_bytes());
        }
        item.push(b'A');
        item.extend_from_slice(&0u32.to_le_bytes());
        item.extend_from_slice(&[7u8; 134]);
        let d = TempDir::new("matrix-fit-only");
        SahBuilder::new(0).file(b"Item/Item.SData", &shaiya_seed::encrypt(&item)).write_to(d.path());
        let kr = observe("kr-ep8", d.path());
        assert_eq!((kr.cell(Cap::SdataItemFits), kr.number(Cap::SdataItemFits)), (Cell::FitOnly, Some(1)));
        assert!(kr.unlisted.is_empty(), "{:?}", kr.unlisted);
        assert!(kr.facts.iter().any(|f| f.contains("layout item-134 (fit-only), 1 records")), "{:?}", kr.facts);
        let md = markdown(&[kr]);
        assert!(md.contains("| sdata `Item` layout fits | FIT-ONLY 1 |"), "{md}");
        // a client that is not in item-134's evidence is listed as unlisted: the consistency test would fail on it
        let ru = observe("ru-ep3", d.path());
        assert_eq!(ru.cell(Cap::SdataItemFits), Cell::FitOnly);
        assert_eq!(ru.unlisted, ["Item/Item.SData = item-134"]);
    }

    #[test]
    fn the_expected_table_marks_every_fit_without_a_loader_as_fit_only() {
        // the expected cells follow the layouts' evidence: OK where the client is a loader client of the layout that reads it
        // (the matrix test checks the layout that reads the real file), FIT-ONLY for kr-ep8, fawkes-v68 and cn-ep4
        for cap in [Cap::SdataItemFits, Cap::SdataSkillFits, Cap::SdataNpcQuestFits] {
            for id in ["kr-ep8", "fawkes-v68", "cn-ep4"] {
                assert_eq!(expected(id, cap), Some(Cell::FitOnly), "{id} {}", cap.label());
            }
        }
        assert_eq!(expected("kr-ep8", Cap::SdataMonsterFits), Some(Cell::FitOnly));
        assert_eq!(expected("cn-ep4", Cap::SdataMonsterFits), Some(Cell::Fails), "cn-ep4 Monster has no fit");
        assert_eq!(expected("ru-ep54", Cap::SdataItemFits), Some(Cell::NotAvailable));
        assert_eq!(Cell::FitOnly.label(), "FIT-ONLY");
    }

    #[test]
    fn layout_evidence_names_only_matrix_clients_and_every_expected_cell_agrees_with_it() {
        let by_cap = [(Cap::SdataItemFits, TableKind::Item), (Cap::SdataSkillFits, TableKind::Skill), (Cap::SdataMonsterFits, TableKind::Monster), (Cap::SdataNpcQuestFits, TableKind::NpcQuest)];
        for l in shaiya_sdata::layouts() {
            for c in l.evidence.loader.iter().chain(&l.evidence.fit_only) {
                assert!(CLIENTS.contains(&c.as_str()), "{}: {c} is not a matrix client id", l.variant);
            }
            assert!(!l.evidence.loader.contains(&"ru-ep54".to_string()) && !l.evidence.fit_only.contains(&"ru-ep54".to_string()), "{}: ru-ep54 has no SData", l.variant);
        }
        // for each client and table, a cell is OK only if some layout of the table lists the client under loader, FIT-ONLY only if
        // some layout lists it (under fit_only) and none under loader
        for (cap, kind) in by_cap {
            for id in CLIENTS {
                let loader = shaiya_sdata::layouts().iter().any(|l| l.table == kind && l.evidence.loader.iter().any(|c| c == id));
                let fit = shaiya_sdata::layouts().iter().any(|l| l.table == kind && l.evidence.fit_only.iter().any(|c| c == id));
                match expected(id, cap).unwrap() {
                    Cell::Ok => assert!(loader, "{id} {}: OK needs a loader client", cap.label()),
                    Cell::FitOnly => assert!(fit && !loader, "{id} {}: FIT-ONLY needs a fit_only client and no loader layout", cap.label()),
                    Cell::Fails | Cell::NotAvailable => assert!(!loader && !fit, "{id} {}: a layout lists this client but the cell says {:?}", cap.label(), expected(id, cap)),
                    Cell::Plain => unreachable!(),
                }
            }
        }
    }

    #[test]
    fn expected_table_shape_and_lookup() {
        assert_eq!(expected("us-108", Cap::SeedUnwrap), Some(Cell::Ok));
        assert_eq!(expected("cn-ep4", Cap::SeedUnwrap), Some(Cell::Plain));
        assert_eq!(expected("kr-ep8", Cap::Monsters), Some(Cell::Fails));
        assert_eq!(expected("ru-ep54", Cap::InspectPe), Some(Cell::NotAvailable));
        assert_eq!(expected("nope", Cap::IndexParse), None);
        assert_eq!(EXPECTED.len(), CAPS.len());
        assert_eq!(CLIENTS[0], "us-108");
        assert_eq!(&CLIENTS[1..], &shaiya_testkit::TEST_CLIENT_IDS);
    }

    #[test]
    fn a_synthetic_full_client_is_observed() {
        let d = TempDir::new("matrix-enc");
        synthetic_client(d.path(), true, true);
        let o = observe("synthetic", d.path());
        assert_eq!(o.cell(Cap::IndexParse), Cell::Ok);
        assert_eq!(o.cell(Cap::ToBytesIdentical), Cell::Ok);
        assert_eq!(o.cell(Cap::InspectSah), Cell::Ok);
        assert_eq!(o.cell(Cap::SeedUnwrap), Cell::Ok);
        assert_eq!(o.cell(Cap::WrapIdentity), Cell::Ok);
        assert_eq!(o.cell(Cap::Monsters), Cell::Fails, "the fake body is not a monster table");
        assert_eq!(o.cell(Cap::NpcQuest), Cell::NotAvailable, "no NpcQuest file");
        assert_eq!(o.cell(Cap::InspectPe), Cell::Ok);
        assert_eq!((o.sah_files, o.sdata_files), (Some(2), 2));
        for cap in [Cap::ArchiveVisibleCount, Cap::ArchiveCheckIndex, Cap::ArchiveFreeList, Cap::ArchiveDataEnd, Cap::ArchiveDiffSelf, Cap::ArchiveEditRollbackStub, Cap::PatchApplyRollbackStub, Cap::SdataBatchCheck, Cap::SdataLikeRoundtrip] {
            assert_eq!(o.cell(cap), Cell::Ok, "{}: {:?}", cap.label(), o.notes);
        }
        assert_eq!((o.number(Cap::ArchiveVisibleCount), o.number(Cap::ArchiveCheckIndex), o.number(Cap::ArchiveFreeList)), (Some(2), Some(0), Some(0)));
        assert!(o.sah_millis.is_some());
    }

    #[test]
    fn numbers_are_grouped_and_shown_for_the_asserted_cells() {
        assert_eq!((group(0), group(999), group(1000), group(19_939), group(6_645_531_195)), ("0".into(), "999".into(), "1,000".into(), "19,939".into(), "6,645,531,195".into()));
        let d = TempDir::new("matrix-numbers");
        synthetic_client(d.path(), true, true);
        let o = observe("us-108", d.path());
        let md = markdown(std::slice::from_ref(&o));
        assert!(md.contains("| `archive_visible_count` | OK 2 |") && md.contains("| `archive_data_end` | OK |"), "{md}");
        let diff = differences(std::slice::from_ref(&o));
        assert!(diff.iter().any(|l| l.contains("`archive_visible_count`: expected 19939, observed 2")), "{diff:?}");
    }

    #[test]
    fn plain_clients_and_missing_pieces() {
        let d = TempDir::new("matrix-plain");
        synthetic_client(d.path(), false, false);
        let o = observe("plain", d.path());
        assert_eq!(o.cell(Cap::SeedUnwrap), Cell::Plain);
        assert_eq!(o.cell(Cap::WrapIdentity), Cell::Ok);
        assert_eq!(o.cell(Cap::InspectPe), Cell::NotAvailable);
        // an empty folder: nothing parses, nothing is available
        let e = TempDir::new("matrix-empty");
        let o = observe("empty", e.path());
        assert_eq!(o.cell(Cap::IndexParse), Cell::Fails);
        assert_eq!(o.cell(Cap::SeedUnwrap), Cell::NotAvailable);
    }

    #[test]
    fn the_fetched_layout_with_files_is_read() {
        let d = TempDir::new("matrix-files");
        let (sah, _) = SahBuilder::new(1).file(b"Item/Item.SData", b"x").build();
        std::fs::write(d.path().join("data.sah"), sah).unwrap();
        let files = d.path().join("files");
        std::fs::create_dir_all(&files).unwrap();
        std::fs::write(files.join("Item__Item.SData"), shaiya_seed::encrypt(b"abc")).unwrap();
        std::fs::write(files.join("sysmsg-uni.txt"), b"not sdata").unwrap();
        let o = observe("fetched", d.path());
        assert_eq!(o.sdata_files, 1);
        assert_eq!(o.cell(Cap::SeedUnwrap), Cell::Ok);
        assert_eq!(sdata_files(d.path()).unwrap()[0].0, "Item/Item.SData");
    }

    #[test]
    fn a_damaged_sdata_is_a_failure_and_differences_are_reported() {
        let d = TempDir::new("matrix-bad");
        let mut enc = shaiya_seed::encrypt(b"some bytes to damage");
        enc[70] ^= 1;
        SahBuilder::new(0).file(b"Item/Item.SData", &enc).write_to(d.path());
        let o = observe("us-108", d.path());
        assert_eq!(o.cell(Cap::SeedUnwrap), Cell::Fails);
        let diff = differences(std::slice::from_ref(&o));
        assert!(diff.iter().any(|l| l.contains("us-108 / `seed::unwrap` + CRC, every SData: expected OK, observed fails") && l.contains("update EXPECTED and docs/version-matrix.md")), "{diff:?}");
        let md = markdown(&[o]);
        assert!(md.starts_with("| Capability | us-108 |") && md.contains("| `Index::parse` | OK |") && md.contains("| SData files | 1 |"), "{md}");
    }

    #[test]
    fn present_clients_lists_only_what_exists() {
        let root = TempDir::new("matrix-root");
        std::fs::create_dir_all(root.path().join("ru-ep3")).unwrap();
        let v = present_clients(Some(Path::new("Z:/us")), Some(root.path()));
        assert_eq!(v.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), vec!["us-108", "ru-ep3"]);
        assert!(present_clients(None, None).is_empty());
    }
}
