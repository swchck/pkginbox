//! Per-language tool managers: one installed list each, all declared the same
//! way under `[packages.<manager>]`.

use anyhow::Result;

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
        registry().is_some_and(|p| p.is_file()) || has_bin("cargo")
    }

    /// Read from cargo's own record of what it installed rather than from
    /// `cargo install --list`, which starts the toolchain to print this file
    /// back in a different shape.
    fn installed(&self) -> Result<Vec<Installed>> {
        let Some(path) = registry() else {
            return Ok(Vec::new());
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Ok(Vec::new());
        };
        Ok(parse_cargo(&serde_json::from_str(&raw)?))
    }

    fn declared(&self, data: &ChezmoiData) -> Vec<String> {
        data.profile_list(&["packages", "cargo"])
    }
}

fn registry() -> Option<std::path::PathBuf> {
    let home = match std::env::var_os("CARGO_HOME") {
        Some(dir) => std::path::PathBuf::from(dir),
        None => crate::util::home().ok()?.join(".cargo"),
    };
    Some(home.join(".crates2.json"))
}

/// `.crates2.json` keys each install as `<name> <version> (<source>)` and
/// lists the executables it put on PATH. Only crates from a registry are kept:
/// one installed from a local path or a git URL cannot be reproduced by a
/// manifest entry that names it.
fn parse_cargo(registry: &serde_json::Value) -> Vec<Installed> {
    let Some(installs) = registry.get("installs").and_then(|i| i.as_object()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (spec, entry) in installs {
        let Some((name, rest)) = spec.split_once(' ') else {
            continue;
        };
        let Some((version, source)) = rest.split_once(' ') else {
            continue;
        };
        if !source.starts_with("(registry+") {
            continue;
        }
        let mut item = Installed::new(name).with_version(version);
        item.binaries = entry
            .get("bins")
            .and_then(|b| b.as_array())
            .into_iter()
            .flatten()
            .filter_map(|b| b.as_str())
            .map(str::to_owned)
            .collect();
        out.push(item);
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// Both `cargo install --list` and `uv tool list` print a package per
/// unindented line and its executables indented underneath. Those executables
/// are what shell history actually holds.
fn with_indented_binaries(
    raw: &str,
    parse_head: impl Fn(&str) -> Option<Installed>,
) -> Vec<Installed> {
    let mut out: Vec<Installed> = Vec::new();
    let mut current: Option<Installed> = None;

    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            if let Some(item) = current.as_mut() {
                let binary = line.trim().trim_start_matches("- ").trim();
                if !binary.is_empty() {
                    item.binaries.push(binary.to_string());
                }
            }
            continue;
        }
        out.extend(current.take());
        current = parse_head(line);
    }
    out.extend(current);
    out
}

/// `<prefix>/lib/node_modules`, derived from where `npm` itself sits:
/// `<prefix>/bin/npm`.
fn global_node_modules() -> Option<std::path::PathBuf> {
    let npm = crate::util::find_bin("npm")?;
    let root = npm.parent()?.parent()?.join("lib/node_modules");
    root.is_dir().then_some(root)
}

/// Every package directly under `node_modules`, with its version. A `@scope`
/// directory is not a package; the packages are the directories inside it.
fn installed_packages(root: &std::path::Path) -> Vec<(String, Option<String>)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if name.starts_with('@') {
            let Ok(scoped) = std::fs::read_dir(entry.path()) else {
                continue;
            };
            for inner in scoped.flatten() {
                let Some(leaf) = inner.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                out.push((format!("{name}/{leaf}"), package_version(&inner.path())));
            }
            continue;
        }
        out.push((name, package_version(&entry.path())));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn package_version(dir: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some(manifest.get("version")?.as_str()?.to_string())
}

impl Provider for Npm {
    fn id(&self) -> &'static str {
        "npm"
    }

    fn available(&self) -> bool {
        has_bin("npm")
    }

    /// Read from the global `node_modules` directory rather than from
    /// `npm ls -g`, which spends most of a second starting Node to describe a
    /// handful of directories that are right there on disk.
    fn installed(&self) -> Result<Vec<Installed>> {
        let Some(root) = global_node_modules() else {
            return Ok(Vec::new());
        };
        Ok(installed_packages(&root)
            .into_iter()
            // npm and corepack ship with node itself
            .filter(|(name, _)| !matches!(name.as_str(), "npm" | "corepack"))
            .map(|(name, version)| {
                // a scoped package's binary is usually its unscoped name
                let binary = name.rsplit('/').next().unwrap_or(&name).to_string();
                let mut item = Installed::new(name).with_binary(binary);
                item.version = version;
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
    with_indented_binaries(raw, |line| {
        if line.starts_with('-') {
            return None;
        }
        let line = line.trim();
        match line.split_once(' ') {
            Some((name, version)) => {
                Some(Installed::new(name).with_version(version.trim_start_matches('v')))
            }
            None => Some(Installed::new(line)),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGISTRY: &str = r#"{"installs":{
        "cargo-update 20.0.0 (registry+https://github.com/rust-lang/crates.io-index)":
            {"bins":["cargo-install-update","cargo-install-update-config"]},
        "ripgrep 14.1.1 (registry+https://github.com/rust-lang/crates.io-index)":
            {"bins":["rg"]},
        "code-stats-ls 0.4.4 (path+file:///Users/me/code-stats-ls)":
            {"bins":["code-stats-ls"]}
    }}"#;

    fn crates() -> Vec<Installed> {
        parse_cargo(&serde_json::from_str(REGISTRY).unwrap())
    }

    #[test]
    fn reads_crates_and_their_versions() {
        let got = crates();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].key, "cargo-update");
        assert_eq!(got[0].version.as_deref(), Some("20.0.0"));
    }

    #[test]
    fn skips_a_crate_installed_from_a_local_path() {
        assert!(!crates().iter().any(|c| c.key == "code-stats-ls"));
    }

    #[test]
    fn keeps_the_executables_a_crate_installs() {
        assert_eq!(
            crates()[0].binaries,
            vec!["cargo-install-update", "cargo-install-update-config"]
        );
    }

    #[test]
    fn reads_uv_tools_without_their_entry_points() {
        let got = parse_uv("ruff v0.17.3\n- ruff\nposting v2.0.0\n- posting\n");
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].key, "posting");
    }
}
