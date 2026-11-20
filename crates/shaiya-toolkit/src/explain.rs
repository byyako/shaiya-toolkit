//! The four rows every confirmation that writes to a client shows under its text: Changes, Writes, Undo, Game.
//!
//! Pure builders: each takes the plan object the write itself uses (an `EditReport` from `edit::preview` of the very `EditPlan` that is
//! applied, the `Plan`s of the patches that are applied, a `Journal` that is rolled back) and returns text. They read file sizes from
//! the client folder; they write nothing.

use shaiya_archive::cleanup;
use shaiya_archive::client::ClientFiles;
use shaiya_archive::edit::EditReport;
use shaiya_archive::journal::{self, Journal};
use shaiya_core::time::human_size;
use shaiya_inspect::counted;
use shaiya_patch::publish::{FileState, PublishPlan};
use shaiya_patch::Plan;
use std::path::Path;

/// `(label, text)` pairs, in the order they are drawn.
pub type Rows = Vec<(String, String)>;

pub const CHANGES: &str = "Changes";
pub const WRITES: &str = "Writes";
pub const UNDO: &str = "Undo";
pub const GAME: &str = "Game";

/// How many file names a row lists before it says "and N more".
const LISTED: usize = 5;

/// The Game row: on Windows the write refuses while the game holds the archive open; elsewhere that cannot be checked.
pub fn game_text(windows: bool) -> String {
    if windows {
        "must be closed (checked)".to_string()
    } else {
        "cannot be checked on this system; make sure the game is not running from this folder".to_string()
    }
}

fn rows(changes: String, writes: String, undo: String, game: String) -> Rows {
    vec![(CHANGES.into(), changes), (WRITES.into(), writes), (UNDO.into(), undo), (GAME.into(), game)]
}

fn size_of(p: &Path) -> String {
    match std::fs::metadata(p) {
        Ok(m) => human_size(m.len()),
        Err(_) => "size unknown".to_string(),
    }
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn listed(names: &[String]) -> String {
    let mut s = names.iter().take(LISTED).cloned().collect::<Vec<_>>().join(", ");
    if names.len() > LISTED {
        s.push_str(&format!(" and {} more", names.len() - LISTED));
    }
    s
}

/// "Apply pending changes" (Archive tab). `report` is `edit::preview(index, plan)` of the plan that is applied.
pub fn apply_rows(report: &EditReport, files: &ClientFiles) -> Rows {
    let mut changes = format!("added {}, replaced {}, removed {}", report.added.len(), report.replaced.len(), report.removed.len());
    if !report.loose.is_empty() {
        changes.push_str(&format!(", {} written to the client folder", counted(report.loose.len() as u64, "loose file", "loose files")));
    }
    if !report.missing.is_empty() {
        changes.push_str(&format!("; {} not in the archive, skipped", report.missing.len()));
    }
    let mut writes = format!(
        "{} (index replaced, now {}); {}: {} appended (now {}); a journal with a copy of the old index in {}\\",
        name_of(&files.sah),
        size_of(&files.sah),
        name_of(&files.saf),
        human_size(report.bytes_appended),
        size_of(&files.saf),
        journal::DEFAULT_DIR
    );
    if !report.loose.is_empty() {
        let names: Vec<String> = report.loose.iter().map(|l| l.rel.clone()).collect();
        writes.push_str(&format!("; loose: {}", listed(&names)));
    }
    let undo = "Apply & history > Roll back (the journal is made when you apply)".to_string();
    rows(changes, writes, undo, game_text(cfg!(windows)))
}

/// "Apply patches" (Apply & history tab). `plans` are the plans shown in the list and summed here.
pub fn patch_rows(plans: &[Plan], files: &ClientFiles) -> Rows {
    let (mut a, mut r, mut d, mut bytes) = (0, 0, 0, 0u64);
    let mut loose: Vec<String> = Vec::new();
    let mut loose_bytes = 0u64;
    for p in plans {
        a += p.added.len();
        r += p.replaced.len();
        d += p.removed.len();
        bytes += p.bytes;
        for l in &p.loose {
            loose.push(l.target.clone());
            loose_bytes += l.size;
        }
    }
    let mut changes = format!("added {a}, replaced {r}, removed {d}");
    if !loose.is_empty() {
        changes.push_str(&format!(", {} written to the client folder", counted(loose.len() as u64, "loose file", "loose files")));
    }
    let mut writes = format!(
        "{} (index replaced, now {}); {}: {} appended (now {}); one journal per patch in {}\\",
        name_of(&files.sah),
        size_of(&files.sah),
        name_of(&files.saf),
        human_size(bytes),
        size_of(&files.saf),
        journal::DEFAULT_DIR
    );
    if !loose.is_empty() {
        writes.push_str(&format!("; loose: {} ({})", listed(&loose), human_size(loose_bytes)));
    }
    let undo = format!("Apply & history > Roll back, newest first ({} made, one per patch)", counted(plans.len() as u64, "journal", "journals"));
    rows(changes, writes, undo, game_text(cfg!(windows)))
}

/// "Compact data.saf". `size` and `live` are the sizes the Archive tab loaded with the index (not read again when the confirmation
/// opens); `shadowed` counts the shadowed entries; `journals` the changes that can still be rolled back before compacting.
pub fn compact_rows(size: u64, live: u64, shadowed: usize, journals: usize, files: &ClientFiles) -> Rows {
    let changes = format!(
        "{} shrinks from {} to about {} (unused {}); drops {} and the free list",
        name_of(&files.saf),
        human_size(size),
        human_size(live),
        human_size(size.saturating_sub(live)),
        counted(shadowed as u64, "shadowed entry", "shadowed entries")
    );
    let writes = format!(
        "{} and {} are rewritten (needs about {} free on that drive); {}.bak is a copy of the old index",
        name_of(&files.saf),
        name_of(&files.sah),
        human_size(live + (256 << 20)),
        name_of(&files.sah)
    );
    let undo = format!(
        "no undo: compacting makes no journal, and {} can then no longer be rolled back",
        if journals == 1 { "1 earlier change".to_string() } else { format!("{journals} earlier changes") }
    );
    rows(changes, writes, undo, game_text(cfg!(windows)))
}

/// "Roll back" one journal.
pub fn rollback_rows(j: &Journal, files: &ClientFiles) -> Rows {
    let restored = j.loose.iter().filter(|l| l.existed).count();
    let deleted = j.loose.iter().filter(|l| !l.existed).count();
    let changes = format!(
        "{}: the index is restored, {} is cut back from {} to {}, {} restored and {deleted} deleted",
        j.label,
        name_of(&files.saf),
        counted(j.saf_len_after, "byte", "bytes"),
        counted(j.saf_len_before, "byte", "bytes"),
        counted(restored as u64, "loose file", "loose files")
    );
    let mut writes = format!("{} (restored from the journal), {} (cut)", name_of(&files.sah), name_of(&files.saf));
    if !j.loose.is_empty() {
        let names: Vec<String> = j.loose.iter().map(|l| l.rel.clone()).collect();
        writes.push_str(&format!("; loose: {}", listed(&names)));
    }
    let undo = format!("no undo of its own: journal {} stays in {}\\ marked as rolled back", j.id, journal::DEFAULT_DIR);
    rows(changes, writes, undo, game_text(cfg!(windows)))
}

/// "Prune history": the journals that go (all but the newest `keep`, oldest first in `all`).
pub fn prune_rows(all: &[Journal], keep: usize) -> Rows {
    let gone = all.len().saturating_sub(keep);
    let ids: Vec<String> = all.iter().take(gone).map(|j| j.id.clone()).collect();
    let changes = format!("{} deleted, the newest {} kept", counted(gone as u64, "journal", "journals"), keep.min(all.len()));
    let writes = if ids.is_empty() { format!("nothing in {}\\", journal::DEFAULT_DIR) } else { format!("{}\\ loses {}", journal::DEFAULT_DIR, listed(&ids)) };
    let undo = "no undo: the deleted journals cannot be brought back, so those changes can no longer be rolled back".to_string();
    rows(changes, writes, undo, "not involved: only backup folders are deleted, no game file is touched".to_string())
}

/// "Clean up" (Health view). `plan` is the plan [`shaiya_archive::cleanup::clean`] moves (one object feeds both).
pub fn cleanup_rows(plan: &cleanup::Plan) -> Rows {
    let names: Vec<String> = plan.items.iter().flat_map(|l| l.files.clone()).collect();
    let changes = format!("moves {} ({}) out of the client folder: {}", counted(plan.files as u64, "file", "files"), human_size(plan.bytes), listed(&names));
    let writes = format!(
        "a new folder {}\\cleanup-<UTC>\\ in the client folder, with manifest.json written before the first move; each file is renamed into it on the same drive, nothing is copied or deleted",
        journal::DEFAULT_DIR
    );
    let undo = "Put back in Health (File > Manage client folders > Health), cleanup-<UTC>".to_string();
    rows(changes, writes, undo, game_text(cfg!(windows)))
}

/// "Put back" one cleanup folder. `manifest` is the manifest [`shaiya_archive::cleanup::put_back`] works from.
pub fn put_back_rows(manifest: &cleanup::Manifest) -> Rows {
    let names: Vec<String> = manifest.items.iter().filter(|i| i.state != "put-back").map(|i| i.from.clone()).collect();
    let changes = format!("moves {} back to where {}: {}", counted(names.len() as u64, "file", "files"), if names.len() == 1 { "it was" } else { "they were" }, listed(&names));
    let writes = format!(
        "files are renamed inside the client folder; manifest.json in {} is updated as they go. A name that is already taken stops the whole Put back before anything moves",
        manifest.id
    );
    let undo = "Clean up... moves them out again".to_string();
    rows(changes, writes, undo, "not involved: no data.sah or data.saf is opened or replaced".to_string())
}

/// "Use another archive pair" (swap). `other` is the folder or `.sah` that goes in.
pub fn swap_rows(files: &ClientFiles, other: &Path) -> Rows {
    let (o_sah, o_saf) = if other.is_dir() {
        match ClientFiles::locate(other) {
            Ok(f) => (f.sah, f.saf),
            Err(_) => (other.join("data.sah"), other.join("data.saf")),
        }
    } else {
        (other.to_path_buf(), shaiya_archive::saf_of(other))
    };
    let changes = format!("{} and {} ({}, {}) go in place of the client's own pair", name_of(&o_sah), name_of(&o_saf), size_of(&o_sah), size_of(&o_saf));
    let writes = format!(
        "{} and {} are renamed to data.<UTC stamp>.sah/.saf in the same folder ({}, {}), then the other pair is renamed into place",
        name_of(&files.sah),
        name_of(&files.saf),
        size_of(&files.sah),
        size_of(&files.saf)
    );
    let undo = "no journal: the client's own pair stays in the folder as data.<UTC stamp>.sah/.saf; swap it back with Use another archive pair".to_string();
    rows(changes, writes, undo, game_text(cfg!(windows)))
}

/// "Publish": `plan` is the plan [`shaiya_patch::publish::publish`] writes (one object feeds both). Publishing writes into a server
/// folder, never into a client, and makes no backup of the ini: the Undo row says what it was.
pub fn publish_rows(plan: &PublishPlan) -> Rows {
    let new: Vec<_> = plan.copies().collect();
    let new_bytes: u64 = new.iter().map(|f| f.size).sum();
    let already = plan.items.iter().filter(|i| i.patch.state == FileState::AlreadyPublished).count();
    let ini_name = name_of(&plan.ini_path);
    let mut changes = if new.is_empty() { "no file to copy".to_string() } else { format!("copies {} ({})", counted(new.len() as u64, "file", "files"), human_size(new_bytes)) };
    if already > 0 {
        changes.push_str(&format!("; {} already there, left alone", counted(already as u64, "patch", "patches")));
    }
    match (&plan.ini_before, plan.ini_changed) {
        (None, _) => changes.push_str(&format!(
            "; creates {ini_name} with CheckVersion {}, UpdaterVersion {}, PatchFileVersion {}",
            plan.check_version, plan.updater_version, plan.patch_file_version_after
        )),
        (Some(_), true) => changes.push_str(&format!(
            "; {ini_name}: PatchFileVersion {} -> {}",
            plan.patch_file_version_before.unwrap_or(0),
            plan.patch_file_version_after
        )),
        (Some(_), false) => changes.push_str(&format!("; {ini_name} stays as it is")),
    }
    if plan.check_version_changed {
        changes.push_str(&format!("; CheckVersion becomes {}: every player is told to reinstall the game", plan.check_version));
    }
    if plan.updater_version_changed {
        changes.push_str(&format!("; UpdaterVersion becomes {}", plan.updater_version));
    }
    let rel = |p: &Path| p.strip_prefix(&plan.dir).unwrap_or(p).display().to_string();
    let mut targets: Vec<String> = new.iter().map(|f| rel(&f.target)).collect();
    if plan.ini_changed {
        targets.push(format!("{} ({})", rel(&plan.ini_path), if plan.ini_before.is_some() { "replaced" } else { "new" }));
    }
    let writes = format!("in {}: {}; each copy is compared with its original after writing", plan.dir.display(), if targets.is_empty() { "nothing".to_string() } else { listed(&targets) });
    let undo = match (&plan.ini_before, plan.ini_changed) {
        (Some(v), true) => format!(
            "no undo of its own, and no backup: the old {ini_name} had CheckVersion {}, PatchFileVersion {}; delete the copied files and set the ini back by hand. Players who already fetched a patch keep it",
            v.check_version.unwrap_or(0),
            v.patch_file_version.unwrap_or(0)
        ),
        (None, true) => format!("no undo of its own: delete the copied files and {ini_name} by hand. Players who already fetched a patch keep it"),
        _ => "no undo needed for the ini; delete the copied files by hand to take them back. Players who already fetched a patch keep it".to_string(),
    };
    rows(changes, writes, undo, "not involved: this writes into a server folder, never into a client folder".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_archive::edit::{EditReport, LooseResult};
    use shaiya_archive::journal::{LooseRecord, State};
    use shaiya_testkit::{SahBuilder, TempDir};

    fn client() -> (TempDir, ClientFiles) {
        let dir = TempDir::new("explain");
        SahBuilder::new(0).file(b"a/b.txt", b"hello").write_to(dir.path());
        let files = ClientFiles::locate(dir.path()).unwrap();
        (dir, files)
    }

    fn text(r: &Rows, label: &str) -> String {
        r.iter().find(|(l, _)| l == label).map(|(_, t)| t.clone()).unwrap_or_else(|| panic!("no {label} row in {r:?}"))
    }

    fn labels(r: &Rows) -> Vec<&str> {
        r.iter().map(|(l, _)| l.as_str()).collect()
    }

    fn journal(label: &str, loose: Vec<LooseRecord>) -> Journal {
        Journal {
            dir: "j".into(),
            id: "20260101T000000Z-x".into(),
            kind: "patch".into(),
            label: label.into(),
            created_utc: "2026-01-01T00:00:00Z".into(),
            tool: "t".into(),
            sah_name: "data.sah".into(),
            saf_name: "data.saf".into(),
            saf_len_before: 100,
            saf_len_after: 160,
            sah_sha256_before: String::new(),
            sah_sha256_after: String::new(),
            loose,
            state: State::Applied,
        }
    }

    #[test]
    fn game_text_differs_by_platform() {
        assert!(game_text(true).contains("closed (checked)"));
        assert!(game_text(false).contains("cannot be checked on this system"));
    }

    #[test]
    fn apply_rows_count_the_report_and_name_the_files() {
        let (_d, files) = client();
        let rep = EditReport {
            added: vec!["a".into(), "b".into()],
            replaced: vec!["c".into()],
            removed: vec!["d".into()],
            missing: vec!["e".into()],
            loose: vec![LooseResult { rel: "notice.txt".into(), overwrote: false }],
            bytes_appended: 2048,
            ..EditReport::default()
        };
        let r = apply_rows(&rep, &files);
        assert_eq!(labels(&r), ["Changes", "Writes", "Undo", "Game"]);
        let c = text(&r, "Changes");
        assert!(c.contains("added 2, replaced 1, removed 1") && c.contains("1 loose file") && c.contains("1 not in the archive"), "{c}");
        let w = text(&r, "Writes");
        assert!(w.contains("data.sah") && w.contains("data.saf") && w.contains("2.0 KB") && w.contains("notice.txt") && w.contains("shaiya-toolkit-backups"), "{w}");
        assert!(text(&r, "Undo").contains("Apply & history > Roll back"));
    }

    #[test]
    fn patch_rows_sum_the_plans() {
        let (_d, files) = client();
        let plan = |added: usize, bytes: u64| Plan {
            patch: "p".into(),
            added: (0..added).map(|i| format!("f{i}")).collect(),
            replaced: vec!["x".into()],
            removed: Vec::new(),
            missing: Vec::new(),
            loose: vec![shaiya_patch::LoosePlan { name: "n.txt".into(), target: "n.txt".into(), overwrites: false, protected: false, size: 10 }],
            bytes,
            temp_bytes: 0,
        };
        let r = patch_rows(&[plan(2, 1024), plan(3, 1024)], &files);
        let c = text(&r, "Changes");
        assert!(c.contains("added 5, replaced 2, removed 0") && c.contains("2 loose files"), "{c}");
        let w = text(&r, "Writes");
        assert!(w.contains("2.0 KB") && w.contains("one journal per patch"), "{w}");
        assert!(text(&r, "Undo").contains("2 journals"), "{}", text(&r, "Undo"));
    }

    #[test]
    fn compact_rows_say_there_is_no_undo() {
        let (_d, files) = client();
        let r = compact_rows(10_000_000, 4_000_000, 3, 2, &files);
        let c = text(&r, "Changes");
        assert!(c.contains("10.0 MB") && c.contains("4.0 MB") && c.contains("6.0 MB") && c.contains("3 shadowed entries"), "{c}");
        let u = text(&r, "Undo");
        assert!(u.starts_with("no undo") && u.contains("2 earlier changes"), "{u}");
        assert!(text(&compact_rows(1, 1, 0, 1, &files), "Undo").contains("1 earlier change "), "singular");
    }

    #[test]
    fn rollback_rows_name_the_journal_and_the_counts() {
        let (_d, files) = client();
        let loose = vec![
            LooseRecord { rel: "a.txt".into(), existed: true, backup: Some("loose/a.txt".into()), sha256_after: None },
            LooseRecord { rel: "b.txt".into(), existed: false, backup: None, sha256_after: None },
        ];
        let r = rollback_rows(&journal("ps0001.patch", loose), &files);
        let c = text(&r, "Changes");
        assert!(c.contains("ps0001.patch") && c.contains("160 bytes to 100 bytes") && c.contains("1 loose file restored and 1 deleted"), "{c}");
        assert!(text(&r, "Writes").contains("a.txt, b.txt"));
        assert!(text(&r, "Undo").contains("20260101T000000Z-x"));
    }

    #[test]
    fn prune_rows_list_the_journals_that_go() {
        let mut a = journal("a", Vec::new());
        a.id = "1".into();
        let mut b = journal("b", Vec::new());
        b.id = "2".into();
        let mut c = journal("c", Vec::new());
        c.id = "3".into();
        let r = prune_rows(&[a, b, c], 1);
        assert!(text(&r, "Changes").contains("2 journals deleted, the newest 1 kept"), "{}", text(&r, "Changes"));
        assert!(text(&r, "Writes").contains("loses 1, 2"));
        assert!(text(&r, "Undo").starts_with("no undo"));
        assert!(text(&prune_rows(&[], 5), "Writes").starts_with("nothing in"));
    }

    #[test]
    fn publish_rows_describe_the_plan_that_is_written() {
        use shaiya_core::Job;
        use shaiya_patch::publish::{plan_publish, PublishOptions};
        let d = TempDir::new("explain-publish");
        let files = d.path().join("files");
        std::fs::create_dir_all(&files).unwrap();
        std::fs::write(files.join("a.txt"), b"hello").unwrap();
        let patch = d.path().join("ps0110.patch");
        shaiya_patch::build(Some(&files), None, None, &patch).unwrap();
        let dest = d.path().join("server");
        std::fs::create_dir_all(dest.join("Shaiya")).unwrap();
        std::fs::write(dest.join("Shaiya/UpdateVersion.ini"), b"[Version]\r\nCheckVersion=3\r\nUpdaterVersion=1\r\nPatchFileVersion=109\r\n").unwrap();
        let plan = plan_publish(std::slice::from_ref(&patch), &dest, &PublishOptions::default(), &Job::silent()).unwrap();
        let r = publish_rows(&plan);
        assert_eq!(labels(&r), ["Changes", "Writes", "Undo", "Game"]);
        let c = text(&r, "Changes");
        assert!(c.contains("copies 1 file") && c.contains("PatchFileVersion 109 -> 110") && !c.contains("reinstall"), "{c}");
        let w = text(&r, "Writes");
        assert!(w.contains("ps0110.patch") && w.contains("UpdateVersion.ini (replaced)"), "{w}");
        let u = text(&r, "Undo");
        assert!(u.contains("no backup") && u.contains("PatchFileVersion 109"), "{u}");
        assert!(text(&r, "Game").contains("never into a client"));
        // a new ini, and a changed CheckVersion
        let fresh = plan_publish(std::slice::from_ref(&patch), &d.path().join("fresh"), &PublishOptions { check_version: Some(3), ..Default::default() }, &Job::silent()).unwrap();
        assert!(text(&publish_rows(&fresh), "Changes").contains("creates UpdateVersion.ini with CheckVersion 3, UpdaterVersion 1, PatchFileVersion 110"));
        let o = PublishOptions { check_version: Some(4), new_check_version: true, ..Default::default() };
        let changed = plan_publish(&[patch], &dest, &o, &Job::silent()).unwrap();
        assert!(text(&publish_rows(&changed), "Changes").contains("every player is told to reinstall the game"));
    }

    #[test]
    fn cleanup_and_put_back_rows_name_the_files_and_the_way_back() {
        use shaiya_archive::cleanup::{Item, Manifest, Plan};
        use shaiya_archive::health::{Kind, Leftover};
        let l = |kind: Kind, files: &[&str], bytes: u64| Leftover { kind, id: format!("{}:{}", kind.name(), files[0]), files: files.iter().map(|f| f.to_string()).collect(), bytes, pre_selected: false, note: String::new() };
        let plan = Plan { items: vec![l(Kind::SahBak, &["data.sah.bak"], 2048), l(Kind::SwapPair, &["data.x.sah", "data.x.saf"], 2048)], files: 3, bytes: 4096 };
        let r = cleanup_rows(&plan);
        assert_eq!(labels(&r), ["Changes", "Writes", "Undo", "Game"]);
        let c = text(&r, "Changes");
        assert!(c.contains("3 files") && c.contains("4.1 KB") && c.contains("data.sah.bak, data.x.sah, data.x.saf"), "{c}");
        let w = text(&r, "Writes");
        assert!(w.contains("shaiya-toolkit-backups\\cleanup-<UTC>\\") && w.contains("manifest.json") && w.contains("nothing is copied or deleted"), "{w}");
        assert!(text(&r, "Undo").contains("Put back in Health") && text(&r, "Undo").contains("cleanup-<UTC>"));
        let item = |from: &str, state: &str| Item { group: from.into(), kind: "stray-temp".into(), from: from.into(), to: from.into(), bytes: 1, state: state.into() };
        let m = Manifest { id: "cleanup-20261101T000000Z".into(), created_utc: String::new(), tool: String::new(), state: "moved".into(), items: vec![item("a", "moved"), item("b", "put-back")] };
        let r = put_back_rows(&m);
        assert_eq!(labels(&r), ["Changes", "Writes", "Undo", "Game"]);
        assert!(text(&r, "Changes").contains("moves 1 file back to where it was: a"), "{}", text(&r, "Changes"));
        assert!(text(&r, "Writes").contains("cleanup-20261101T000000Z") && text(&r, "Undo").contains("Clean up"));
    }

    #[test]
    fn swap_rows_name_both_pairs() {
        let (_d, files) = client();
        let other = TempDir::new("explain-other");
        SahBuilder::new(0).file(b"x.txt", b"x").write_to(other.path());
        let r = swap_rows(&files, other.path());
        assert!(text(&r, "Changes").contains("data.sah") && text(&r, "Changes").contains("data.saf"));
        assert!(text(&r, "Writes").contains("data.<UTC stamp>"));
        assert!(text(&r, "Undo").contains("no journal"));
    }
}
