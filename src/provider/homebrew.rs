use std::sync::OnceLock;

use anyhow::{Context, Result};
use serde::Deserialize;

use super::{Installed, Provider};
use crate::chezmoi::ChezmoiData;
use crate::util::{capture, has_bin};

pub struct Formula;
pub struct Cask;
pub struct Tap;

#[derive(Deserialize)]
struct BrewInfo {
    formulae: Vec<BrewFormula>,
    casks: Vec<BrewCask>,
}

#[derive(Deserialize)]
struct BrewFormula {
    name: String,
    full_name: String,
    tap: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    oldnames: Vec<String>,
    installed: Vec<BrewInstall>,
}

#[derive(Deserialize)]
struct BrewInstall {
    version: String,
    installed_on_request: bool,
}

#[derive(Deserialize)]
struct BrewCask {
    token: String,
    full_token: String,
    tap: String,
    #[serde(default)]
    old_tokens: Vec<String>,
    #[serde(default)]
    installed: Option<String>,
    #[serde(default)]
    artifacts: Vec<serde_json::Value>,
}

/// The `.app` bundles installed casks own, so a scan of /Applications can tell
/// a hand-dragged app from one brew put there.
pub fn cask_app_names() -> Result<Vec<String>> {
    Ok(info()?
        .casks
        .iter()
        .flat_map(|c| &c.artifacts)
        .filter_map(|artifact| artifact.get("app")?.as_array())
        .flatten()
        .filter_map(|app| app.as_str())
        .map(str::to_owned)
        .collect())
}

/// One `brew info` call feeds both the formula and the cask provider; it is the
/// slowest thing a scan does.
fn info() -> Result<&'static BrewInfo> {
    static INFO: OnceLock<BrewInfo> = OnceLock::new();
    if let Some(cached) = INFO.get() {
        return Ok(cached);
    }
    let raw = capture("brew", &["info", "--json=v2", "--installed"])?;
    let parsed: BrewInfo = serde_json::from_str(&raw).context("unexpected `brew info` output")?;
    let _ = INFO.set(parsed);
    Ok(INFO.get().expect("just initialized"))
}

fn formula_item(f: &BrewFormula) -> Installed {
    let key = canonical(&f.name, &f.full_name, &f.tap);
    let mut item = Installed::new(key);
    if let Some(install) = f.installed.first() {
        item.version = Some(install.version.clone());
    }
    item.aliases.push(f.name.clone());
    item.aliases.push(f.full_name.clone());
    item.aliases.extend(f.aliases.iter().cloned());
    item.aliases.extend(f.oldnames.iter().cloned());
    item
}

/// A one-line description for the manifest comment, for packages brew knows
/// about. Missing is fine — a tap, or a cask brew cannot resolve offline,
/// simply gets no comment.
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
        has_bin("brew")
    }

    // `brew leaves` would be faster, but it hides any formula that something
    // else happens to depend on — and cleanup deletes those just the same.
    fn installed(&self) -> Result<Vec<Installed>> {
        Ok(info()?
            .formulae
            .iter()
            .filter(|f| f.installed.first().is_some_and(|i| i.installed_on_request))
            .map(formula_item)
            .collect())
    }

    fn present(&self) -> Result<Vec<Installed>> {
        Ok(info()?.formulae.iter().map(formula_item).collect())
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
        has_bin("brew")
    }

    fn installed(&self) -> Result<Vec<Installed>> {
        Ok(info()?
            .casks
            .iter()
            .map(|c| {
                let key = canonical(&c.token, &c.full_token, &c.tap);
                let mut item = Installed::new(key);
                if let Some(version) = &c.installed {
                    item.version = Some(version.clone());
                }
                item.aliases.push(c.token.clone());
                item.aliases.push(c.full_token.clone());
                item.aliases.extend(c.old_tokens.iter().cloned());
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
        has_bin("brew")
    }

    fn installed(&self) -> Result<Vec<Installed>> {
        let raw = capture("brew", &["tap"])?;
        Ok(raw
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            // homebrew/* taps are managed by brew itself and never declared
            .filter(|l| !l.starts_with("homebrew/"))
            .map(Installed::new)
            .collect())
    }

    fn declared(&self, data: &ChezmoiData) -> Vec<String> {
        data.package_list("taps")
    }
}

#[cfg(test)]
mod tests {
    use super::canonical;

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
    }
}
