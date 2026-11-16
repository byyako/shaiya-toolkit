// Temporary until shaiya-build exists (T0 A8): records the git hash and the target for `shaiya version`.
fn main() {
    let hash = std::process::Command::new("git")
        .args(["-c", "safe.directory=*", "rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=SHAIYA_GIT_HASH={hash}");
    println!("cargo:rustc-env=SHAIYA_TARGET={}", std::env::var("TARGET").unwrap_or_default());
    println!("cargo:rerun-if-changed=build.rs");
}
