use std::path::{Path, PathBuf};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::adopt::{self, Profile};
use crate::chezmoi::ChezmoiData;
use crate::config::Paths;
use crate::db::{Db, Package, Status};
use crate::provider::homebrew;
use crate::util::{age, capture, now};
use crate::{manifest, quarantine};

const QUARANTINE_DAYS: i64 = 30;

pub fn run(db: &Db, paths: &Paths, data: &ChezmoiData, source_dir: &Path) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = App::new(db, paths, data, source_dir)?.run(&mut terminal);
    ratatui::restore();
    result
}

enum Mode {
    Browse,
    Note { text: String },
    Adopt(Form),
    Confirm { prompt: String, action: Pending },
    Help,
}

#[derive(Clone)]
enum Pending {
    Uninstall,
    /// Take a declared package back out of the manifest.
    Undeclare,
    /// Put a declared package back on this machine.
    Reinstall,
}

struct Form {
    step: Step,
    profiles: Vec<Profile>,
    profile: usize,
    keys: Vec<String>,
    key: usize,
    groups: Vec<String>,
    group: usize,
    comment: String,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Step {
    Profile,
    Key,
    Group,
    Comment,
}

struct App<'a> {
    db: &'a Db,
    paths: &'a Paths,
    data: &'a ChezmoiData,
    source_dir: PathBuf,
    others: Vec<crate::export::Snapshot>,
    items: Vec<Package>,
    state: ListState,
    mode: Mode,
    message: Option<String>,
    quit: bool,
}

impl<'a> App<'a> {
    fn new(db: &'a Db, paths: &'a Paths, data: &'a ChezmoiData, source_dir: &Path) -> Result<Self> {
        let mut app = Self {
            db,
            paths,
            data,
            source_dir: source_dir.to_path_buf(),
            others: crate::export::read_others(source_dir, data.hostname()),
            items: Vec::new(),
            state: ListState::default(),
            mode: Mode::Browse,
            message: None,
            quit: false,
        };
        app.reload()?;
        Ok(app)
    }

    fn reload(&mut self) -> Result<()> {
        let keep = self.state.selected().unwrap_or(0);
        self.items = self
            .db
            .all()?
            .into_iter()
            .filter(|p| {
                matches!(
                    p.status,
                    Status::Pending | Status::Quarantined | Status::Drifted
                )
            })
            .collect();
        self.state.select(if self.items.is_empty() {
            None
        } else {
            Some(keep.min(self.items.len() - 1))
        });
        Ok(())
    }

    fn selected(&self) -> Option<&Package> {
        self.state.selected().and_then(|i| self.items.get(i))
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            terminal.draw(|frame| self.draw(frame))?;
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.on_key(key)?;
            }
        }
        Ok(())
    }

    fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return Ok(());
        }
        match &self.mode {
            Mode::Browse => self.on_browse_key(key),
            Mode::Help => {
                self.mode = Mode::Browse;
                Ok(())
            }
            Mode::Confirm { .. } => self.on_confirm_key(key),
            Mode::Note { .. } => self.on_note_key(key),
            Mode::Adopt(_) => self.on_form_key(key),
        }
    }

    fn on_browse_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => self.quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.state.select_next(),
            KeyCode::Char('k') | KeyCode::Up => self.state.select_previous(),
            KeyCode::Char('g') => self.state.select_first(),
            KeyCode::Char('G') => self.state.select_last(),
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('r') => {
                let summary = crate::scan::run(self.db, self.data)?;
                quarantine::write(self.db, &self.paths.quarantine_brewfile())?;
                self.reload()?;
                self.message = Some(format!(
                    "rescanned: {} outside the manifest, {} missing",
                    summary.unmanaged_count(),
                    summary.drifted_count()
                ));
            }
            KeyCode::Char('q') => self.quarantine_selected()?,
            KeyCode::Char('d') => self.dismiss_selected()?,
            KeyCode::Char('x') => self.ask_uninstall(),
            KeyCode::Char('e') => self.open_note(),
            KeyCode::Char('i') => self.ask_reinstall(),
            KeyCode::Char('a') | KeyCode::Enter => self.open_form()?,
            _ => {}
        }
        Ok(())
    }

    fn quarantine_selected(&mut self) -> Result<()> {
        let Some(pkg) = self.selected() else {
            return Ok(());
        };
        if pkg.status == Status::Drifted {
            self.message = Some("a declared package has nothing to quarantine".into());
            return Ok(());
        }
        let (provider, key) = (pkg.provider.clone(), pkg.key.clone());
        self.db
            .quarantine(&provider, &key, now() + QUARANTINE_DAYS * 86_400)?;
        quarantine::write(self.db, &self.paths.quarantine_brewfile())?;
        self.reload()?;
        self.message = Some(format!("{key} held for {QUARANTINE_DAYS}d"));
        Ok(())
    }

    fn dismiss_selected(&mut self) -> Result<()> {
        let Some(pkg) = self.selected() else {
            return Ok(());
        };
        let (provider, key) = (pkg.provider.clone(), pkg.key.clone());
        self.db.set_status(&provider, &key, Status::Dismissed)?;
        quarantine::write(self.db, &self.paths.quarantine_brewfile())?;
        self.reload()?;
        self.message = Some(format!("{key} left unmanaged"));
        Ok(())
    }

    fn ask_uninstall(&mut self) {
        let Some(pkg) = self.selected() else { return };
        // it is already gone from the machine; what is left is the declaration
        if pkg.status == Status::Drifted {
            self.mode = Mode::Confirm {
                prompt: format!("stop declaring {} in the manifest?", pkg.key),
                action: Pending::Undeclare,
            };
            return;
        }
        if pkg.provider == "mas" {
            self.message = Some("App Store apps have to be removed in Finder".into());
            return;
        }
        self.mode = Mode::Confirm {
            prompt: format!("uninstall {} from this machine?", pkg.key),
            action: Pending::Uninstall,
        };
    }

    fn ask_reinstall(&mut self) {
        let Some(pkg) = self.selected() else { return };
        if pkg.status != Status::Drifted {
            self.message = Some("that package is already installed".into());
            return;
        }
        let Some(command) = crate::install::describe(&pkg.provider, &pkg.key) else {
            self.message = Some(format!("nothing can reinstall a `{}` entry", pkg.provider));
            return;
        };
        self.mode = Mode::Confirm {
            prompt: format!("run `{command}`?"),
            action: Pending::Reinstall,
        };
    }

    fn undeclare_selected(&mut self) -> Result<()> {
        let Some(pkg) = self.selected().cloned() else {
            return Ok(());
        };
        let outcome = adopt::undeclare(&self.source_dir, self.data, &pkg)
            .and_then(|changes| adopt::commit(self.db, &pkg, &changes, Status::Removed));
        match outcome {
            Ok(files) => {
                self.reload()?;
                let names: Vec<String> = files
                    .iter()
                    .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .collect();
                self.message = Some(format!("{} dropped from {}", pkg.key, names.join(" + ")));
            }
            Err(err) => self.message = Some(first_line(&err.to_string())),
        }
        Ok(())
    }

    fn reinstall_selected(&mut self) -> Result<()> {
        let Some(pkg) = self.selected().cloned() else {
            return Ok(());
        };
        let Some((bin, args)) = crate::install::command(&pkg.provider, &pkg.key) else {
            return Ok(());
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match capture(bin, &args) {
            Ok(_) => {
                self.db.mark_managed(&pkg.provider, &pkg.key)?;
                self.reload()?;
                self.message = Some(format!("{} reinstalled", pkg.key));
            }
            Err(err) => self.message = Some(first_line(&err.to_string())),
        }
        Ok(())
    }

    fn open_note(&mut self) {
        let Some(pkg) = self.selected() else { return };
        self.mode = Mode::Note {
            text: pkg.note.clone().unwrap_or_default(),
        };
    }

    fn on_note_key(&mut self, key: KeyEvent) -> Result<()> {
        let Mode::Note { text } = &mut self.mode else {
            return Ok(());
        };
        match key.code {
            KeyCode::Char(c) => text.push(c),
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Esc => self.mode = Mode::Browse,
            KeyCode::Enter => {
                let text = text.clone();
                self.mode = Mode::Browse;
                if let Some(pkg) = self.selected() {
                    let (provider, key) = (pkg.provider.clone(), pkg.key.clone());
                    self.db.set_note(&provider, &key, Some(text.as_str()))?;
                    self.reload()?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn on_confirm_key(&mut self, key: KeyEvent) -> Result<()> {
        let Mode::Confirm { action, .. } = &self.mode else {
            return Ok(());
        };
        let action = action.clone();
        match key.code {
            KeyCode::Char('y') => {
                self.mode = Mode::Browse;
                match action {
                    Pending::Uninstall => self.uninstall_selected()?,
                    Pending::Undeclare => self.undeclare_selected()?,
                    Pending::Reinstall => self.reinstall_selected()?,
                }
            }
            KeyCode::Char('n') | KeyCode::Esc => self.mode = Mode::Browse,
            _ => {}
        }
        Ok(())
    }

    fn uninstall_selected(&mut self) -> Result<()> {
        let Some(pkg) = self.selected() else {
            return Ok(());
        };
        let (provider, key) = (pkg.provider.clone(), pkg.key.clone());
        let outcome = match provider.as_str() {
            "brew" => capture("brew", &["uninstall", &key]),
            "cask" => capture("brew", &["uninstall", "--cask", &key]),
            "tap" => capture("brew", &["untap", &key]),
            other => anyhow::bail!("cannot uninstall a `{other}` package"),
        };
        match outcome {
            Ok(_) => {
                self.db.set_status(&provider, &key, Status::Removed)?;
                quarantine::write(self.db, &self.paths.quarantine_brewfile())?;
                self.reload()?;
                self.message = Some(format!("{key} uninstalled"));
            }
            Err(err) => self.message = Some(first_line(&err.to_string())),
        }
        Ok(())
    }

    fn open_form(&mut self) -> Result<()> {
        let Some(pkg) = self.selected() else {
            return Ok(());
        };
        if pkg.status == Status::Drifted {
            self.message = Some("that package is already declared — reinstall or drop it".into());
            return Ok(());
        }

        let profiles = vec![Profile::Common, Profile::Personal, Profile::Work];
        let here = Profile::parse(self.data.machine_type()).unwrap_or(Profile::Common);
        let keys: Vec<String> = adopt::keys_for(&pkg.provider)
            .iter()
            .map(|k| (*k).to_string())
            .collect();
        let default_key = adopt::default_key(&pkg.provider, &pkg.key);
        let comment = homebrew::describe(&pkg.provider, &pkg.key).unwrap_or_default();

        let mut form = Form {
            step: Step::Profile,
            profile: profiles.iter().position(|p| *p == here).unwrap_or(0),
            profiles,
            key: keys.iter().position(|k| k == default_key).unwrap_or(0),
            keys,
            groups: Vec::new(),
            group: 0,
            comment,
        };
        if form.keys.is_empty() {
            form.step = Step::Comment;
        }
        self.mode = Mode::Adopt(form);
        Ok(())
    }

    fn on_form_key(&mut self, key: KeyEvent) -> Result<()> {
        let Mode::Adopt(form) = &mut self.mode else {
            return Ok(());
        };
        if key.code == KeyCode::Esc {
            self.mode = Mode::Browse;
            return Ok(());
        }

        if form.step == Step::Comment {
            match key.code {
                KeyCode::Char(c) => form.comment.push(c),
                KeyCode::Backspace => {
                    form.comment.pop();
                }
                KeyCode::Enter => return self.commit_form(),
                _ => {}
            }
            return Ok(());
        }

        let len = match form.step {
            Step::Profile => form.profiles.len(),
            Step::Key => form.keys.len(),
            Step::Group => form.groups.len() + 1,
            Step::Comment => 0,
        };
        let cursor = match form.step {
            Step::Profile => &mut form.profile,
            Step::Key => &mut form.key,
            _ => &mut form.group,
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => *cursor = (*cursor + 1) % len.max(1),
            KeyCode::Char('k') | KeyCode::Up => *cursor = (*cursor + len.max(1) - 1) % len.max(1),
            KeyCode::Enter => self.advance_form()?,
            _ => {}
        }
        Ok(())
    }

    fn advance_form(&mut self) -> Result<()> {
        let groups = self.form_groups()?;
        let Mode::Adopt(form) = &mut self.mode else {
            return Ok(());
        };
        form.step = match form.step {
            Step::Profile if form.keys.is_empty() => Step::Comment,
            Step::Profile => Step::Key,
            Step::Key => {
                form.groups = groups;
                form.group = 0;
                if form.groups.is_empty() {
                    Step::Comment
                } else {
                    Step::Group
                }
            }
            Step::Group | Step::Comment => Step::Comment,
        };
        Ok(())
    }

    /// Group headings of the array the form currently points at.
    fn form_groups(&self) -> Result<Vec<String>> {
        let (Mode::Adopt(form), Some(pkg)) = (&self.mode, self.selected()) else {
            return Ok(Vec::new());
        };
        if form.step != Step::Key || form.keys.is_empty() {
            return Ok(Vec::new());
        }
        let profile = form.profiles[form.profile];
        let plan = adopt::plan(
            &self.source_dir,
            self.data,
            pkg,
            profile,
            &form.keys[form.key],
        );
        let crate::adopt::Insert::Array { table, key } = &plan.insert else {
            return Ok(Vec::new());
        };
        let source = std::fs::read_to_string(&plan.file)?;
        Ok(manifest::groups(&source, table, key).unwrap_or_default())
    }

    fn commit_form(&mut self) -> Result<()> {
        let (plan, comment, group, pkg) = {
            let (Mode::Adopt(form), Some(pkg)) = (&self.mode, self.selected()) else {
                return Ok(());
            };
            let profile = form.profiles[form.profile];
            let key = form.keys.get(form.key).cloned().unwrap_or_default();
            let group = (form.step == Step::Comment && !form.groups.is_empty() && form.group > 0)
                .then(|| form.groups[form.group - 1].clone());
            (
                adopt::plan(&self.source_dir, self.data, pkg, profile, &key),
                form.comment.clone(),
                group,
                pkg.clone(),
            )
        };

        self.mode = Mode::Browse;
        let outcome = adopt::render(
            &pkg,
            &plan,
            Some(comment.as_str()).filter(|c| !c.is_empty()),
            group.as_deref(),
        )
        .and_then(|changes| adopt::commit(self.db, &pkg, &changes, Status::Adopted));
        match outcome {
            Ok(files) => {
                quarantine::write(self.db, &self.paths.quarantine_brewfile())?;
                self.reload()?;
                let names: Vec<String> = files
                    .iter()
                    .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .collect();
                self.message = Some(format!("{} declared in {}", pkg.key, names.join(" + ")));
            }
            Err(err) => self.message = Some(first_line(&err.to_string())),
        }
        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(5),
                Constraint::Length(1),
            ])
            .split(frame.area());

        let counts = self.counts();
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("pkginbox", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!(
                    "  {} undecided · {} held · {} declared but missing",
                    counts.0, counts.1, counts.2
                )),
            ])),
            chunks[0],
        );

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(chunks[1]);

        let items: Vec<ListItem> = self
            .items
            .iter()
            .map(|p| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{:<5} ", p.provider),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::raw(format!("{:<34}", p.display())),
                    Span::styled(status_label(p), Style::default().fg(status_color(p))),
                ]))
            })
            .collect();
        frame.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" packages "))
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
            body[0],
            &mut self.state,
        );

        frame.render_widget(self.details(), body[1]);

        let hint = match &self.message {
            Some(msg) => Line::from(Span::styled(
                msg.clone(),
                Style::default().fg(Color::Yellow),
            )),
            None => Line::from(Span::styled(
                "a adopt · q hold · d leave · x remove · i reinstall · e note · r rescan · ? quit Esc",
                Style::default().fg(Color::DarkGray),
            )),
        };
        frame.render_widget(Paragraph::new(hint), chunks[2]);

        match &self.mode {
            Mode::Adopt(form) => self.draw_form(frame, form),
            Mode::Confirm { prompt, .. } => draw_modal(
                frame,
                " confirm ",
                Paragraph::new(vec![
                    Line::from(prompt.clone()),
                    Line::from(""),
                    Line::from(Span::styled(
                        "y to go ahead · n to cancel",
                        Style::default().fg(Color::DarkGray),
                    )),
                ]),
                5,
            ),
            Mode::Note { text } => draw_modal(
                frame,
                " note ",
                Paragraph::new(vec![
                    Line::from(format!("{text}_")),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Enter to save · Esc to cancel",
                        Style::default().fg(Color::DarkGray),
                    )),
                ]),
                5,
            ),
            Mode::Help => draw_modal(frame, " keys ", Paragraph::new(help_text()), 14),
            Mode::Browse => {}
        }
    }

    fn counts(&self) -> (usize, usize, usize) {
        let mut counts = (0, 0, 0);
        for pkg in &self.items {
            match pkg.status {
                Status::Pending => counts.0 += 1,
                Status::Quarantined => counts.1 += 1,
                Status::Drifted => counts.2 += 1,
                _ => {}
            }
        }
        counts
    }

    fn details(&self) -> Paragraph<'_> {
        let block = Block::default().borders(Borders::ALL).title(" details ");
        let Some(pkg) = self.selected() else {
            return Paragraph::new("nothing to triage").block(block);
        };
        let mut lines = vec![
            Line::from(vec![
                Span::styled("name    ", Style::default().fg(Color::DarkGray)),
                Span::raw(pkg.key.clone()),
            ]),
            Line::from(vec![
                Span::styled("source  ", Style::default().fg(Color::DarkGray)),
                Span::raw(pkg.provider.clone()),
            ]),
            Line::from(vec![
                Span::styled("version ", Style::default().fg(Color::DarkGray)),
                Span::raw(pkg.version.clone().unwrap_or_else(|| "-".into())),
            ]),
            Line::from(vec![
                Span::styled("seen    ", Style::default().fg(Color::DarkGray)),
                Span::raw(age(pkg.first_seen)),
            ]),
        ];
        lines.push(Line::from(vec![
            Span::styled("used    ", Style::default().fg(Color::DarkGray)),
            Span::raw(match pkg.last_used {
                None => "no way to tell".to_string(),
                Some(0) => "never".to_string(),
                Some(at) => format!("{} ago", age(at)),
            }),
        ]));
        if let Ok(Some(command)) = self.db.last_command(&pkg.provider, &pkg.key) {
            lines.push(Line::from(vec![
                Span::styled("ran     ", Style::default().fg(Color::DarkGray)),
                Span::raw(command),
            ]));
        }
        if let Some(note) = &pkg.note {
            lines.push(Line::from(vec![
                Span::styled("note    ", Style::default().fg(Color::DarkGray)),
                Span::raw(note.clone()),
            ]));
        }
        if let Some(at) = pkg.expires_at {
            lines.push(Line::from(vec![
                Span::styled("held    ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!("another {}", age(now() * 2 - at))),
            ]));
        }
        let elsewhere = crate::export::elsewhere(&self.others, &pkg.provider, &pkg.key);
        if !elsewhere.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("also on ", Style::default().fg(Color::DarkGray)),
                Span::raw(
                    elsewhere
                        .iter()
                        .map(|(host, status)| format!("{host} ({status})"))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ]));
        }
        if pkg.status == Status::Drifted {
            lines.push(Line::from(""));
            lines.push(Line::from(
                "declared in the manifest but not installed — the next apply puts it back",
            ));
            lines.push(Line::from("i reinstalls it here · x stops declaring it"));
        }
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true })
    }

    fn draw_form(&self, frame: &mut Frame, form: &Form) {
        let (title, lines) = match form.step {
            Step::Profile => (
                " which machines? ",
                choices(
                    &form
                        .profiles
                        .iter()
                        .map(|p| p.as_str().to_string())
                        .collect::<Vec<_>>(),
                    form.profile,
                ),
            ),
            Step::Key => (" which list? ", choices(&form.keys, form.key)),
            Step::Group => {
                let mut options = vec!["end of the list".to_string()];
                options.extend(form.groups.clone());
                (" which group? ", choices(&options, form.group))
            }
            Step::Comment => (
                " comment ",
                vec![
                    Line::from(format!("{}_", form.comment)),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Enter to write it into the manifest · Esc to cancel",
                        Style::default().fg(Color::DarkGray),
                    )),
                ],
            ),
        };
        let height = (lines.len() + 2).min(14) as u16;
        draw_modal(frame, title, Paragraph::new(lines), height);
    }
}

fn choices(options: &[String], selected: usize) -> Vec<Line<'static>> {
    options
        .iter()
        .enumerate()
        .map(|(idx, option)| {
            let style = if idx == selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            Line::from(Span::styled(format!("  {option}  "), style))
        })
        .collect()
}

fn help_text() -> Vec<Line<'static>> {
    [
        "j / k      move",
        "a / Enter  adopt into the chezmoi manifest",
        "q          hold: survive brew cleanup for 30 days",
        "d          leave unmanaged, stop asking",
        "x          uninstall here, or stop declaring a missing one",
        "i          reinstall a declared package that went missing",
        "e          write a note on why it is here",
        "r          rescan",
        "Esc        quit",
    ]
    .into_iter()
    .map(Line::from)
    .collect()
}

fn status_label(pkg: &Package) -> String {
    match pkg.status {
        Status::Quarantined => format!("held {}", age(pkg.first_seen)),
        Status::Drifted => "missing".into(),
        _ if pkg.never_used(crate::report::SETTLED) => {
            format!("{} · never used", age(pkg.first_seen))
        }
        _ => age(pkg.first_seen),
    }
}

fn status_color(pkg: &Package) -> Color {
    match pkg.status {
        Status::Quarantined => Color::Cyan,
        Status::Drifted => Color::Red,
        _ if pkg.never_used(crate::report::SETTLED) => Color::Yellow,
        _ => Color::DarkGray,
    }
}

fn draw_modal(frame: &mut Frame, title: &str, body: Paragraph<'_>, height: u16) {
    let area = centered(frame.area(), 60, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        body.block(
            Block::default()
                .borders(Borders::ALL)
                .title(title.to_string()),
        ),
        area,
    );
}

fn centered(area: Rect, width_percent: u16, height: u16) -> Rect {
    let width = area.width * width_percent / 100;
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(text).to_string()
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    const DATA: &str = r#"{
        "machine_type": "work",
        "chezmoi": {"os": "darwin"},
        "packages": {"darwin": {"common": {"cli": []}, "work": {"cli": []}}}
    }"#;

    struct Fixture {
        db: Db,
        paths: Paths,
        data: ChezmoiData,
    }

    fn fixture() -> Fixture {
        let db = Db::open_in_memory().unwrap();
        db.observe_unmanaged("brew", "mole", None, Some("1.68.0"))
            .unwrap();
        db.observe_unmanaged("cask", "google-chrome", None, None)
            .unwrap();
        db.mark_drifted("brew", "hashicorp/tap/vault", None)
            .unwrap();

        let state_dir = std::env::temp_dir().join(format!("pkginbox-test-{}", std::process::id()));
        Fixture {
            db,
            paths: Paths { state_dir },
            data: ChezmoiData::from_json(DATA).unwrap(),
        }
    }

    fn app(f: &Fixture) -> App<'_> {
        App::new(&f.db, &f.paths, &f.data, Path::new("/nonexistent")).unwrap()
    }

    fn press(app: &mut App<'_>, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
    }

    fn render(app: &mut App<'_>) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn the_list_shows_everything_still_undecided() {
        let f = fixture();
        let mut app = app(&f);
        let screen = render(&mut app);
        assert!(screen.contains("mole"), "{screen}");
        assert!(screen.contains("google-chrome"), "{screen}");
        assert!(screen.contains("2 undecided"), "{screen}");
        assert!(screen.contains("1 declared but missing"), "{screen}");
    }

    #[test]
    fn j_and_k_move_the_selection() {
        let f = fixture();
        let mut app = app(&f);
        assert_eq!(app.state.selected(), Some(0));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.state.selected(), Some(1));
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.state.selected(), Some(0));
    }

    #[test]
    fn dismissing_takes_the_package_off_the_list() {
        let f = fixture();
        let mut app = app(&f);
        let before = app.items.len();
        let pending = app
            .items
            .iter()
            .position(|p| p.status == Status::Pending)
            .unwrap();
        app.state.select(Some(pending));
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.items.len(), before - 1);
        assert_eq!(f.db.by_status(Status::Dismissed).unwrap().len(), 1);
    }

    #[test]
    fn uninstalling_asks_first() {
        let f = fixture();
        let mut app = app(&f);
        let pending = app
            .items
            .iter()
            .position(|p| p.status == Status::Pending)
            .unwrap();
        app.state.select(Some(pending));
        press(&mut app, KeyCode::Char('x'));
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        let screen = render(&mut app);
        assert!(screen.contains("uninstall"), "{screen}");
        press(&mut app, KeyCode::Char('n'));
        assert!(matches!(app.mode, Mode::Browse));
    }

    #[test]
    fn a_declared_package_cannot_be_adopted_again() {
        let f = fixture();
        let mut app = app(&f);
        let drifted = app
            .items
            .iter()
            .position(|p| p.status == Status::Drifted)
            .unwrap();
        app.state.select(Some(drifted));
        press(&mut app, KeyCode::Char('a'));
        assert!(matches!(app.mode, Mode::Browse));
        assert!(app.message.as_deref().unwrap().contains("already declared"));
    }

    #[test]
    fn help_opens_and_closes() {
        let f = fixture();
        let mut app = app(&f);
        press(&mut app, KeyCode::Char('?'));
        let screen = render(&mut app);
        assert!(
            screen.contains("adopt into the chezmoi manifest"),
            "{screen}"
        );
        press(&mut app, KeyCode::Char('j'));
        assert!(matches!(app.mode, Mode::Browse));
    }

    #[test]
    fn a_note_is_typed_and_saved() {
        let f = fixture();
        let mut app = app(&f);
        let pending = app
            .items
            .iter()
            .position(|p| p.status == Status::Pending)
            .unwrap();
        app.state.select(Some(pending));
        press(&mut app, KeyCode::Char('e'));
        for c in "for the demo".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        let screen = render(&mut app);
        assert!(screen.contains("for the demo"), "{screen}");
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Browse));
        assert_eq!(
            app.selected().unwrap().note.as_deref(),
            Some("for the demo")
        );
    }

    #[test]
    fn esc_abandons_a_note() {
        let f = fixture();
        let mut app = app(&f);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Browse));
        assert!(app.selected().unwrap().note.is_none());
    }

    #[test]
    fn esc_quits() {
        let f = fixture();
        let mut app = app(&f);
        press(&mut app, KeyCode::Esc);
        assert!(app.quit);
    }
}
