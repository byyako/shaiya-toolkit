//! The signature settings of one client, shared by Apply & history and Sign & verify: the public key (a file or pasted hex), the
//! CheckVersion and, only when the person ticked "Remember where my secret key is", the path of the secret key file. Shaiya Toolkit never
//! stores key contents.

use crate::clients::ClientInfo;
use crate::{KeySettings, ToolkitSettings};
use shaiya_patch::VerifyKey;
use std::path::Path;

#[derive(Default, Clone, Debug)]
pub struct KeyForm {
    for_client: Option<String>,
    loaded: bool,
    pub public_path: String,
    pub public_hex: String,
    pub check_version: String,
    pub secret_path: String,
    pub remember_secret: bool,
}

impl KeyForm {
    /// Loads the settings of `client` when it is not the one loaded already (CheckVersion pre-filled from `Version.ini`).
    pub fn load(&mut self, client: Option<&ClientInfo>, settings: &ToolkitSettings) {
        let name = client.map(|c| c.name.clone());
        if self.loaded && self.for_client == name {
            return;
        }
        self.loaded = true;
        self.for_client = name.clone();
        let k = name.as_ref().and_then(|n| settings.keys.get(n)).cloned().unwrap_or_default();
        self.public_path = k.public_key_path;
        self.public_hex = k.public_hex;
        self.check_version = if k.check_version.is_empty() { client.and_then(|c| c.check_version).map(|v| v.to_string()).unwrap_or_default() } else { k.check_version };
        self.remember_secret = !k.secret_key_path.is_empty();
        self.secret_path = k.secret_key_path;
    }

    /// True when the loaded settings belong to a different client than `client` (the tab must `load` again).
    pub fn is_for(&self, client: Option<&ClientInfo>) -> bool {
        self.loaded && self.for_client == client.map(|c| c.name.clone())
    }

    pub fn store(&self, settings: &mut ToolkitSettings) {
        let Some(name) = &self.for_client else { return };
        settings.keys.insert(
            name.clone(),
            KeySettings {
                public_key_path: self.public_path.clone(),
                public_hex: self.public_hex.clone(),
                check_version: self.check_version.clone(),
                secret_key_path: if self.remember_secret { self.secret_path.clone() } else { String::new() },
            },
        );
    }

    /// The public key and CheckVersion, when both are usable (64 hex characters, a number).
    pub fn verify_key(&self) -> Option<VerifyKey> {
        let check_version: u32 = self.check_version.trim().parse().ok()?;
        let public_hex = if !self.public_hex.trim().is_empty() {
            self.public_hex.trim().to_string()
        } else if !self.public_path.trim().is_empty() {
            shaiya_patch::sign::read_key(Path::new(self.public_path.trim())).ok()?
        } else {
            return None;
        };
        (public_hex.len() == 64 && public_hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some(VerifyKey { public_hex, check_version })
    }

    pub fn check_version_number(&self) -> Option<u32> {
        self.check_version.trim().parse().ok()
    }
}
