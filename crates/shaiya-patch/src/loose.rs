//! A patch applied to a loose data folder: the archive entries of `update.sah`/`update.saf`, the loose files of the ZIP and the paths of
//! `remove.txt` become one [`LooseChange`] by path, which `shaiya_archive::loose_edit` checks and applies with its journal (`kind =
//! loose-patch`). Names are paths below the data folder, as the archive entries are.
//!
//! What differs from an archive client: nothing is appended to `data.saf` (there is none); a loose file of the patch has to go into a
//! top-level folder the data folder already has (the patch's loose files are meant for the install folder, which the toolkit never writes
//! for a loose client); a remove of a file that is not there is left out and listed; on a Shaiya-GX layout any entry that would write or
//! remove converted media (a `remove.txt` line for `Sound/x.wav` included) refuses the whole patch with the list (the check of
//! `loose_edit::plan`).

use super::{checked_loose, loose_entries, open_zip, read_files, read_index, read_removals, VerifyKey};
use shaiya_archive::loose::LooseLayout;
use shaiya_archive::loose_edit::{self, FormatCheck, LooseChange, LooseOptions, LooseReport, LoosePlan, LoosePut, MAX_CHANGE_BYTES, MAX_CHANGE_FILES, MAX_PUT_BYTES};
use shaiya_core::{Error, Job, Result};
use std::path::Path;

/// The change a patch makes to a loose folder, and what was left out of it.
pub struct LoosePatch {
    pub change: LooseChange,
    /// Paths of `remove.txt` that are not in the data folder (nothing to remove).
    pub missing: Vec<String>,
}

fn problems(list: Vec<String>) -> Error {
    Error::Refused(if list.len() == 1 { list.into_iter().next().unwrap_or_default() } else { format!("{} problems:\n  {}", list.len(), list.join("\n  ")) })
}

/// Reads a patch into a change for `layout` (nothing is written): the sizes are checked against the limits of one change before any file
/// is read, and every file the patch lists must have been read.
pub fn change_of(patch: &Path, layout: &LooseLayout, job: &Job) -> Result<LoosePatch> {
    let mut z = open_zip(patch)?;
    let idx = read_index(&mut z)?;
    let loose = checked_loose(patch, &mut z)?;
    let removals = read_removals(&mut z)?;
    let sizes: Vec<(String, u64)> = idx.files().into_iter().map(|(p, e)| (p, e.length as u64)).chain(loose_entries(&mut z).into_iter().map(|(_, n, s)| (n, s))).collect();
    let mut list: Vec<String> = Vec::new();
    if sizes.len() + removals.len() > MAX_CHANGE_FILES {
        return Err(Error::Refused(format!("the patch holds {} files; a change may hold at most {MAX_CHANGE_FILES}", sizes.len() + removals.len())));
    }
    let mut total = 0u64;
    for (name, size) in &sizes {
        total = total.saturating_add(*size);
        if *size > MAX_PUT_BYTES {
            list.push(format!("{name}: {size} bytes is more than the {MAX_PUT_BYTES} one file may hold"));
        }
    }
    if total > MAX_CHANGE_BYTES {
        list.push(format!("the patch holds {total} bytes; a change may hold at most {MAX_CHANGE_BYTES}"));
    }
    // a loose file goes into a folder the data folder has
    for (_, name, _) in &loose {
        if !loose_edit::top_folder_exists(layout, name) {
            list.push(format!("{name}: a loose file of a patch is written to the install folder, which the toolkit does not change for a loose client (the data folder has no folder of that name)"));
        }
    }
    if !list.is_empty() {
        return Err(problems(list));
    }
    let mut puts: Vec<LoosePut> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut visit = |path: &str, bytes: &[u8]| {
        seen.push(path.to_string());
        puts.push(LoosePut { rel: path.to_string(), bytes: bytes.to_vec() });
    };
    job.stage("reading the patch");
    read_files(patch, MAX_PUT_BYTES, &|_| true, &mut visit, job)?;
    // files of length 0 are not handed out by the reader: they are puts of no bytes
    for (name, size) in &sizes {
        if *size == 0 && !seen.iter().any(|s| s == name) {
            puts.push(LoosePut { rel: name.clone(), bytes: Vec::new() });
            seen.push(name.clone());
        }
    }
    let unread: Vec<String> = sizes.iter().map(|(n, _)| n.clone()).filter(|n| !seen.contains(n)).collect();
    if !unread.is_empty() {
        return Err(Error::Refused(format!("the patch could not be read completely: {}", unread.join(", "))));
    }
    let mut removes = Vec::new();
    let mut missing = Vec::new();
    for r in removals {
        // a removal of converted media on a Shaiya-GX layout is kept in the change, so that `plan` refuses the whole patch with the list (it is
        // not "a file that is not there": Shaiya-GX holds that file under another name)
        if loose_edit::file_exists(layout, &r) || (layout.gx && loose_edit::gx_problem(&r).is_some()) {
            removes.push(r);
        } else {
            missing.push(r);
        }
    }
    Ok(LoosePatch { change: LooseChange { puts, removes }, missing })
}

/// The checked plan of a patch for a loose folder, nothing written (for a dry run and a confirmation).
pub fn plan_loose(patch: &Path, layout: &LooseLayout, check: FormatCheck<'_>, job: &Job) -> Result<(LoosePlan, Vec<String>)> {
    let p = change_of(patch, layout, job)?;
    Ok((loose_edit::plan(layout, p.change, check)?, p.missing))
}

/// Applies a patch to a loose folder through the loose journal (`kind = loose-patch`, the label is the patch's file name). A refused
/// name or a failed format check refuses the whole patch before a byte is written.
pub fn apply_loose(patch: &Path, layout: &LooseLayout, opts: &LooseOptions, check: FormatCheck<'_>, job: &Job) -> Result<LooseReport> {
    apply_loose_signed(patch, layout, opts, check, None, job)
}

/// [`apply_loose`] that first requires the patch's `<patch>.sig` to verify with `key` (nothing is read from the folder when it does not): the
/// same check `apply_with` makes for an archive client, made again at apply time so a patch file swapped after a verify step is refused.
pub fn apply_loose_signed(patch: &Path, layout: &LooseLayout, opts: &LooseOptions, check: FormatCheck<'_>, key: Option<&VerifyKey>, job: &Job) -> Result<LooseReport> {
    if let Some(k) = key {
        crate::sign::verify_file(&k.public_hex, k.check_version, patch, None, job).map_err(|e| match e {
            Error::Cancelled => Error::Cancelled,
            Error::Refused(m) => Error::Refused(m),
            other => Error::Refused(format!("signature check failed for {}: {other}", patch.display())),
        })?;
    }
    let (plan, _) = plan_loose(patch, layout, check, job)?;
    let mut o = opts.clone();
    o.patch = true;
    if o.label.is_empty() {
        o.label = patch.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "patch".to_string());
    }
    loose_edit::apply(layout, plan, &o, check, job)
}
