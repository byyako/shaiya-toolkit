//! Opening a file or folder with the operating system's own opener (Explorer, Finder, the desktop's default). No shell and no URL: the
//! path is handed over as one argument.

use crate::settings::Os;
use std::ffi::OsString;
use std::path::Path;

/// The program and arguments that open `path` on `os`: one argument, the path itself.
pub fn opener(os: Os, path: &Path) -> (&'static str, Vec<OsString>) {
    let program = match os {
        Os::Windows => "explorer.exe",
        Os::Mac => "open",
        Os::Linux => "xdg-open",
    };
    (program, vec![path.as_os_str().to_os_string()])
}

/// Opens `path` (a folder, or a file with its default program) with the system opener.
pub fn open_path(path: &Path) -> Result<(), String> {
    let (program, args) = opener(Os::CURRENT, path);
    std::process::Command::new(program).args(args).spawn().map(|_| ()).map_err(|e| format!("cannot open {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opener_per_os() {
        let p = Path::new("C:/Some Folder/Tom & Jerry/log.txt");
        let one = vec![p.as_os_str().to_os_string()];
        assert_eq!(opener(Os::Windows, p), ("explorer.exe", one.clone()));
        assert_eq!(opener(Os::Mac, p), ("open", one.clone()));
        assert_eq!(opener(Os::Linux, p), ("xdg-open", one));
    }
}
