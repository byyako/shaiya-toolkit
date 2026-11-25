//! Opt-in: files of a real US client copied into a new Shaiya-GX-shaped folder read as a loose client (`SHAIYA_CLIENT` + `SHAIYA_SCRATCH`).
//! The folder is made under the scratch folder from the client's own bytes, read back through `LooseTree`, and removed by a `Drop` guard;
//! the client is only read.

use shaiya_archive::loose::detect;
use shaiya_archive::Archive;
use shaiya_clientdata::LooseTree;
use shaiya_core::{Job, Silent};
use shaiya_testkit::{env_client, env_scratch};
use shaiya_toolkit::preview::{make_preview_look, MeshChoice, MeshInfoCache, PreviewData, TextureIndexCache};
use shaiya_toolkit::preview_tex::Look;
use shaiya_toolkit::source::FileSource;
use std::path::PathBuf;

struct Gone(PathBuf);

impl Drop for Gone {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rgba_of(d: &PreviewData) -> Vec<u8> {
    match d {
        PreviewData::Model { rgba, .. } => rgba.clone(),
        other => panic!("not a model preview: {}", other.kind()),
    }
}

#[test]
fn an_extracted_copy_of_us_client_files_reads_as_a_loose_client() {
    let (Some(client), Some(scratch)) = (env_client(), env_scratch()) else {
        println!("skipped: SHAIYA_CLIENT or SHAIYA_SCRATCH not set");
        return;
    };
    let a = Archive::open(&client).unwrap();
    let all: Vec<String> = { let mut v: Vec<String> = a.index().visible().into_iter().map(|f| f.path).collect(); v.sort(); v };
    let first = |pred: &dyn Fn(&str) -> bool| all.iter().find(|p| pred(&p.to_ascii_lowercase())).unwrap_or_else(|| panic!("no such file in the client")).clone();
    let ani = first(&|p| p.starts_with("character/human/ani/huwm_") && p.ends_with(".ani"));
    let mesh = first(&|p| p.starts_with("character/human/3dc/huwm_torso") && p.ends_with(".3dc"));
    let mlt = first(&|p| p.starts_with("character/human/") && p.ends_with(".mlt"));
    let eft = first(&|p| p.ends_with(".eft"));
    let wav = first(&|p| p.ends_with(".wav"));
    // every texture of the human-woman rig (the names the list files give end in .dds)
    let dds: Vec<String> = all.iter().filter(|p| p.to_ascii_lowercase().starts_with("character/human/") && p.to_ascii_lowercase().contains("/dds/") && p.to_ascii_lowercase().ends_with(".dds")).cloned().collect();
    assert!(!dds.is_empty(), "the client has textures under Character/Human");
    let mut chosen: Vec<String> = vec![ani.clone(), mesh.clone(), mlt, eft, wav.clone()];
    chosen.extend(dds);
    // two more marker folders so the folder is a client, and the GX sign
    chosen.push(first(&|p| p.starts_with("item/") && p.ends_with(".sdata")));

    let root = scratch.join(format!("loose-optin-{}", std::process::id()));
    let _gone = Gone(root.clone());
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("Client").join("data");
    for p in &chosen {
        let target = data.join(p.replace('\\', "/"));
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, a.read(p).unwrap()).unwrap();
    }
    std::fs::create_dir_all(data.join("Sound/Music")).unwrap();
    std::fs::write(data.join("Sound/Music/theme.ogg"), b"OggS").unwrap();

    // detect: layout 2 under Client/, a Shaiya-GX sign
    let layout = detect(&root.join("Client")).unwrap().expect("detected");
    assert_eq!((layout.root.clone(), layout.data.clone(), layout.gx), (root.join("Client"), data.clone(), true));
    let tree = LooseTree::open(&layout, &Job::silent()).unwrap();
    assert_eq!(tree.files().len(), chosen.len() + 1, "{:?}", tree.notes());
    assert!(tree.notes().is_empty());
    println!("loose files {}, gx {}", tree.files().len(), layout.gx);
    // the bytes are the client's own, found in any letter case
    for p in [&ani, &mesh, &wav] {
        assert_eq!(tree.read(&p.to_uppercase(), u64::MAX, &Job::silent()).unwrap(), a.read(p).unwrap(), "{p}");
        assert_eq!(FileSource::stamp(&tree, p).map(|s| s.1), Some(a.read(p).unwrap().len() as u64));
    }
    // the textured and plain previews of the mesh from the loose folder have the same outcome as from the archive
    let (cache, textures) = (MeshInfoCache::default(), TextureIndexCache::default());
    let job = Job::new(&Silent, shaiya_core::Cancel::new());
    let look = |textured| Look { textured, ..Look::default() };
    let preview = |src: &dyn FileSource, textured: bool| make_preview_look(src, &mesh, false, 0, &MeshChoice::Default, &cache, &textures, &look(textured), &job).unwrap();
    let (loose_on, loose_off) = (rgba_of(&preview(&tree, true)), rgba_of(&preview(&tree, false)));
    assert!(!loose_on.is_empty() && !loose_off.is_empty());
    let (arch_on, arch_off) = (rgba_of(&preview(&a, true)), rgba_of(&preview(&a, false)));
    println!("textured differs from plain: loose {}, archive {}", loose_on != loose_off, arch_on != arch_off);
    assert_eq!(loose_on != loose_off, arch_on != arch_off, "the texture outcome of the loose folder is the archive's");
    // the animation previews from the loose folder, and exports to JSON that imports to the file's bytes
    let ani_preview = make_preview_look(&tree, &ani, false, 0, &MeshChoice::Default, &MeshInfoCache::default(), &TextureIndexCache::default(), &Look::default(), &job).unwrap();
    assert_eq!(ani_preview.kind(), "anim");
    let bytes = tree.read(&ani, u64::MAX, &Job::silent()).unwrap();
    let json = shaiya_clientdata::modeljson::export(shaiya_clientdata::modeljson::ModelFormat::Ani, &bytes, &ani).unwrap();
    assert_eq!(shaiya_clientdata::modeljson::import(json.as_bytes()).unwrap().1, bytes);
}
