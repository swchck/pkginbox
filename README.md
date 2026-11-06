# pkginbox

An inbox for packages installed by hand, outside the declarative dotfiles setup.

Everything on this machine is supposed to be installed from a chezmoi manifest. In
practice trial packages get installed by hand and are then either forgotten or — worse —
silently removed again by `brew bundle cleanup --force` on the next apply. `pkginbox`
notices those packages, keeps them alive while you decide, and writes the ones you keep
back into the manifest.

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
pkginbox brewfile     emit the quarantine Brewfile that protects packages from cleanup
```

In the TUI: `a` adopt, `q` hold, `d` leave unmanaged, `x` uninstall, `r` rescan,
`?` keys, `Esc` quit.

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

## State

The database lives at `${XDG_STATE_HOME:-~/.local/state}/pkginbox/inbox.db` and is local
to the machine. Quarantine is local by nature: a package is installed on one machine, and
that is the only machine where it needs protection from cleanup.

## License

MIT
