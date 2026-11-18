//! Shaiya server map files (`PS_Game/Data/Map/<id>.svmap`): model, reader, writer, validation, bitmap operations and JSON.
//! No client files and no GUI here. The format is documented in `docs/formats/svmap.md`; it was verified on the original EP4.5
//! server map files.

pub mod bitgrid;
pub mod diff;
pub mod json;
pub mod model;
pub mod read;
pub mod validate;
pub mod write;

pub use bitgrid::{BitGrid, Run};
pub use json::{from_json, to_json, to_json_pretty, JsonError};
pub use model::*;
pub use read::{read, ReadError, ReadErrorKind, SECTION_NAMES};
pub use validate::{tally, validate, validate_cross, Finding, SectionId, Severity};
pub use write::write;
