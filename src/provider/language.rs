//! Per-language tool managers: one installed list each, all declared the same
//! way under `[packages.<manager>]`.

use anyhow::Result;
use serde::Deserialize;

use super::{Installed, Provider};
use crate::chezmoi::ChezmoiData;
use crate::util::{capture, has_bin};

pub struct Cargo;
pub struct Npm;
pub struct Uv;

impl Provider for Cargo {
    fn id(&self) -> &'static str {
        "cargo"
    }

    fn available(&self) -> bool {
        has_bin("cargo")
    }

    fn installed(&self) -> Result<Vec<Installed>> {
        Ok(parse_cargo(&capture("cargo", &["install", "--list"])?))
    }

    fn declared(&self, data: &ChezmoiData) -> Vec<String> {
        data.profile_list(&["packages", "cargo"])
    }
}

/// `cargo install --list` prints `name v1.2.3:` per crate, with its binaries
/// indented underneath. A crate installed from a local path gets its path in
/// parentheses — the manifest cannot reproduce that, so it is skipped.
fn parse_cargo(raw: &str) -> Vec<Installed> {
    raw.lines()
        .filter(|line| !line.starts_with(char::is_whitespace))
        .filter_map(|line| {
            let line = line.trim_end_matches(':').trim();
            if line.ends_with(')') {
                return None;
            }
            let (name, version) = line.split_once(' ')?;
            Some(Installed::new(name).with_version(version.trim_start_matches('v')))
        })
        .collect()
}

#[derive(Deserialize)]
struct NpmTree {
    #[serde(default)]
    dependencies: std::collections::BTreeMap<String, NpmDep>,
}

#[derive(Deserialize)]
struct NpmDep {
    version: Option<String>,
}

impl Provider for Npm {
    fn id(&self) -> &'static str {
        "npm"
    }

    fn available(&self) -> bool {
        has_bin("npm")
    }

    fn installed(&self) -> Result<Vec<Installed>> {
        let raw = capture("npm", &["ls", "-g", "--depth=0", "--json"])?;
        let tree: NpmTree = serde_json::from_str(&raw)?;
        Ok(tree
            .dependencies
            .into_iter()
            // npm and corepack ship with node itself
            .filter(|(name, _)| !matches!(name.as_str(), "npm" | "corepack"))
            .map(|(name, dep)| {
                let mut item = Installed::new(name);
                item.version = dep.version;
                item
            })
            .collect())
    }

    fn declared(&self, data: &ChezmoiData) -> Vec<String> {
        data.profile_list(&["packages", "npm"])
    }
}

impl Provider for Uv {
    fn id(&self) -> &'static str {
        "uv"
    }

    fn available(&self) -> bool {
        has_bin("uv")
    }

    fn installed(&self) -> Result<Vec<Installed>> {
        Ok(parse_uv(&capture("uv", &["tool", "list"])?))
    }

    fn declared(&self, data: &ChezmoiData) -> Vec<String> {
        data.profile_list(&["packages", "uv"])
    }
}

/// `uv tool list` prints `name v1.2.3` per tool, with its entry points indented
/// as `- binary` underneath.
fn parse_uv(raw: &str) -> Vec<Installed> {
    raw.lines()
        .filter(|line| !line.starts_with(char::is_whitespace) && !line.starts_with('-'))
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            match line.split_once(' ') {
                Some((name, version)) => {
                    Some(Installed::new(name).with_version(version.trim_start_matches('v')))
                }
                None => Some(Installed::new(line)),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_crates_and_their_versions() {
        let got = parse_cargo(
            "cargo-update v20.0.0:\n    cargo-install-update\nripgrep v14.1.1:\n    rg\n",
        );
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].key, "cargo-update");
        assert_eq!(got[0].version.as_deref(), Some("20.0.0"));
    }

    #[test]
    fn skips_a_crate_installed_from_a_local_path() {
        let got =
            parse_cargo("code-stats-ls v0.4.4 (/Users/me/code-stats-ls):\n    code-stats-ls\n");
        assert!(got.is_empty());
    }

    #[test]
    fn reads_uv_tools_without_their_entry_points() {
        let got = parse_uv("ruff v0.17.3\n- ruff\nposting v2.0.0\n- posting\n");
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].key, "posting");
    }
}
