# animesh — Requirements Bible

**This is the master matrix every agent works against.** One row = one testable requirement.
After each milestone the owner (human) runs that milestone's **Acceptance Checklist** and flips
rows to ✅. Nothing ships past a milestone gate without the human pass.

Sources of truth, in priority order:

1. README "What it is" (the 10-point vision — `manifesto.md` is referenced by CLAUDE.md but does
   not exist on disk; the README list is the operative manifesto until one is written)
2. `docs/DATA_LIFECYCLE.md` — the network/storage/budget contract
3. `docs/SOURCES_NEXT_STEPS.md` + `docs/SOURCES_REUSE_CLEANUP_PLAN.md` — the active sequence
4. CLAUDE.md — architecture law (Library chokepoint, layering, error discipline)

**Status legend:** ✅ done & tested · 🔶 partial / in flight · ⬜ not started · 🚫 explicitly out of scope for now

---

## 0. Vision (the ten commandments)

| # | Vision point (README) | Covered by |
|---|---|---|
| V1 | Follow shows; animesh remembers them | FR-LIB-* |
| V2 | New episode drops → desktop notification, no phone/browser | FR-NOTIF-* |
| V3 | One view: aired today / airing tomorrow / late | FR-SCHED-* |
| V4 | Backlog view for "I have an hour free" | FR-BACKLOG-* |
| V5 | Where to stream it (Crunchyroll, Netflix, …) | FR-STREAM-* |
| V6 | Add / mark watched / snooze / drop — all keyboard, all instant | FR-TUI-*, FR-ENG-* |
| V7 | Local-first: one SQLite file, no login, no account | NFR-LOCAL-*, NFR-DUR-* |
| V8 | tmux overlay or standalone, native either way | FR-TMUX-* |
| V9 | Anime first, substrate is cross-media (TV, music, anything scheduled) | FR-XMEDIA-* |
| V10 | Tiny, fast, boring — useful in ten years | NFR-PERF-*, NFR-LONG-* |

---

## 1. Functional requirements

### 1.1 Library & engagement (FR-LIB, FR-ENG)

| ID | Requirement | Status | Milestone | Verify |
|---|---|---|---|---|
| FR-LIB-01 | Follow a candidate: atomic upsert canonical + attach source_ref + set followed_at | ✅ | M0 | `a` → pick candidate → row in all 3 tables; `cargo test library` |
| FR-LIB-02 | Drop = soft-delete, idempotent; re-follow restores | ✅ | M0 | `d` on a show → disappears; re-follow same show → history intact |
| FR-LIB-03 | Library facade is the only mutation path (TUI/CLI never touch store directly) | ✅ | M0 | CI architecture gate; grep: no `store::` calls above Library |
| FR-LIB-04 | List active follows newest-first; resolved read joins canonical + source_ref + cache + last events in one query | ✅ | M0 | TUI loads instantly with N follows; `load_resolved` tests |
| FR-ENG-01 | Mark watched (+1) appends engagement event, updates detail progress | ✅ | M0 | `w` on a show → progress increments in detail pane |
| FR-ENG-02 | Engagement event log: Opened/Completed/Verified/Paused/Snoozed/Rated, append-only | ✅ | M0 | `engagement_for` tests |
| FR-ENG-03 | Snooze: suppress a show from active panes until date/next episode | ⬜ | M4 | snooze a show → leaves Playable; reappears on schedule. (Event type exists; UI was deliberately removed — re-wire in M4 with notifications) |
| FR-ENG-04 | Rate a show (Rated event surfaced in UI) | ⬜ | M6 | rate from detail pane → persisted, shown |

### 1.2 Discovery & search (FR-DISC)

| ID | Requirement | Status | Milestone | Verify |
|---|---|---|---|---|
| FR-DISC-01 | Typing in discovery palette = **0 network requests**, local FTS over source candidates | ✅ | M0 | type with network off → results still appear; ingest tests assert 0 requests |
| FR-DISC-02 | Enter = budgeted fan-out to all enabled searchable adapters | ✅ | M0 | Enter query → candidates across registered source adapters appear; max_enter_search_requests bounds calls |
| FR-DISC-03 | Search cache: same source+query within 24 h → 0 requests for that source | ✅ | M0 | Enter same query twice → second is instant, cache tests |
| FR-DISC-04 | Discovery results show candidate **Type** (`ReleaseKind`) plus source evidence | ✅ | M1 | `a` → query → results include `type:... source:...` |
| FR-DISC-05 | No user-facing search scopes or per-media commands (`SearchScope`, `:msearch`, etc.) | ✅ | M1 | grep source; command parser rejects retired direct follow aliases |
| FR-DISC-06 | Old `:follow <id>` numeric escape hatch removed; `a` palette is the single follow surface | ✅ | M1 | `:follow 21` → UnknownVerb |
| FR-DISC-07 | Follow a candidate = exactly **1** detail-ingest request to the selected source | ✅ | M0 | follow → one raw_source_payload row for that source/id |
| FR-DISC-08 | Force-refresh command bypassing search cache | ⬜ | M3 | `:sync --force`-style path documented and working |

### 1.3 Schedule & panes (FR-SCHED, FR-BACKLOG)

| ID | Requirement | Status | Milestone | Verify |
|---|---|---|---|---|
| FR-SCHED-01 | Three panes (Playable / Dropping / Following) render from local SQLite only | ✅ | M0 | launch with network off → panes populate |
| FR-SCHED-02 | Schedule projection: observations → `canonical_schedule_event` rows on follow & sync | ✅ | M0 | follow a TVMaze-rich show → event rows exist |
| FR-SCHED-03 | **Panes/detail read from `canonical_schedule_event` projection, not legacy `metadata_cache` fields** | ⬜ | M1 | follow a TVMaze candidate with embedded episodes → next-episode shows without any AniList data |
| FR-SCHED-04 | `ResolvedRelease` (or Library read) exposes `next_schedule_event` summary (kind/season/episode/when/source) | ⬜ | M1 | detail pane shows projected next event |
| FR-SCHED-05 | Today/Late bucketing windows tunable (`ANIMESH_TODAY_WINDOW_HOURS`, `ANIMESH_LATE_WINDOW_HOURS`) | ✅ | M0 | set env vars → buckets shift |
| FR-SCHED-06 | Past/history view (what aired, watched vs missed) | ⬜ | M4 | needs historical episode data; pairs with notifications |
| FR-BACKLOG-01 | Backlog ordering: what's next when you have time | 🔶 | M5 | Following pane exists; "next unwatched" ordering rule TBD |
| FR-BACKLOG-02 | Runtime-window filter ("I have 45 min") — SP-4 | ⬜ | M5 | filter backlog by runtime |

### 1.4 Streaming / where-to-watch (FR-STREAM)

| ID | Requirement | Status | Milestone | Verify |
|---|---|---|---|---|
| FR-STREAM-01 | Detail pane lists verified streaming links with provider badges | ✅ | M0 | open detail → links render |
| FR-STREAM-02 | `g` opens primary streaming URL in browser, preferring subscribed streamers | ✅ | M0 | `g` → browser opens correct provider |
| FR-STREAM-03 | Streamer subscriptions persisted (`:subs`), influence link ranking & Playable classification | ✅ | M0 | `:subs` → set Crunchyroll → links reorder |
| FR-STREAM-04 | Streaming-source brand expansion (SP-5): more providers, region awareness | ⬜ | M6 | brands beyond current set resolve |
| FR-STREAM-05 | Links come from **source APIs only — never HTML scraping** (standing law) | ✅ law | all | no scraper/html5ever deps ever appear in Cargo.toml |

### 1.5 Sync & refresh lifecycle (FR-SYNC)

| ID | Requirement | Status | Milestone | Verify |
|---|---|---|---|---|
| FR-SYNC-01 | Manual `:sync` refreshes due source_refs, bounded at 50 | ✅ | M0 | `:sync` with due refs → toast reports counts |
| FR-SYNC-02 | Per-ref refresh state: last attempt/success/error, failure_count, exponential backoff (15 m·2^n, cap 24 h) | ✅ | M0 | failure tests; doctor shows state |
| FR-SYNC-03 | TTL by state: active 6 h · event<48 h 1 h · finished 30 d · music artist 7 d | ✅ | M0 | budget.rs constants + tests |
| FR-SYNC-04 | Manual sync reports: attempted / succeeded / failed / skipped_missing_adapter / remaining_due | 🔶 | M3 | `:sync` toast/report shows all five |
| FR-SYNC-05 | **Periodic background sync**: every 30 min, max 5 requests/tick, never blocks rendering | ⬜ | M3 | leave TUI open 30 min → refresh state advances, UI never stutters |
| FR-SYNC-06 | **Startup background refresh**: 0 blocking requests; after first render, min(due,10) | ⬜ | M3 | cold start with network off is instant & full |
| FR-SYNC-07 | Sync is idempotent — re-running produces no duplicate events/rows | ✅ | M0 | run `:sync` twice → identical row counts |
| FR-SYNC-08 | Failure backoff computed inside Library, not by each caller (cleanup 3a) | ⬜ | M2 | `record_source_ingest_failure` drops `next_due_at` param; one backoff site |
| FR-SYNC-09 | Uniform id-mismatch handling: recorded failure, never projected (cleanup 3b — needs sign-off) | ⬜ | M2 | mismatch test in both follow & refresh paths |

### 1.6 Notifications (FR-NOTIF) — SP-3

| ID | Requirement | Status | Milestone | Verify |
|---|---|---|---|---|
| FR-NOTIF-01 | Desktop notification when a followed show's episode drops | ⬜ | M4 | episode airs → macOS notification, app not focused |
| FR-NOTIF-02 | **Verify-then-notify**: only notify when a source-API streaming URL confirms availability (no scraping) | ⬜ | M4 | notification deep-links to a working stream |
| FR-NOTIF-03 | Notification dedup (kv-backed): one notification per episode, survives restarts | ⬜ | M4 | restart app → no re-notification |
| FR-NOTIF-04 | Snooze interacts with notifications (snoozed show ⇒ silent) | ⬜ | M4 | snooze → no notification for that show |

### 1.7 TUI shell (FR-TUI)

| ID | Requirement | Status | Milestone | Verify |
|---|---|---|---|---|
| FR-TUI-01 | Full keymap: j/k/arrows, Tab cycle, 1/2/3 jump, h/l switch, w/d/g/a/:/?/Esc/q | ✅ | M0 | help overlay matches behavior, all keys live |
| FR-TUI-02 | Command palette `:` with verbs watched/sync/doctor/help/quit/context/subs/theme/drop/open + aliases | ✅ | M0 | each verb executes; bad verb → UnknownVerb toast |
| FR-TUI-03 | `/` fuzzy jump within followed shows | ✅ | M0 | `/` → type → selection jumps |
| FR-TUI-04 | Help overlay (`?`), theme picker (`t`, Catppuccin ×4, persisted via kv) | ✅ | M0 | switch theme → restart → theme retained |
| FR-TUI-05 | Toasts for every command outcome (success and failure) | ✅ | M0 | actions produce visible feedback |
| FR-TUI-06 | Detail pane: title+aliases, watch progress, links, source ref + confidence, next episode | ✅ | M0 | visual check per show |
| FR-TUI-07 | LLM context copy (`c`): structured show metadata → clipboard | ✅ | M0 | `c` → paste into editor → well-formed context |
| FR-TUI-08 | Command palette fuzzy matching (nucleo) for verbs/candidates | ⬜ | M5 | partial verb input ranks correctly |
| FR-TUI-09 | Cover-art rendering: sixel / kitty / half-block fallback ladder | ⬜ | M5 | covers render in kitty, ghostty, Terminal.app (fallback) |
| FR-TMUX-01 | tmux overlay mode (popup) feels native | ⬜ | M7 | `tmux display-popup -E animesh` documented & clean |

### 1.8 Cross-media substrate (FR-XMEDIA)

| ID | Requirement | Status | Milestone | Verify |
|---|---|---|---|---|
| FR-XMEDIA-01 | Canonical model is media-agnostic (`CanonicalId` + `ReleaseKind`, no anime-only columns) | ✅ | M0 | schema review; ids.rs |
| FR-XMEDIA-02 | Cross-media discovery candidates carry `ReleaseKind` Type (`anime`, `tv`, `film`, `music_artist`) | ✅ | M0 | discovery dropdown and candidate tests show kind/type |
| FR-XMEDIA-03 | Music **ingest**: MusicBrainz/iTunes detail ingest (currently search-only) | ⬜ | M7 | follow an artist → release-group events project |
| FR-XMEDIA-04 | TV/film/music discovery does not require separate user-facing search modes | ✅ | M1 | unified discovery queries all searchable adapters; Type disambiguates results |
| FR-XMEDIA-05 | Beyond-anime sources (SP-7): F1/podcasts/films — adapter cost ≤ ~40 lines post-cleanup | ⬜ | M7+ | new adapter = parser + endpoint shape only |

### 1.9 Diagnostics & headless (FR-DIAG, FR-CLI)

| ID | Requirement | Status | Milestone | Verify |
|---|---|---|---|---|
| FR-DIAG-01 | `:doctor` — DB path, schema version, counts, refresh-state summary, rate headroom | ✅ | M0 | `:doctor` output complete & truthful |
| FR-DIAG-02 | Parse errors recorded durably (`source_parse_error`) and surfaced in doctor | 🔶 | M3 | recorder exists (dead_code) — wire into doctor |
| FR-CLI-01 | **Decision needed:** README documents CLI subcommands (follow/list/drop/sync/schedule/doctor) that no longer exist — either restore a headless surface or fix README + exit-code story | ⬜ | M3 | README and binary agree; scripts can still branch on exit codes 0/1/2/3 |

---

## 2. Non-functional requirements

### 2.1 Local-first & network discipline (NFR-LOCAL) — from DATA_LIFECYCLE.md, these are HARD budgets

| ID | Requirement | Budget | Status | Verify |
|---|---|---|---|---|
| NFR-LOCAL-01 | TUI reads local SQLite; network never required for ordinary reads | 0 | ✅ | airplane-mode launch: everything renders |
| NFR-LOCAL-02 | Typing | 0 requests | ✅ | test-asserted |
| NFR-LOCAL-03 | Enter discovery | ≤ 6 requests, skipped per fresh source_search_cache rows | ✅ | unified discovery/cache tests |
| NFR-LOCAL-04 | Follow | 1 request | ✅ | test-asserted |
| NFR-LOCAL-05 | Startup blocking | 0 requests | ✅ | cold-start offline |
| NFR-LOCAL-06 | Startup background | ≤ min(due,10) | ⬜ M3 | request-count test |
| NFR-LOCAL-07 | Periodic tick | ≤ min(due,5) / 30 min | ⬜ M3 | request-count test |
| NFR-LOCAL-08 | Manual sync | ≤ min(due,50) | ✅ | test-asserted |
| NFR-LOCAL-09 | Per-source politeness: Jikan ≤1 req/s, MusicBrainz ≤1 req/s + mandatory User-Agent | — | ✅ | adapter tests (UA pinned through M2 http.rs extraction) |
| NFR-LOCAL-10 | No login, no account, single SQLite file, `ANIMESH_DB_PATH` override | — | ✅ | path table in README |

### 2.2 Durability (NFR-DUR) — Postgres-grade bar

| ID | Requirement | Status | Verify |
|---|---|---|---|
| NFR-DUR-01 | Versioned migrations (V0001–V0007), one-shot, no destructive drift | ✅ | fresh DB == migrated DB schema |
| NFR-DUR-02 | FK constraints, UNIQUE indexes, idempotent upserts everywhere | ✅ | store tests (66) |
| NFR-DUR-03 | Raw source payloads stored as durable evidence; observations re-derivable | ✅ | raw_source_payload rows hash-keyed |
| NFR-DUR-04 | **WAL mode + foreign_keys pragma verified at open** | 🔶 M1 | `PRAGMA journal_mode` returns `wal` in doctor — currently unconfirmed, audit & pin with a test |
| NFR-DUR-05 | Kill -9 during sync → no corruption, no partial follow (atomic 3-write) | ✅ design / ⬜ tested | crash test in M3 |
| NFR-DUR-06 | DB survives version upgrades 10 years out (additive schema, kv for misc state) | ✅ policy | migration review each PR |

### 2.3 Architecture law (NFR-ARCH) — CI-enforced

| ID | Requirement | Status | Verify |
|---|---|---|---|
| NFR-ARCH-01 | `rusqlite` imports **only** under `src/store/` | 🔶 M1 | architecture.yml gate; ids.rs cleanup uncommitted on sp-1.6 — land it |
| NFR-ARCH-02 | `reqwest` **only** under `src/sources/` | ✅ | architecture.yml gate |
| NFR-ARCH-03 | Library = only mutation chokepoint; ingest orchestrates, never the reverse | ✅ | review + tests |
| NFR-ARCH-04 | TUI has no search scopes and never owns concrete source routing/budgets | ✅ M1 | grep TUI/source for `SearchScope`; TUI only calls ingest/Library |
| NFR-ARCH-05 | Reuse before building: shared plumbing extracted (http.rs / obs.rs / detail.rs), source N+1 ≈ 40 lines | ⬜ M2 | cleanup plan lands; line-count check on next adapter |
| NFR-ARCH-06 | No dead API surface: `SourceRequest`, unused report fields resolved (use or delete) | ⬜ M2 | zero `#[allow(dead_code)]` without a written justification |

### 2.4 Errors, performance, longevity (NFR-ERR / NFR-PERF / NFR-LONG)

| ID | Requirement | Status | Verify |
|---|---|---|---|
| NFR-ERR-01 | Exit codes: 0 ok · 1 user · 2 durable · 3 network; `UserError`/`NetworkError` wrappers used intentionally | ✅ | errors.rs + spot checks |
| NFR-ERR-02 | Network failures degrade gracefully: stale-but-rendered beats blank | ✅ design | offline behavior check each milestone |
| NFR-PERF-01 | Every keystroke instant (<16 ms frame); no network or blocking I/O on input path | ✅ | manual feel test each milestone; FR-DISC-01 |
| NFR-PERF-02 | Task-Manager-grade idle efficiency: near-0 CPU idle, bounded memory | 🔶 M3 | `top` while idle after periodic sync lands |
| NFR-PERF-03 | Cold start to first render: instant (no blocking requests, single query load) | ✅ | stopwatch test |
| NFR-LONG-01 | Tests green at every milestone gate (`cargo test`, currently ~188) | ✅ standing | CI |
| NFR-LONG-02 | Lint policy: warn locally, deny in CI; no `unreachable_pub` in local gate | ✅ | lint config matches policy |
| NFR-LONG-03 | deny.toml license/source hygiene; minimal dependency tree | ✅ | CI |
| NFR-LONG-04 | README never drifts from binary behavior past a milestone gate | ⬜ M3 | doc-vs-binary review at each gate (currently FAILING — see FR-CLI-01) |

---

## 3. Milestones & human acceptance gates

> Order follows `docs/SOURCES_NEXT_STEPS.md` exactly. Each gate = the human runs the checklist;
> all boxes ticked → next milestone unlocks.

### M0 — Substrate (SHIPPED, baseline)
Library facade, store v7, 6 adapters, E2E search→follow→refresh, TUI shell, ~188 tests.
**Gate (regression baseline):** launch offline → panes render · `a`→search→follow online · `w`/`d`/`g`/`:sync`/`:doctor` all work · `cargo test` green.

### M1 — Finish SP-1.6: boundaries + unified discovery + projection-backed TUI
Scope: land ids.rs→store/id_sql.rs, retire `:follow <id>`, WAL audit (NFR-DUR-04), unified discovery results with Type visible (FR-DISC-04/05), render `canonical_schedule_event` in shelf/detail (FR-SCHED-03/04).
**Human gate:**
- [ ] `a` opens discovery; typing stays local/offline; Enter performs bounded all-adapter discovery
- [ ] Discovery results show `type:<kind>` so the user picks the right candidate
- [ ] Follow a TVMaze-rich show → next episode appears with **no** AniList involvement
- [ ] `:follow 21` is gone; `ids.rs` has zero rusqlite; doctor reports `journal_mode=wal`

### M2 — Reuse cleanup + adapter robustness (the −430-line pass)
Scope: `sources/http.rs`, `ingest/obs.rs`, `ingest/detail.rs`, convergences 3a/3b (3b needs explicit sign-off — follow mismatch becomes recorded failure), dead-surface kill, per-adapter request-shape tests (Phase 4).
**Human gate:**
- [ ] `cargo test` green after each extraction; behavior unchanged in TUI walkthrough
- [ ] MusicBrainz still sends User-Agent (test-pinned)
- [ ] Decision recorded: itunes/musicbrainz ingest — finish or park with doc comment

### M3 — Sync lifecycle + headless story
Scope: rich sync report (FR-SYNC-04), periodic 30-min tick (FR-SYNC-05), startup background refresh (FR-SYNC-06), parse errors in doctor (FR-DIAG-02), kill-9 crash test (NFR-DUR-05), CLI/README decision (FR-CLI-01), idle-CPU check (NFR-PERF-02).
**Human gate:**
- [ ] Leave TUI open 35 min → refresh advances, zero UI stutter, ≤5 requests/tick
- [ ] Cold start offline instant; background refresh kicks in after render when online
- [ ] README matches the binary

### M4 — Notifications + snooze (SP-3)
Scope: FR-NOTIF-01..04, FR-ENG-03 (snooze re-wired), FR-SCHED-06 historical data. Law: verify-then-notify via source APIs, never scraping.
**Human gate:**
- [ ] Episode airs → one macOS notification with working deep link; restart → no duplicate
- [ ] Snoozed show: silent + hidden until due

### M5 — Visual & backlog polish (SP-4 + cover art)
Scope: FR-TUI-08/09 (palette fuzzy, cover art ladder), FR-BACKLOG-01/02 (runtime-window filter).
**Human gate:**
- [ ] Covers render in kitty + fallback in Terminal.app; backlog answers "I have 45 minutes"

### M6 — Streaming brand expansion (SP-5)
Scope: FR-STREAM-04, FR-ENG-04.
**Human gate:** providers beyond current set resolve correctly for 10 sampled shows.

### M7 — Cross-media + overlay (SP-7)
Scope: FR-XMEDIA-03/05 (music ingest, adapter N+1 ≤ ~40 lines proof), FR-TMUX-01.
**Human gate:** follow a music artist → release events in panes; tmux popup workflow documented and pleasant.

---

## 4. Standing laws (apply to every milestone, every agent)

1. **Library is the only mutation chokepoint.** Add the primitive there; callers stay trivial.
2. **rusqlite → store/ only. reqwest → sources/ only.** CI enforces; don't fight it.
3. **No HTML scraping, ever.** Source APIs only (verify-then-notify included).
4. **No network on the typing path. 0 blocking requests at startup.** Non-negotiable budgets (§2.1).
5. **Reuse before building.** Deep-dive what exists; extend or justify; recommend before adding.
6. **Migrations atomic & one-shot** — include the contract phase (drops/deletes) in the same change.
7. **`cargo test` green after every phase.** Warn locally, deny in CI.
8. **No new source adapters until the M2 cleanup lands** (explicit instruction in NEXT_STEPS).
9. **Marvel-tier bar:** Postgres-grade durability, Task-Manager-grade efficiency. "Good enough" gets flagged.
