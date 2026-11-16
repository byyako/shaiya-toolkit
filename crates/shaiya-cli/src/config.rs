//! The data folder and the saved client folders (`@name`), read from the environment here and nowhere else (library code never reads it).

use shaiya_core::config::{self, ClientFolder, DataDirInputs};
use shaiya_core::{Error, Result};
use std::path::{Path, PathBuf};

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn inputs_from_env() -> DataDirInputs {
    DataDirInputs {
        toolkit_home: env_path("SHAIYA_TOOLKIT_HOME"),
        exe_dir: std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)),
        appdata: env_path("APPDATA"),
        home: env_path("HOME").or_else(|| env_path("USERPROFILE")),
        xdg_config_home: env_path("XDG_CONFIG_HOME"),
    }
}

/// The toolkit data folder; `--data-dir` overrides everything (it plays the part of `SHAIYA_TOOLKIT_HOME`).
pub fn data_root(data_dir: Option<&Path>) -> Option<PathBuf> {
    let mut i = inputs_from_env();
    if let Some(d) = data_dir {
        i.toolkit_home = Some(d.to_path_buf());
    }
    config::data_root(&i)
}

pub fn load_clients(data_dir: Option<&Path>) -> Result<Vec<ClientFolder>> {
    match data_root(data_dir) {
        Some(root) => config::load_clients(&root),
        None => Ok(Vec::new()),
    }
}

/// `@name` -> the saved folder; anything else is a path as given. An unknown name is an input error (exit 3) with a hint.
pub fn resolve(data_dir: Option<&Path>, arg: &str) -> Result<PathBuf> {
    let Some(name) = arg.strip_prefix('@') else { return Ok(PathBuf::from(arg)) };
    let clients = load_clients(data_dir)?;
    match config::find_client(&clients, name) {
        Some(c) => Ok(c.path.clone()),
        None => Err(Error::io(
            PathBuf::from(arg),
            std::io::Error::new(std::io::ErrorKind::NotFound, "no saved client folder with this name (hint: shaiya clients list)"),
        )),
    }
}
