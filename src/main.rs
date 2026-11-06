mod adopt;
mod cache;
mod chezmoi;
mod config;
mod db;
mod export;
mod install;
mod manifest;
mod provider;
mod quarantine;
mod record;
mod report;
mod scan;
mod script;
mod theme;
mod tui;
mod usage;
mod util;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::chezmoi::ChezmoiData;
use crate::config::Paths;
use crate::db::{Db, Status};
use crate::util::age;

#[derive(Parser)]
#[command(
    name = "pkginbox",
    version,
    about = "Inbox for packages installed by hand"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Compare the system against the chezmoi manifest and record the drift
    Scan {
        /// Print nothing on success
        #[arg(long)]
        quiet: bool,
    },
    /// One-line summary for the end of `chezmoi apply`
    Status,
    /// List tracked packages
    List {
        /// Only this status (pending, quarantined, adopted, dismissed, removed, drifted)
        #[arg(long)]
        status: Option<String>,
    },
    /// Keep a package alive through `brew bundle cleanup` for a while
    Quarantine {
        provider: String,
        key: String,
        /// Days before the quarantine expires
        #[arg(long, default_value_t = 30)]
        days: i64,
    },
    /// Stop tracking a package
    Dismiss { provider: String, key: String },
    /// Show which kinds of package are watched on this machine
    Sources,
    /// Stop watching a kind of package, and forget what it found
    Untrack { provider: String },
    /// Watch a kind of package again
    Track { provider: String },
    /// Write a package into the chezmoi manifest
    Adopt {
        provider: String,
        key: String,
        /// common, personal or work (default: this machine's profile)
        #[arg(long)]
        profile: Option<String>,
        /// Which array to declare it in (cli, languages, casks, fonts, taps)
        #[arg(long)]
        list: Option<String>,
        /// Group heading inside the array
        #[arg(long)]
        group: Option<String>,
        /// Trailing comment (default: `brew desc`)
        #[arg(long)]
        comment: Option<String>,
        /// Show the lines that would be added and write nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Record an install command, as seen by the shell hook
    Record {
        /// The command line that just ran
        #[arg(long)]
        shell_command: String,
        /// Its exit status; anything but 0 is ignored
        #[arg(long, default_value_t = 0)]
        status: i32,
    },
    /// Attach a note to a package, or clear it with no text
    Note {
        provider: String,
        key: String,
        text: Vec<String>,
    },
    /// Write this machine's snapshot into the dotfiles repository
    Export {
        /// Write the file but leave it uncommitted
        #[arg(long)]
        no_commit: bool,
    },
    /// Take a package back out of the manifest
    Undeclare {
        provider: String,
        key: String,
        /// Show the lines that would go and write nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Regenerate the quarantine Brewfile
    Brewfile,
}

/// Folds a command the shell hook saw into the inbox.
///
/// The manifest is consulted first so that installing something already
/// declared does not raise a record that the next scan would only have to
/// close again.
fn apply_record(db: &Db, paths: &Paths, recorded: &record::Recorded, command: &str) -> Result<()> {
    let data = ChezmoiData::load()?;
    let declared: std::collections::HashSet<String> = provider::by_id(recorded.provider)
        .map(|p| p.declared(&data))
        .unwrap_or_default()
        .into_iter()
        .collect();

    for key in &recorded.keys {
        match (recorded.removal, declared.contains(key)) {
            (true, true) => db.mark_drifted(recorded.provider, key, None)?,
            (true, false) => db.set_status(recorded.provider, key, Status::Removed)?,
            (false, true) => db.mark_managed(recorded.provider, key)?,
            (false, false) => {
                db.observe_unmanaged(recorded.provider, key, None, None)?;
                db.add_observation(recorded.provider, key, "hook", Some(command))?;
            }
        }
    }

    quarantine::write(db, &paths.quarantine_brewfile())?;
    Ok(())
}

/// Keeps this machine's snapshot current without committing it. Committing on
/// every scan would put a commit in the dotfiles repository behind the user's
/// back; `pkginbox export` is the command that does that deliberately.
fn refresh_snapshot(db: &Db, data: &ChezmoiData) {
    if let Ok(source_dir) = ChezmoiData::source_dir() {
        let _ = export::write(db, &source_dir, data.hostname(), data.machine_type());
    }
}

/// Writes one line, reporting whether to keep going. `pkginbox list | head`
/// closes the pipe underneath us, and `println!` turns that into a panic.
fn emit(out: &mut impl std::io::Write, line: &str) -> bool {
    writeln!(out, "{line}").is_ok()
}

/// Rejects a provider name nothing would ever report on, so a typo does not
/// silently become a setting that does nothing.
fn known(provider: &str) -> Result<()> {
    if crate::provider::by_id(provider).is_none() {
        let names: Vec<&str> = crate::provider::all().iter().map(|p| p.id()).collect();
        anyhow::bail!("unknown source `{provider}` (one of: {})", names.join(", "));
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::discover()?;
    paths.ensure()?;
    let db = Db::open(&paths.db())?;

    let Some(command) = cli.command else {
        // no scan here: the interface opens on what the database already knows
        // and refreshes itself once the background scan lands
        let data = ChezmoiData::load()?;
        return tui::run(&db, &paths, &data, &ChezmoiData::source_dir()?);
    };

    match command {
        Command::Scan { quiet } => {
            let data = ChezmoiData::load()?;
            let summary = scan::run(&db, &data)?;
            quarantine::write(&db, &paths.quarantine_brewfile())?;
            refresh_snapshot(&db, &data);
            if !quiet {
                println!(
                    "scanned: {} outside the manifest, {} declared but missing, {} new",
                    summary.unmanaged_count(),
                    summary.drifted_count(),
                    summary.new_count
                );
                if !summary.skipped.is_empty() {
                    println!("skipped (not installed): {}", summary.skipped.join(", "));
                }
            }
        }
        Command::Status => {
            let data = ChezmoiData::load()?;
            scan::run(&db, &data)?;
            quarantine::write(&db, &paths.quarantine_brewfile())?;
            refresh_snapshot(&db, &data);
            let line = report::status_line(
                &db.by_status(Status::Pending)?,
                &db.by_status(Status::Drifted)?,
                &db.by_status(Status::Quarantined)?,
            );
            if let Some(line) = line {
                println!("{line}");
            }
        }
        Command::List { status } => {
            let packages = match status {
                Some(s) => db.by_status(Status::parse(&s)?)?,
                None => db.all()?,
            };
            let mut out = std::io::stdout().lock();
            for pkg in packages {
                let version = pkg.version.as_deref().unwrap_or("-");
                let note = pkg.note.as_deref().unwrap_or("");
                let line = format!(
                    "{:<12} {:<6} {:<32} {:<14} {:<5} {}",
                    pkg.status,
                    pkg.provider,
                    pkg.display(),
                    version,
                    age(pkg.first_seen),
                    note
                );
                if !emit(&mut out, &line) {
                    break;
                }
            }
        }
        Command::Quarantine {
            provider,
            key,
            days,
        } => {
            db.quarantine(&provider, &key, util::now() + days * 86_400)?;
            let n = quarantine::write(&db, &paths.quarantine_brewfile())?;
            println!("quarantined {provider}/{key} for {days}d ({n} protected in total)");
        }
        Command::Dismiss { provider, key } => {
            db.set_status(&provider, &key, Status::Dismissed)?;
            quarantine::write(&db, &paths.quarantine_brewfile())?;
            println!("dismissed {provider}/{key}");
        }
        Command::Sources => {
            let untracked = db.untracked()?;
            let mut out = std::io::stdout().lock();
            for p in provider::all() {
                let state = if untracked.contains(p.id()) {
                    "untracked"
                } else if p.available() {
                    "watched"
                } else {
                    "unavailable"
                };
                if !emit(&mut out, &format!("{:<8} {state}", p.id())) {
                    break;
                }
            }
        }
        Command::Untrack { provider } => {
            known(&provider)?;
            let dropped = db.untrack(&provider)?;
            quarantine::write(&db, &paths.quarantine_brewfile())?;
            println!("no longer watching {provider} ({dropped} records forgotten)");
        }
        Command::Track { provider } => {
            known(&provider)?;
            db.track(&provider)?;
            println!("watching {provider} again — run `pkginbox scan` to fill it in");
        }
        Command::Adopt {
            provider,
            key,
            profile,
            list,
            group,
            comment,
            dry_run,
        } => {
            let data = ChezmoiData::load()?;
            let pkg = db
                .find(&provider, &key)?
                .ok_or_else(|| anyhow::anyhow!("{provider}/{key} is not in the inbox"))?;
            let profile = adopt::Profile::parse(profile.as_deref().unwrap_or(data.machine_type()))?;
            let list = list.unwrap_or_else(|| adopt::default_key(&provider, &key).to_string());
            let comment = match comment {
                Some(text) => Some(text),
                None => provider::homebrew::describe(&provider, &key),
            };
            let plan = adopt::plan(&ChezmoiData::source_dir()?, &data, &pkg, profile, &list);
            let changes = adopt::render(
                &pkg,
                &plan,
                comment.as_deref().filter(|c| !c.is_empty()),
                group.as_deref(),
            )?;

            if dry_run {
                for change in &changes {
                    for (number, line) in change.added() {
                        println!("{}:{number}", change.path.display());
                        println!("+{line}");
                    }
                }
            } else {
                let files = adopt::commit(&db, &pkg, &changes, Status::Adopted)?;
                quarantine::write(&db, &paths.quarantine_brewfile())?;
                for file in files {
                    println!("wrote {}", file.display());
                }
            }
        }
        Command::Record {
            shell_command,
            status,
        } => {
            if status == 0
                && let Some(recorded) = record::parse(&shell_command)
            {
                apply_record(&db, &paths, &recorded, &shell_command)?;
            }
        }
        Command::Note {
            provider,
            key,
            text,
        } => {
            let note = text.join(" ");
            db.set_note(&provider, &key, Some(note.as_str()))?;
            println!("noted {provider}/{key}");
        }
        Command::Export { no_commit } => {
            let data = ChezmoiData::load()?;
            let source_dir = ChezmoiData::source_dir()?;
            let path = export::write(&db, &source_dir, data.hostname(), data.machine_type())?;
            println!("wrote {}", path.display());
            if !no_commit && export::commit(&source_dir, &path)? {
                println!("committed");
            }
        }
        Command::Undeclare {
            provider,
            key,
            dry_run,
        } => {
            let data = ChezmoiData::load()?;
            let pkg = db
                .find(&provider, &key)?
                .ok_or_else(|| anyhow::anyhow!("{provider}/{key} is not in the inbox"))?;
            let changes = adopt::undeclare(&ChezmoiData::source_dir()?, &data, &pkg)?;
            if dry_run {
                for change in &changes {
                    for (number, line) in change.removed() {
                        println!("{}:{number}", change.path.display());
                        println!("-{line}");
                    }
                }
            } else {
                for file in adopt::commit(&db, &pkg, &changes, Status::Removed)? {
                    println!("wrote {}", file.display());
                }
            }
        }
        Command::Brewfile => {
            let n = quarantine::write(&db, &paths.quarantine_brewfile())?;
            println!(
                "{n} package(s) protected in {}",
                paths.quarantine_brewfile().display()
            );
        }
    }

    Ok(())
}
