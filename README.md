# animesh

A local-first release radar for macOS and Linux. Follow the shows you care
about, see what is next, and get notified when an episode drops. Everything
lives in a SQLite database on your machine — no account, no login, nothing
uploaded. Anime first, with a core that can later carry TV, film and music.

I built it because keeping up meant opening Crunchyroll and a countdown site
and doing it again an hour later:
[I just wanted to know when an episode dropped](https://syntropicsystems.dev/writing/animesh/)
is the long version — why one process owns the data, and how a small utility
turned into a notification pipeline.

## Expect it to break

animesh is under heavy development and changes often. Commands, output,
the JSON shape, the stored schema and the daemon protocol are all still moving,
and releases will break them without ceremony. Install it to use it, and expect
to reinstall. Do not build anything on top of it yet.

## Install

```bash
brew tap abhi-gautam/animesh https://github.com/Abhi-Gautam/animesh
brew install abhi-gautam/animesh/animesh
animesh service start
```

The tap is this repository, so the formula always matches the code.

Prebuilt tarballs for macOS (Apple Silicon and Intel) and Linux x86_64 are
attached to each [release](https://github.com/Abhi-Gautam/animesh/releases),
each with an `INSTALL.txt`.

`service start` registers the background process with launchd or systemd and
keeps it running across restarts. On macOS it asks for notification permission
the first time; declining is fine, since nothing in the CLI depends on it.

## Commands

```bash
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

Every command takes `--json` and answers with one line:

```bash
$ animesh --json next -n 3
{"data":[...],"kind":"upcoming","ok":true}
```

Failures answer `{"error":{"code":...},"ok":false}` on stdout. The `code` is
meant to be branched on; the message is prose and will change.

`animesh skill install` writes an [Agent Skill](https://agentskills.io) to
`~/.agents/skills/animesh/` — the vendor-neutral location read by Codex,
Cursor, Gemini CLI, Copilot, OpenCode and Goose — and mirrors it to
`~/.claude/skills/` when Claude Code is installed. An agent can then answer
what is airing tonight, follow something for you, or read what you actually
watch before recommending anything, against your library, on your machine.

`animesh skill status` says where it landed; `animesh skill uninstall` removes it.

## Development

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

On macOS the app has to run from a bundle, because notifications need a bundle
identifier. This builds one and links the CLI into `~/.local/bin`, which needs
to be on your `PATH`:

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
