use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Runs a command and returns its stdout, failing if the exit status is not success.
pub fn capture(bin: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(bin)
        .args(args)
        .output()
        .with_context(|| format!("failed to run `{bin}`"))?;
    if !out.status.success() {
        bail!(
            "`{bin} {}` exited with {}: {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Reports whether an executable of this name is on PATH.
pub fn has_bin(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(bin).is_file())
}

pub fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// Renders a past timestamp as a coarse age ("3d", "5w"). Precision beyond the
/// largest unit is noise in a triage list.
pub fn age(from: i64) -> String {
    let secs = (now() - from).max(0);
    match secs {
        s if s < 3600 => "just now".to_string(),
        s if s < 86_400 => format!("{}h", s / 3600),
        s if s < 86_400 * 14 => format!("{}d", s / 86_400),
        s if s < 86_400 * 60 => format!("{}w", s / (86_400 * 7)),
        s => format!("{}mo", s / (86_400 * 30)),
    }
}
