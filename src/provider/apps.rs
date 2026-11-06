use anyhow::Result;

use super::homebrew;
use super::{Installed, Provider};
use crate::chezmoi::ChezmoiData;
use crate::usage;

pub struct Apps;

impl Provider for Apps {
    fn id(&self) -> &'static str {
        "app"
    }

    fn available(&self) -> bool {
        std::path::Path::new(usage::APPLICATIONS).is_dir()
    }

    /// Apps in /Applications that neither a cask nor the App Store put there —
    /// dragged out of a disk image, or installed by a vendor's own installer.
    ///
    /// Nothing can adopt these: there is no manifest entry that reproduces a
    /// hand-dragged bundle. They are listed so they can be recognised and
    /// replaced with a cask, or deliberately left alone.
    fn installed(&self) -> Result<Vec<Installed>> {
        let owned = homebrew::cask_app_names().unwrap_or_default();
        let mut out: Vec<Installed> = usage::applications()
            .iter()
            .filter(|(name, meta)| {
                // Apple ships Safari and friends from /Applications rather than
                // /System/Applications, and no manifest will ever install them
                let apple = meta
                    .bundle_id
                    .as_deref()
                    .is_some_and(|id| id.starts_with("com.apple."));
                !apple && !owned.iter().any(|a| a == *name) && !from_app_store(name)
            })
            .map(|(name, _)| {
                Installed::new(name)
                    .with_label(name.trim_end_matches(".app"))
                    .with_app(name)
            })
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(out)
    }

    fn declared(&self, _data: &ChezmoiData) -> Vec<String> {
        Vec::new()
    }
}

fn from_app_store(name: &str) -> bool {
    std::path::Path::new(usage::APPLICATIONS)
        .join(name)
        .join("Contents")
        .join("_MASReceipt")
        .exists()
}
