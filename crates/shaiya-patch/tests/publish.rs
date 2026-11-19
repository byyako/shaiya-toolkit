//! Publishing for the regular updater: plans (what would be written, what is refused) and the write with its read-back, on synthetic
//! patches built by `shaiya_patch::build`.

use shaiya_core::{Error, Job};
use shaiya_patch::publish::{plan_publish, publish, publish_with, FileState, Layout, PublishOptions, INI_NAME};
use shaiya_testkit::{SahBuilder, TempDir};
use std::fs;
use std::path::{Path, PathBuf};

/// `base/src/psNNNN.patch` holding one archive file whose bytes depend on `tag`.
fn make(base: &Path, number: u32, tag: &str) -> PathBuf {
    let files = base.join(format!("files-{number}-{tag}"));
    fs::create_dir_all(files.join("Item")).unwrap();
    fs::write(files.join("Item/Item.SData"), format!("item data {number} {tag}")).unwrap();
    let dir = base.join(format!("src-{tag}"));
    fs::create_dir_all(&dir).unwrap();
    let out = dir.join(shaiya_patch::patch_name(number));
    shaiya_patch::build(Some(&files), None, None, &out).unwrap();
    out
}

fn opts(check: Option<u32>) -> PublishOptions {
    PublishOptions { check_version: check, ..Default::default() }
}

fn refused(r: shaiya_core::Result<impl std::fmt::Debug>) -> String {
    match r {
        Err(Error::Refused(m)) => m,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn ini_of(dir: &Path) -> Vec<u8> {
    fs::read(dir.join("Shaiya").join(INI_NAME)).unwrap()
}

#[test]
fn the_first_publish_into_an_empty_folder_writes_the_patch_and_a_new_ini() {
    let t = TempDir::new("pub-first");
    let p = make(t.path(), 109, "a");
    let out = t.path().join("server");
    let plan = plan_publish(std::slice::from_ref(&p), &out, &opts(Some(3)), &Job::silent()).unwrap();
    assert!(!out.exists(), "planning writes nothing");
    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.items[0].number, 109);
    assert_eq!(plan.items[0].patch.state, FileState::New);
    assert_eq!((plan.patch_file_version_before, plan.patch_file_version_after), (None, 109));
    assert!(plan.updater_version_defaulted && plan.updater_version == 1);
    assert!(plan.warnings.iter().any(|w| w.contains("UpdaterVersion is set to 1")), "{:?}", plan.warnings);
    let rep = publish(&plan, &Job::silent()).unwrap();
    assert!(rep.read_back);
    assert_eq!(rep.copied.len(), 1);
    assert_eq!(fs::read(out.join("Shaiya/patch/ps0109.patch")).unwrap(), fs::read(&p).unwrap());
    assert_eq!(ini_of(&out), b"[Version]\r\nCheckVersion=3\r\nUpdaterVersion=1\r\nPatchFileVersion=109\r\n");
    let leftovers: Vec<String> = fs::read_dir(out.join("Shaiya/patch")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(leftovers, vec!["ps0109.patch"], "no temporary file stays");
}

#[test]
fn a_new_ini_needs_a_check_version() {
    let t = TempDir::new("pub-needs-check");
    let p = make(t.path(), 5, "a");
    let m = refused(plan_publish(&[p], &t.path().join("server"), &opts(None), &Job::silent()));
    assert!(m.contains("--check-version"), "{m}");
}

#[test]
fn the_next_number_is_appended_and_the_other_lines_stay_byte_for_byte() {
    let t = TempDir::new("pub-append");
    let out = t.path().join("server");
    fs::create_dir_all(out.join("Shaiya/patch")).unwrap();
    let before = b"; our server\r\n[Version]\r\nCheckVersion=3\r\nUpdaterVersion=7\r\nPatchFileVersion=109\r\nNote = keep   \r\n\r\n[Mirror]\r\nHost=example.invalid\r\n";
    fs::write(out.join("Shaiya").join(INI_NAME), before).unwrap();
    let p = make(t.path(), 110, "a");
    let plan = plan_publish(&[p], &out, &opts(None), &Job::silent()).unwrap();
    assert_eq!((plan.patch_file_version_before, plan.patch_file_version_after), (Some(109), 110));
    assert!(!plan.updater_version_defaulted && plan.updater_version == 7);
    publish(&plan, &Job::silent()).unwrap();
    let after = ini_of(&out);
    let expected = String::from_utf8_lossy(before).replace("PatchFileVersion=109", "PatchFileVersion=110");
    assert_eq!(String::from_utf8_lossy(&after), expected);
    assert!(out.join("Shaiya/patch/ps0110.patch").is_file());
}

#[test]
fn a_gap_after_patch_file_version_is_refused() {
    let t = TempDir::new("pub-gap");
    let out = t.path().join("server");
    fs::create_dir_all(out.join("Shaiya")).unwrap();
    fs::write(out.join("Shaiya").join(INI_NAME), b"[Version]\r\nCheckVersion=3\r\nUpdaterVersion=1\r\nPatchFileVersion=109\r\n").unwrap();
    let p = make(t.path(), 111, "a");
    let m = refused(plan_publish(&[p], &out, &opts(None), &Job::silent()));
    assert!(m.contains("gap") && m.contains("ps0110.patch"), "{m}");
    // two new numbers must follow each other too
    let (a, b) = (make(t.path(), 110, "b"), make(t.path(), 112, "b"));
    let m = refused(plan_publish(&[a, b], &out, &opts(None), &Job::silent()));
    assert!(m.contains("gap") && m.contains("ps0111.patch"), "{m}");
}

#[test]
fn several_patches_in_any_order_are_published_in_number_order() {
    let t = TempDir::new("pub-several");
    let out = t.path().join("server");
    fs::create_dir_all(out.join("Shaiya")).unwrap();
    fs::write(out.join("Shaiya").join(INI_NAME), b"[Version]\r\nCheckVersion=3\r\nUpdaterVersion=1\r\nPatchFileVersion=109\r\n").unwrap();
    let (a, b) = (make(t.path(), 110, "c"), make(t.path(), 111, "c"));
    let plan = plan_publish(&[b, a], &out, &opts(Some(3)), &Job::silent()).unwrap();
    assert_eq!(plan.items.iter().map(|i| i.number).collect::<Vec<_>>(), [110, 111]);
    publish(&plan, &Job::silent()).unwrap();
    assert!(String::from_utf8(ini_of(&out)).unwrap().contains("PatchFileVersion=111"));
}

#[test]
fn a_different_check_version_is_refused_unless_said_so() {
    let t = TempDir::new("pub-check");
    let out = t.path().join("server");
    fs::create_dir_all(out.join("Shaiya")).unwrap();
    fs::write(out.join("Shaiya").join(INI_NAME), b"[Version]\r\nCheckVersion=3\r\nUpdaterVersion=1\r\nPatchFileVersion=109\r\n").unwrap();
    let p = make(t.path(), 110, "a");
    let m = refused(plan_publish(std::slice::from_ref(&p), &out, &opts(Some(4)), &Job::silent()));
    assert!(m.contains("reinstall") && m.contains("--new-check-version"), "{m}");
    // the same number is fine
    assert!(plan_publish(std::slice::from_ref(&p), &out, &opts(Some(3)), &Job::silent()).is_ok());
    // allowed on purpose: the plan says so
    let mut o = opts(Some(4));
    o.new_check_version = true;
    let plan = plan_publish(&[p], &out, &o, &Job::silent()).unwrap();
    assert!(plan.check_version_changed);
    assert!(plan.warnings.iter().any(|w| w.contains("told to reinstall")), "{:?}", plan.warnings);
    publish(&plan, &Job::silent()).unwrap();
    assert!(String::from_utf8(ini_of(&out)).unwrap().contains("CheckVersion=4"));
}

#[test]
fn publishing_the_same_patch_again_changes_nothing() {
    let t = TempDir::new("pub-again");
    let out = t.path().join("server");
    let p = make(t.path(), 109, "a");
    publish(&plan_publish(std::slice::from_ref(&p), &out, &opts(Some(3)), &Job::silent()).unwrap(), &Job::silent()).unwrap();
    let ini = ini_of(&out);
    let plan = plan_publish(&[p], &out, &opts(Some(3)), &Job::silent()).unwrap();
    assert!(plan.nothing_to_do());
    assert_eq!(plan.items[0].patch.state, FileState::AlreadyPublished);
    let rep = publish(&plan, &Job::silent()).unwrap();
    assert!(rep.copied.is_empty() && rep.ini_written.is_none() && rep.already_published.len() == 1);
    assert_eq!(ini_of(&out), ini);
}

#[test]
fn a_different_file_under_a_published_name_is_never_replaced() {
    let t = TempDir::new("pub-differs");
    let out = t.path().join("server");
    let p = make(t.path(), 109, "a");
    publish(&plan_publish(&[p], &out, &opts(Some(3)), &Job::silent()).unwrap(), &Job::silent()).unwrap();
    let on_server = fs::read(out.join("Shaiya/patch/ps0109.patch")).unwrap();
    let other = make(t.path(), 109, "b");
    let m = refused(plan_publish(&[other], &out, &opts(Some(3)), &Job::silent()));
    assert!(m.contains("never replaced") && m.contains("next number"), "{m}");
    assert_eq!(fs::read(out.join("Shaiya/patch/ps0109.patch")).unwrap(), on_server);
}

#[test]
fn a_file_name_that_is_not_psnnnn_is_refused() {
    let t = TempDir::new("pub-name");
    let p = make(t.path(), 109, "a");
    let renamed = p.with_file_name("update.patch");
    fs::copy(&p, &renamed).unwrap();
    let m = refused(plan_publish(&[renamed], &t.path().join("server"), &opts(Some(3)), &Job::silent()));
    assert!(m.contains("psNNNN.patch"), "{m}");
    let short = p.with_file_name("ps109.patch");
    fs::copy(&p, &short).unwrap();
    let m = refused(plan_publish(&[short], &t.path().join("server"), &opts(Some(3)), &Job::silent()));
    assert!(m.contains("ps0109.patch"), "{m}");
}

#[test]
fn the_flat_layout_puts_the_ini_and_the_patches_in_the_folder() {
    let t = TempDir::new("pub-flat");
    let out = t.path().join("server");
    let p = make(t.path(), 3, "a");
    let o = PublishOptions { layout: Layout::Flat, check_version: Some(1), updater_version: Some(2), ..Default::default() };
    publish(&plan_publish(&[p], &out, &o, &Job::silent()).unwrap(), &Job::silent()).unwrap();
    assert!(out.join("ps0003.patch").is_file());
    assert_eq!(fs::read(out.join(INI_NAME)).unwrap(), b"[Version]\r\nCheckVersion=1\r\nUpdaterVersion=2\r\nPatchFileVersion=3\r\n");
    assert!(!out.join("Shaiya").exists());
}

#[test]
fn a_client_folder_is_never_a_destination() {
    let t = TempDir::new("pub-client");
    let client = t.path().join("client");
    fs::create_dir_all(&client).unwrap();
    SahBuilder::new(0).file(b"a/b.txt", b"hi").write_to(&client);
    let p = make(t.path(), 109, "a");
    for dest in [client.clone(), client.join("web"), client.join("web").join("deeper")] {
        let m = refused(plan_publish(std::slice::from_ref(&p), &dest, &opts(Some(3)), &Job::silent()));
        assert!(m.contains("client folder"), "{}: {m}", dest.display());
    }
    assert!(!client.join("web").exists() && !client.join("Shaiya").exists(), "nothing was created in the client");
    // a saved client without archive files in it is refused by the list too
    let saved = t.path().join("saved");
    fs::create_dir_all(&saved).unwrap();
    let o = PublishOptions { check_version: Some(3), client_dirs: vec![saved.clone()], ..Default::default() };
    let m = refused(plan_publish(&[p], &saved.join("out"), &o, &Job::silent()));
    assert!(m.contains("client folder"), "{m}");
}

#[test]
fn a_client_below_the_destination_is_never_written_into() {
    let t = TempDir::new("pub-client-below");
    let p = make(t.path(), 109, "a");
    // DIR/Shaiya is a client: the default layout would write DIR/Shaiya/UpdateVersion.ini and DIR/Shaiya/patch/ into it
    let dir = t.path().join("server");
    let client = dir.join("Shaiya");
    fs::create_dir_all(&client).unwrap();
    SahBuilder::new(0).file(b"a/b.txt", b"hi").write_to(&client);
    let before = fs::read_dir(&client).unwrap().count();
    let m = refused(plan_publish(std::slice::from_ref(&p), &dir, &opts(Some(3)), &Job::silent()));
    assert!(m.contains("client folder") && m.contains("Shaiya"), "{m}");
    assert_eq!(fs::read_dir(&client).unwrap().count(), before, "nothing was created in the client");
    // the flat layout writes into DIR itself, which is not a client
    let flat = PublishOptions { layout: Layout::Flat, check_version: Some(3), ..Default::default() };
    assert!(plan_publish(std::slice::from_ref(&p), &dir, &flat, &Job::silent()).is_ok());

    // the patch folder DIR/Shaiya/patch is a client, DIR/Shaiya is not
    let dir2 = t.path().join("server2");
    let inner = dir2.join("Shaiya").join("patch");
    fs::create_dir_all(&inner).unwrap();
    SahBuilder::new(0).file(b"a/b.txt", b"hi").write_to(&inner);
    let m = refused(plan_publish(std::slice::from_ref(&p), &dir2, &opts(Some(3)), &Job::silent()));
    assert!(m.contains("client folder"), "{m}");
    assert!(!dir2.join("Shaiya").join(INI_NAME).exists());

    // a saved client (no archive files in it) in DIR/Shaiya
    let dir3 = t.path().join("server3");
    let saved = dir3.join("Shaiya");
    fs::create_dir_all(&saved).unwrap();
    let o = PublishOptions { check_version: Some(3), client_dirs: vec![saved.clone()], ..Default::default() };
    let m = refused(plan_publish(&[p], &dir3, &o, &Job::silent()));
    assert!(m.contains("client folder"), "{m}");
    assert_eq!(fs::read_dir(&saved).unwrap().count(), 0);
}

#[test]
fn an_ini_changed_during_the_copies_is_not_overwritten_and_the_copied_files_are_named() {
    let t = TempDir::new("pub-window");
    let out = t.path().join("server");
    fs::create_dir_all(out.join("Shaiya")).unwrap();
    let ini = out.join("Shaiya").join(INI_NAME);
    fs::write(&ini, b"[Version]\r\nCheckVersion=3\r\nUpdaterVersion=1\r\nPatchFileVersion=109\r\n").unwrap();
    let p = make(t.path(), 110, "a");
    let plan = plan_publish(std::slice::from_ref(&p), &out, &opts(None), &Job::silent()).unwrap();
    let theirs: &[u8] = b"[Version]\r\nCheckVersion=3\r\nUpdaterVersion=1\r\nPatchFileVersion=115\r\n";
    let m = refused(publish_with(&plan, &Job::silent(), &mut || fs::write(&ini, theirs).unwrap()));
    assert!(m.contains("changed since the plan"), "{m}");
    assert!(m.contains("already copied") && m.contains("ps0110.patch") && m.contains("was not changed"), "{m}");
    assert_eq!(fs::read(&ini).unwrap(), theirs, "the other change was not overwritten");
    assert!(out.join("Shaiya/patch/ps0110.patch").is_file(), "the copied file stays");
    // a new plan finds the copy as already published
    fs::write(&ini, b"[Version]\r\nCheckVersion=3\r\nUpdaterVersion=1\r\nPatchFileVersion=109\r\n").unwrap();
    let again = plan_publish(&[p], &out, &opts(None), &Job::silent()).unwrap();
    assert_eq!(again.items[0].patch.state, FileState::AlreadyPublished);
    publish(&again, &Job::silent()).unwrap();
    assert!(String::from_utf8(ini_of(&out)).unwrap().contains("PatchFileVersion=110"));
}

#[test]
fn a_target_that_appears_during_the_copies_is_refused_and_the_earlier_copies_are_named() {
    let t = TempDir::new("pub-appears");
    let out = t.path().join("server");
    let (a, b) = (make(t.path(), 1, "a"), make(t.path(), 2, "a"));
    let plan = plan_publish(&[a, b], &out, &opts(Some(1)), &Job::silent()).unwrap();
    // ps0002.patch shows up between the plan and its copy
    fs::create_dir_all(out.join("Shaiya/patch")).unwrap();
    fs::write(out.join("Shaiya/patch/ps0002.patch"), b"someone else's").unwrap();
    let m = refused(publish(&plan, &Job::silent()));
    assert!(m.contains("appeared") && m.contains("already copied") && m.contains("ps0001.patch") && m.contains("was not changed"), "{m}");
    assert_eq!(fs::read(out.join("Shaiya/patch/ps0002.patch")).unwrap(), b"someone else's");
    assert!(!out.join("Shaiya").join(INI_NAME).exists(), "no ini was written");
}

#[test]
fn bytes_that_are_not_ascii_in_an_existing_ini_stay_as_they_are() {
    let t = TempDir::new("pub-cp949");
    let out = t.path().join("server");
    fs::create_dir_all(out.join("Shaiya")).unwrap();
    let before: &[u8] = b"; \xBC\xAD\xB9\xF6\r\n[Version]\r\nCheckVersion=3\r\nUpdaterVersion=1\r\nPatchFileVersion=109\r\nNote=\xC7\xD1\xB1\xDB\r\n";
    fs::write(out.join("Shaiya").join(INI_NAME), before).unwrap();
    let p = make(t.path(), 110, "a");
    publish(&plan_publish(&[p], &out, &opts(None), &Job::silent()).unwrap(), &Job::silent()).unwrap();
    let expected: &[u8] = b"; \xBC\xAD\xB9\xF6\r\n[Version]\r\nCheckVersion=3\r\nUpdaterVersion=1\r\nPatchFileVersion=110\r\nNote=\xC7\xD1\xB1\xDB\r\n";
    assert_eq!(ini_of(&out), expected);
}

#[test]
fn a_changed_ini_between_plan_and_write_is_refused() {
    let t = TempDir::new("pub-stale");
    let out = t.path().join("server");
    fs::create_dir_all(out.join("Shaiya")).unwrap();
    let ini = out.join("Shaiya").join(INI_NAME);
    fs::write(&ini, b"[Version]\r\nCheckVersion=3\r\nUpdaterVersion=1\r\nPatchFileVersion=109\r\n").unwrap();
    let p = make(t.path(), 110, "a");
    let plan = plan_publish(&[p], &out, &opts(None), &Job::silent()).unwrap();
    fs::write(&ini, b"[Version]\r\nCheckVersion=3\r\nUpdaterVersion=1\r\nPatchFileVersion=115\r\n").unwrap();
    let m = refused(publish(&plan, &Job::silent()));
    assert!(m.contains("changed since the plan"), "{m}");
    assert!(!out.join("Shaiya/patch/ps0110.patch").exists(), "nothing was copied");
}

#[test]
fn an_unreadable_existing_ini_is_refused_and_left_alone() {
    let t = TempDir::new("pub-badini");
    let out = t.path().join("server");
    fs::create_dir_all(out.join("Shaiya")).unwrap();
    let ini = out.join("Shaiya").join(INI_NAME);
    for text in ["[Version]\r\nCheckVersion=3\r\n", "[Version]\r\nCheckVersion=3\r\nPatchFileVersion=abc\r\n", "[Version]\r\nCheckVersion=3\r\nPatchFileVersion=1\r\nPatchFileVersion=2\r\n"] {
        fs::write(&ini, text).unwrap();
        let p = make(t.path(), 3, "a");
        let _ = refused(plan_publish(&[p], &out, &opts(None), &Job::silent()));
        assert_eq!(fs::read_to_string(&ini).unwrap(), text);
    }
}

#[test]
fn removals_and_signatures_are_reported_and_carried() {
    let t = TempDir::new("pub-extra");
    let files = t.path().join("files");
    fs::create_dir_all(&files).unwrap();
    fs::write(files.join("a.txt"), b"a").unwrap();
    let list = t.path().join("remove.txt");
    fs::write(&list, "old/file.txt\n").unwrap();
    let src = t.path().join("src");
    fs::create_dir_all(&src).unwrap();
    let p = src.join("ps0001.patch");
    shaiya_patch::build(Some(&files), None, Some(&list), &p).unwrap();
    fs::write(shaiya_patch::signature_path(&p), b"signature bytes").unwrap();
    let out = t.path().join("server");
    let plan = plan_publish(&[p], &out, &opts(Some(1)), &Job::silent()).unwrap();
    assert!(plan.items[0].has_removals);
    assert!(plan.warnings.iter().any(|w| w.contains("ignores removals")), "{:?}", plan.warnings);
    assert_eq!(plan.copies().count(), 2);
    publish(&plan, &Job::silent()).unwrap();
    assert_eq!(fs::read(out.join("Shaiya/patch/ps0001.patch.sig")).unwrap(), b"signature bytes");
}

#[test]
fn a_patch_that_is_not_a_zip_is_refused() {
    let t = TempDir::new("pub-notzip");
    let p = t.path().join("ps0001.patch");
    fs::write(&p, b"this is not a patch").unwrap();
    let r = plan_publish(&[p], &t.path().join("server"), &opts(Some(1)), &Job::silent());
    assert!(r.is_err());
    assert!(!t.path().join("server").exists());
}
