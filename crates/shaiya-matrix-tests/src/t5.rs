//! The world and map capabilities of the matrix: `.wld` parsing and lossless writing, `world/<id>.txt` line-preserving writing,
//! gatekeeper destinations and the map-name ids. Everything is read: a full client (`data.saf`) through `Archive`, a fetched client
//! through `files/<archive path with "/" replaced by "__">`. A client without the world files gets `n/a` on the `.wld` and
//! `.txt` rows (the eight test clients hold SData files and `sysmsg-uni.txt` only).

use crate::{Cap, Cell, Observed};
use shaiya_archive::Archive;
use shaiya_clientdata::area_txt::AreaTxt;
use shaiya_clientdata::text::{map_name_msg, SysMsg};
use shaiya_clientdata::wld::Wld;
use shaiya_clientdata::wld_raw::WldRaw;
use shaiya_clientdata::NpcQuest;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use Cell::{Fails as F, NotAvailable as NA, Ok as O};

/// The readable files of one client folder, by archive path (with `/`).
enum Files {
    Archive(Box<Archive>),
    Folder(Vec<(String, PathBuf)>),
    None,
}

impl Files {
    fn open(dir: &Path) -> Files {
        let has = |name: &str| {
            std::fs::read_dir(dir).ok().is_some_and(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name)))
        };
        if has("data.saf") {
            return match Archive::open(dir) {
                Ok(a) => Files::Archive(Box::new(a)),
                Err(_) => Files::None,
            };
        }
        match std::fs::read_dir(dir.join("files")) {
            Ok(rd) => Files::Folder(rd.flatten().map(|e| (e.file_name().to_string_lossy().replace("__", "/"), e.path())).collect()),
            Err(_) => Files::None,
        }
    }

    fn list(&self) -> Vec<String> {
        let mut v: Vec<String> = match self {
            Files::Archive(a) => a.index().files().into_iter().map(|(p, _)| p).collect(),
            Files::Folder(f) => f.iter().map(|(n, _)| n.clone()).collect(),
            Files::None => Vec::new(),
        };
        v.sort();
        v
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        match self {
            Files::Archive(a) => {
                let e = a.index().get(path).ok_or_else(|| format!("{path}: not in the archive"))?;
                a.read_entry(e).map_err(|e| e.to_string())
            }
            Files::Folder(f) => {
                let (_, p) = f.iter().find(|(n, _)| n == path).ok_or_else(|| format!("{path}: not fetched"))?;
                std::fs::read(p).map_err(|e| e.to_string())
            }
            Files::None => Err(format!("{path}: no files")),
        }
    }
}

fn ends_with_ci(p: &str, tail: &str) -> bool {
    p.to_ascii_lowercase().ends_with(&tail.to_ascii_lowercase())
}

/// Sets the five world and map capabilities of `o` from the client folder `dir`.
pub(crate) fn world_caps(o: &mut Observed, dir: &Path) {
    let files = Files::open(dir);
    let list = files.list();

    // every `.wld`, parsed and written back
    let wlds: Vec<&String> = list.iter().filter(|p| ends_with_ci(p, ".wld")).collect();
    if wlds.is_empty() {
        o.set(Cap::WldParse, NA);
        o.set(Cap::WldLossless, NA);
    } else {
        let (mut parsed, mut identical) = (0u64, 0u64);
        for p in &wlds {
            match files.read(p) {
                Ok(bytes) => {
                    match Wld::parse(&bytes, p) {
                        Ok(_) => parsed += 1,
                        Err(e) => o.notes.push(format!("Wld::parse {p}: {e}")),
                    }
                    match WldRaw::parse(&bytes, p) {
                        Ok(raw) if raw.to_bytes() == bytes => identical += 1,
                        Ok(_) => o.notes.push(format!("wld_raw {p}: written back differently")),
                        Err(e) => o.notes.push(format!("wld_raw {p}: {e}")),
                    }
                }
                Err(e) => o.notes.push(e),
            }
        }
        let total = wlds.len() as u64;
        o.set(Cap::WldParse, if parsed == total { O } else { F });
        o.set(Cap::WldLossless, if identical == total { O } else { F });
        o.numbers.push((Cap::WldParse, parsed));
        o.numbers.push((Cap::WldLossless, identical));
        o.facts.push(format!("world: {total} .wld files, {parsed} parsed, {identical} written back identically"));
    }

    // every `world/*.txt`
    let txts: Vec<&String> = list
        .iter()
        .filter(|p| p.rsplit_once('/').is_some_and(|(d, _)| d.eq_ignore_ascii_case("world")) && ends_with_ci(p, ".txt"))
        .collect();
    if txts.is_empty() {
        o.set(Cap::AreaTxtRoundtrip, NA);
    } else {
        let mut identical = 0u64;
        for p in &txts {
            match files.read(p) {
                Ok(bytes) if AreaTxt::parse(&bytes).to_bytes() == bytes => identical += 1,
                Ok(_) => o.notes.push(format!("area_txt {p}: written back differently")),
                Err(e) => o.notes.push(e),
            }
        }
        o.set(Cap::AreaTxtRoundtrip, if identical == txts.len() as u64 { O } else { F });
        o.numbers.push((Cap::AreaTxtRoundtrip, identical));
        o.facts.push(format!("world: {} .txt files, {identical} written back identically", txts.len()));
    }

    // the gatekeepers of NpcQuest.SData
    match list.iter().find(|p| ends_with_ci(p, "Npc/NpcQuest.SData")) {
        None => o.set(Cap::NpcQuestGates, NA),
        Some(name) => {
            let parsed = files
                .read(name)
                .and_then(|raw| shaiya_seed::unwrap(&raw, name).map_err(|e| e.to_string()))
                .and_then(|(plain, _)| NpcQuest::parse(&plain, name).map_err(|e| e.to_string()));
            match parsed {
                Err(e) => {
                    o.notes.push(format!("{name}: {e}"));
                    o.set(Cap::NpcQuestGates, F);
                }
                Ok(q) => {
                    let keepers = q.lists.get(1).map_or(&[][..], |l| l.as_slice());
                    let good = keepers.iter().all(|n| n.gates.len() == 3 && n.gates.iter().all(|g| g.map <= 255 && g.pos.iter().all(|v| v.is_finite())));
                    if !good {
                        o.notes.push(format!("{name}: a gatekeeper has other than 3 gates, a map id above 255 or a position that is not finite"));
                    }
                    o.set(Cap::NpcQuestGates, if good { O } else { F });
                    o.numbers.push((Cap::NpcQuestGates, keepers.len() as u64));
                    o.facts.push(format!("gatekeepers: {}", keepers.len()));
                }
            }
        }
    }

    // the map-name ids in sysmsg-uni.txt
    match list.iter().find(|p| p.eq_ignore_ascii_case("sysmsg-uni.txt") || ends_with_ci(p, "/sysmsg-uni.txt")) {
        None => o.set(Cap::MapNameIds, NA),
        Some(name) => match files.read(name).map_err(|e| e.to_string()).and_then(|raw| SysMsg::parse(&raw, name).map_err(|e| e.to_string())) {
            Err(e) => {
                o.notes.push(format!("{name}: {e}"));
                o.set(Cap::MapNameIds, F);
            }
            Ok(msgs) => {
                let ids: BTreeSet<u32> = (0..=255u32).filter_map(map_name_msg).collect();
                let with_text = ids.iter().filter(|id| msgs.get(**id).is_some_and(|t| !t.trim().is_empty())).count() as u64;
                o.set(Cap::MapNameIds, O);
                o.numbers.push((Cap::MapNameIds, with_text));
                o.facts.push(format!("map-name table: {} ids, {with_text} with text in sysmsg", ids.len()));
            }
        },
    }
}
