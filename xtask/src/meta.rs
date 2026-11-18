//! `cargo metadata` and the dependency graphs of the shipped programs.

use crate::tools::TOOLS;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

pub struct Pkg {
    pub name: String,
    pub version: String,
    pub license: Option<String>,
    pub manifest_path: PathBuf,
    pub is_workspace: bool,
}

impl Pkg {
    pub fn dir(&self) -> PathBuf {
        self.manifest_path.parent().map(PathBuf::from).unwrap_or_default()
    }
}

pub struct Graph {
    pub packages: BTreeMap<String, Pkg>,
    /// Package ids reachable from the programs of `TOOLS` through normal (non-dev, non-build) dependencies, the programs included.
    pub shipped: BTreeSet<String>,
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask sits in the repository").to_path_buf()
}

/// Loads the metadata of the workspace and the graph of the shipped programs.
pub fn load() -> Result<Graph, String> {
    let out = Command::new(cargo())
        .args(["metadata", "--format-version", "1", "--locked"])
        .current_dir(root())
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata gave bad JSON: {e}"))?;
    graph_from(&v)
}

pub fn graph_from(v: &Value) -> Result<Graph, String> {
    let ws: BTreeSet<&str> = v["workspace_members"].as_array().ok_or("no workspace_members")?.iter().filter_map(Value::as_str).collect();
    let mut packages = BTreeMap::new();
    for p in v["packages"].as_array().ok_or("no packages")? {
        let id = p["id"].as_str().ok_or("package without id")?.to_string();
        packages.insert(
            id.clone(),
            Pkg {
                name: p["name"].as_str().unwrap_or("?").to_string(),
                version: p["version"].as_str().unwrap_or("?").to_string(),
                license: p["license"].as_str().map(str::to_string),
                manifest_path: PathBuf::from(p["manifest_path"].as_str().unwrap_or("")),
                is_workspace: ws.contains(id.as_str()),
            },
        );
    }
    let mut deps: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for n in v["resolve"]["nodes"].as_array().ok_or("no resolve nodes")? {
        let id = n["id"].as_str().unwrap_or("");
        let list = n["deps"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| d["dep_kinds"].as_array().is_some_and(|k| k.iter().any(|k| k["kind"].is_null())))
            .filter_map(|d| d["pkg"].as_str())
            .collect();
        deps.insert(id, list);
    }
    let roots: Vec<String> = packages.iter().filter(|(_, p)| p.is_workspace && TOOLS.iter().any(|t| t.package == p.name)).map(|(id, _)| id.clone()).collect();
    if roots.len() != TOOLS.len() {
        return Err(format!("expected {} program packages, found {}", TOOLS.len(), roots.len()));
    }
    let mut shipped = BTreeSet::new();
    let mut stack: Vec<String> = roots;
    while let Some(id) = stack.pop() {
        if !shipped.insert(id.clone()) {
            continue;
        }
        for d in deps.get(id.as_str()).into_iter().flatten() {
            stack.push((*d).to_string());
        }
    }
    Ok(Graph { packages, shipped })
}

/// The workspace version from the root `Cargo.toml` (`[workspace.package] version = "x.y.z"`).
pub fn workspace_version() -> Result<String, String> {
    let text = std::fs::read_to_string(root().join("Cargo.toml")).map_err(|e| format!("Cargo.toml: {e}"))?;
    version_in(&text)
}

pub fn version_in(toml: &str) -> Result<String, String> {
    let mut in_pkg = false;
    for line in toml.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_pkg = l == "[workspace.package]";
        } else if in_pkg {
            if let Some(rest) = l.strip_prefix("version") {
                if let Some(v) = rest.trim_start().strip_prefix('=') {
                    return Ok(v.trim().trim_matches('"').to_string());
                }
            }
        }
    }
    Err("no version in [workspace.package]".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_comes_from_workspace_package() {
        let t = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"1.2.3\"\nedition = \"2021\"\n\n[profile.release]\nversion = \"9\"\n";
        assert_eq!(version_in(t).unwrap(), "1.2.3");
        assert!(version_in("[package]\nversion = \"1\"\n").is_err());
        assert!(version_in(&std::fs::read_to_string(root().join("Cargo.toml")).unwrap()).is_ok());
    }

    #[test]
    fn graphs_follow_normal_dependencies_only() {
        let meta = serde_json::json!({
            "workspace_members": ["cli 0.1.0", "toolkit 0.1.0"],
            "packages": [
                {"id": "cli 0.1.0", "name": "shaiya-cli", "version": "0.1.0", "license": "MIT", "manifest_path": "/w/cli/Cargo.toml"},
                {"id": "toolkit 0.1.0", "name": "shaiya-toolkit", "version": "0.1.0", "license": "MIT", "manifest_path": "/w/toolkit/Cargo.toml"},
                {"id": "a 1.0.0", "name": "a", "version": "1.0.0", "license": "MIT", "manifest_path": "/r/a/Cargo.toml"},
                {"id": "dev 1.0.0", "name": "dev", "version": "1.0.0", "license": "MIT", "manifest_path": "/r/dev/Cargo.toml"},
                {"id": "build 1.0.0", "name": "build", "version": "1.0.0", "license": "MIT", "manifest_path": "/r/build/Cargo.toml"}
            ],
            "resolve": {"nodes": [
                {"id": "cli 0.1.0", "deps": [
                    {"pkg": "a 1.0.0", "dep_kinds": [{"kind": null}]},
                    {"pkg": "dev 1.0.0", "dep_kinds": [{"kind": "dev"}]},
                    {"pkg": "build 1.0.0", "dep_kinds": [{"kind": "build"}]}]},
                {"id": "toolkit 0.1.0", "deps": []},
                {"id": "a 1.0.0", "deps": []}, {"id": "dev 1.0.0", "deps": []}, {"id": "build 1.0.0", "deps": []}
            ]}
        });
        let g = graph_from(&meta).unwrap();
        let names: Vec<&str> = g.shipped.iter().map(|id| g.packages[id].name.as_str()).collect();
        assert_eq!(names, vec!["a", "shaiya-cli", "shaiya-toolkit"]);
    }
}
