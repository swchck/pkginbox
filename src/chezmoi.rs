use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::util::capture;

/// The merged chezmoi template data.
///
/// Read through `chezmoi data` rather than by parsing `.chezmoidata/*.toml`:
/// the manifest is split across common/personal/work files and merged by Go
/// templates, and reimplementing that merge here would be a second source of
/// truth that silently drifts from the install scripts.
pub struct ChezmoiData {
    root: Value,
}

impl ChezmoiData {
    pub fn load() -> Result<Self> {
        let raw = capture("chezmoi", &["data", "--format=json"])
            .context("could not read chezmoi data — is chezmoi installed and initialized?")?;
        Ok(Self {
            root: serde_json::from_str(&raw).context("chezmoi data is not valid JSON")?,
        })
    }

    /// The chezmoi source directory — where the manifest files live. With a
    /// `.chezmoiroot` in play this is the subdirectory, not the repository.
    pub fn source_dir() -> Result<PathBuf> {
        let raw = capture("chezmoi", &["source-path"])?;
        Ok(PathBuf::from(raw.trim()))
    }

    /// Re-reads the data to prove the manifest still parses after an edit.
    pub fn verify() -> Result<()> {
        capture("chezmoi", &["data", "--format=json"])?;
        Ok(())
    }

    #[cfg(test)]
    pub fn from_json(raw: &str) -> Result<Self> {
        Ok(Self {
            root: serde_json::from_str(raw)?,
        })
    }

    pub fn machine_type(&self) -> &str {
        self.root
            .get("machine_type")
            .and_then(Value::as_str)
            .unwrap_or("personal")
    }

    pub fn os(&self) -> &str {
        self.root
            .get("chezmoi")
            .and_then(|c| c.get("os"))
            .and_then(Value::as_str)
            .unwrap_or("darwin")
    }

    fn at(&self, path: &[&str]) -> Option<&Value> {
        path.iter().try_fold(&self.root, |node, key| node.get(key))
    }

    /// Concatenates `packages.<os>.common.<key>` with the profile-specific list,
    /// mirroring how the install scripts build their package lists.
    pub fn package_list(&self, key: &str) -> Vec<String> {
        let os = self.os();
        let mut out = Vec::new();
        for profile in ["common", self.machine_type()] {
            if let Some(Value::Array(items)) = self.at(&["packages", os, profile, key]) {
                out.extend(items.iter().filter_map(Value::as_str).map(str::to_owned));
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Same merge for `appstore_apps`, which is a name → App Store id table.
    pub fn package_table(&self, key: &str) -> BTreeMap<String, String> {
        let os = self.os();
        let mut out = BTreeMap::new();
        for profile in ["common", self.machine_type()] {
            if let Some(Value::Object(items)) = self.at(&["packages", os, profile, key]) {
                for (name, id) in items {
                    let id = match id {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    out.insert(name.clone(), id);
                }
            }
        }
        out
    }
}
