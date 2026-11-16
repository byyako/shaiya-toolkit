//! `shaiya version`

use crate::output::Report;
use serde::Serialize;
use std::io::Write;

#[derive(Serialize)]
pub struct VersionReport {
    pub version: &'static str,
    pub git: &'static str,
    pub target: &'static str,
}

pub fn current() -> VersionReport {
    VersionReport { version: env!("CARGO_PKG_VERSION"), git: env!("SHAIYA_GIT_HASH"), target: env!("SHAIYA_TARGET") }
}

impl VersionReport {
    pub fn line(&self) -> String {
        format!("shaiya {} (git {}, {})", self.version, self.git, self.target)
    }
}

impl Report for VersionReport {
    fn human(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "{}", self.line())
    }
}
