use std::collections::{BTreeMap, HashSet};

use anyhow::Result;

use crate::chezmoi::ChezmoiData;
use crate::db::Db;
use crate::provider;
use crate::usage::{self, Signals};

/// Shell history for a command, Spotlight for an application.
///
/// `None` means nothing can be said: a tap, a font or an editor extension runs
/// nothing and opens nothing. `Some(0)` means the signal exists and found no
/// use at all — the difference is what makes "installed, never used" safe to
/// print.
fn last_use(signals: &Signals, item: &provider::Installed) -> Option<i64> {
    if !item.has_usage_signal() {
        return None;
    }
    if let Some(app) = &item.app {
        return Some(usage::app_last_use(app).unwrap_or(0));
    }
    if signals.is_empty() {
        return None;
    }
    Some(signals.command_last_use(&item.binaries).unwrap_or(0))
}

#[derive(Default)]
pub struct Summary {
    /// Unmanaged packages per provider, newly seen ones first in the caller's view.
    pub unmanaged: BTreeMap<String, Vec<String>>,
    pub drifted: BTreeMap<String, Vec<String>>,
    pub new_count: usize,
    pub skipped: Vec<String>,
}

impl Summary {
    pub fn unmanaged_count(&self) -> usize {
        self.unmanaged.values().map(Vec::len).sum()
    }

    pub fn drifted_count(&self) -> usize {
        self.drifted.values().map(Vec::len).sum()
    }
}

pub fn run(db: &Db, data: &ChezmoiData) -> Result<Summary> {
    let mut summary = Summary::default();
    let signals = Signals::collect();

    for provider in provider::all() {
        let id = provider.id();
        if !provider.available() {
            summary.skipped.push(id.to_string());
            continue;
        }

        let installed = provider.installed()?;
        let declared: HashSet<String> = provider.declared(data).into_iter().collect();
        let mut unmatched = declared.clone();
        for item in provider.present()? {
            if let Some(name) = item.identities().find(|name| declared.contains(*name)) {
                unmatched.remove(name);
            }
        }
        let mut touched: HashSet<String> = HashSet::new();

        for item in &installed {
            if item.identities().any(|name| declared.contains(name)) {
                db.mark_managed(id, &item.key)?;
                continue;
            }
            let is_new = db.observe_unmanaged(
                id,
                &item.key,
                item.label.as_deref(),
                item.version.as_deref(),
            )?;
            if is_new {
                summary.new_count += 1;
            }
            touched.insert(item.key.clone());
            db.set_last_used(id, &item.key, last_use(&signals, item))?;
            summary
                .unmanaged
                .entry(id.to_string())
                .or_default()
                .push(item.label.clone().unwrap_or_else(|| item.key.clone()));
        }

        for key in &unmatched {
            db.mark_drifted(id, key, None)?;
            touched.insert(key.clone());
            summary
                .drifted
                .entry(id.to_string())
                .or_default()
                .push(key.clone());
        }

        db.close_absent(id, &touched)?;
    }

    for list in summary
        .unmanaged
        .values_mut()
        .chain(summary.drifted.values_mut())
    {
        list.sort();
    }

    Ok(summary)
}
