use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::chezmoi::ChezmoiData;
use crate::db::{Db, Package, Status};
use crate::{manifest, script};

/// Which slice of the manifest an adopted package goes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Common,
    Personal,
    Work,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Common => "common",
            Profile::Personal => "personal",
            Profile::Work => "work",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "common" => Profile::Common,
            "personal" => Profile::Personal,
            "work" => Profile::Work,
            other => bail!("unknown profile `{other}` (common, personal, work)"),
        })
    }

    fn file_name(self) -> &'static str {
        match self {
            Profile::Common => "packages.toml",
            Profile::Personal => "packages.personal.toml",
            Profile::Work => "packages.work.toml",
        }
    }
}

/// The arrays a provider's packages may be declared in, most likely first.
/// Empty when the manifest leaves no choice — a Go tool goes under its profile
/// and nowhere else.
pub fn keys_for(provider: &str) -> &'static [&'static str] {
    match provider {
        "brew" => &["cli", "languages"],
        "cask" => &["casks", "fonts"],
        "tap" => &["taps"],
        _ => &[],
    }
}

/// The array a package should land in when nothing else is chosen. Fonts are
/// casks, and the manifest keeps them apart only by name.
pub fn default_key(provider: &str, name: &str) -> &'static str {
    match provider {
        "cask"
            if name
                .rsplit('/')
                .next()
                .is_some_and(|t| t.starts_with("font-")) =>
        {
            "fonts"
        }
        _ => keys_for(provider).first().copied().unwrap_or(""),
    }
}

/// Where a package's declaration goes, and in what shape.
pub enum Insert {
    /// An array of strings, optionally under a group heading.
    Array {
        table: String,
        key: String,
    },
    /// A `Name = value` table, as the App Store apps are kept.
    Table {
        table: String,
    },
    /// A `[[…]]` block, as the Claude plugins are kept.
    ArrayTable {
        table: String,
        fields: Vec<(String, String)>,
    },
    Unsupported(&'static str),
}

pub struct Plan {
    pub file: PathBuf,
    pub insert: Insert,
    pub profile: Profile,
    /// The install script, when declaring the package is not enough on its own.
    pub script: Option<PathBuf>,
}

pub fn plan(
    source_dir: &Path,
    data: &ChezmoiData,
    pkg: &Package,
    profile: Profile,
    key: &str,
) -> Plan {
    let manifests = source_dir.join(".chezmoidata");
    let os = data.os();
    let scoped = format!("packages.{os}.{}", profile.as_str());

    let (file, insert, script) = match pkg.provider.as_str() {
        "brew" | "cask" | "tap" => (
            manifests.join(profile.file_name()),
            Insert::Array {
                table: scoped,
                key: key.to_string(),
            },
            (pkg.provider == "tap").then(|| {
                source_dir
                    .join(".chezmoiscripts")
                    .join(os)
                    .join("run_onchange_before_02_install_packages.sh.tmpl")
            }),
        ),
        "mas" => (
            manifests.join(profile.file_name()),
            Insert::Table {
                table: format!("{scoped}.appstore_apps"),
            },
            None,
        ),
        manager @ ("go" | "cargo" | "npm" | "uv") => (
            manifests.join(format!("packages.{manager}.toml")),
            Insert::Array {
                table: format!("packages.{manager}"),
                key: profile.as_str().to_string(),
            },
            None,
        ),
        "vscode" => (
            manifests.join("vscode_extensions.toml"),
            Insert::Array {
                table: "vscode".into(),
                key: "extensions".into(),
            },
            None,
        ),
        // The models file declares one profile, so the choice does not apply.
        "ollama" => (
            manifests.join("packages.ollama.models.toml"),
            Insert::Array {
                table: format!("packages.{os}.personal.ollama"),
                key: "models".into(),
            },
            None,
        ),
        "claude" => {
            let (name, marketplace) = pkg.key.split_once('@').unwrap_or((&pkg.key, ""));
            (
                manifests.join("claude_plugins.toml"),
                Insert::ArrayTable {
                    table: "claude.plugins.install".into(),
                    fields: vec![
                        ("name".into(), name.to_string()),
                        ("marketplace".into(), marketplace.to_string()),
                    ],
                },
                None,
            )
        }
        "app" => (
            manifests.join(profile.file_name()),
            Insert::Unsupported("a hand-installed app has no manifest entry — find a cask for it"),
            None,
        ),
        _ => (
            manifests.join(profile.file_name()),
            Insert::Unsupported("this package manager cannot be declared yet"),
            None,
        ),
    };

    Plan {
        file,
        insert,
        profile,
        script,
    }
}

/// A file the adoption would rewrite.
pub struct Change {
    pub path: PathBuf,
    pub before: String,
    pub after: String,
}

impl Change {
    /// The lines this change adds, with their new line numbers.
    ///
    /// Matching is positional rather than by content: the same `brew tap …`
    /// line can already exist in another profile's branch, and a set
    /// difference would quietly drop it from the preview.
    pub fn added(&self) -> Vec<(usize, &str)> {
        let before: Vec<&str> = self.before.lines().collect();
        let mut old = 0;
        let mut out = Vec::new();
        for (idx, line) in self.after.lines().enumerate() {
            if old < before.len() && before[old] == line {
                old += 1;
            } else {
                out.push((idx + 1, line));
            }
        }
        out
    }
}

/// Works out every edit the adoption needs, without touching the disk.
pub fn render(
    pkg: &Package,
    plan: &Plan,
    comment: Option<&str>,
    group: Option<&str>,
) -> Result<Vec<Change>> {
    let mut changes = Vec::new();

    let before = fs::read_to_string(&plan.file)?;
    let after = match &plan.insert {
        Insert::Unsupported(why) => bail!("{why}"),
        Insert::Array { table, key } => {
            manifest::insert_into_array(&before, table, key, &pkg.key, comment, group)?
        }
        Insert::Table { table } => {
            let Some(label) = pkg.label.as_deref() else {
                bail!("{} has no name to declare it under", pkg.key);
            };
            manifest::insert_into_table(&before, table, label, &pkg.key, comment)?
        }
        Insert::ArrayTable { table, fields } => {
            let fields: Vec<(&str, &str)> = fields
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            manifest::append_array_table(&before, table, &fields)?
        }
    };
    changes.push(Change {
        path: plan.file.clone(),
        before,
        after,
    });

    if let Some(script_path) = &plan.script {
        let before = fs::read_to_string(script_path)?;
        let after = script::add_tap(&before, plan.profile.as_str(), &pkg.key)?;
        changes.push(Change {
            path: script_path.clone(),
            before,
            after,
        });
    }

    Ok(changes)
}

/// Writes the rendered edits and closes the package's inbox record.
///
/// Every file is restored if the manifest stops parsing afterwards: a
/// half-applied edit here breaks every later `chezmoi apply`, not just this one.
pub fn commit(db: &Db, pkg: &Package, changes: &[Change]) -> Result<Vec<PathBuf>> {
    for change in changes {
        fs::write(&change.path, &change.after)?;
    }

    if let Err(err) = ChezmoiData::verify() {
        for change in changes {
            fs::write(&change.path, &change.before)?;
        }
        return Err(err.context("the manifest stopped parsing; the edit was rolled back"));
    }

    db.set_status(&pkg.provider, &pkg.key, Status::Adopted)?;
    Ok(changes.iter().map(|c| c.path.clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_keeps_a_line_that_also_exists_elsewhere() {
        let change = Change {
            path: PathBuf::new(),
            before: "a\nb\n".into(),
            after: "a\nb\na\n".into(),
        };
        assert_eq!(change.added(), vec![(3, "a")]);
    }

    #[test]
    fn fonts_go_to_their_own_array() {
        assert_eq!(default_key("cask", "font-inter"), "fonts");
        assert_eq!(default_key("cask", "arc"), "casks");
    }

    #[test]
    fn a_tap_qualified_font_is_still_a_font() {
        assert_eq!(default_key("cask", "someone/tap/font-inter"), "fonts");
    }
}
