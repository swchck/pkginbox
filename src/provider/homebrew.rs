//! Homebrew's installed state, read from the prefix rather than from `brew`.
//!
//! `brew info --json=v2 --installed` answers the same question but costs two
//! and a half seconds of Ruby startup — more than everything else a scan does
//! put together. Every field needed here is already on disk in the install
//! receipts, and reading those takes twenty milliseconds.
//!
//! It is also the more truthful answer: `brew info` silently omits a formula
//! whose tap is no longer tapped, while the keg is still sitting in the Cellar
//! and `brew bundle cleanup` would still delete it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};

use super::{Installed, Provider};
use crate::chezmoi::ChezmoiData;
use crate::util::{capture, has_bin};

pub struct Formula;
pub struct Cask;
pub struct Tap;

struct Keg {
    name: String,
    version: String,
    tap: String,
    on_request: bool,
    aliases: Vec<String>,
}

struct CaskKeg {
    token: String,
    version: String,
    tap: String,
    apps: Vec<String>,
}

struct Brew {
    formulae: Vec<Keg>,
    casks: Vec<CaskKeg>,
    taps: Vec<String>,
}

/// Where Homebrew lives. Probed rather than asked for: `brew --prefix` is
/// another Ruby start, and the answer is one of two paths on macOS.
fn prefix() -> Option<&'static Path> {
    static PREFIX: OnceLock<Option<PathBuf>> = OnceLock::new();
    PREFIX
        .get_or_init(|| {
            if let Ok(set) = std::env::var("HOMEBREW_PREFIX") {
                return Some(PathBuf::from(set));
            }
            for candidate in ["/opt/homebrew", "/usr/local"] {
                let path = Path::new(candidate);
                if path.join("Cellar").is_dir() {
                    return Some(path.to_path_buf());
                }
            }
            capture("brew", &["--prefix"])
                .ok()
                .map(|p| PathBuf::from(p.trim()))
        })
        .as_deref()
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn dirs(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let name = name.to_str()?;
            (!name.starts_with('.') && entry.path().is_dir()).then(|| entry.path())
        })
        .collect()
}

fn file_name(path: &Path) -> Option<String> {
    Some(path.file_name()?.to_str()?.to_string())
}

/// The keg a receipt belongs to, picking the most recently installed one when
/// several versions are kept side by side.
fn newest_receipt(cellar_entry: &Path) -> Option<(String, serde_json::Value)> {
    dirs(cellar_entry)
        .into_iter()
        .filter_map(|keg| {
            let receipt = read_json(&keg.join("INSTALL_RECEIPT.json"))?;
            Some((file_name(&keg)?, receipt))
        })
        .max_by_key(|(_, receipt)| receipt.get("time").and_then(|t| t.as_i64()).unwrap_or(0))
}

fn tap_of(receipt: &serde_json::Value) -> String {
    receipt
        .get("source")
        .and_then(|s| s.get("tap"))
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Every other name a keg answers to, from the symlink farm in `opt`:
/// `opt/python -> ../Cellar/python@3.15/3.15.2`. The manifest declares
/// `python`, nothing on disk is called that, and without this the formula
/// reads as declared-but-missing on every scan.
fn read_aliases(prefix: &Path) -> HashMap<String, Vec<String>> {
    let Ok(entries) = std::fs::read_dir(prefix.join("opt")) else {
        return HashMap::new();
    };
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for entry in entries.flatten() {
        let Some(alias) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(target) = std::fs::read_link(entry.path()) else {
            continue;
        };
        // ../Cellar/<name>/<version>
        let Some(name) = target.parent().and_then(file_name) else {
            continue;
        };
        if name != alias {
            out.entry(name).or_default().push(alias);
        }
    }
    out
}

fn read_formulae(prefix: &Path) -> Vec<Keg> {
    let mut aliases = read_aliases(prefix);
    dirs(&prefix.join("Cellar"))
        .into_iter()
        .filter_map(|entry| {
            let name = file_name(&entry)?;
            let (version, receipt) = newest_receipt(&entry)?;
            Some(Keg {
                aliases: aliases.remove(&name).unwrap_or_default(),
                name,
                version,
                tap: tap_of(&receipt),
                on_request: receipt
                    .get("installed_on_request")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// A cask's receipt lists what it put on the system under `uninstall_artifacts`,
/// the `app` entries among them. That is how a scan of /Applications tells a
/// hand-dragged bundle from one brew owns.
fn cask_apps(receipt: &serde_json::Value) -> Vec<String> {
    receipt
        .get("uninstall_artifacts")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
        .filter_map(|artifact| artifact.get("app")?.as_array())
        .flatten()
        .filter_map(|app| app.as_str())
        .map(str::to_owned)
        .collect()
}

fn read_casks(prefix: &Path) -> Vec<CaskKeg> {
    dirs(&prefix.join("Caskroom"))
        .into_iter()
        .filter_map(|entry| {
            let token = file_name(&entry)?;
            let receipt = read_json(&entry.join(".metadata/INSTALL_RECEIPT.json"))?;
            let version = receipt
                .get("source")
                .and_then(|s| s.get("version"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            Some(CaskKeg {
                token,
                version,
                tap: tap_of(&receipt),
                apps: cask_apps(&receipt),
            })
        })
        .collect()
}

/// Taps are directories: `Library/Taps/<user>/homebrew-<repo>` is `user/repo`.
fn read_taps(prefix: &Path) -> Vec<String> {
    dirs(&prefix.join("Library/Taps"))
        .into_iter()
        .flat_map(|user| {
            let owner = file_name(&user).unwrap_or_default();
            dirs(&user).into_iter().filter_map(move |repo| {
                let name = file_name(&repo)?.strip_prefix("homebrew-")?.to_string();
                Some(format!("{owner}/{name}"))
            })
        })
        .collect()
}

fn brew() -> Result<&'static Brew> {
    static BREW: OnceLock<Option<Brew>> = OnceLock::new();
    BREW.get_or_init(|| {
        let prefix = prefix()?;
        Some(Brew {
            formulae: read_formulae(prefix),
            casks: read_casks(prefix),
            taps: read_taps(prefix),
        })
    })
    .as_ref()
    .context("could not find a Homebrew prefix to read")
}

/// The `.app` bundles installed casks own.
pub fn cask_app_names() -> Result<Vec<String>> {
    Ok(brew()?
        .casks
        .iter()
        .flat_map(|c| c.apps.iter().cloned())
        .collect())
}

/// The commands a formula puts on PATH, read from its keg. The package name is
/// often not one of them: ripgrep installs `rg`, rm-improved installs `rip`.
fn formula_binaries(name: &str) -> Vec<String> {
    let Some(prefix) = prefix() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(prefix.join("opt").join(name).join("bin")) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| Some(entry.ok()?.file_name().to_str()?.to_string()))
        .collect()
}

fn formula_item(f: &Keg) -> Installed {
    let full = full_name(&f.name, &f.tap);
    let mut item = Installed::new(canonical(&f.name, &full, &f.tap));
    item.binaries = formula_binaries(&f.name);
    item.version = Some(f.version.clone());
    item.aliases.push(f.name.clone());
    item.aliases.push(full);
    item.aliases.extend(f.aliases.iter().cloned());
    item
}

fn full_name(short: &str, tap: &str) -> String {
    match tap {
        "homebrew/core" | "homebrew/cask" | "" => short.to_string(),
        _ => format!("{tap}/{short}"),
    }
}

/// A one-line description for the manifest comment, for packages brew knows
/// about. Asked for one package at a time, at adoption, never during a scan.
pub fn describe(provider: &str, name: &str) -> Option<String> {
    let kind = match provider {
        "brew" => "--formula",
        "cask" => "--cask",
        // asking brew about a crate or an npm package invites a coincidental
        // match on an unrelated formula of the same name
        _ => return None,
    };
    let raw = capture("brew", &["desc", kind, name]).ok()?;
    let line = raw.lines().next()?.trim();
    let text = line
        .split_once(':')
        .map(|(_, rest)| rest)
        .unwrap_or(line)
        .trim();
    // Casks come back as "token: (Full Name) description"; the full name is
    // already the thing being declared, so it is noise in the comment.
    let text = match text.strip_prefix('(').and_then(|rest| rest.split_once(')')) {
        Some((_, rest)) => rest.trim(),
        None => text,
    };
    Some(text.to_string()).filter(|t| !t.is_empty())
}

/// The manifest spells tap packages out in full (`derailed/k9s/k9s`) and core
/// ones short, so the stored key has to follow the same rule or adoption would
/// write a name brew cannot resolve.
fn canonical(short: &str, full: &str, tap: &str) -> String {
    match tap {
        "homebrew/core" | "homebrew/cask" | "" => short.to_string(),
        _ => full.to_string(),
    }
}

impl Provider for Formula {
    fn id(&self) -> &'static str {
        "brew"
    }

    fn available(&self) -> bool {
        prefix().is_some() || has_bin("brew")
    }

    // Not `brew leaves`: it hides any formula that something else happens to
    // depend on — and cleanup deletes those just the same.
    fn installed(&self) -> Result<Vec<Installed>> {
        Ok(brew()?
            .formulae
            .iter()
            .filter(|f| f.on_request)
            .map(formula_item)
            .collect())
    }

    fn present(&self) -> Result<Vec<Installed>> {
        Ok(brew()?.formulae.iter().map(formula_item).collect())
    }

    fn declared(&self, data: &ChezmoiData) -> Vec<String> {
        let mut out = data.package_list("languages");
        out.extend(data.package_list("cli"));
        out.sort();
        out.dedup();
        out
    }
}

impl Provider for Cask {
    fn id(&self) -> &'static str {
        "cask"
    }

    fn available(&self) -> bool {
        prefix().is_some() || has_bin("brew")
    }

    fn installed(&self) -> Result<Vec<Installed>> {
        Ok(brew()?
            .casks
            .iter()
            .map(|c| {
                let full = full_name(&c.token, &c.tap);
                let mut item = Installed::new(canonical(&c.token, &full, &c.tap));
                if !c.version.is_empty() {
                    item.version = Some(c.version.clone());
                }
                item.app = c.apps.first().cloned();
                item.aliases.push(c.token.clone());
                item.aliases.push(full);
                item
            })
            .collect())
    }

    // Fonts are casks too; they are only a separate array in the manifest.
    fn declared(&self, data: &ChezmoiData) -> Vec<String> {
        let mut out = data.package_list("casks");
        out.extend(data.package_list("fonts"));
        out.sort();
        out.dedup();
        out
    }
}

impl Provider for Tap {
    fn id(&self) -> &'static str {
        "tap"
    }

    fn available(&self) -> bool {
        prefix().is_some() || has_bin("brew")
    }

    fn installed(&self) -> Result<Vec<Installed>> {
        Ok(brew()?
            .taps
            .iter()
            // homebrew/* taps are managed by brew itself and never declared
            .filter(|t| !t.starts_with("homebrew/"))
            .map(Installed::new)
            .collect())
    }

    fn declared(&self, data: &ChezmoiData) -> Vec<String> {
        data.package_list("taps")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_packages_keep_their_short_name() {
        assert_eq!(canonical("ripgrep", "ripgrep", "homebrew/core"), "ripgrep");
    }

    #[test]
    fn tap_packages_are_spelled_out_in_full() {
        assert_eq!(
            canonical("k9s", "derailed/k9s/k9s", "derailed/k9s"),
            "derailed/k9s/k9s"
        );
        assert_eq!(full_name("k9s", "derailed/k9s"), "derailed/k9s/k9s");
    }

    #[test]
    fn a_cask_receipt_lists_the_apps_it_owns() {
        let receipt = serde_json::json!({
            "uninstall_artifacts": [
                {"uninstall": [{"quit": "net.ankiweb.anki"}]},
                {"app": ["Anki.app"]},
                {"zap": [{"trash": ["~/Library/Caches/Anki"]}]}
            ]
        });
        assert_eq!(cask_apps(&receipt), vec!["Anki.app"]);
    }

    #[test]
    fn a_cask_that_installs_no_app_owns_nothing() {
        let receipt = serde_json::json!({
            "uninstall_artifacts": [{"binary": ["copilot"]}]
        });
        assert!(cask_apps(&receipt).is_empty());
    }

    #[test]
    fn the_newest_keg_wins_when_two_versions_are_kept() {
        let dir = tempdir();
        for (version, time) in [("1.0", 100), ("2.0", 200)] {
            let keg = dir.join(version);
            std::fs::create_dir_all(&keg).unwrap();
            std::fs::write(
                keg.join("INSTALL_RECEIPT.json"),
                format!("{{\"time\": {time}, \"installed_on_request\": true}}"),
            )
            .unwrap();
        }
        assert_eq!(newest_receipt(&dir).unwrap().0, "2.0");
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pkginbox-brew-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
