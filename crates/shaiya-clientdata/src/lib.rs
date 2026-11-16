//! Read-only readers for Shaiya client data: files in `data.sah`/`data.saf` or an extracted
//! folder, `.wld` maps, `.dg` dungeons, SEED-encrypted SData (`Monster`, `NpcQuest`), `sysmsg-uni.txt`, map names and textures.
//! Read-only: nothing here writes client files.

pub mod dg;
pub mod error;
pub mod sdata;
pub mod seed;
pub mod smod;
pub mod source;
pub mod text;
pub mod texture;
pub mod wld;

pub use dg::{dungeon_map_size, Dg};
pub use error::{Error, Result};
pub use sdata::{Monster, Monsters, Npc, NpcQuest};
pub use smod::Smod;
pub use source::ClientSource;
pub use text::{map_name, map_name_msg, AreaNames, SysMsg};
pub use texture::Image;
pub use wld::{Terrain, Wld};
