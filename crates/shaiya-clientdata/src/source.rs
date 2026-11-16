//! Access to the client's files: read in place from `data.sah` + `data.saf` (archive mode) or from an extracted folder
//! (folder mode). Paths use `/` or `\` and are case-insensitive, like the client's own lookups. Nothing is written.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use shaiya_archive::Archive;

use crate::dg::Dg;
use crate::error::{Error, Result};
use crate::sdata::{Monsters, NpcQuest};
use crate::seed;
use crate::smod::Smod;
use crate::text::{self, AreaNames, SysMsg};
use crate::texture::{self, Image};
use crate::wld::Wld;

enum Backing {
    Archive(Archive),
    Folder,
}

pub struct ClientSource {
    dir: PathBuf,
    backing: Backing,
    /// Texture problems already reported (a missing texture is logged once).
    problems: Mutex<Vec<String>>,
    reported: Mutex<HashSet<String>>,
}

impl ClientSource {
    /// `dir` holds `data.sah` + `data.saf` (archive mode) or the extracted folders (`world/`, `Monster/`, ...).
    pub fn open(dir: &Path) -> Result<ClientSource> {
        if let Some(sah) = ["data.sah", "Data.sah", "DATA.SAH"].iter().map(|n| dir.join(n)).find(|p| p.is_file()) {
            return Self::open_archive(&sah);
        }
        if !dir.is_dir() {
            return Err(Error::NotFound(dir.display().to_string()));
        }
        Ok(ClientSource { dir: dir.to_path_buf(), backing: Backing::Folder, problems: Mutex::new(Vec::new()), reported: Mutex::new(HashSet::new()) })
    }

    /// Open a specific archive index (`<name>.sah` with `<name>.saf` beside it), e.g. `data.orig.sah`.
    pub fn open_archive(sah: &Path) -> Result<ClientSource> {
        let archive = Archive::open(sah)?;
        let dir = sah.parent().unwrap_or(Path::new(".")).to_path_buf();
        Ok(ClientSource { dir, backing: Backing::Archive(archive), problems: Mutex::new(Vec::new()), reported: Mutex::new(HashSet::new()) })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn is_archive(&self) -> bool {
        matches!(self.backing, Backing::Archive(_))
    }

    /// Whether the client has this file.
    pub fn exists(&self, path: &str) -> bool {
        match &self.backing {
            Backing::Archive(a) => a.index().get(path).is_some(),
            Backing::Folder => self.folder_path(path).is_some(),
        }
    }

    /// Resolve a path case-insensitively inside the extracted folder.
    fn folder_path(&self, path: &str) -> Option<PathBuf> {
        let mut cur = self.dir.clone();
        for part in path.split(['/', '\\']).filter(|p| !p.is_empty()) {
            if part == ".." || part == "." || part.contains(':') {
                return None;
            }
            let direct = cur.join(part);
            if direct.exists() {
                cur = direct;
                continue;
            }
            let hit = std::fs::read_dir(&cur).ok()?.filter_map(|e| e.ok()).find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?;
            cur = hit.path();
        }
        cur.is_file().then_some(cur)
    }

    /// The bytes of a client file.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        match &self.backing {
            Backing::Archive(a) => {
                let e = a.index().get(path).ok_or_else(|| Error::NotFound(path.to_string()))?;
                Ok(a.read_entry(e)?)
            }
            Backing::Folder => {
                let p = self.folder_path(path).ok_or_else(|| Error::NotFound(path.to_string()))?;
                std::fs::read(&p).map_err(|s| Error::Io { path: p.display().to_string(), source: s })
            }
        }
    }

    /// All file paths of the client (archive paths as stored, or the extracted folder's relative paths), in no
    /// particular order.
    pub fn list(&self) -> Vec<String> {
        match &self.backing {
            Backing::Archive(a) => a.index().files().into_iter().map(|(p, _)| p).collect(),
            Backing::Folder => {
                fn walk(d: &Path, prefix: &str, out: &mut Vec<String>) {
                    let Ok(rd) = std::fs::read_dir(d) else { return };
                    for e in rd.filter_map(|e| e.ok()) {
                        let name = e.file_name().to_string_lossy().into_owned();
                        let p = e.path();
                        if p.is_dir() {
                            walk(&p, &format!("{prefix}{name}/"), out);
                        } else {
                            out.push(format!("{prefix}{name}"));
                        }
                    }
                }
                let mut out = Vec::new();
                walk(&self.dir, "", &mut out);
                out
            }
        }
    }

    /// Every `.wld` file of the client (`world/<name>.wld`): the numbered maps and the login/select/strip scenes.
    pub fn wld_paths(&self) -> Vec<String> {
        let mut v: Vec<String> = self.list().into_iter().filter(|p| p.to_ascii_lowercase().ends_with(".wld")).collect();
        v.sort();
        v
    }

    /// Map ids that have a `world/<id>.wld`, ascending.
    pub fn map_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .list()
            .iter()
            .filter_map(|p| {
                let (dir, name) = p.rsplit_once('/')?;
                if !dir.eq_ignore_ascii_case("world") {
                    return None;
                }
                let stem = name.strip_suffix(".wld").or_else(|| name.strip_suffix(".WLD"))?;
                stem.parse().ok()
            })
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    // ------------------------------------------------------------ typed readers

    pub fn wld(&self, map: u32) -> Result<Wld> {
        let path = format!("world/{map}.wld");
        Wld::parse(&self.read(&path)?, &path)
    }

    /// `world/<map>.txt` area names; no file gives an empty list.
    pub fn area_names(&self, map: u32) -> Result<AreaNames> {
        let path = format!("world/{map}.txt");
        match self.read(&path) {
            Ok(b) => text::parse_area_names(&b, &path),
            Err(Error::NotFound(_)) => Ok(AreaNames::default()),
            Err(e) => Err(e),
        }
    }

    /// A dungeon's geometry by the name a `.wld` gives (`world/dungeon/<name>.dg`; the extension is added when missing).
    pub fn dg(&self, name: &str) -> Result<Dg> {
        let file = if name.to_ascii_lowercase().ends_with(".dg") { name.to_string() } else { format!("{name}.dg") };
        let path = format!("world/dungeon/{file}");
        Dg::parse(&self.read(&path)?, &path)
    }

    /// A static model a `.wld` places: `dir` is `Building`, `Shape` or `Tree` (`Entity/<dir>/<name>`).
    pub fn smod(&self, dir: &str, name: &str) -> Result<Smod> {
        let path = format!("Entity/{dir}/{}", name.trim());
        Smod::parse(&self.read(&path)?, &path)
    }

    pub fn monsters(&self) -> Result<Monsters> {
        let path = "Monster/Monster.SData";
        let plain = seed::decrypt(&self.read(path)?, path)?;
        Monsters::parse(&plain, path)
    }

    pub fn npc_quest(&self) -> Result<NpcQuest> {
        let path = "Npc/NpcQuest.SData";
        let plain = seed::decrypt(&self.read(path)?, path)?;
        NpcQuest::parse(&plain, path)
    }

    pub fn sysmsg(&self) -> Result<SysMsg> {
        let path = "sysmsg-uni.txt";
        SysMsg::parse(&self.read(path)?, path)
    }

    /// Map name as the client shows it (see [`text::map_name`]).
    pub fn map_name(&self, map: u32) -> String {
        text::map_name(map, self.sysmsg().ok().as_ref())
    }

    /// DDS / TGA / BMP file to RGBA8.
    pub fn texture(&self, path: &str) -> Result<Image> {
        texture::decode(&self.read(path)?, path)
    }

    /// The path of a texture the client has under `dir`: the name as a `.wld`/`.dg` gives it, else the same name with the other
    /// texture extension (the cleaned `data.sah` keeps the `.tga` textures as `.dds`, while the files still name `.tga`).
    pub fn find_texture(&self, dir: &str, name: &str) -> Option<String> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        [name.to_string(), format!("{stem}.dds"), format!("{stem}.tga")].into_iter().map(|c| format!("{dir}/{c}")).find(|p| self.exists(p))
    }

    /// [`texture`](Self::texture) by the name a map file gives, with the extension fallback of [`find_texture`](Self::find_texture).
    pub fn texture_flex(&self, dir: &str, name: &str) -> Result<Image> {
        match self.find_texture(dir, name) {
            Some(p) => self.texture(&p),
            None => Err(Error::NotFound(format!("{dir}/{name}"))),
        }
    }

    /// Like [`texture`](Self::texture) but never fails: a missing or broken texture becomes the magenta checker and is
    /// added to [`problems`](Self::problems) (once per path).
    pub fn texture_or_checker(&self, path: &str) -> Image {
        match self.texture(path) {
            Ok(i) => i,
            Err(e) => {
                let first = self.reported.lock().unwrap_or_else(|p| p.into_inner()).insert(path.to_ascii_lowercase());
                if first {
                    self.problems.lock().unwrap_or_else(|p| p.into_inner()).push(e.to_string());
                }
                Image::checker(32)
            }
        }
    }

    /// Problems collected so far (textures that were missing or broken).
    pub fn problems(&self) -> Vec<String> {
        self.problems.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}
