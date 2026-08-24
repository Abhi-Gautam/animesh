# animesh

A local-first personal release radar for macOS and Linux. Anime first, with a
core that can later support TV, music, and other scheduled media.

## Product goal

Follow the shows you care about, see what is coming next, and get notified when
a new episode releases. Your library stays on your machine in a local SQLite
database—no login or account required.

## Where this is going

The schedule is the wedge, not the point. What accumulates is a durable local
record of what you follow, watch, miss, and come back to—anime first, then TV,
film, music, and anything else with a release date.

That record is meant to have two readers: a person asking what is on tonight,
and an agent that has to know what you are into before it can answer anything
useful about it. Local, structured, and yours, instead of re-derived badly by
every tool that asks.

## Status

animesh is in active development. The daemon, the CLI, notifications and the
agent skill are shipped; installation is from Homebrew on macOS and Linux.

The broader goals—richer local data, backlog and history, a window and other
surfaces, streaming availability, and cross-media support—remain unchanged.

## Surfaces

One background process owns the database, the AniList client, and the schedule.
Everything else is a client of it over a user-private Unix socket.

- **CLI** — complete. Every action is reachable here, with no desktop session.
- **Menu bar** — a glance at what is next, and a refresh. macOS only.
- **Notifications** — a reminder at airtime. Optional; nothing else depends on it.
- **Agents** — the same CLI in JSON, published as an Agent Skill. See below.

## Install

```bash
brew tap abhi-gautam/animesh https://github.com/Abhi-Gautam/animesh
brew install abhi-gautam/animesh/animesh
animesh service start
```

The tap is this repository, so the formula is always the one that matches the
code.

Prebuilt tarballs for macOS (Apple Silicon and Intel) and Linux x86_64 are
attached to each [release](https://github.com/Abhi-Gautam/animesh/releases), and
each one carries an `INSTALL.txt`.

`service start` registers the daemon with launchd or systemd, starts it, and
keeps it running across restarts. On macOS it will ask for notification
permission the first time; declining is fine, since nothing in the CLI depends
on it.

## Commands

```bash
animesh service start        # register the daemon; installing does this once
animesh search "one piece"   # find a title on AniList
animesh follow 21            # follow it, by AniList id
animesh next                 # upcoming episodes; local only, never hits the network
animesh list                 # everything you follow
animesh drop 1               # stop following, by media id
animesh refresh              # pull schedules now
animesh status               # health, and what to do about it
animesh skill install        # let any AI agent read and edit your library
```

`service` also takes `stop`, `restart` and `status`. It is a repair tool — the
daemon is registered at install and is not otherwise your concern.

Exit codes: `0` success, `1` bad input, `2` needs intervention, `3` temporary—retry.

## Agents

Every command takes `--json` and answers with one line: `{"data":..,"kind":..,
"ok":true}`, or `{"error":{"code":..},"ok":false}` on failure. The `code` is
stable; the message is prose.

```bash
animesh --json next -n 3
```

`animesh skill install` writes an [Agent Skill](https://agentskills.io) to
`~/.agents/skills/animesh/`, the vendor-neutral location read by Codex, Cursor,
Gemini CLI, Copilot, OpenCode and Goose, and mirrors it to `~/.claude/skills/`
when Claude Code is installed. After that, an agent can answer what is airing
tonight, follow a show for you, or read what you watch before recommending
anything — against your library, on your machine, with no account anywhere.

`animesh skill status` says where it landed; `animesh skill uninstall` removes it.

## Development

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

To run the real thing from a checkout on macOS, build and install the app
bundle. It links the CLI into `~/.local/bin`, which needs to be on your `PATH`:

```bash
cargo xtask install
animesh service start
```

On Linux the two binaries are all there is:

```bash
cargo build --release
./target/release/animesh service start
```

The database lives at `~/Library/Application Support/Animesh/library.db` on
macOS and `~/.local/share/animesh/library.db` on Linux. Build with
`--features test-harness` to relocate it; set both `ANIMESH_DATA_ROOT` and
`ANIMESH_LOG_ROOT`, or neither takes effect.

## License

MIT
