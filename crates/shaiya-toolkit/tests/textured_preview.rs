//! The textured previews of the Archive tab: a `.3DC` and the parts of an `.ANI` get the texture their list file names, a `.3DO` its `.ITM`
//! texture, a `.3DE` its own; the alpha rule, the texture choice, the "Textures" switch, the shared texture index and the cancel. Synthetic
//! files only (generated textures, the testkit's list and mesh builders).

use shaiya_archive::Archive;
use shaiya_core::{Cancel, Error, Job, Result, Silent};
use shaiya_testkit::anim::{animated_3do, arm_ani, arm_mesh_3dc, box_3de, box_3do, moving_3de};
use shaiya_testkit::lists::{itm_bytes, mlt_bytes, mon_bytes, ItmRec, MonRec};
use shaiya_testkit::tex::{checker, dds_rgba};
use shaiya_testkit::{SahBuilder, TempDir};
use shaiya_toolkit::preview::{make_preview, make_preview_look, MeshChoice, MeshInfoCache, PreviewData, TextureIndexCache};
use shaiya_toolkit::preview_models::EFFECT_NOTE;
use shaiya_toolkit::preview_tex::{Look, NOT_LISTED, TEXTURES_OFF};
use shaiya_toolkit::source::FileSource;
use std::collections::HashMap;
use std::sync::Mutex;

const ANI: &str = "Character/Human/ANI/huwm_000_a.ANI";
const TORSO: &str = "Character/Human/3DC/huwm_torso001.3DC";
const LOWER: &str = "Character/Human/3DC/huwm_lower001.3DC";
const MLT: &str = "Character/Human/huwm.MLT";

fn open(d: &TempDir, files: &[(&str, Vec<u8>)]) -> Archive {
    let mut b = SahBuilder::new(0);
    for (p, data) in files {
        b = b.file(p.as_bytes(), data);
    }
    b.write_to(d.path());
    Archive::open(d.path()).unwrap()
}

/// An archive that counts what is read from it: every `read` and `read_range` by lower-cased path.
struct Counting<'a> {
    inner: &'a Archive,
    reads: Mutex<HashMap<String, usize>>,
}

impl<'a> Counting<'a> {
    fn new(inner: &'a Archive) -> Counting<'a> {
        Counting { inner, reads: Mutex::new(HashMap::new()) }
    }

    fn reads_of(&self, path: &str) -> usize {
        self.reads.lock().unwrap().get(&path.to_lowercase()).copied().unwrap_or(0)
    }
}

impl FileSource for Counting<'_> {
    fn paths(&self) -> Vec<String> {
        self.inner.paths()
    }
    fn len(&self, path: &str) -> Option<u64> {
        self.inner.len(path)
    }
    fn read(&self, path: &str, cap: u64, job: &Job) -> Result<Vec<u8>> {
        *self.reads.lock().unwrap().entry(path.to_lowercase()).or_default() += 1;
        FileSource::read(self.inner, path, cap, job)
    }
    fn read_range(&self, path: &str, from: u64, n: usize) -> Result<Vec<u8>> {
        FileSource::read_range(self.inner, path, from, n)
    }
    fn stamp(&self, path: &str) -> Option<(u64, u64)> {
        self.inner.stamp(path)
    }
}

/// Pixels that are drawn in the orange of the test textures (the plain picture is grey with a blue-ish wire; orange has far more red than blue).
fn coloured(rgba: &[u8]) -> usize {
    rgba.chunks(4).filter(|p| p[3] > 0 && p[0] as i32 - p[2] as i32 > 80).count()
}

fn drawn(rgba: &[u8]) -> usize {
    rgba.chunks(4).filter(|p| p[3] > 0).count()
}

fn skin() -> Vec<u8> {
    checker(64, 8, [230, 150, 40], [30, 140, 160])
}

/// A character archive: the animation, two parts, and a list file that gives each part `skin.dds` (flag `flag`).
fn character(d: &TempDir, flag: i32, texture: Vec<u8>) -> Archive {
    let mlt = mlt_bytes(&["huwm_torso001.3DC", "huwm_lower001.3DC"], &["skin.dds"], &[(0, 0, flag), (1, 0, flag)]);
    open(d, &[(ANI, arm_ani(60)), (TORSO, arm_mesh_3dc()), (LOWER, arm_mesh_3dc()), (MLT, mlt), ("Character/Human/DDS/skin.dds", texture)])
}

fn preview(src: &dyn FileSource, path: &str, look: &Look) -> PreviewData {
    make_preview_look(src, path, false, 0, &MeshChoice::Default, &MeshInfoCache::default(), &TextureIndexCache::default(), look, &Job::silent()).unwrap_or_else(|e| panic!("{path}: {e}"))
}

struct Model {
    notes: Vec<String>,
    line: Option<String>,
    choices: usize,
    rgba: Vec<u8>,
}

fn model(p: PreviewData) -> Model {
    match p {
        PreviewData::Model { notes, texture, rgba, .. } => Model { notes, line: texture.as_ref().map(|t| t.line.clone()), choices: texture.map_or(0, |t| t.choices.len()), rgba },
        other => panic!("{other:?}"),
    }
}

fn frames(p: PreviewData) -> (Vec<Vec<u8>>, Vec<String>, Vec<String>) {
    match p {
        PreviewData::Anim { frames, notes, header, .. } => (frames.frames, notes, header),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_3dc_with_an_mlt_record_is_drawn_textured() {
    let d = TempDir::new("w6-tex-3dc");
    let a = character(&d, 0, skin());
    let m = model(preview(&a, TORSO, &Look::default()));
    assert!(coloured(&m.rgba) > 0, "the texture's colours are in the picture");
    assert_eq!(m.line.as_deref(), Some("texture skin.dds from Character/Human/huwm.MLT (record 0); alpha used as transparency"));
    assert_eq!(m.notes, ["shown in the mesh's own rest pose"]);
    // the second part is record 1 of the same list
    assert_eq!(model(preview(&a, LOWER, &Look::default())).line.as_deref(), Some("texture skin.dds from Character/Human/huwm.MLT (record 1); alpha used as transparency"));
}

#[test]
fn a_glow_texture_is_drawn_opaque_and_the_header_says_so() {
    // every texel has alpha 0: cut out it vanishes, as a glow (flag 1) the colour is drawn and the alpha ignored
    let clear = dds_rgba(64, 64, |x, y| if (x / 8 + y / 8) % 2 == 0 { [230, 150, 40, 0] } else { [30, 140, 160, 0] });
    let d = TempDir::new("w6-tex-glow");
    let glow = model(preview(&character(&d, 1, clear.clone()), TORSO, &Look::default()));
    assert!(coloured(&glow.rgba) > 0);
    assert!(glow.line.as_deref().is_some_and(|l| l.ends_with("alpha used as glow: ignored")), "{:?}", glow.line);
    let d2 = TempDir::new("w6-tex-glow-off");
    let cut = model(preview(&character(&d2, 0, clear), TORSO, &Look::default()));
    assert_eq!(coloured(&cut.rgba), 0, "a transparency texture with alpha 0 draws nothing of the texture");
    assert!(cut.line.as_deref().is_some_and(|l| l.ends_with("alpha used as transparency")));
}

#[test]
fn a_monster_texture_the_list_does_not_describe_is_cut_out() {
    let d = TempDir::new("w6-tex-mon");
    let mon = mon_bytes(&[MonRec { name: "Bat", flag: 0, slots: [""; 17], parts: &[("bat.3DC", "bat.dds")], size: 1.0, attachments: &[] }]);
    let a = open(&d, &[("Monster/bat.MON", mon), ("Monster/3DC/bat.3DC", arm_mesh_3dc()), ("Monster/DDS/bat.dds", skin())]);
    let m = model(preview(&a, "Monster/3DC/bat.3DC", &Look::default()));
    assert!(coloured(&m.rgba) > 0);
    assert_eq!(m.line.as_deref(), Some("texture bat.dds from Monster/bat.MON (record 0); alpha: cut out (the list does not say)"));
}

#[test]
fn a_3dc_without_a_record_stays_grey_and_says_why() {
    let d = TempDir::new("w6-tex-none");
    let a = open(&d, &[(TORSO, arm_mesh_3dc())]);
    let m = model(preview(&a, TORSO, &Look::default()));
    assert_eq!((m.line, coloured(&m.rgba) > 0, drawn(&m.rgba) > 0), (None, false, true));
    assert_eq!(m.notes, ["shown in the mesh's own rest pose", NOT_LISTED]);
    // a record whose texture is not in the archive
    let d2 = TempDir::new("w6-tex-gone");
    let mlt = mlt_bytes(&["huwm_torso001.3DC"], &["gone.dds"], &[(0, 0, 0)]);
    let b = open(&d2, &[(TORSO, arm_mesh_3dc()), (MLT, mlt)]);
    let m = model(preview(&b, TORSO, &Look::default()));
    assert_eq!(m.notes, ["shown in the mesh's own rest pose", "texture gone.dds is not in the archive; drawn grey"]);
    assert!(m.line.is_some() && coloured(&m.rgba) == 0, "the line still says what the list names");
    // a texture file that does not decode
    let d3 = TempDir::new("w6-tex-bad");
    let mlt = mlt_bytes(&["huwm_torso001.3DC"], &["bad.dds"], &[(0, 0, 0)]);
    let c = open(&d3, &[(TORSO, arm_mesh_3dc()), (MLT, mlt), ("Character/Human/DDS/bad.dds", b"not a texture".to_vec())]);
    let m = model(preview(&c, TORSO, &Look::default()));
    assert_eq!(m.notes, ["shown in the mesh's own rest pose", "texture bad.dds could not be read; drawn grey"]);
}

#[test]
fn the_texture_choice_switches_the_variant() {
    let d = TempDir::new("w6-tex-choice");
    let red = checker(64, 8, [220, 30, 30], [220, 30, 30]);
    let mlt = mlt_bytes(&["huwm_torso001.3DC"], &["a.dds", "b.dds"], &[(0, 0, 0), (0, 1, 0)]);
    let a = open(&d, &[(TORSO, arm_mesh_3dc()), (MLT, mlt), ("Character/Human/DDS/a.dds", skin()), ("Character/Human/DDS/b.dds", red)]);
    let first = model(preview(&a, TORSO, &Look::default()));
    assert_eq!(first.choices, 2);
    assert!(first.line.as_deref().unwrap().starts_with("texture a.dds from"));
    let second = model(preview(&a, TORSO, &Look { textured: true, texture: 1, ..Look::default() }));
    assert!(second.line.as_deref().unwrap().starts_with("texture b.dds from") && second.line.as_deref().unwrap().contains("(record 1)"));
    assert_ne!(first.rgba, second.rgba, "another texture, another picture");
    // a choice past the last is the last
    assert!(model(preview(&a, TORSO, &Look { textured: true, texture: 9, ..Look::default() })).line.as_deref().unwrap().starts_with("texture b.dds"));
}

#[test]
fn turning_textures_off_draws_grey() {
    let d = TempDir::new("w6-tex-off");
    let a = character(&d, 0, skin());
    let off = Look { textured: false, texture: 0, ..Look::default() };
    let m = model(preview(&a, TORSO, &off));
    assert_eq!((m.line, coloured(&m.rgba), drawn(&m.rgba) > 0), (None, 0, true));
    assert!(m.notes.iter().any(|n| n == TEXTURES_OFF), "{:?}", m.notes);
    let (fr, notes, _) = frames(preview(&a, ANI, &off));
    assert!(!fr.is_empty() && fr.iter().all(|f| coloured(f) == 0), "the animation is grey too");
    assert!(notes.iter().any(|n| n == TEXTURES_OFF));
    // and on again
    let (fr, ..) = frames(preview(&a, ANI, &Look::default()));
    assert!(fr.iter().any(|f| coloured(f) > 0));
}

#[test]
fn an_ani_on_default_parts_textures_each_part_and_decodes_each_texture_once() {
    let d = TempDir::new("w6-tex-ani");
    let a = character(&d, 0, skin());
    let counting = Counting::new(&a);
    let (fr, notes, _) = frames(preview(&counting, ANI, &Look::default()));
    assert_eq!(fr.len(), 61);
    assert!(fr.iter().all(|f| coloured(f) > 0), "every frame shows the texture");
    assert!(notes.is_empty(), "{notes:?}");
    // two parts use skin.dds: it is read (and decoded) once
    assert_eq!(counting.reads_of("Character/Human/DDS/skin.dds"), 1);
    // the grey picture of the same frames is different
    let (grey, ..) = frames(preview(&a, ANI, &Look { textured: false, texture: 0, ..Look::default() }));
    assert_ne!(fr[30], grey[30]);
}

#[test]
fn an_ani_part_the_lists_do_not_name_is_listed_once_in_the_notes() {
    let d = TempDir::new("w6-tex-ani-notes");
    // torso has a record whose texture is missing; lower is in no list
    let mlt = mlt_bytes(&["huwm_torso001.3DC"], &["gone.dds"], &[(0, 0, 0)]);
    let a = open(&d, &[(ANI, arm_ani(10)), (TORSO, arm_mesh_3dc()), (LOWER, arm_mesh_3dc()), (MLT, mlt)]);
    let (fr, notes, _) = frames(preview(&a, ANI, &Look::default()));
    assert!(fr.iter().all(|f| coloured(f) == 0 && drawn(f) > 0));
    assert_eq!(notes, ["texture gone.dds is not in the archive; drawn grey", "no list file names a texture for huwm_lower001.3DC; drawn grey"]);
}

#[test]
fn a_3do_gets_its_itm_texture() {
    let d = TempDir::new("w6-tex-3do");
    let itm = itm_bytes(&["crate.3DO"], &["crate.dds"], &[ItmRec { words: [0; 6], extra: None }]);
    let a = open(&d, &[("Item/crate.ITM", itm), ("Item/3DO/crate.3DO", box_3do()), ("Item/DDS/crate.dds", skin()), ("Item/3DO/plain.3DO", box_3do())]);
    let m = model(preview(&a, "Item/3DO/crate.3DO", &Look::default()));
    assert!(coloured(&m.rgba) > 0);
    assert_eq!(m.notes, ["texture crate.dds from Item/crate.ITM (record 0); alpha: cut out (the list does not say)"]);
    // a model no list names and whose own name is not in Item/DDS stays grey
    let m = model(preview(&a, "Item/3DO/plain.3DO", &Look::default()));
    assert_eq!((coloured(&m.rgba), m.notes), (0, vec![NOT_LISTED.to_string()]));
}

#[test]
fn an_animated_3do_plays_textured_from_its_itm_record() {
    let d = TempDir::new("w6-tex-3do-anim");
    let itm = itm_bytes(&["arrow.3DO"], &["arrow.dds"], &[ItmRec { words: [0; 6], extra: None }]);
    let a = open(&d, &[("Item/arrow.ITM", itm), ("Item/3DO/arrow.3DO", animated_3do(5, 10)), ("Item/DDS/arrow.dds", skin())]);
    let (fr, notes, header) = frames(preview(&a, "Item/3DO/arrow.3DO", &Look::default()));
    assert_eq!((fr.len(), header[0].as_str()), (5, "3DO mesh: 8 vertices, 12 triangles"));
    assert!(fr.iter().all(|f| coloured(f) > 0), "{notes:?}");
}

#[test]
fn a_3de_uses_its_own_texture_and_plays_its_frames() {
    let d = TempDir::new("w6-tex-3de");
    // the file names fx.tga; the archive holds it as fx.dds in Effect/DDS
    let a = open(&d, &[("Effect/3DE/glow.3de", moving_3de(4)), ("Effect/DDS/fx.dds", skin()), ("Effect/3DE/still.3de", box_3de(None)), ("Effect/3DE/none.3de", box_3de(Some(0)))]);
    let (fr, notes, header) = frames(preview(&a, "Effect/3DE/glow.3de", &Look::default()));
    assert_eq!(fr.len(), 4, "the frames play");
    assert!(fr.iter().all(|f| coloured(f) > 0), "textured");
    assert!(fr[0] != fr[3], "the frames differ");
    assert_eq!(header[0], "Effect mesh: 8 vertices, 12 triangles");
    assert_eq!(header[1], "4 frames, played as positions (inferred) at 30 frames per second (the file stores no rate)");
    assert!(notes.iter().any(|n| n == EFFECT_NOTE), "{notes:?}");
    // the static layout and an empty frame block stay single pictures
    let still = model(preview(&a, "Effect/3DE/still.3de", &Look::default()));
    assert!(coloured(&still.rgba) > 0 && still.notes[0] == "no frame block: the file ends after the faces" && still.notes.iter().any(|n| n == EFFECT_NOTE));
    assert_eq!(model(preview(&a, "Effect/3DE/none.3de", &Look::default())).notes[0], "0 frames");
    // without the texture file: grey, with the reason
    let d2 = TempDir::new("w6-tex-3de-none");
    let b = open(&d2, &[("Effect/3DE/glow.3de", moving_3de(4))]);
    let (fr, notes, _) = frames(preview(&b, "Effect/3DE/glow.3de", &Look::default()));
    assert!(fr.iter().all(|f| coloured(f) == 0 && drawn(f) > 0));
    assert!(notes.iter().any(|n| n == "texture fx.tga is not in the archive; drawn grey"), "{notes:?}");
}

#[test]
fn the_texture_index_is_built_once_per_archive() {
    let d = TempDir::new("w6-tex-once");
    let a = character(&d, 0, skin());
    let counting = Counting::new(&a);
    let cache = TextureIndexCache::default();
    let (mesh, info) = (MeshInfoCache::default(), Look::default());
    for p in [TORSO, LOWER, ANI, TORSO] {
        make_preview_look(&counting, p, false, 0, &MeshChoice::Default, &mesh, &cache, &info, &Job::silent()).unwrap();
    }
    assert!(cache.is_built());
    assert_eq!(counting.reads_of(MLT), 1, "the list file is read for the first preview only");
}

#[test]
fn a_cancelled_textured_preview_ends_cancelled() {
    let d = TempDir::new("w6-tex-cancel");
    let a = character(&d, 0, skin());
    let cancel = Cancel::new();
    cancel.cancel();
    let job = Job::new(&Silent, cancel);
    for p in [TORSO, ANI] {
        let e = make_preview_look(&a, p, false, 0, &MeshChoice::Default, &MeshInfoCache::default(), &TextureIndexCache::default(), &Look::default(), &job).unwrap_err();
        assert!(matches!(e, Error::Cancelled), "{p}: {e:?}");
    }
    // cancelled after the first item of the work: the texture decode is asked of the job too
    let cache = TextureIndexCache::default();
    assert!(cache.get_or_build(&a, &Job::silent()).is_some());
    let cancel = Cancel::new();
    let counting = Counting::new(&a);
    cancel.cancel();
    let job = Job::new(&Silent, cancel);
    let e = make_preview_look(&counting, ANI, false, 0, &MeshChoice::Default, &MeshInfoCache::default(), &cache, &Look::default(), &job).unwrap_err();
    assert!(matches!(e, Error::Cancelled));
    assert_eq!(counting.reads_of("Character/Human/DDS/skin.dds"), 0, "no texture is decoded after the cancel");
}

/// A source that cancels the job at the first read of a path that ends with `trigger` and then reads on like the archive (whose read
/// ends `Cancelled`); every read is logged in order.
struct Cancelling<'a> {
    inner: &'a Archive,
    trigger: &'static str,
    cancel: Cancel,
    log: Mutex<Vec<String>>,
}

impl FileSource for Cancelling<'_> {
    fn paths(&self) -> Vec<String> {
        self.inner.paths()
    }
    fn len(&self, path: &str) -> Option<u64> {
        self.inner.len(path)
    }
    fn read(&self, path: &str, cap: u64, job: &Job) -> Result<Vec<u8>> {
        self.log.lock().unwrap().push(path.to_lowercase());
        if path.to_lowercase().ends_with(self.trigger) {
            self.cancel.cancel();
        }
        FileSource::read(self.inner, path, cap, job)
    }
    fn read_range(&self, path: &str, from: u64, n: usize) -> Result<Vec<u8>> {
        FileSource::read_range(self.inner, path, from, n)
    }
    fn stamp(&self, path: &str) -> Option<(u64, u64)> {
        self.inner.stamp(path)
    }
}

fn two_skins(d: &TempDir) -> Archive {
    let mlt = mlt_bytes(&["huwm_torso001.3DC", "huwm_lower001.3DC"], &["skin.dds", "skin2.dds"], &[(0, 0, 0), (1, 1, 0)]);
    open(d, &[(ANI, arm_ani(60)), (TORSO, arm_mesh_3dc()), (LOWER, arm_mesh_3dc()), (MLT, mlt), ("Character/Human/DDS/skin.dds", skin()), ("Character/Human/DDS/skin2.dds", skin())])
}

#[test]
fn a_cancel_inside_the_list_read_keeps_no_index_and_reads_no_texture() {
    let d = TempDir::new("w6fix-cancel-mlt");
    let a = two_skins(&d);
    let cancel = Cancel::new();
    let src = Cancelling { inner: &a, trigger: ".mlt", cancel: cancel.clone(), log: Mutex::new(Vec::new()) };
    let cache = TextureIndexCache::default();
    let job = Job::new(&Silent, cancel);
    let e = make_preview_look(&src, ANI, false, 0, &MeshChoice::Default, &MeshInfoCache::default(), &cache, &Look::default(), &job).unwrap_err();
    assert!(matches!(e, Error::Cancelled), "{e:?}");
    assert!(!cache.is_built(), "a build that was cancelled keeps nothing");
    assert!(!src.log.lock().unwrap().iter().any(|p| p.ends_with(".dds")), "no texture is read after the cancel");
}

#[test]
fn a_cancel_inside_the_first_texture_read_reads_no_second_texture() {
    let d = TempDir::new("w6fix-cancel-tex");
    let a = two_skins(&d);
    let cache = TextureIndexCache::default();
    assert!(cache.get_or_build(&a, &Job::silent()).is_some());
    let cancel = Cancel::new();
    let src = Cancelling { inner: &a, trigger: ".dds", cancel: cancel.clone(), log: Mutex::new(Vec::new()) };
    let job = Job::new(&Silent, cancel);
    let e = make_preview_look(&src, ANI, false, 0, &MeshChoice::Default, &MeshInfoCache::default(), &cache, &Look::default(), &job).unwrap_err();
    assert!(matches!(e, Error::Cancelled), "{e:?}");
    assert_eq!(src.log.lock().unwrap().iter().filter(|p| p.ends_with(".dds")).count(), 1, "the second texture is not read");
    assert!(cache.is_built(), "the index was built before the cancel and stays");
}

/// Counts how often a read announces its size.
#[derive(Default)]
struct Totals(std::cell::Cell<usize>);

impl shaiya_core::Progress for Totals {
    fn total(&self, _total: Option<u64>, _unit: shaiya_core::Unit) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn only_the_main_file_reports_progress() {
    let d = TempDir::new("w6fix-progress");
    let a = two_skins(&d);
    for p in [ANI, TORSO] {
        let totals = Totals::default();
        let job = Job::new(&totals, Cancel::new());
        make_preview_look(&a, p, false, 0, &MeshChoice::Default, &MeshInfoCache::default(), &TextureIndexCache::default(), &Look::default(), &job).unwrap();
        assert_eq!(totals.0.get(), 1, "{p}: the list files, the candidate meshes and the textures report nothing");
    }
}

#[test]
fn a_texture_found_in_another_folder_says_so() {
    let d = TempDir::new("w6fix-elsewhere");
    let mlt = mlt_bytes(&["huwm_torso001.3DC"], &["skin.dds"], &[(0, 0, 0)]);
    let a = open(&d, &[(TORSO, arm_mesh_3dc()), (MLT, mlt), ("Character/Orc/DDS/skin.dds", skin())]);
    let m = model(preview(&a, TORSO, &Look::default()));
    assert_eq!(m.line.as_deref(), Some("texture skin.dds (found in another folder) from Character/Human/huwm.MLT (record 0); alpha used as transparency"));
    assert!(coloured(&m.rgba) > 0);
}

#[test]
fn the_textures_switch_shows_only_where_it_does_something() {
    use shaiya_toolkit::preview_tex::switch_applies;
    for p in ["Character/Human/3DC/a.3DC", "Character/Human/ANI/a.ANI", "Item/3DO/a.3do", "Effect/3DE/a.3de"] {
        assert!(switch_applies(p), "{p}");
    }
    for p in ["Entity/Model/a.smod", "Entity/Model/a.VANI", "a.dds", "noext"] {
        assert!(!switch_applies(p), "{p}");
    }
}

#[test]
fn a_plain_archive_without_lists_previews_as_before_apart_from_the_notes() {
    let d = TempDir::new("w6-tex-plain");
    let a = open(&d, &[(ANI, arm_ani(10)), (TORSO, arm_mesh_3dc())]);
    let (fr, notes, _) = frames(make_preview(&a, ANI, false, 0, &Job::silent()).unwrap());
    assert_eq!(fr.len(), 11);
    assert!(fr.iter().all(|f| coloured(f) == 0 && drawn(f) > 0));
    assert_eq!(notes, ["no list file names a texture for huwm_torso001.3DC; drawn grey"]);
}

#[test]
fn a_3do_no_list_names_uses_the_name_it_holds_from_item_dds() {
    let d = TempDir::new("w6-tex-3do-own");
    // the animated 3DO holds the texture name box.tga; the archive has it as box.dds in Item/DDS
    let a = open(&d, &[("Item/3DO/own.3DO", animated_3do(3, 10)), ("Item/DDS/box.dds", skin())]);
    let (fr, notes, _) = frames(preview(&a, "Item/3DO/own.3DO", &Look::default()));
    assert!(fr.iter().all(|f| coloured(f) > 0), "{notes:?}");
    assert_eq!(notes, ["texture box.dds (the name the model file holds; no list file names one); alpha: cut out"]);
}

// ------------------------------------------------------------------------------------------------ a loose folder (W17)

/// The files of `character` as a loose data folder (`data/` below `d`, with an `Item` folder so that the detection accepts it) and its tree.
fn loose_character(d: &TempDir, flag: i32, texture: Option<Vec<u8>>) -> shaiya_clientdata::LooseTree {
    let mlt = mlt_bytes(&["huwm_torso001.3DC", "huwm_lower001.3DC"], &["skin.dds"], &[(0, 0, flag), (1, 0, flag)]);
    let mut files: Vec<(&str, Vec<u8>)> = vec![(ANI, arm_ani(60)), (TORSO, arm_mesh_3dc()), (LOWER, arm_mesh_3dc()), (MLT, mlt), ("Item/readme.txt", b"x".to_vec())];
    if let Some(t) = texture {
        files.push(("Character/Human/DDS/skin.dds", t));
    }
    for (p, b) in &files {
        let at = d.path().join("data").join(p);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, b).unwrap();
    }
    let layout = shaiya_archive::loose::detect(d.path()).unwrap().expect("a loose layout");
    shaiya_clientdata::LooseTree::open(&layout, &Job::silent()).unwrap()
}

#[test]
fn a_textured_preview_resolves_its_texture_on_a_loose_folder_as_in_an_archive() {
    let (da, dl) = (TempDir::new("w17-tex-archive"), TempDir::new("w17-tex-loose"));
    let a = character(&da, 0, skin());
    let t = loose_character(&dl, 0, Some(skin()));
    // a 3DC: the same texture line and the same pixels, and the texture's colours are in the picture
    for part in [TORSO, LOWER] {
        let (ma, ml) = (model(preview(&a, part, &Look::default())), model(preview(&t, part, &Look::default())));
        assert!(coloured(&ml.rgba) > 0, "{part}: the loose folder's texture is drawn");
        assert!(ml.line.as_deref().is_some_and(|l| l.starts_with("texture skin.dds from Character/Human/huwm.MLT (record ")), "{part}: {:?}", ml.line);
        assert_eq!(ml.line, ma.line, "{part}: the same line as the archive form");
        assert_eq!(ml.rgba, ma.rgba, "{part}: the same picture as the archive form");
    }
    // an animation on the default parts: every frame textured, and the frames equal the archive's
    let (fa, _, _) = frames(preview(&a, ANI, &Look::default()));
    let (fl, _, _) = frames(preview(&t, ANI, &Look::default()));
    assert!(fl.iter().any(|f| coloured(f) > 0), "the loose folder's animation frames carry the texture");
    assert_eq!(fl, fa);
}

#[test]
fn a_loose_folder_without_the_texture_file_draws_grey_and_says_so() {
    // the test that can fail: the same folder with the DDS left out must not look textured
    let (dg, db) = (TempDir::new("w17-tex-good"), TempDir::new("w17-tex-bad"));
    let good = model(preview(&loose_character(&dg, 0, Some(skin())), TORSO, &Look::default()));
    let tree = loose_character(&db, 0, None);
    let bad = model(preview(&tree, TORSO, &Look::default()));
    assert!(coloured(&good.rgba) > 0);
    assert_eq!(coloured(&bad.rgba), 0, "no texture file: nothing of the texture is drawn");
    assert_ne!(good.rgba, bad.rgba);
    assert!(bad.notes.iter().any(|n| n == "texture skin.dds is not in the folder; drawn grey"), "{:?}", bad.notes);
    assert!(bad.notes.iter().all(|n| !n.contains("archive")), "a loose folder is not called an archive: {:?}", bad.notes);
    let da2 = TempDir::new("w17-tex-archive-missing");
    let gone = open(&da2, &[(TORSO, arm_mesh_3dc()), (MLT, mlt_bytes(&["huwm_torso001.3DC"], &["skin.dds"], &[(0, 0, 0)]))]);
    assert!(model(preview(&gone, TORSO, &Look::default())).notes.iter().any(|n| n == "texture skin.dds is not in the archive; drawn grey"));
    // letter case of the folder and file names on disk does not matter
    let dc = TempDir::new("w17-tex-case");
    let at = dc.path().join("data/CHARACTER/HUMAN/dds/SKIN.DDS");
    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
    std::fs::write(at, skin()).unwrap();
    for (p, b) in [(TORSO, arm_mesh_3dc()), (MLT, mlt_bytes(&["huwm_torso001.3DC"], &["skin.dds"], &[(0, 0, 0)])), ("item/readme.txt", b"x".to_vec())] {
        let f = dc.path().join("data").join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, b).unwrap();
    }
    let layout = shaiya_archive::loose::detect(dc.path()).unwrap().unwrap();
    let t = shaiya_clientdata::LooseTree::open(&layout, &Job::silent()).unwrap();
    assert!(coloured(&model(preview(&t, TORSO, &Look::default())).rgba) > 0);
}
