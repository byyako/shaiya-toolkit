//! Shared basics for the Shaiya toolkit: one error type, the job/progress/cancel API, safe file writing and backups, a bounds-checked
//! binary reader, text encodings, hashing and UTC time formatting. No game data, no network, no printing.

pub mod bin;
pub mod error;
pub mod fs;
pub mod hash;
pub mod job;
pub mod text;
pub mod time;

pub use error::{Error, ErrorKind, Result};
pub use job::{Cancel, FnProgress, Job, Level, Progress, Silent, Unit};
