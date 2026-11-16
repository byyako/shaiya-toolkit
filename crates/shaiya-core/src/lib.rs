//! Shared basics for the Shaiya toolkit: one error type, the job/progress/cancel API, safe file writing and backups, a bounds-checked
//! binary reader, text encodings, hashing, UTC time formatting, glob patterns, byte sources, and (feature `config`) the data folder and
//! the shared client list. No game data, no network, no printing.

pub mod bin;
#[cfg(feature = "config")]
pub mod config;
pub mod error;
pub mod fs;
pub mod glob;
pub mod hash;
pub mod job;
pub mod source;
pub mod text;
pub mod time;

pub use error::{Error, ErrorKind, Result};
pub use job::{Cancel, FnProgress, Job, Level, Progress, Silent, Unit};
