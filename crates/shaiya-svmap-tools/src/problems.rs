//! Problems beyond one map: the folder-wide check (every map of the open folder, file rules + cross-map rules + client rules, on
//! worker threads; open maps use their live copy) and the grouping and filtering the Problems tab does.
//! The totals are the ones `shaiya svmap check <folder> --client <client>` prints.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use shaiya_svmap::{validate, validate_cross, Finding, Severity, Svmap};

use crate::client_rules::ClientRules;
use crate::cross_rules::{self, CrossFinding};
use crate::server_data::ServerData;
use crate::keys::TargetServer;

/// Where a map of the folder comes from.
pub enum Source {
    File(PathBuf),
    /// An open tab's map as it is now (unsaved edits included).
    Live(Arc<Svmap>),
}

pub struct MapReport {
    pub read_error: Option<String>,
    pub findings: Vec<Finding>,
}

/// The server data folder of a folder check and what it is judged for.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerCfg {
    pub dir: PathBuf,
    pub afterglow: Option<PathBuf>,
    pub target: TargetServer,
}

impl ServerCfg {
    /// What the loaded data depends on (the target only changes severities, so it is not part of the key).
    pub fn key(&self) -> String {
        format!("{}|{}", self.dir.display(), self.afterglow.as_ref().map_or(String::new(), |p| p.display().to_string()))
    }
}

#[derive(Default)]
pub struct FolderResult {
    pub maps: BTreeMap<u32, MapReport>,
    /// (tab uid, `edit_gen`) of the tabs when the check started; the result is stale when they moved.
    pub gens: Vec<(u64, u64)>,
    /// The "Server and client" findings (empty without a server data folder).
    pub cross: Vec<CrossFinding>,
    /// The server data the cross rules read, with its key (so the next check can reuse it).
    pub server: Option<(String, Arc<ServerData>)>,
}

impl FolderResult {
    /// (errors, warnings, info) over all maps and the cross findings; a map that could not be read counts as one error.
    pub fn totals(&self) -> (usize, usize, usize) {
        let mut t = cross_rules::tally(&self.cross);
        for r in self.maps.values() {
            t.0 += r.read_error.is_some() as usize;
            let (e, w, i) = shaiya_svmap::tally(&r.findings);
            t.0 += e;
            t.1 += w;
            t.2 += i;
        }
        t
    }

    /// One map's counts, with the cross findings that name it.
    pub fn tally_of(&self, id: u32) -> Option<(usize, usize, usize)> {
        let mut t = shaiya_svmap::tally(&self.maps.get(&id)?.findings);
        for f in self.cross.iter().filter(|f| f.map == Some(id)) {
            match f.finding.severity {
                Severity::Error => t.0 += 1,
                Severity::Warning => t.1 += 1,
                Severity::Info => t.2 += 1,
            }
        }
        Some(t)
    }
}

/// `2 E · 5 W` for the map list (infos are not counted there); empty when the map is clean.
pub fn badge(t: (usize, usize, usize)) -> String {
    match (t.0, t.1) {
        (0, 0) => String::new(),
        (e, 0) => format!("{e} E"),
        (0, w) => format!("{w} W"),
        (e, w) => format!("{e} E · {w} W"),
    }
}

/// Checks every map. The files are read first (so a portal's destination can be looked up), then the maps are checked on a few
/// threads. `rules` adds the client rules.
/// With a server data folder the "Server and client" rules run too. `cached` is data loaded for the same key by an earlier check.
pub fn check_folder(sources: Vec<(u32, Source)>, rules: Option<&ClientRules>, server: Option<&ServerCfg>, cached: Option<(String, Arc<ServerData>)>) -> FolderResult {
    let mut loaded: BTreeMap<u32, Result<Arc<Svmap>, String>> = BTreeMap::new();
    for (id, s) in sources {
        let m = match s {
            Source::Live(m) => Ok(m),
            Source::File(p) => std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display())).and_then(|b| shaiya_svmap::read(&b).map_err(|e| format!("{}: {e}", p.display()))).map(Arc::new),
        };
        loaded.insert(id, m);
    }
    let ids: Vec<u32> = loaded.keys().copied().collect();
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4);
    let loaded_ref = &loaded;
    let mut parts: Vec<(u32, MapReport)> = Vec::new();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let Some(id) = ids.get(k).copied() else { break };
                        let report = match &loaded_ref[&id] {
                            Err(e) => MapReport { read_error: Some(e.clone()), findings: Vec::new() },
                            Ok(m) => {
                                let mut f = validate(m);
                                f.extend(validate_cross(m, &|to| loaded_ref.get(&to).and_then(|r| r.as_ref().ok()).map(|a| &**a)));
                                if let Some(r) = rules {
                                    f.extend(r.check(Some(id), m));
                                }
                                f.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.section.cmp(&b.section)).then(a.index.cmp(&b.index)));
                                MapReport { read_error: None, findings: f }
                            }
                        };
                        mine.push((id, report));
                    }
                    mine
                })
            })
            .collect();
        for h in handles {
            if let Ok(v) = h.join() {
                parts.extend(v);
            }
        }
    });
    let mut result = FolderResult { maps: parts.into_iter().collect(), gens: Vec::new(), cross: Vec::new(), server: None };
    if let Some(cfg) = server {
        let key = cfg.key();
        let data = match cached {
            Some((k, d)) if k == key => d,
            _ => Arc::new(ServerData::load(&cfg.dir, cfg.afterglow.as_deref())),
        };
        let maps: BTreeMap<u32, Arc<Svmap>> = loaded.iter().filter_map(|(id, m)| m.as_ref().ok().map(|m| (*id, m.clone()))).collect();
        result.cross = cross_rules::check(&cross_rules::Inputs { maps: &maps, client: rules, server: Some(&data), target: cfg.target });
        result.server = Some((key, data));
    }
    result
}

/// The findings (indices into `findings`) that pass the severity switches and the text filter, grouped by rule: the group with
/// the worst severity first, then by code. Without grouping, one group with an empty code holds everything in the given order.
pub fn grouped<'a>(findings: &'a [Finding], show: [bool; 3], text: &str, group: bool) -> Vec<(&'a str, Vec<usize>)> {
    let q = text.trim().to_lowercase();
    let pass = |f: &Finding| {
        let k = match f.severity {
            Severity::Error => 0,
            Severity::Warning => 1,
            Severity::Info => 2,
        };
        show[k] && (q.is_empty() || matches_text(f, &q))
    };
    let idx: Vec<usize> = findings.iter().enumerate().filter(|(_, f)| pass(f)).map(|(i, _)| i).collect();
    if !group {
        return vec![("", idx)];
    }
    let mut by: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for i in idx {
        by.entry(findings[i].code).or_default().push(i);
    }
    let mut v: Vec<(&str, Vec<usize>)> = by.into_iter().collect();
    v.sort_by(|a, b| {
        let sa = a.1.iter().map(|i| findings[*i].severity).max();
        let sb = b.1.iter().map(|i| findings[*i].severity).max();
        sb.cmp(&sa).then(a.0.cmp(b.0))
    });
    v
}

fn matches_text(f: &Finding, q: &str) -> bool {
    let idx = f.index.map_or(String::new(), |i| format!("#{i}"));
    let hay = format!("{} {} {} {}", f.code, f.message, f.section.label(), idx).to_lowercase();
    q.split_whitespace().all(|w| hay.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_svmap::SectionId;

    fn f(sev: Severity, code: &'static str, index: usize, msg: &str) -> Finding {
        Finding { severity: sev, section: SectionId::MobAreas, index: Some(index), sub: None, code, message: msg.to_string() }
    }

    #[test]
    fn grouping_and_the_text_filter() {
        let fs = vec![
            f(Severity::Warning, "area-count-high", 1, "entry 0: count 12"),
            f(Severity::Error, "box-inverted", 2, "area: min x"),
            f(Severity::Warning, "area-count-high", 3, "entry 0: count 9"),
            f(Severity::Info, "area-empty", 4, "no entries"),
        ];
        let g = grouped(&fs, [true, true, true], "", true);
        assert_eq!(g.iter().map(|(c, v)| (*c, v.len())).collect::<Vec<_>>(), vec![("box-inverted", 1), ("area-count-high", 2), ("area-empty", 1)]);
        // info switched off, and a text filter on the message and on the index
        let g = grouped(&fs, [true, true, false], "", true);
        assert_eq!(g.len(), 2);
        let g = grouped(&fs, [true, true, true], "count 12", true);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].1, vec![0]);
        assert_eq!(grouped(&fs, [true, true, true], "#3", false)[0].1, vec![2]);
        // ungrouped keeps the order
        assert_eq!(grouped(&fs, [true, true, true], "", false)[0].1, vec![0, 1, 2, 3]);
        assert_eq!(badge((2, 5, 9)), "2 E · 5 W");
        assert_eq!(badge((0, 5, 9)), "5 W");
        assert_eq!(badge((0, 0, 9)), "");
    }

    #[test]
    fn folder_check_uses_live_maps_and_reports_unreadable_files() {
        let dir = crate::testutil::TempDir::new("folder-check");
        let mut ok = Svmap::new(64);
        ok.portals.push(shaiya_svmap::Portal { pos: [5.0, 0.0, 5.0], country: 0, level_min: 0, level_max: 0, to_map: 2, to_pos: [9.0, 0.0, 9.0] });
        std::fs::write(dir.join("1.svmap"), shaiya_svmap::write(&ok)).unwrap();
        std::fs::write(dir.join("2.svmap"), b"junk").unwrap();
        std::fs::write(dir.join("3.svmap"), shaiya_svmap::write(&Svmap::new(64))).unwrap();
        // map 3 is open with an unsaved bad area: the live copy counts, not the file
        let mut live = Svmap::new(64);
        live.mob_areas.push(shaiya_svmap::MobArea { min: [9.0, 0.0, 9.0], max: [1.0, 2.0, 1.0], mobs: vec![] });
        let sources = vec![(1, Source::File(dir.join("1.svmap"))), (2, Source::File(dir.join("2.svmap"))), (3, Source::Live(Arc::new(live)))];
        let r = check_folder(sources, None, None, None);
        assert_eq!(r.maps.len(), 3);
        assert!(r.maps[&2].read_error.is_some());
        assert!(r.maps[&3].findings.iter().any(|x| x.code == "box-inverted"));
        // portal 0 of map 1 points at map 2, which is unreadable: it counts as missing, as in the CLI
        assert!(r.maps[&1].findings.iter().any(|x| x.code == "portal-map-missing"));
        let (e, _, _) = r.totals();
        assert!(e >= 2, "the unreadable file and the inverted box are errors");
        assert!(r.tally_of(3).unwrap().0 >= 1 && r.tally_of(9).is_none());
    }

    #[test]
    fn folder_problems_match_cli() {
        // `[both vars]`: the totals the CLI prints for the 77 originals with the client
        let (Some(o), Some(c)) = (std::env::var_os("SVMAP_ORIGINALS"), std::env::var_os("SHAIYA_CLIENT")) else {
            eprintln!("skipped: SVMAP_ORIGINALS and SHAIYA_CLIENT not set");
            return;
        };
        let (o, c) = (PathBuf::from(o), PathBuf::from(c));
        let rules = ClientRules::open(&c).unwrap();
        let mut sources = Vec::new();
        for e in std::fs::read_dir(&o).unwrap().flatten() {
            let p = e.path();
            if let Some(id) = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) {
                if p.extension().is_some_and(|x| x == "svmap") {
                    sources.push((id, Source::File(p)));
                }
            }
        }
        assert_eq!(sources.len(), 77);
        let t0 = std::time::Instant::now();
        let r = check_folder(sources, Some(&rules), None, None);
        eprintln!("folder problems on the 77 originals with the client: {:?} in {:?}", r.totals(), t0.elapsed());
        assert_eq!(r.totals(), (0, 146, 123));
    }
}
