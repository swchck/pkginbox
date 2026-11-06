mod chezmoi;
mod config;
mod db;
mod provider;
mod quarantine;
mod report;
mod scan;
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
    /// Regenerate the quarantine Brewfile
    Brewfile,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::discover()?;
    paths.ensure()?;
    let db = Db::open(&paths.db())?;

    match cli.command.unwrap_or(Command::Status) {
        Command::Scan { quiet } => {
            let data = ChezmoiData::load()?;
            let summary = scan::run(&db, &data)?;
            quarantine::write(&db, &paths.quarantine_brewfile())?;
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
            for pkg in packages {
                let version = pkg.version.as_deref().unwrap_or("-");
                let note = pkg.note.as_deref().unwrap_or("");
                println!(
                    "{:<12} {:<6} {:<32} {:<14} {:<5} {}",
                    pkg.status,
                    pkg.provider,
                    pkg.display(),
                    version,
                    age(pkg.first_seen),
                    note
                );
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
