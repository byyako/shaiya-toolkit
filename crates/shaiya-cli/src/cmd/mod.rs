//! One module per command group. Each group adds `src/cmd/<group>.rs` and one enum variant in `main.rs`.

pub mod archive;
pub mod clients;
pub mod info;
pub mod patch;
pub mod sdata;
pub mod svmap;
pub mod util;
pub mod version;
