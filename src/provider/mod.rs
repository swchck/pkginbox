pub mod apps;
pub mod editor;
pub mod gotool;
pub mod homebrew;
pub mod language;
pub mod mas;
pub mod models;

use anyhow::Result;

use crate::chezmoi::ChezmoiData;

/// A package as the system reports it.
pub struct Installed {
    /// Identity used for matching against the manifest.
    pub key: String,
    /// Human-readable name when `key` is not one (App Store ids).
    pub label: Option<String>,
    pub version: Option<String>,
    /// Other spellings the manifest may legitimately use for this package:
    /// short vs tap-qualified names, brew aliases, renamed formulae.
    pub aliases: Vec<String>,
    /// Commands this package puts on PATH. The package name is often not one
    /// of them — ripgrep installs `rg`, rm-improved installs `rip`.
    pub binaries: Vec<String>,
    /// The application bundle this package installs, if any.
    pub app: Option<String>,
}

impl Installed {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: None,
            version: None,
            aliases: Vec::new(),
            binaries: Vec::new(),
            app: None,
        }
    }

    pub fn with_binary(mut self, binary: impl Into<String>) -> Self {
        self.binaries.push(binary.into());
        self
    }

    pub fn with_app(mut self, app: impl Into<String>) -> Self {
        self.app = Some(app.into());
        self
    }

    /// Whether anything can be said about this package being used. A tap, a
    /// font or an editor extension runs nothing and opens nothing, so silence
    /// about it is the honest answer.
    pub fn has_usage_signal(&self) -> bool {
        self.app.is_some() || !self.binaries.is_empty()
    }

    /// Every name this package answers to, most canonical first.
    pub fn identities(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.key.as_str()).chain(self.aliases.iter().map(String::as_str))
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        self.aliases.push(alias.into());
        self
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }
}

pub trait Provider {
    fn id(&self) -> &'static str;

    /// Whether this provider can run here at all.
    fn available(&self) -> bool;

    /// Packages this provider installed because they were asked for.
    fn installed(&self) -> Result<Vec<Installed>>;

    /// Everything present, including packages pulled in as dependencies. Drift
    /// is measured against this: a declared formula that is only here as
    /// someone else's dependency is still here, and reporting it as missing
    /// would be a permanent false alarm.
    fn present(&self) -> Result<Vec<Installed>> {
        self.installed()
    }

    fn declared(&self, data: &ChezmoiData) -> Vec<String>;
}

pub fn by_id(id: &str) -> Option<Box<dyn Provider + Send + Sync>> {
    all().into_iter().find(|p| p.id() == id)
}

pub fn all() -> Vec<Box<dyn Provider + Send + Sync>> {
    vec![
        Box::new(homebrew::Formula),
        Box::new(homebrew::Cask),
        Box::new(homebrew::Tap),
        Box::new(mas::Mas),
        Box::new(gotool::Go),
        Box::new(language::Cargo),
        Box::new(language::Npm),
        Box::new(language::Uv),
        Box::new(editor::VsCode),
        Box::new(editor::ClaudePlugins),
        Box::new(models::Ollama),
        Box::new(apps::Apps),
    ]
}
