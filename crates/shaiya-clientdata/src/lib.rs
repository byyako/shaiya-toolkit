//! Readers for Shaiya client data: files in `data.sah`/`data.saf` or an extracted
//! folder, `.wld` maps, `.dg` dungeons, `.ANI` animations, `.3DC` skinned meshes, `.VANI` vertex-animated models, `.MANI` building motion, `.3DO` meshes, `.3DE` effect meshes, the model list files `.MLT`, `.MON` and `.ITM` (and the texture index built from them), SEED-encrypted SData (`Monster`, `NpcQuest`), `sysmsg-uni.txt`, map names and textures.
//! Readers only, except the JSON form of the six model formats (`modeljson`), the lossless `.wld` model (`wld_raw`), the `<id>.txt` line model (`area_txt`), the `.ANI` model (`anim`), the `.3DC` model (`mesh3dc`), the `.VANI` model (`vani`), the `.MANI` file (`mani`), the `.3DO` model (`mesh3do`), the `.3DE` model (`mesh3de`) and the list files (`modellist`) and the glTF export (`interchange`), which build bytes
//! in memory: nothing here writes a file.

pub mod anim;
pub mod animedit;
pub mod area_txt;
pub mod dg;
pub mod error;
pub mod interchange;
pub mod mani;
pub mod mesh3dc;
pub mod mesh3de;
pub mod mesh3do;
pub mod meshpick;
pub mod modellist;
pub mod modeljson;
pub mod render;
pub mod sdata;
pub mod seed;
pub mod smod;
pub mod source;
pub mod texindex;
pub mod text;
pub mod texture;
pub mod vani;
pub mod wld;
pub mod wld_raw;

pub use anim::Ani;
pub use dg::{dungeon_map_size, Dg};
pub use error::{Error, Result};
pub use mani::Mani;
pub use mesh3dc::C3dc;
pub use mesh3de::Mesh3de;
pub use mesh3do::Mesh3do;
pub use modellist::{Itm, Mlt, Mon};
pub use sdata::{Monster, Monsters, Npc, NpcQuest};
pub use smod::Smod;
pub use source::{texture_candidates, ClientSource, Probe, Probed};
pub use text::{map_name, map_name_msg, AreaNames, SysMsg};
pub use texindex::{AlphaUse, Found, IndexReport, TexRef, TextureIndex};
pub use texture::Image;
pub use vani::Vani;
pub use wld::{Terrain, Wld};
