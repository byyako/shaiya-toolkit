//! The SData table rows of the matrix: whether the `Item`, `Skill`, `Monster` and `NpcQuest` layouts read the client's own files to
//! the last byte (with the number of records), and whether `auto` picks exactly one layout, or none, for every SData file. A file
//! that no layout fits is shown raw by the viewer; that is a valid measured result, not an error. A fit is `OK` only when the layout's
//! `evidence` lists the client under `loader`; listed under `fit_only` it is `FIT-ONLY`; a layout that fits a client it does not list
//! at all is recorded in `Observed::unlisted` (the consistency test fails on it).

use crate::{Cap, Cell, Observed};
use shaiya_core::text::Encoding;
use shaiya_sdata::{read, Hint, TableKind};

use Cell::{Fails as F, FitOnly as FO, NotAvailable as NA, Ok as O};

fn by_name<'a>(files: &'a [(String, Vec<u8>)], tail: &str) -> Option<&'a (String, Vec<u8>)> {
    files.iter().find(|(p, _)| p.to_ascii_lowercase().ends_with(&tail.to_ascii_lowercase()))
}

pub(crate) fn sdata_table_caps(o: &mut Observed, files: &[(String, Vec<u8>)]) {
    for (cap, kind, tail) in [
        (Cap::SdataItemFits, TableKind::Item, "Item/Item.SData"),
        (Cap::SdataSkillFits, TableKind::Skill, "Character/Skill.SData"),
        (Cap::SdataMonsterFits, TableKind::Monster, "Monster/Monster.SData"),
        (Cap::SdataNpcQuestFits, TableKind::NpcQuest, "Npc/NpcQuest.SData"),
    ] {
        let c = match by_name(files, tail) {
            None => NA,
            Some((name, raw)) => match shaiya_seed::unwrap(raw, name) {
                Err(e) => {
                    o.notes.push(format!("{tail}: {e}"));
                    F
                }
                Ok((plain, _)) => match read(&plain, Hint::Table(kind), Encoding::Cp949) {
                    Ok(t) => {
                        o.numbers.push((cap, t.primary().rows.len() as u64));
                        o.facts.push(format!("{tail}: layout {} ({}), {} records, {} bytes", t.layout.variant, t.layout.status().label(), t.primary().rows.len(), plain.len()));
                        if t.layout.evidence.loader.contains(&o.client) {
                            O
                        } else {
                            FO
                        }
                    }
                    Err(e) => {
                        o.notes.push(format!("{tail}: {e}; stopped: {}", e.tried.iter().map(|(v, at, why)| format!("{v} at {at} ({why})")).collect::<Vec<_>>().join("; ")));
                        F
                    }
                },
            },
        };
        o.set(cap, c);
    }

    // every SData file, `auto`
    let (mut fit, mut raw, mut ambiguous, mut unreadable) = (0u64, 0u64, 0u64, 0u64);
    let mut fitted = Vec::new();
    for (name, bytes) in files {
        match shaiya_seed::unwrap(bytes, name) {
            Err(_) => unreadable += 1,
            Ok((plain, _)) => match read(&plain, Hint::Auto, Encoding::Cp949) {
                Ok(t) => {
                    fit += 1;
                    fitted.push(format!("{name} = {}", t.layout.variant));
                    if !t.layout.evidence.loader.contains(&o.client) && !t.layout.evidence.fit_only.contains(&o.client) {
                        o.unlisted.push(format!("{name} = {}", t.layout.variant));
                    }
                }
                Err(e) if !e.ambiguous.is_empty() => {
                    ambiguous += 1;
                    o.notes.push(format!("{name}: {e}"));
                }
                Err(_) => raw += 1,
            },
        }
    }
    o.facts.push(format!("auto over {} SData files: {fit} fit one layout, {raw} raw, {ambiguous} ambiguous, {unreadable} not unwrapped; fits: {}", files.len(), fitted.join(", ")));
    o.numbers.push((Cap::SdataAutoPicks, fit));
    o.set(Cap::SdataAutoPicks, if ambiguous == 0 { O } else { F });
}
