use std::path::Path;

use anyhow::Result;

use super::homebrew;
use super::{Installed, Provider};
use crate::chezmoi::ChezmoiData;
use crate::util::capture;

const APPLICATIONS: &str = "/Applications";

pub struct Apps;

impl Provider for Apps {
    fn id(&self) -> &'static str {
        "app"
    }

    fn available(&self) -> bool {
        Path::new(APPLICATIONS).is_dir()
    }

    /// Apps in /Applications that neither a cask nor the App Store put there —
    /// dragged out of a disk image, or installed by a vendor's own installer.
    ///
    /// Nothing can adopt these: there is no manifest entry that reproduces a
    /// hand-dragged bundle. They are listed so they can be recognised and
    /// replaced with a cask, or deliberately left alone.
    fn installed(&self) -> Result<Vec<Installed>> {
        let owned = homebrew::cask_app_names().unwrap_or_default();
        let mut out = Vec::new();
        for entry in std::fs::read_dir(APPLICATIONS)? {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.ends_with(".app") || owned.iter().any(|a| a == name) {
                continue;
            }
            if path.join("Contents").join("_MASReceipt").exists() || is_apple(&path) {
                continue;
            }
            out.push(Installed::new(name).with_label(name.trim_end_matches(".app")));
        }
        out.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(out)
    }

    fn declared(&self, _data: &ChezmoiData) -> Vec<String> {
        Vec::new()
    }
}

/// Apple ships Safari and friends in /Applications rather than
/// /System/Applications, and no manifest will ever install them.
fn is_apple(app: &Path) -> bool {
    let plist = app.join("Contents").join("Info.plist");
    // Info.plist is usually a binary plist, so it is read through PlistBuddy
    // rather than parsed here.
    capture(
        "/usr/libexec/PlistBuddy",
        &["-c", "Print :CFBundleIdentifier", &plist.to_string_lossy()],
    )
    .is_ok_and(|id| id.trim().starts_with("com.apple."))
}
