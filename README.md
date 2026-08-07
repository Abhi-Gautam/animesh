# animesh

A local-first personal release radar for macOS. Anime first, with a core that can
later support TV, music, and other scheduled media.

## Product goal

Follow the shows you care about, see what is coming next, and get notified when
a new episode releases. Your library stays on your Mac in a local SQLite
database—no login or account required.

## Status

animesh is in active development. The current code is a CLI and daemon
prototype; it is not yet ready for normal installation or daily use.

**Next milestone:** install once, run automatically in the background like
Tailscale, show a small macOS menu-bar icon, send a native notification at the
AniList scheduled airtime, and expose the upcoming list through `animesh next`.

The broader goals—richer local data, backlog and history, TUI and other surfaces,
streaming availability, and cross-media support—remain unchanged and come after
this daily-driver milestone.

## Current prototype

```bash
# Terminal 1: start the daemon
cargo run -- daemon

# Terminal 2: search AniList
cargo run -- search "one piece"

# Check the next episode for an AniList ID
cargo run -- schedule 21

# Add or refresh it in the local watchlist
cargo run -- watchlist 21
```

The daemon currently prints due notifications to the terminal. Automatic
startup, background refresh, the menu-bar app, native notifications, and
`animesh next` are the next milestone.

## Development

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

Set `ANIMESH_DB_PATH` to use a custom database path while developing or testing.

## License

MIT
