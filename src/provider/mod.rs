pub mod homebrew;
pub mod mas;

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
}

impl Installed {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: None,
            version: None,
            aliases: Vec::new(),
        }
    }

    /// Every name this package answers to, most canonical first.
    pub fn identities(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.key.as_str()).chain(self.aliases.iter().map(String::as_str))
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
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

pub fn all() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(homebrew::Formula),
        Box::new(homebrew::Cask),
        Box::new(homebrew::Tap),
        Box::new(mas::Mas),
    ]
}
