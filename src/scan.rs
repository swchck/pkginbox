use std::collections::{BTreeMap, HashSet};

use anyhow::Result;

use crate::chezmoi::ChezmoiData;
use crate::db::Db;
use crate::provider;
use crate::usage::{self, Signals};

#[derive(Default)]
pub struct Summary {
    /// Unmanaged packages per provider.
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

/// Everything one provider has to say, gathered off the calling thread.
struct Gathered {
    id: &'static str,
    available: bool,
    installed: Vec<provider::Installed>,
    present: Vec<provider::Installed>,
    declared: HashSet<String>,
    last_use: Vec<Option<i64>>,
}

/// Asks every provider at once.
///
/// Each one spends its time waiting on a subprocess — brew, mas, npm, go — so
/// asking them in sequence added up latencies that overlap perfectly. Nothing
/// touches the database here: writes stay on the calling thread.
fn gather(data: &ChezmoiData, signals: &Signals) -> Vec<Gathered> {
    let providers = provider::all();
    std::thread::scope(|scope| {
        let handles: Vec<_> = providers
            .iter()
            .map(|p| {
                scope.spawn(move || {
                    if !p.available() {
                        return Gathered {
                            id: p.id(),
                            available: false,
                            installed: Vec::new(),
                            present: Vec::new(),
                            declared: HashSet::new(),
                            last_use: Vec::new(),
                        };
                    }
                    let installed = p.installed().unwrap_or_default();
                    let last_use = installed.iter().map(|i| last_use(signals, i)).collect();
                    Gathered {
                        id: p.id(),
                        available: true,
                        present: p.present().unwrap_or_default(),
                        declared: p.declared(data).into_iter().collect(),
                        installed,
                        last_use,
                    }
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    })
}

pub fn run(db: &Db, data: &ChezmoiData) -> Result<Summary> {
    let mut summary = Summary::default();
    let signals = Signals::collect();

    for gathered in gather(data, &signals) {
        let id = gathered.id;
        if !gathered.available {
            summary.skipped.push(id.to_string());
            continue;
        }

        let declared = &gathered.declared;
        let mut unmatched = declared.clone();
        for item in &gathered.present {
            if let Some(name) = item.identities().find(|name| declared.contains(*name)) {
                unmatched.remove(name);
            }
        }
        let mut touched: HashSet<String> = HashSet::new();

        for (item, last_used) in gathered.installed.iter().zip(gathered.last_use) {
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
            db.set_last_used(id, &item.key, last_used)?;
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
    summary.skipped.sort();

    Ok(summary)
}
