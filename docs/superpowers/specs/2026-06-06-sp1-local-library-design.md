# SP-1: Local Library Foundation — Design

**Status:** Approved (pending user review of this document)
**Date:** 2026-06-06
**Sub-project:** SP-1 of the animesh roadmap
**Successor:** SP-3 (Notification Daemon)

---

## 1. Context

animesh today is a single-command CLI: `animesh schedule` queries AniList and renders a table. There is no local state, no concept of a followed show, no persistence. The README sells a much larger vision — follow shows, get desktop notifications, see your backlog — but none of it is built.

SP-1 is the foundation that makes the rest of the roadmap possible. It introduces the local storage layer, the concept of a "tracked item," and the commands that let a user curate their library.

**Roadmap ordering (locked):**
SP-1 (this) → SP-3 (notifications) → SP-2 (watch progress) → SP-4 (backlog suggester) → SP-5 (streaming sources) → SP-6 (overlay UX) → SP-7 (generalize beyond anime).

## 2. The engineering bar

animesh is built to "engineering marvel" standard: Postgres-grade durability of the on-disk format, Task-Manager-grade efficiency of the hot path. This is not a hobby CLI. The bar applies to every decision in this spec.

Practical consequences:
- **The on-disk format is a 10-year API.** Versioned, atomic writes, forward-compatible migrations. A library created in v0.3 must open cleanly in v3.0.
- **Performance is budgeted, not hoped for.** Cold-start targets are stated in Section 8 and gated by benchmark in CI.
- **Boring-stable substrates are preferred** over novel ones, even when the novel option is more aesthetically pleasing.
- **No half-implementations.** Every command has `--help`, handles SIGINT, has tests, and is honest about failure modes.

## 3. Goals and non-goals

### Goals

- Persist a user's followed-show list locally, durably, and forever.
- Make read-path commands (`list`, `schedule` on followed shows, `doctor`) feel instant — sub-15ms cold start.
- Provide an interactive `follow` UX that hits the network only at commit time.
- Make the picker corpus warm itself through normal usage of other commands.
- Establish the schema-versioning and migration discipline that all future sub-projects depend on.
- Establish the durable-vs-ephemeral state boundary that future sub-projects build inside.

### Non-goals

- Watch progress tracking — that is SP-2.
- Notifications / background daemon — that is SP-3.
- Backlog suggestions — that is SP-4.
- Streaming-source lookup — that is SP-5.
- Generalizing the data model to non-anime sources — SP-7 will extract the abstraction. SP-1 names the schema neutrally (`tracked_item`, `kind`) but ships only the AniList adapter.

## 4. Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        animesh CLI                          │
│  follow │ unfollow │ drop │ list │ schedule │ sync │ doctor │
└─────────────────────────────────────────────────────────────┘
           │                  │                │
           ▼                  ▼                ▼
   ┌───────────────┐   ┌──────────────┐   ┌──────────────┐
   │  picker UI    │   │  renderer    │   │  observer    │
   │  (crossterm)  │   │ (comfy-table)│   │  (doctor)    │
   └───────┬───────┘   └──────┬───────┘   └──────┬───────┘
           │                  │                  │
           └──────────┬───────┴──────────┬───────┘
                      ▼                  ▼
            ┌─────────────────┐   ┌──────────────────┐
            │     store       │   │   anilist client │
            │  (rusqlite)     │◀──│   (reqwest)      │
            └────────┬────────┘   └──────────────────┘
                     │
                     ▼
             SQLite file (XDG path)
```

### Module boundaries

- **`src/store/`** — the only module that imports `rusqlite`. Exposes a typed API (`add_follow`, `list_follows`, `drop`, `cache_get`, `cache_put`, `search_fuzzy`, etc.). All schema lives here. Driver swap to `turso` someday is a single-module rewrite.
- **`src/anilist/`** — the only module that imports `reqwest`. Exposes typed methods (`search`, `by_id`, `schedule_window`). Adding a new source (SP-7) is a new sibling module.
- **`src/picker/`** — interactive crossterm UI. No I/O — delegates to `store` for local search and to `anilist` for fallback. Designed to be reused by SP-4's `next` and SP-6's overlay.
- **`src/renderer/`** — `comfy-table` output. No I/O.
- **`src/observer/`** — `doctor` and friends. Read-only.
- **`src/commands/`** — one file per command. Wires modules together.

This layout is the marvel-relevant boundary discipline: no module above the store knows what storage substrate is used; no module above `anilist` knows what HTTP client is used.

## 5. Data model

### 5.1 The durable / ephemeral split

The schema separates state into two categories with explicit different lifecycles:

- **Durable state.** Things that are *yours* — your follow act, your drops, your notes. Survives forever. Never auto-evicted. Losing it is a data-loss incident.
- **Ephemeral state.** Cached metadata from AniList — titles in alternate scripts, episode counts, next-airing times. TTL-bounded. Auto-evicted on read if stale. Losing it is a no-op (refetch on next access).

This split is the design's central invariant. It tells every future contributor which code paths need belt-and-braces and which can be casual.

### 5.2 Durable table: `tracked_item`

| column | type | notes |
|---|---|---|
| `id` | INTEGER PRIMARY KEY | our stable internal ID |
| `source` | TEXT NOT NULL | `"anilist"` today |
| `source_id` | TEXT NOT NULL | AniList's numeric ID, stored as text for source-neutrality |
| `kind` | TEXT NOT NULL | `"anime"` today; future: `"manga"`, `"podcast"`, etc. |
| `display_title` | TEXT NOT NULL | denormalized — offline `list` shows real names, not IDs |
| `followed_at` | INTEGER NOT NULL | unix seconds |
| `dropped_at` | INTEGER | NULL = active; non-NULL = soft-dropped |
| `user_note` | TEXT | reserved for future use; harmless to declare now |

Indexes:
- `UNIQUE(source, source_id)` — cannot follow the same show twice
- `(kind, dropped_at)` — fast "active anime" queries
- `(followed_at)` — ordering by recency of follow

### 5.3 Ephemeral table: `metadata_cache`

| column | type | notes |
|---|---|---|
| `source` | TEXT | composite PK part |
| `source_id` | TEXT | composite PK part |
| `display_title` | TEXT | current title (may differ from durable snapshot) |
| `title_english` | TEXT | for picker search |
| `title_native` | TEXT | for picker search |
| `status` | TEXT | `"releasing"`, `"finished"`, `"not_yet_released"` |
| `total_episodes` | INTEGER | NULL if ongoing/unknown |
| `format` | TEXT | `"TV"`, `"MOVIE"`, `"OVA"`, etc. |
| `next_episode_number` | INTEGER | for releasing shows |
| `next_episode_airs_at` | INTEGER | unix seconds |
| `fetched_at` | INTEGER NOT NULL | when we got this from source |
| `expires_at` | INTEGER NOT NULL | computed at fetch time per TTL policy |

`PRIMARY KEY(source, source_id)`. Index on `(expires_at)` for sweep queries.

### 5.4 Search index: `search_fts`

A SQLite FTS5 virtual table over `metadata_cache`'s title columns:

```sql
CREATE VIRTUAL TABLE search_fts USING fts5(
    display_title, title_english, title_native,
    content='metadata_cache', content_rowid='rowid',
    tokenize='unicode61 remove_diacritics 2'
);
```

Kept in sync with `metadata_cache` via triggers. Sub-millisecond fuzzy matching at any realistic library size, zero new dependencies (FTS5 ships with bundled SQLite).

### 5.5 TTL policy

| status | TTL | rationale |
|---|---|---|
| `releasing` | 6 hours | air dates change, new episodes drop |
| `not_yet_released` | 48 hours | premiere dates shift but slowly |
| `finished` | 30 days | almost never changes; still trim to drop dead caches |

Configurable via env vars (`ANIMESH_TTL_RELEASING`, etc.) for testing and power users.

### 5.6 Deliberately *not* stored

Cover image URLs, full descriptions, character lists, streaming source URLs (those come in SP-5), genres, popularity scores. Anything that's "AniList catalog data the user doesn't need on the hot path" lives only in AniList — fetched on demand for the rare detail-view command (not in SP-1).

This is the scope discipline: we are not copying AniList's database. We own only what's needed for the hot path.

## 6. Command surface

| command | purpose | network? |
|---|---|---|
| `animesh follow <query>` | open picker; commit a show to library | only on commit (or on AniList fallback) |
| `animesh follow --id <n>` | scripted/non-interactive follow | yes (validate) |
| `animesh unfollow <query-or-id>` | hard-remove (rare; mostly for mistakes) | no |
| `animesh drop <query-or-id>` | soft-delete; hides from default views | no |
| `animesh list` | show active library | no |
| `animesh list --all` | include dropped | no |
| `animesh list --dropped` | only dropped | no |
| `animesh schedule` | followed-only schedule (defaults to local cache); empty library prints a hint to run `follow` | only if cache stale |
| `animesh schedule --all` | global schedule (current behavior); warms cache as side effect | yes |
| `animesh sync` | refresh metadata cache for followed shows | yes |
| `animesh doctor` | observability surface | no |

**Breaking change:** `animesh schedule` post-SP-1 defaults to *your* shows. The pre-SP-1 behavior is preserved under `--all`. The README documents the change.

**Deliberately not shipped in SP-1:** `watched`/`unwatch` (SP-2), a generic `search` browse command, a detail view.

## 7. Picker UX

### 7.1 The principle

**Local-first. Network only on commit.** The picker is a fuzzy-search UI over the local FTS5 index. AniList is never queried during typing — only when the user presses Enter on a candidate (or explicitly requests a broader search).

### 7.2 Flow

1. `animesh follow naruto` opens picker instantly (single-digit ms).
2. Picker fuzzy-matches `naruto` against `search_fts`. Results appear as the user types.
3. User arrows + Enter → one AniList call to validate + enrich → write `tracked_item` + populate `metadata_cache`.
4. If local results are empty, or the user hits Tab ("search AniList"), the picker falls through to a network query with a visible "searching anilist…" indicator.

### 7.3 Corpus warming

The picker's search corpus is `metadata_cache`. It is populated by:

1. **Your followed shows** — always there.
2. **Implicit warming from `animesh schedule --all`** — every show in the response is upserted into `metadata_cache`. Free, automatic.
3. **Explicit `animesh sync`** — refreshes followed shows.
4. **First-run seeding** — see below.
5. **AniList fallback** — for the long-tail case.

Under normal use, the picker has a warm corpus and item #5 is exceptional. This is the 99% claim that drives the local-first design.

### 7.4 First-run experience

On the first invocation of `animesh follow` against an empty DB:

- A welcome screen renders immediately: keybindings cheatsheet, "what is animesh," tip of the day.
- In parallel (background task), the top-200 currently-airing shows are fetched from AniList and inserted into `metadata_cache`.
- By the time the user has read the welcome screen and started typing, seeding is complete.

This trades a ~1s cold first-run for a permanently warm picker afterward. Perceived latency: zero.

## 8. Cold-start budget

This is the Task-Manager-marvel surface. Targets are stated up front and gated by CI benchmark.

| command | target | notes |
|---|---|---|
| `animesh list` (pure local read) | **< 10 ms** | no tokio, no network, single SQL query |
| `animesh schedule` (followed-only, cache fresh) | **< 15 ms** | same path + light rendering |
| `animesh doctor` | **< 10 ms** | read-only, no network |
| `animesh follow --id <n>` (network) | **< 400 ms** | dominated by one AniList round-trip |
| `animesh sync` | bounded by AniList rate limits | not a cold-start target |

### How we hit these

1. **No tokio runtime unless network I/O is happening.** `main` is plain `fn main()`. Commands that need async (`follow`, `sync`, `schedule --all`) build a minimal `Runtime::new_current_thread` only when reached.
2. **Lazy SQLite open.** Connection opens only when first needed. `--help` and `--version` never touch disk.
3. **Lazy migrations.** `refinery::Runner::run` is called only on commands that need write access. Reads can proceed against any compatible schema version.
4. **No serde for hot paths.** `list`'s SQL maps directly into a `Vec<TrackedItem>` via `rusqlite` row mapping.
5. **Binary trimming.** `strip = true`, `lto = "thin"`, `codegen-units = 1` in release profile. Target binary < 5 MB.

### Verification

Every release runs a `criterion` benchmark suite. CI fails if any target regresses > 10%. Numbers go in the README. This is the public-discipline move Postgres makes with its performance regression tracking.

## 9. Migrations

- **Tool:** `refinery` with embedded SQL files under `migrations/`.
- **Naming:** `V0001__initial.sql`, `V0002__<change>.sql`, etc. Forward-only. Old files are immutable once shipped.
- **Application:** transactional, applied on first write-path command per process. Read-only commands run against any compatible version (forward-compatibility within a major version).
- **Refusal to run on unknown-newer version:** if the DB's schema version is greater than what the binary knows, the binary refuses to run with a clear "binary too old" error and exits with code 2. This prevents data loss from accidental downgrade.
- **Cache table migrations are non-events:** if `metadata_cache` or `search_fts` schema changes, we drop and re-create. Only `tracked_item` migrations require real care. This asymmetry is a direct consequence of the durable/ephemeral split.

## 10. Configuration and file paths

- **DB path:** XDG-compliant via the `directories` crate.
  - Linux: `$XDG_DATA_HOME/animesh/library.db` (default `~/.local/share/animesh/library.db`)
  - macOS: `~/Library/Application Support/animesh/library.db`
  - Windows: `%APPDATA%\animesh\library.db`
- **Override:** `ANIMESH_DB_PATH` env var (testing, power users).
- **TTL overrides:** `ANIMESH_TTL_RELEASING`, `ANIMESH_TTL_NOT_YET_RELEASED`, `ANIMESH_TTL_FINISHED` — integer seconds.
- **No config file in SP-1.** When configuration grows, a TOML config file in `$XDG_CONFIG_HOME/animesh/config.toml` will be introduced. SP-1 ships zero config; everything is convention or env var.

## 11. Error handling

Three categories, three behaviors:

1. **Durable-state errors** (DB corrupt, schema downgrade attempted, migration failed mid-way): refuse to run, print clear remediation including DB path and how to back up + reset. Never auto-repair durable state.
2. **Cache-state errors** (stale cache, FTS index corrupt, can't parse a cache row): silently invalidate the offending entry and refetch. Never user-visible. The cache is disposable by design.
3. **Network errors** (AniList down, rate-limited, timeout): degrade gracefully. `follow` on a cached candidate writes to durable with the cached metadata + queues refresh. `sync` reports partial success with retry-after. `schedule --all` falls back to local cache with `(offline — last synced N hours ago)` indicator.

### Exit codes

| code | meaning |
|---|---|
| 0 | success |
| 1 | user error (bad input, no match) |
| 2 | durable error (DB issue — needs intervention) |
| 3 | network error (transient — try again) |

Scripts branch on exit code. That is the marvel-spirit boundary contract.

## 12. Observability: `animesh doctor`

A read-only, no-network, sub-10ms surface that reports:

- DB file path
- Schema version (and what version this binary expects)
- Count of tracked items (active / dropped)
- Cache health: oldest entry, newest entry, count of stale entries, count expired-but-not-swept
- Last successful AniList sync timestamp; last sync error (with message) if any
- AniList rate-limit headroom — parsed from `X-RateLimit-Remaining` / `X-RateLimit-Reset` headers on the last response and persisted in a small `kv` table
- Binary version, build profile

This is the EXPLAIN of animesh. It is the surface a user shows in a bug report. It is the surface that gives the tool the "honest about itself" quality that Task Manager and Postgres share.

## 13. Testing strategy

- **Unit tests** in every module. `store` carries the bulk; in-memory SQLite makes setup trivial.
- **Property tests via `proptest`** over the durable layer. Invariant: for any sequence of `follow`/`unfollow`/`drop`/`migrate`/`crash-and-reopen` operations, the resulting DB state is consistent — no orphans, no duplicate `(source, source_id)`, `schema_version` matches, every row parseable. This is the marvel-correctness contract.
- **Snapshot tests via `insta`** for every command's stdout/stderr. Reformatting changes are visible as diffs.
- **Mock AniList via `mockito`.** Network is never hit in tests.
- **Cold-start benchmarks via `criterion`.** CI gates on the budgets in Section 8.
- **End-to-end smoke test** that builds the binary and runs `follow → list → schedule → sync → drop → list` against a real AniList account. Nightly CI only.

## 14. Dependencies

New dependencies introduced in SP-1:

| crate | purpose | rationale |
|---|---|---|
| `rusqlite` (bundled) | SQLite driver | 25-year-stable file format; bundled = no system dep |
| `refinery` | migration runner | well-trodden path; outsources runner discipline |
| `directories` | XDG paths | trivial; correct on all three OSes |
| `proptest` (dev) | property tests | marvel-correctness contract |
| `insta` (dev) | snapshot tests | reformatting changes become visible diffs |
| `criterion` (dev) | benchmarks | CI gate on cold-start budgets |
| `mockito` (dev) | HTTP mocking | network never hit in tests |

`crossterm`, `reqwest`, `serde`, `serde_json`, `tokio`, `chrono`, `chrono-tz`, `clap`, `comfy-table`, `colored`, `anyhow`, `async-trait` are already in the project.

## 15. What this unlocks

- **SP-3 (notifications)** becomes a thin layer: it polls `metadata_cache` for `next_episode_airs_at` transitions and emits desktop notifications. No new state needed beyond a "last notified for episode" dedupe table.
- **SP-2 (watch progress)** adds one table (`watch_progress`) and two commands. No changes to existing modules.
- **SP-4 (backlog suggester)** is a single ranking query over `tracked_item` ⋈ `watch_progress` ⋈ `metadata_cache`. Pure SQL.
- **SP-7 (generalization)** is a new `anilist`-sibling module per source. The `source` and `kind` columns are already there; nothing in the schema needs to change.

The durable/ephemeral split, the local-first picker, and the boundary discipline established here are the load-bearing decisions for the next year of work.

---

*End of design.*
