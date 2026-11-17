//! Patch signatures (Ed25519). The patch author keeps the secret key; the updater has the public key built in and
//! refuses patches whose `psNNNN.patch.sig` does not verify. The signed message binds the client generation
//! (`CheckVersion`) and the file name (and so the version number) to the file's SHA-256, so a signed patch cannot be
//! served under another version's name or replayed to a client of another generation.
//!
//! Message: `"shaiya-patch-v2\0" + CheckVersion (decimal) + "\0" + lower-case file name + "\0" + SHA-256(file)`.
//! Keys and signatures are stored as lower-case hex text.
//!
//! Rule for publishers: never sign a different file under a version number that was already published (an old signature
//! for that name would still verify); ship fixes as the next version. Bumping CheckVersion invalidates every old signature.
//!
//! The updater's own new version (`new_updater.exe`) is signed the same way under the name [`updater_name`]
//! (`shaiya-updater-<UpdaterVersion>.exe`), so a patch signature can never pass as an updater signature, and a signed
//! updater cannot be offered under a higher version number than it was signed for.
//!
//! The message format is unchanged: deployed updaters verify it.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use shaiya_core::fs::{write_atomic, Existing};
use shaiya_core::{Error, Job, Result};
use std::path::{Path, PathBuf};

fn message(check_version: u32, file_name: &str, digest: &[u8; 32]) -> Vec<u8> {
    let mut m = b"shaiya-patch-v2\0".to_vec();
    m.extend_from_slice(check_version.to_string().as_bytes());
    m.push(0);
    m.extend_from_slice(file_name.to_ascii_lowercase().as_bytes());
    m.push(0);
    m.extend_from_slice(digest);
    m
}

/// Lower-case hex (one implementation, in `shaiya_core::hash`).
pub fn to_hex(b: &[u8]) -> String {
    shaiya_core::hash::to_hex(b)
}

pub fn from_hex<const N: usize>(s: &str) -> Result<[u8; N]> {
    shaiya_core::hash::from_hex(s)
}

pub fn sha256_file(path: &Path) -> Result<[u8; 32]> {
    shaiya_core::hash::sha256_file(path, &Job::silent())
}

/// Name an updater build is signed under: binds its `UpdaterVersion`.
pub fn updater_name(updater_version: u32) -> String {
    format!("shaiya-updater-{updater_version}.exe")
}

/// New key pair as (secret hex, public hex).
pub fn keygen() -> (String, String) {
    let sk = SigningKey::generate(&mut rand_core::OsRng);
    (to_hex(&sk.to_bytes()), to_hex(sk.verifying_key().as_bytes()))
}

pub fn sign(secret_hex: &str, check_version: u32, file_name: &str, digest: &[u8; 32]) -> Result<String> {
    let sk = SigningKey::from_bytes(&from_hex::<32>(secret_hex)?);
    Ok(to_hex(&sk.sign(&message(check_version, file_name, digest)).to_bytes()))
}

pub fn verify(public_hex: &str, check_version: u32, file_name: &str, digest: &[u8; 32], sig_hex: &str) -> Result<()> {
    let pk = VerifyingKey::from_bytes(&from_hex::<32>(public_hex)?).map_err(|e| Error::format("public key", None, e.to_string()))?;
    let sig = Signature::from_bytes(&from_hex::<64>(sig_hex)?);
    pk.verify(&message(check_version, file_name, digest), &sig).map_err(|_| {
        Error::format(file_name, None, "signature does not match (file altered, or signed for another name/version or with another key)")
    })
}

/// `<file>.sig`.
pub fn signature_path(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_os_string();
    s.push(".sig");
    PathBuf::from(s)
}

/// Reads a key or signature file: trims, checks for hex of the given length.
fn read_hex_file(path: &Path, digits: usize, what: &str) -> Result<String> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let t = text.trim();
    if t.len() != digits || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::format(path.display().to_string(), None, format!("not a {what}: expected {digits} hex characters")));
    }
    Ok(t.to_ascii_lowercase())
}

/// A key file (secret or public): trims; checks 64 hex characters.
pub fn read_key(path: &Path) -> Result<String> {
    read_hex_file(path, 64, "key file")
}

/// `create_new` for both files (never overwrites; `Error::Refused` when either exists); the secret is written with mode 0600 on Unix.
/// Returns the public key as hex.
pub fn write_key_pair(secret: &Path, public: &Path) -> Result<String> {
    for p in [secret, public] {
        if p.exists() {
            return Err(Error::Refused(format!("{} already exists; keys are never overwritten", p.display())));
        }
    }
    let (sk, pk) = keygen();
    let create = |p: &Path, secret: bool| -> Result<()> {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        if secret {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let _ = secret;
        let mut f = o.open(p).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                Error::Refused(format!("{} already exists; keys are never overwritten", p.display()))
            } else {
                Error::io(p, e)
            }
        })?;
        use std::io::Write;
        let text = format!("{}\n", if secret { &sk } else { &pk });
        f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| Error::io(p, e))
    };
    create(secret, true)?;
    if let Err(e) = create(public, false) {
        let _ = std::fs::remove_file(secret);
        return Err(e);
    }
    Ok(pk)
}

fn name_for(file: &Path, name: Option<&str>) -> String {
    match name {
        Some(n) => n.to_string(),
        None => file.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default(),
    }
}

/// Signs `file` under its own lower-cased file name, or under `name` (updater: [`updater_name`]); writes `<file>.sig` per `existing`
/// (`Refuse` -> `Error::Refused` when it exists; `Skip` leaves it). Returns the signature file's path.
pub fn sign_file(secret_hex: &str, check_version: u32, file: &Path, name: Option<&str>, existing: Existing, job: &Job) -> Result<PathBuf> {
    let sig_path = signature_path(file);
    if sig_path.exists() {
        match existing {
            Existing::Refuse => return Err(Error::Refused(format!("{} already exists; use --force to replace it (never re-sign a published patch number)", sig_path.display()))),
            Existing::Skip => return Ok(sig_path),
            Existing::Overwrite => {}
        }
    }
    let digest = shaiya_core::hash::sha256_file(file, job)?;
    let sig = sign(secret_hex, check_version, &name_for(file, name), &digest)?;
    write_atomic(&sig_path, format!("{sig}\n").as_bytes())?;
    Ok(sig_path)
}

/// Checks `<file>.sig` against `file`; `Error::Refused` when the signature file is missing, `Error::Format` when it does not match.
pub fn verify_file(public_hex: &str, check_version: u32, file: &Path, name: Option<&str>, job: &Job) -> Result<()> {
    let sig_path = signature_path(file);
    if !sig_path.is_file() {
        return Err(Error::Refused(format!("signature file {} not found", sig_path.display())));
    }
    let sig = read_hex_file(&sig_path, 128, "signature file")?;
    let digest = shaiya_core::hash::sha256_file(file, job)?;
    verify(public_hex, check_version, &name_for(file, name), &digest, &sig)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify() {
        let (sk, pk) = keygen();
        let d = [7u8; 32];
        let s = sign(&sk, 3, "ps0109.patch", &d).unwrap();
        assert!(verify(&pk, 3, "PS0109.PATCH", &d, &s).is_ok());
        assert!(verify(&pk, 3, "ps0110.patch", &d, &s).is_err(), "renamed patch must fail");
        assert!(verify(&pk, 3, "ps0109.patch", &[8u8; 32], &s).is_err(), "altered patch must fail");
        assert!(verify(&pk, 4, "ps0109.patch", &d, &s).is_err(), "other client generation must fail");
        let (_, other) = keygen();
        assert!(verify(&other, 3, "ps0109.patch", &d, &s).is_err(), "other key must fail");
    }

    #[test]
    fn updater_signature_is_bound_to_its_version() {
        let (sk, pk) = keygen();
        let d = [9u8; 32];
        let s = sign(&sk, 3, &updater_name(2), &d).unwrap();
        assert!(verify(&pk, 3, &updater_name(2), &d, &s).is_ok());
        assert!(verify(&pk, 3, &updater_name(3), &d, &s).is_err(), "must not pass as a later version");
        let p = sign(&sk, 3, "ps0002.patch", &d).unwrap();
        assert!(verify(&pk, 3, &updater_name(2), &d, &p).is_err(), "a patch signature is not an updater signature");
    }

    /// The signed message is a wire format that deployed updaters check: pin it with a fixed key, so a change cannot slip through.
    #[test]
    fn the_message_bytes_are_pinned() {
        let m = message(3, "PS0109.patch", &[0xAB; 32]);
        let mut want = b"shaiya-patch-v2\x003\x00ps0109.patch\x00".to_vec();
        want.extend_from_slice(&[0xAB; 32]);
        assert_eq!(m, want);
        // Ed25519 is deterministic: the same key signs the same message to the same bytes
        let sk = to_hex(&[1u8; 32]);
        let a = sign(&sk, 3, "ps0109.patch", &[0xAB; 32]).unwrap();
        assert_eq!(a, sign(&sk, 3, "PS0109.PATCH", &[0xAB; 32]).unwrap());
        assert_eq!(a.len(), 128);
    }
}
