# Sources/search/ingest next steps

This document defines the next implementation sequence after the sources E2E work,
while explicitly incorporating `docs/SOURCES_REUSE_CLEANUP_PLAN.md`.

The current direction is correct:

```text
search/  owns query normalization and candidate result shapes
sources/ owns source adapters, HTTP, raw payload construction, parser attachment
ingest/  owns bounded discovery orchestration across sources + Library
library/ owns semantic durable mutations
store/   owns SQLite/rusqlite only
```

The next work should avoid new source-specific leaks and avoid adding more
copy-paste adapter plumbing. We already have enough sources to expose value; the
next goal is to make that value visible in the TUI and then extract duplicated
plumbing safely.

## Current expected network and adapter-routing behavior

```text
Startup blocking network: 0 requests
Typing in discovery/follow palette: 0 requests
Enter discovery: all enabled searchable source adapters, max 6 requests
Discovery results: title + Type + source evidence
Follow candidate: 1 selected-source detail ingest request
Manual sync: bounded due source_ref refreshes only
Periodic sync: bounded due source_ref refreshes only
```

There is no user-facing search scope. The user types a query, sees candidates with
`Type`, and picks the right candidate. Source adapter selection and request budget
are ingest/source infrastructure details, not TUI concepts.

## Gate for every phase

After each phase:

```sh
cargo test
```

Also check the boundaries:

```text
reqwest should appear only under src/sources/ except comments/tests if explicitly justified.
rusqlite should appear only under src/store/.
TUI should call Library/ingest services, never store/source adapters directly.
```

---

## Phase 0 — finish boundary cleanup before more feature wiring

### Objective

Make the architecture mechanically clean before adding more UI surface.

### Required work

#### 0.1 Move CanonicalId rusqlite impls out of `ids.rs`

`ids.rs` should be pure domain identity code:

```text
ids.rs
  CanonicalId
  ReleaseKind
  parse/display/validation
```

Move these into `store/`:

```rust
impl rusqlite::ToSql for CanonicalId
impl rusqlite::types::FromSql for CanonicalId
```

Recommended file:

```text
src/store/id_sql.rs
```

Then wire it from:

```text
src/store/mod.rs
```

Move the SQLite round-trip tests with it.

Why this is first:

```text
The project rule is strict: store/ is the only module that imports rusqlite.
```

#### 0.2 Remove or justify dead surface that is already obsolete

Do not do the whole cleanup plan yet, but remove obvious retired code if it is no
longer referenced.

Candidates:

```text
commands/follow.rs if still present and unused
old direct `:follow <id>` command path/messages — removed from TUI command registry
old AniList metadata-cache-only sync helpers if still present
```

Keep test-only constructors like `with_base_url` for now; they are useful for
mocked adapter tests.

### Done when

```text
cargo test passes
ids.rs has no rusqlite references
library/ has no rusqlite references
reqwest remains confined to sources/
```

---

## Phase 1 — complete unified discovery search

### Objective

Search is one user flow:

```text
a → type query → Enter → all enabled searchable adapters are queried within budget
  → dropdown shows title + Type + source evidence
  → user selects candidate → follow selected source candidate
```

The user-facing discriminator is candidate `Type`, not a source/search scope.
Do not add `SearchScope`, `:msearch`, `:asearch`, one key per media type, or any
other source/media-specific command surface.

### Required work

#### 1.1 Remove the scope abstraction from discovery

Delete the `SearchScope` enum and all adapter `search_scopes` /
`enrichment_scopes` plumbing. Source adapters expose search capability by
implementing `SourceAdapter::search`; concrete endpoint semantics stay inside the
adapter and parser.

`SourceRegistry` should expose one discovery list:

```rust
SourceRegistry::search_adapters() -> Vec<&dyn SourceAdapter>
```

No caller passes media/source scope into registry search routing.

#### 1.2 Keep Enter search bounded and source-agnostic

`IngestSearchService::refresh_candidates(...)` is the only discovery search
entrypoint for the TUI. It should:

```text
normalize query
for each enabled searchable adapter, up to RequestBudget::max_enter_search_requests:
  skip fresh source_search_cache rows for source/query
  call adapter.search(query, limit, now)
  store raw payload
  parse observations
  materialize source candidates
return Library::search_source_candidates(query, limit)
```

Initial global Enter-search budget:

```text
max_enter_search_requests = number of production searchable adapters = 6
```

If that becomes too expensive later, add adapter priority/budget policy in
`ingest/` or `sources/`; do not expose that as TUI scope.

#### 1.3 Make Type visible in the discovery dropdown

The discovery dropdown must show the candidate kind/type that already exists on
`SourceCandidateResult`:

```rust
pub kind: ReleaseKind
```

First rendering shape:

```text
Title                                  type:anime          source:jikan:5114
Severance                              type:tv             source:tvmaze:123
Taylor Swift                           type:music_artist   source:musicbrainz:...
```

A later result filter may hide/show candidates by `candidate.kind`, but that is a
local result filter, not source routing.

### Tests

Add/keep tests proving:

```text
SourceRegistry::search_adapters() returns all enabled searchable adapters.
IngestSearchService::refresh_candidates(...) queries plugged adapters up to global budget.
Typing in the TUI discovery palette performs 0 network requests.
Enter uses source_search_cache and skips fresh cached source/query pairs.
Direct `:follow <id>` / `:add <id>` / `:track <id>` commands are rejected.
```

### Done when

```text
No SearchScope symbol exists in src/.
No source/media-specific discovery command exists.
Typing is local-only.
Enter discovery is bounded and source-agnostic.
Dropdown shows Type for each candidate.
Selecting a candidate follows the selected source and detail-ingests only that source.
```

---

## Phase 2 — make canonical_schedule_event visible in the TUI

### Objective

The new source-agnostic ingest path writes durable schedule projection. The TUI
should render that projection instead of relying on the old AniList-shaped
`metadata_cache` serving fields.

Current durable projection:

```text
canonical_schedule_event
```

### Required work

#### 2.1 Extend the resolved read model

Either extend `ResolvedRelease` or add a separate detail-read method:

```rust
Library::next_schedule_events_for_followed(...)
```

The key is that TUI code should still call `Library`, not `store`.

Recommended first shape:

```rust
pub struct ResolvedRelease {
    ...
    pub next_schedule_event: Option<CanonicalScheduleEventSummary>,
}
```

Keep the event summary small for shelf rendering:

```rust
pub struct CanonicalScheduleEventSummary {
    pub event_kind: String,
    pub title: Option<String>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub scheduled_at: Option<i64>,
    pub source: String,
}
```

#### 2.2 Update shelf/pane classification

Use projected events for:

```text
PLAYABLE / DROPPING / FOLLOWING scheduling decisions
next episode display
release date display
```

Do not delete `metadata_cache` yet. Treat it as a legacy/cache read path until the
TUI no longer depends on it.

#### 2.3 Add projection-driven tests

Tests should prove:

```text
follow TVMaze candidate with embedded episodes
→ canonical_schedule_event rows exist
→ shelf displays/classifies using projected next event
```

### Done when

Following a source with schedule data creates visible TUI changes without needing
AniList-specific `metadata_cache` fields.

---

## Phase 3 — apply `SOURCES_REUSE_CLEANUP_PLAN.md` leaf-first

The E2E behavior is now valuable enough to expose. After that, do the extraction
pass from `SOURCES_REUSE_CLEANUP_PLAN.md`.

Important rule:

```text
No architecture change. No behavior change unless explicitly called out.
```

### 3.1 `sources/http.rs`

Extract duplicated transport helpers:

```rust
get_json(...)
url_with_path(...)
url_with_params(...)
raw_payload(...)
```

This should remove per-adapter copies from:

```text
jikan.rs
kitsu.rs
tvmaze.rs
musicbrainz.rs
itunes.rs
anilist.rs raw_payload only
```

Keep source-specific URL shape in each adapter.

MusicBrainz must keep its `User-Agent` header through the shared helper:

```rust
headers: &[("User-Agent", "...")]
```

### 3.2 `ingest/obs.rs`

Extract repeated observation builders:

```rust
push_alias(...)
push_image(...)
date_event(...)
```

Do not fold TVMaze episode event logic into this helper; it has source-specific
precision/timezone semantics.

### 3.3 `ingest/detail.rs`

Extract the duplicated detail-ingest ladder shared by follow and refresh:

```text
adapter.ingest
→ parse_fetch
→ store raw on parse failure
→ record parse error
→ compute next_event_at
→ record success/failure through Library
```

Keep `FollowIngestReport` and `RefreshReport` separate. The shared core should
return a neutral detail outcome.

### 3.4 Sign-off decisions before Phase 3 convergence

The cleanup plan flags two behavior convergences. Do not bundle them without an
explicit decision.

#### 3.4.1 Move failure backoff into Library

Recommended: yes.

Why:

```text
Library reads existing failure_count.
Library should compute next_due_at for failure consistently.
Callers should not duplicate or guess backoff.
```

This changes:

```rust
record_source_ingest_failure(source, source_id, error, next_due_at)
```

to something like:

```rust
record_source_ingest_failure(source, source_id, error)
```

or:

```rust
record_source_ingest_failure(source, source_id, error, policy)
```

#### 3.4.2 Uniform ID mismatch handling

Needs decision.

Current distinction:

```text
follow mismatch: hard error
refresh mismatch: should be recorded failure in batch context
```

Recommended eventual behavior:

```text
record as source ingest failure, do not project mismatched observation
```

But this changes follow error semantics, so decide explicitly.

### Done when

```text
cargo test passes after each extraction phase
source adapter boilerplate drops materially
new source N+1 requires parser + endpoint shape, not copy-pasted plumbing
```

---

## Phase 4 — source-specific robustness tests

After shared helpers land, add adapter-level request-shape tests for every source.

For each adapter:

```text
search builds expected URL/request
search raw payload has expected source/endpoint/method/request_key
search parser produces candidates through IngestSearchService
ingest builds expected URL/request
ingest parser produces observation
```

High-value cases:

```text
TVMaze ingest with _embedded.episodes projects multiple events
Jikan ingest /anime/{id}/full parses full payload
Kitsu search filter[text] and page[limit] are encoded correctly
MusicBrainz sends User-Agent
iTunes lookup strips source-id prefix track:/collection:/artist:
```

---

## Phase 5 — sync lifecycle polish

The refresh service exists conceptually; polish the lifecycle around it.

### Required work

#### 5.1 Manual sync

Manual sync should report:

```text
attempted
succeeded
failed
skipped_missing_adapter
remaining_due if known
```

#### 5.2 Periodic sync

Add bounded periodic refresh:

```text
max 5 due refs per tick
```

This should not block rendering.

#### 5.3 Startup background refresh

Startup should remain:

```text
0 blocking requests
```

Optional background startup refresh may run after first render:

```text
min(due refs, 10)
```

Only do this once the TUI can surface status without confusing the user.

---

## Phase 6 — dead-code and legacy cleanup

Do this after unified discovery and schedule projection are visible, otherwise
some items are only temporarily unused.

Known cleanup candidates:

```text
SourceRequest
  Delete if raw payload remains the only request evidence type.

SourceParser::source
  Use in parse diagnostics or remove.

FollowIngestReport::next_due_at
  Display it, use it in tests, or remove from report.

metadata_cache / TtlConfig / CacheStatus
  Decide whether this remains a legacy cache path or is replaced by source observations + schedule projection.

with_base_url constructors
  Keep if used by tests; otherwise make test-only or move behind cfg(test).

Theme dead_code suppressions
  unrelated to sources; handle separately.
```

---

## Recommended exact order from here

```text
0. Finish ids.rs → store/id_sql.rs boundary cleanup.
1. Complete unified discovery search cleanup: no SearchScope, bounded all-adapter Enter discovery, Type visible.
2. Add/keep discovery tests for typing=0 requests, cache skips, global request budget, and retired direct follow commands.
3. Render canonical_schedule_event in shelf/detail paths.
4. Run SOURCES_REUSE_CLEANUP_PLAN Phase 1: sources/http.rs.
5. Run SOURCES_REUSE_CLEANUP_PLAN Phase 2: ingest/obs.rs.
6. Run SOURCES_REUSE_CLEANUP_PLAN Phase 3: ingest/detail.rs.
7. Decide and apply Phase 3a/3b convergences.
8. Add deeper adapter request-shape tests.
9. Add periodic/startup background refresh.
10. Clean dead_code/legacy metadata_cache surface.
```

## What not to do next

Do not add another source before cleanup. The six-source shape is enough to prove
the architecture. Adding another adapter before `sources/http.rs` and `ingest/obs.rs`
would multiply the boilerplate this cleanup plan is trying to kill.

Do not move network orchestration into `Library`. `Library` records durable
mutations; `ingest/` orchestrates source adapters.

Do not let adapters from one media/domain leak into another scope by default.
Scope filtering is part of the request budget contract, but source-list and
budget decisions belong below the TUI.
