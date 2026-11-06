use std::fs;
use std::path::PathBuf;

use anyhow::Result;

use crate::util::home;

/// Filesystem locations pkginbox owns. Everything here is machine-local: a
/// package is installed on one machine, so its triage state belongs there too.
pub struct Paths {
    pub state_dir: PathBuf,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        let base = match std::env::var_os("XDG_STATE_HOME") {
            Some(dir) => PathBuf::from(dir),
            None => home()?.join(".local").join("state"),
        };
        Ok(Self {
            state_dir: base.join("pkginbox"),
        })
    }

    pub fn db(&self) -> PathBuf {
        self.state_dir.join("inbox.db")
    }

    pub fn quarantine_brewfile(&self) -> PathBuf {
        self.state_dir.join("quarantine.brewfile")
    }

    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.state_dir)?;
        Ok(())
    }
}
