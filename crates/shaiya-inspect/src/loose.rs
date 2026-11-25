//! What the readers say about the files of a loose data folder: the [`format_check`] that `shaiya_archive::loose_edit` calls before a file
//! is written, and the read-only [`health`] of a loose folder (the subset of the archive health that makes sense without an index).

use shaiya_archive::loose::LooseLayout;
use shaiya_archive::loose_edit;
use shaiya_clientdata::modeljson::{self, ModelFormat};
use shaiya_clientdata::{texture, LooseTree, TextureIndex};
use shaiya_core::{Error, Job, Result, Unit};
use shaiya_seed as seed;
use std::collections::HashSet;

use crate::validate::{is_sdata_name, is_texture_name};

/// Most list entries a health report keeps per problem kind (the counts are exact).
pub const HEALTH_LIST_LIMIT: usize = 200;
/// A list file (`.MLT`, `.MON`, `.ITM`) bigger than this is not read for the texture check.
const LIST_FILE_CAP: u64 = 16 << 20;
/// An SData file bigger than this is not read for the check.
const SDATA_CAP: u64 = 256 << 20;

/// Whether `rel` is a type the toolkit reads (so its bytes are checked before they are written): the six model formats, SData files and
/// textures (`.dds`, `.tga`, `.bmp`).
pub fn is_known_type(rel: &str) -> bool {
    ModelFormat::from_file_name(rel).is_some() || is_sdata_name(rel) || is_texture_name(rel)
}

/// The check of one file about to be written: `Ok` for a type the toolkit does not read; for a model it must read to the last byte (the
/// JSON export's own read-back), a texture must decode. For an SData file only the SEED wrapper is checked: a wrapped file must have a good
/// checksum and no second wrapper; the table inside (a plain file, or the body of a wrapped one) is NOT read, because which table it is
/// depends on the client version and the toolkit does not guess. An empty file of a known type is refused.
pub fn format_check(rel: &str, bytes: &[u8]) -> Result<()> {
    let known = is_known_type(rel);
    if known && bytes.is_empty() {
        return Err(Error::Refused(format!("{rel} is empty (0 bytes)")));
    }
    if let Some(f) = ModelFormat::from_file_name(rel) {
        return modeljson::export(f, bytes, rel).map(|_| ()).map_err(|e| Error::Refused(format!("{rel} does not read to its last byte as {} ({e})", f.label())));
    }
    if is_sdata_name(rel) {
        if seed::is_encrypted(bytes) {
            match seed::decrypt_unchecked(bytes, rel) {
                Ok((plain, true)) if !seed::is_encrypted(&plain) => {}
                Ok((_, false)) => return Err(Error::Refused(format!("{rel}: the SEED checksum fails"))),
                Ok(_) => return Err(Error::Refused(format!("{rel} is SEED-encrypted twice"))),
                Err(e) => return Err(Error::Refused(format!("{rel}: the SEED wrapper is damaged ({e})"))),
            }
        }
        return Ok(());
    }
    if is_texture_name(rel) {
        return texture::decode(bytes, rel).map(|_| ()).map_err(|e| Error::Refused(format!("{rel}: the toolkit cannot read this texture ({e})")));
    }
    Ok(())
}

/// What [`health`] found. Counts are exact; the lists keep the first [`HEALTH_LIST_LIMIT`] entries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LooseHealth {
    pub files: usize,
    /// Files that are listed but cannot be opened or read now (gone, a link now, no permission).
    pub unreadable: usize,
    pub unreadable_list: Vec<String>,
    /// Model files that do not read to their last byte and SData files whose SEED wrapper fails (checksum, a second wrapper), with the
    /// reader's reason. The table inside an SData file is not read.
    pub failing: usize,
    pub failing_list: Vec<(String, String)>,
    /// Model textures named by a list file that are not in the folder under the name, nor as `.dds`, `.tga`, `.png` or `_dds.png`.
    pub unresolved: usize,
    pub unresolved_list: Vec<(String, String)>,
    /// List files (`.MLT`, `.MON`, `.ITM`) that could not be read.
    pub lists_refused: usize,
    /// `*.shaiya-tmp` files of the toolkit in the folder (the listing leaves them out).
    pub temps: usize,
    /// An interrupted change or rollback (see `loose_edit::recover`). `None` while a change is running.
    pub unfinished: Option<String>,
    /// A change, rollback, prune or recovery of this client is running now (its lock is held): nothing here is to be recovered.
    pub running: bool,
    /// Journal folders whose `journal.txt` does not read (they cannot be rolled back and block the rollback of older changes).
    pub damaged_journals: Vec<String>,
    /// What the tree's listing left out (links, case twins, ...).
    pub notes: Vec<String>,
    /// Journals in the backup folder, and how many can still be rolled back (`None` without a confirmed backup folder).
    pub journals: Option<(usize, usize)>,
}

impl LooseHealth {
    /// True when nothing needs the person's attention.
    pub fn is_clean(&self) -> bool {
        self.unreadable == 0 && self.failing == 0 && self.unresolved == 0 && self.lists_refused == 0 && self.unfinished.is_none() && !self.running && self.damaged_journals.is_empty() && self.temps == 0
    }
}

fn push_limited<T>(v: &mut Vec<T>, item: T) {
    if v.len() < HEALTH_LIST_LIMIT {
        v.push(item);
    }
}

fn is_list_file(rel: &str) -> bool {
    let l = rel.to_lowercase();
    l.ends_with(".mlt") || l.ends_with(".mon") || l.ends_with(".itm")
}

fn is_mesh(rel: &str) -> bool {
    let l = rel.to_lowercase();
    l.ends_with(".3dc") || l.ends_with(".3do")
}

/// Reads a loose folder and reports: files that cannot be read, model files that fail their reader and SData files whose SEED wrapper fails, and model textures that
/// resolve nowhere. It writes nothing. Reading all model files takes a while on a real client: the job is checked per file and a cancel
/// ends the report with `Error::Cancelled`.
pub fn health(layout: &LooseLayout, tree: &LooseTree, job: &Job) -> Result<LooseHealth> {
    let mut h = LooseHealth { files: tree.files().len(), notes: tree.notes(), temps: tree.skipped().temps, unfinished: loose_edit::unfinished(layout), running: loose_edit::change_running(layout), damaged_journals: loose_edit::damaged_journals(layout), ..LooseHealth::default() };
    if layout.backups.is_some() {
        if let Ok(list) = loose_edit::list(layout) {
            let possible = list.iter().filter(|(_, r)| matches!(r, shaiya_archive::journal::Rollback::Possible)).count();
            h.journals = Some((list.len(), possible));
        }
    }
    job.stage("reading the files");
    job.total(Some(h.files as u64), Unit::Items);
    let quiet = Job::new(&shaiya_core::Silent, job.cancel_token().clone());
    for f in tree.files() {
        job.check()?;
        job.advance(1);
        let readable = tree.read_range(&f.rel, 0, 1).is_ok();
        if !readable {
            h.unreadable += 1;
            push_limited(&mut h.unreadable_list, f.rel.clone());
            continue;
        }
        let model = ModelFormat::from_file_name(&f.rel).is_some();
        let sdata = is_sdata_name(&f.rel);
        if (model && f.len <= modeljson::MAX_MODEL_BYTES) || (sdata && f.len <= SDATA_CAP) {
            // read no more than the size was checked against (a file that grew since the listing is cut there, and then fails its reader)
            let cap = if model { modeljson::MAX_MODEL_BYTES } else { SDATA_CAP };
            match tree.read(&f.rel, cap, &quiet).and_then(|b| format_check(&f.rel, &b)) {
                Ok(()) => {}
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(e) => {
                    h.failing += 1;
                    push_limited(&mut h.failing_list, (f.rel.clone(), match e {
                        Error::Refused(s) => s,
                        other => other.to_string(),
                    }));
                }
            }
        }
    }
    job.stage("looking for textures");
    let paths = tree.paths();
    let built = TextureIndex::build(
        &paths,
        &mut |p: &str| if is_list_file(p) { tree.read(p, LIST_FILE_CAP, &quiet).ok() } else { None },
        &mut || job.check().is_err(),
    );
    let Some((idx, report)) = built else { return Err(Error::Cancelled) };
    h.lists_refused = report.lists_refused.len();
    let names: HashSet<String> = paths.iter().map(|p| p.rsplit('/').next().unwrap_or(p).to_lowercase()).collect();
    for p in paths.iter().filter(|p| is_mesh(p)) {
        job.check()?;
        for t in idx.for_mesh(p).iter().filter(|t| !t.found) {
            let lower = t.texture.to_lowercase();
            let bare = lower.rsplit('/').next().unwrap_or(&lower).to_string();
            let stem = bare.rsplit_once('.').map_or(bare.as_str(), |(s, _)| s).to_string();
            let alt = [bare.clone(), format!("{stem}.dds"), format!("{stem}.tga"), format!("{stem}.png"), format!("{stem}_dds.png")];
            if !alt.iter().any(|a| names.contains(a)) {
                h.unresolved += 1;
                push_limited(&mut h.unresolved_list, (p.clone(), t.texture.clone()));
            }
        }
    }
    Ok(h)
}
