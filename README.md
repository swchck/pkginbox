# pkginbox

An inbox for packages installed by hand, outside the declarative dotfiles setup.

Everything on this machine is supposed to be installed from a chezmoi manifest. In
practice trial packages get installed by hand and are then either forgotten or — worse —
silently removed again by `brew bundle cleanup --force` on the next apply. `pkginbox`
notices those packages, keeps them alive while you decide, and writes the ones you keep
back into the manifest.

## Install

```sh
brew tap swchck/tap
brew trust --tap swchck/tap
brew install swchck/tap/pkginbox
```

`brew install --build-from-source swchck/tap/pkginbox` compiles the same
source instead of taking the bottle. Without Homebrew:

```sh
cargo install --git https://github.com/swchck/pkginbox
```

## How it works

Three states per package:

- **declared** — present in the chezmoi manifest for this machine
- **installed** — actually present on the system
- **recorded** — tracked in the local pkginbox database

Two kinds of drift follow: `installed ∧ ¬declared` (an incoming package to triage) and
`declared ∧ ¬installed` (something removed by hand that the next apply will bring back).

## Commands

```
pkginbox              interactive triage (TUI)
pkginbox scan         refresh the database from the system and the manifest
pkginbox status       one-line summary, for the end of `chezmoi apply`
pkginbox list         print tracked packages, optionally filtered by status
pkginbox adopt        write a package into the manifest (`--dry-run` to preview)
pkginbox quarantine   hold a package through the next cleanup
pkginbox dismiss      stop tracking a package
pkginbox sources      show which kinds of package are watched here
pkginbox untrack      stop watching a kind of package entirely
pkginbox track        watch it again
pkginbox brewfile     emit the quarantine Brewfile that protects packages from cleanup
```

In the TUI: `a` adopt, `q` hold, `d` leave unmanaged, `x` uninstall (or stop
declaring a missing package), `i` reinstall, `e` note, `t` stop watching the
current source, `/` filter by name, `Tab` to walk the sources, `r` rescan,
`?` keys, `Esc` quit.

It is drawn in One Dark, taken from the colours the shell is already themed
with, with Nerd Font marks for the sources. A terminal without those fonts shows
a box where the mark is, not a broken layout.

The interface opens on what the database already knows and refreshes itself when
the background scan lands, rather than waiting for a scan before the first
frame.

## Speed

A full scan takes about 0.1 seconds. The process itself holds around 4 MB;
`/usr/bin/time` reports a much larger peak, and that is `chezmoi data`, a Go
binary, not pkginbox.

Almost all of it came from not asking a package manager what it has installed.
Every one of them starts a language runtime to print back something already
sitting on disk in a form that is easier to read:

| Instead of | Read |
|---|---|
| `brew info --json=v2 --installed` (2.3 s) | the install receipts in `Cellar` and `Caskroom` |
| `claude plugin list` (1.0 s) | `~/.claude/plugins/installed_plugins.json` |
| `npm ls -g --depth=0` (0.8 s) | the global `node_modules` directory |
| `cargo install --list` (50 ms) | `~/.cargo/.crates2.json`, which is where cargo keeps it |

Reading the Cellar is also the more truthful answer: `brew info` quietly omits
a formula whose tap has been removed, while the keg is still there and cleanup
would still delete it.

Two tools genuinely have to be run, so their answers are cached against a
stamp of the files they describe — sizes and modification times — and the
process only starts when that stamp changes:

- `go version -m` reads module metadata baked into each binary. Stamped
  against `$GOBIN`.
- `mas list` talks to the App Store. Stamped against every application
  bundle, not only the App Store ones, so the stamp errs towards asking again.

`chezmoi data` is the third, and stays. The manifest is merged by Go
templates, and re-implementing that merge would be a second source of truth
that could disagree with the real one.

What is left runs in parallel, including the receipt reading itself, which is
a few hundred small JSON files and splits across cores for free.

Spotlight is the one cache that ages rather than being stamped against files:
when an app was last opened changes without any file changing. It is reused
for an hour, which is far inside the resolution of the only question asked of
it — whether something has gone untouched for weeks.

`PKGINBOX_TIMING=1 pkginbox scan` prints what each provider cost.

### Things that did not help

Measured, and kept out:

- **Profile-guided optimization.** 165.7 ms to 160.0 ms, inside the noise.
  The tool spends its time waiting on Spotlight, on `chezmoi` and on the
  filesystem, not computing, so there is little for a profile to improve.
  Against that, the formula would have to build twice with a training run in
  between.
- **An allocator that never frees**, on the theory that a one-shot tool need
  not clean up. It was 25% *slower*: nothing can be reused, so every
  allocation goes back to the kernel for fresh pages.
- **One transaction per scan** instead of a few hundred. No measurable
  difference — WAL with `synchronous = NORMAL` had already made the commits
  cheap. It is in anyway, because it makes a scan atomic: an interrupted one
  can no longer leave half of a provider's rows updated.

What is left is the floor set by the system calls the tool has to make.

## What it watches

| Provider | Installed state | Declared in |
|---|---|---|
| `brew` | install receipts in `Cellar` | `cli`, `languages` |
| `cask` | install receipts in `Caskroom` | `casks`, `fonts` |
| `tap` | the directories in `Library/Taps` | `taps`, plus tap/trust lines in the install script |
| `mas` | `mas list`, cached | `appstore_apps` |
| `go` | `go version -m $GOBIN`, cached | `[packages.go]` |
| `cargo` | `~/.cargo/.crates2.json` | `[packages.cargo]` |
| `npm` | the global `node_modules` directory | `[packages.npm]` |
| `uv` | `uv tool list` | `[packages.uv]` |
| `vscode` | `code --list-extensions` | `[vscode] extensions` |
| `claude` | `~/.claude/plugins/installed_plugins.json` | `[[claude.plugins.install]]` |
| `ollama` | `ollama list` | `[packages.<os>.personal.ollama] models` |
| `app` | `/Applications`, minus casks and App Store apps | nothing — these cannot be declared |

A provider whose tool is not installed is skipped rather than reported as empty.

The `app` provider is the one that only ever reports. A bundle dragged out of a
disk image has no manifest entry that would reproduce it, so the useful outcome
is recognising it and either finding a cask or leaving it alone deliberately.

## Adoption

Adopting writes the package into the chezmoi manifest and nothing else — the
manifest stays the source of truth. The edit is line-based rather than a TOML
rewrite, so it preserves the hand-alignment and the group headings those files
are kept in, and adds exactly one line:

```
+    "mole",                 # Deep clean and optimize your Mac
```

A tap needs more than a manifest entry: `brew bundle` refuses formulae from an
untrusted tap, so adopting one also adds the matching `brew tap` and
`brew trust --tap` lines to the install script, inside the branch for the
machine profile you picked. If the manifest stops parsing after an edit, every
file is restored before anything else happens.

## Catching an install as it happens

A scan can tell that a package appeared; it cannot tell how. The exact
invocation — `--HEAD`, a pinned `@v1.2.3`, which of two taps a formula came
from — exists only while the command is running. A fish hook captures it:

```fish
function __pkginbox_record --on-event fish_postexec
    set -l exit_status $status
    pkginbox record --status $exit_status --shell-command $argv[1] &
end
```

This is a post-exec event rather than wrapper functions around `brew`, `go` and
the rest. A wrapper shadows the real binary, so one mistake in it breaks that
command everywhere — including inside the dotfiles manager's own scripts. The
event runs after the command and cannot change its result.

The hook only covers interactive fish, which is exactly where packages get
installed by hand. Anything that happens elsewhere is picked up by the next
scan, so the two mechanisms cover each other.

Installing something the manifest already declares records nothing: the
manifest is checked first, so there is no entry for the next scan to close.

`pkginbox note <provider> <key> <text>`, or `e` in the TUI, attaches the reason
a package is there — the one thing neither mechanism can work out on its own.

## When something declared goes missing

The other direction of drift: the manifest declares a package, the machine no
longer has it, and the next apply will quietly put it back. Two answers, both
on the entry itself — `i` reinstalls it here, `x` stops declaring it.

Dropping a declaration is the mirror of adopting one, and for a tap it is three
lines in two files: the manifest entry, the `brew tap` line and the
`brew trust --tap` line. Removing the last member of a group takes the group's
heading with it, so the manifest does not fill up with empty sections.

Which profile declared the package is not recorded anywhere, so every candidate
file is tried until one of them holds the entry.

## Across machines

The database is local, but a snapshot of it is written next to the manifest:

```
inbox/<hostname>.json
```

One writer per file, so two machines never produce a merge conflict, and the
machine is identified by the file name itself. Each entry shows what the other
machines decided about the same package — useful when the laptop adopted
something the desktop is still undecided about.

Scans refresh the snapshot but never commit it; committing behind your back in
your own dotfiles repository is not something a status line should do.
`pkginbox export` is the command that commits it deliberately.

Applications are left out. A bundle dragged out of a disk image has no
manifest entry for another machine to have an opinion about, so the entry
would say nothing — and a dotfiles repository is often public, which makes a
list of every application on a work laptop something not to publish by
accident.

## Is it actually being used?

Neither the manifest nor the inbox can tell a tool you reach for daily from one
you installed once and forgot. Two signals can: atuin's shell history for
commands, and Spotlight's last-used date for applications.

The package name is usually not the command name — ripgrep installs `rg`,
rm-improved installs `rip` — so the commands come from the package itself: the
keg's `bin` for a formula, the executables `cargo install --list` and
`uv tool list` print under each entry, the last path segment for a Go tool.

Three outcomes, kept apart on purpose:

- a timestamp — last run or opened then
- never — the history was searched and found nothing
- nothing at all — this kind of package runs nothing and opens nothing, so
  there is no honest answer. Taps, fonts and editor extensions land here.

That distinction is what makes "installed and never used" safe to print: it is
only ever said about a package whose history was actually searched, and only
after it has been around long enough for the absence to mean something.

The signal sees commands you typed. A tool invoked by an editor, a script or
another tool — a debugger launched from the IDE, a linter run by a test
harness — looks unused because, as far as the shell is concerned, it is.

## State

The database lives at `${XDG_STATE_HOME:-~/.local/state}/pkginbox/inbox.db`, beside the
caches described above, and is local
to the machine. Quarantine is local by nature: a package is installed on one machine, and
that is the only machine where it needs protection from cleanup.

## License

MIT
