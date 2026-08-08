# animesh

A local-first personal release radar for macOS and Linux. Anime first, with a
core that can later support TV, music, and other scheduled media.

## Product goal

Follow the shows you care about, see what is coming next, and get notified when
a new episode releases. Your library stays on your machine in a local SQLite
database—no login or account required.

## Status

animesh is in active development and not yet packaged for installation.

**Next milestone:** ship v1 as a daemon and CLI on both macOS and Linux —
installable from Homebrew, with the full search/follow/`next` path usable with or
without notifications, and native notifications wherever the desktop provides them.

The broader goals—richer local data, backlog and history, TUI and other surfaces,
streaming availability, and cross-media support—remain unchanged and come after
this milestone.

## Surfaces

One background process owns the database, the AniList client, and the schedule.
Everything else is a client of it over a user-private Unix socket.

- **CLI** — complete. Every action is reachable here, with no desktop session.
- **Menu bar** — a glance at what is next, and a refresh. macOS only.
- **Notifications** — a reminder at airtime. Optional; nothing else depends on it.

## Commands

```bash
animesh search "one piece"   # find a title on AniList
animesh follow 21            # follow it, by AniList id
animesh next                 # upcoming episodes; local only, never hits the network
animesh list                 # everything you follow
animesh drop 1               # stop following, by media id
animesh refresh              # pull schedules now
animesh status               # health, and what to do about it
```

Exit codes: `0` success, `1` bad input, `2` needs intervention, `3` temporary—retry.

## Development

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

To run the real thing, build and install the app bundle. It links the CLI into
`~/.local/bin`, which needs to be on your `PATH`:

```bash
cargo xtask install
open /Applications/Animesh.app   # launch once to grant notification permission
```

The database lives at `~/Library/Application Support/Animesh/library.db`. Build
with `--features test-harness` to relocate it; set both `ANIMESH_DATA_ROOT` and
`ANIMESH_LOG_ROOT`, or neither takes effect.

## License

MIT
