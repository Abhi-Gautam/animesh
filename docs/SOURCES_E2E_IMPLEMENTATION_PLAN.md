# Sources/search/ingest/library E2E implementation plan

This document is the concrete implementation plan for completing the sources
part of animesh end-to-end.

It assumes the data lifecycle/request contract in `docs/DATA_LIFECYCLE.md`:

```text
startup = 0 blocking requests
typing = 0 requests
Enter discovery = max 6 searchable source requests
follow candidate = 1 selected-source detail ingest request
sync = due followed source_refs only, bounded
TUI reads local store
```

## Phase 0: ground rules

Searchable discovery sources:

```text
AniList
Jikan
Kitsu
TVMaze
MusicBrainz
iTunes
```

Rules:

```text
Startup:
  0 blocking network requests

Typing:
  0 network requests

Enter discovery:
  max 6 searchable source requests

Follow candidate:
  1 selected-source ingest request

Manual sync:
  max 50 due source_ref ingest requests

Periodic sync:
  max 5 due source_ref ingest requests per tick

Discovery sources:
  all enabled searchable adapters participate in bounded Enter discovery
  candidates carry Type so the user chooses the right result
```

## Phase 1: source registry completeness

### Objective

Make the source registry expose searchable adapters without media/source scopes.
The user searches once; candidate Type disambiguates results.

Adapter shape:

```rust
pub trait SourceAdapter {
    fn source(&self) -> &'static str;
    fn parser(&self) -> &dyn SourceParser;

    fn search(...) -> SourceFuture<Vec<RawSourcePayload>>;
    fn ingest(...) -> SourceFuture<Option<RawSourcePayload>>;
}
```

No `SearchScope`, `search_scopes`, or `enrichment_scopes` plumbing belongs in the
adapter port.

Initial discovery behavior:

```text
Enter discovery queries all enabled searchable adapters within budget:
  AniList
  Jikan
  Kitsu
  TVMaze
  MusicBrainz
  iTunes

Dropdown candidates show:
  title + Type + source evidence
```

### Registry lookup

Add/keep:

```rust
SourceRegistry::search_adapters()
SourceRegistry::adapter(source)
```

Needed by:

- Enter discovery
- follow-time ingest
- sync/refresh

Example:

```rust
for adapter in registry.search_adapters() {
    ...
}
```

and:

```rust
let adapter = registry.adapter(candidate.source.as_str())?;
```

## Phase 2: request budget constants

### Objective

Centralize request limits and TTLs.

Add:

```text
src/ingest/budget.rs
```

Request budget:

```rust
pub struct RequestBudget {
    pub max_enter_search_requests: usize,       // 4
    pub max_follow_ingest_requests: usize,      // 1
    pub max_startup_background_requests: usize, // 10
    pub max_periodic_requests: usize,           // 5
    pub max_manual_sync_requests: usize,        // 50
}
```

Default:

```rust
impl Default for RequestBudget {
    fn default() -> Self {
        Self {
            max_enter_search_requests: 4,
            max_follow_ingest_requests: 1,
            max_startup_background_requests: 10,
            max_periodic_requests: 5,
            max_manual_sync_requests: 50,
        }
    }
}
```

TTL constants:

```rust
pub const SEARCH_CACHE_TTL_SECS: i64 = 24 * 3600;
pub const ACTIVE_REFRESH_TTL_SECS: i64 = 6 * 3600;
pub const NEAR_EVENT_REFRESH_TTL_SECS: i64 = 3600;
pub const FINISHED_REFRESH_TTL_SECS: i64 = 30 * 24 * 3600;
pub const MUSIC_ARTIST_REFRESH_TTL_SECS: i64 = 7 * 24 * 3600;
```

## Phase 3: search cache enforcement

### Objective

Repeated Enter searches should not repeatedly hit sources for identical recent
queries.

Table already exists:

```text
source_search_cache
```

### Query normalization

Use:

```rust
normalize_query_key(query)
```

Expected behavior:

```text
"  Frieren   Beyond " → "frieren beyond"
```

### Update `IngestSearchService`

Current behavior:

```text
loop adapters
→ source.search
```

Target behavior:

```text
for adapter in primary search adapters:
    query_key = normalize_query_key(query)
    cache = Library.get_source_search_cache(source, query_key)

    if cache.next_due_at > now:
        skip source request

    else:
        source.search(...)
        store raw
        parse/store observations
        upsert source_search_cache(source, query_key, now, now+24h)
```

Request count:

```text
<= 4
```

If some sources are cached:

```text
requests < 4
```

### Failure behavior

If source fails:

```text
do not update last_success_at
optionally set short next_due_at, e.g. now + 15m
```

First version can simply not update cache on failure.

## Phase 4: follow-time detail ingest

### Objective

Following a candidate should complete useful local state.

Flow:

```text
candidate selected
→ Library.follow_source_candidate(candidate)
→ selected adapter.ingest(source_id)
→ store raw detail payload
→ parse fetch observation
→ store observation
→ project release events into canonical_schedule_event
→ initialize refresh state
```

### Create service

Add either:

```text
src/ingest/follow.rs
```

or extend `ingest/service.rs` with separated methods.

Recommended:

```rust
pub struct FollowIngestService<'a> {
    library: &'a Library,
    sources: &'a SourceRegistry,
}
```

Method:

```rust
pub async fn follow_and_ingest(
    &self,
    candidate: &SourceCandidateResult,
    now: i64,
) -> Result<FollowIngestReport>
```

Report:

```rust
pub struct FollowIngestReport {
    pub outcome: CanonicalFollowOutcome,
    pub candidate: SourceCandidateResult,
    pub detail_ingested: bool,
    pub projected_events: usize,
    pub next_due_at: Option<i64>,
}
```

### Canonical ID rule

Use:

```rust
CanonicalId::legacy_from_source(
    candidate.kind,
    &candidate.source,
    &candidate.source_id,
)
```

`Library::follow_source_candidate` already uses this. The service should compute
the same ID for projection.

Potential helper:

```rust
Library::canonical_id_for_source_candidate(candidate)
```

### Detail ingest request count

Exactly:

```text
1 request
```

Only:

```text
candidate.source.ingest(candidate.source_id)
```

No fan-out.

### Projection

If detail ingest returns an observation with events:

```rust
Library.project_canonical_schedule_events(
    &canonical_id,
    &candidate.source,
    &observation,
)
```

### Refresh state

After success:

```rust
SourceRefRefreshState {
    source,
    source_id,
    last_attempt_at: Some(now),
    last_success_at: Some(now),
    last_error: None,
    next_due_at: Some(now + ttl_for_observation(...)),
    failure_count: 0,
}
```

If ingest fails:

```text
follow still succeeds
refresh state stores failure:
  last_attempt_at = now
  last_success_at = previous/null
  last_error = Some(error)
  next_due_at = now + failure_backoff(1)
  failure_count += 1
```

Decision:

```text
follow succeeds even if detail ingest fails
```

Reason: user intent should be persisted. The ingest failure is surfaced as a
warning/toast and retried later.

## Phase 5: schedule TTL function

### Objective

Compute next refresh due time from observation facts.

Add function:

```rust
fn next_refresh_due_at(
    kind: ReleaseKind,
    status: Option<&str>,
    next_event_at: Option<i64>,
    now: i64,
) -> i64
```

Rules:

```text
if next_event_at within 48h and next_event_at > now:
    now + 1h

else if status active/upcoming/releasing/running/airing:
    now + 6h

else if kind == MusicArtist:
    now + 7d

else if status finished/released/ended:
    now + 30d

else:
    now + 6h
```

Status strings differ by source:

```text
AniList: RELEASING, FINISHED, NOT_YET_RELEASED
TVMaze: Running, Ended
Jikan: Currently Airing, Finished Airing, Not yet aired
Kitsu: current, finished, tba, unreleased, upcoming
```

Start with lowercase matching/contains.

## Phase 6: refresh service

### Objective

Replace source-specific sync with source-agnostic bounded refresh.

Add:

```text
src/ingest/refresh.rs
```

or separate service inside `ingest/service.rs`.

Recommended:

```rust
pub struct RefreshService<'a> {
    library: &'a Library,
    sources: &'a SourceRegistry,
}
```

Method:

```rust
pub async fn refresh_due(
    &self,
    budget: usize,
    now: i64,
) -> Result<RefreshReport>
```

Report:

```rust
pub struct RefreshReport {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped_missing_adapter: usize,
    pub failures: Vec<(String, String, String)>, // source, source_id, error
}
```

### Due selection

Use:

```rust
Library.due_source_ref_refresh_states(limit)
```

Newly followed refs must have refresh state.

Old followed refs may be handled with a backfill method later:

```rust
list_followed_source_refs_missing_refresh_state(limit)
```

Since the project is in active development, first version can assume follow-time
ingest creates refresh state.

### Refresh each due ref

Flow:

```text
for state in due states up to budget:
    adapter = registry.adapter(state.source)
    if missing:
        mark skipped / next_due later
        continue

    raw = adapter.ingest(state.source_id, now)
    store raw
    obs = parser.parse_fetch(raw)
    store obs
    canonical_id = source_ref lookup
    project events
    update refresh state success/failure
```

Needed Library/store method:

```rust
find_source_ref(source, source_id) -> Option<SourceRef>
```

or:

```rust
canonical_id_for_source_ref(source, source_id)
```

### Manual sync

Change `commands/sync.rs` to call:

```rust
RefreshService.refresh_due(50, now)
```

### Periodic sync

Later TUI tick can call:

```rust
RefreshService.refresh_due(5, now)
```

Startup remains:

```text
0 blocking network requests
```

If background startup refresh exists, it must not block initial render.

## Phase 7: source adapters

Once lifecycle is wired, add remaining primary adapters.

### JikanSource

File:

```text
src/sources/jikan.rs
```

Add:

```rust
pub struct JikanSource {
    client: reqwest::Client,
    parser: JikanParser,
    base_url: String,
}
```

Search:

```text
GET https://api.jikan.moe/v4/anime?q={query}&limit={limit}
```

Ingest:

```text
GET https://api.jikan.moe/v4/anime/{id}/full
```

Request count:

```text
search = 1
ingest = 1
```

Rate:

```text
1 request/sec budget
```

Tests:

- mock search response → raw payload → generic ingest service → candidate
- mock detail response → follow ingest service → schedule projection

### KitsuSource

Search:

```text
GET https://kitsu.io/api/edge/anime?filter[text]={query}&page[limit]={limit}
```

Ingest:

```text
GET https://kitsu.io/api/edge/anime/{id}
```

Tests same pattern.

### TvMazeSource

Search:

```text
GET https://api.tvmaze.com/search/shows?q={query}
```

Ingest:

```text
GET https://api.tvmaze.com/shows/{id}?embed=episodes
```

This is high-value because detail ingest can project full episode events.

Tests:

- detail ingest with `_embedded.episodes`
- projected canonical events count > 0

### Registry production

After adapters exist:

```rust
SourceRegistry::production() = vec![
    AniListSource,
    JikanSource,
    KitsuSource,
    TvMazeSource,
    MusicBrainzSource,
    ItunesSource,
]
```

This preserves:

```text
Enter discovery max = 6 requests
```

### Cross-media discovery sources

MusicBrainz and iTunes participate in the same bounded discovery fan-out as the
other searchable adapters. They are not exposed through separate user-facing
search modes.

## Phase 8: unified discovery routing

Once adapters exist, enforce:

```rust
registry.search_adapters()
```

Production discovery returns exactly:

```text
AniList
Jikan
Kitsu
TVMaze
MusicBrainz
iTunes
```

Candidates expose Type (`ReleaseKind`) so the user can distinguish media kinds in
the dropdown.

Tests:

```rust
production_discovery_searches_all_enabled_adapters()
production_discovery_budget_matches_enabled_searchable_adapters()
```

## Phase 9: UI integration

### Search bar

Behavior:

```text
typing:
  local FTS only

Enter:
  online discovery via IngestSearchService across enabled searchable adapters
```

Service call:

```rust
refresh_candidates(query, limit, now)
```

The service owns request budgeting and cache skips. The TUI never passes media or
source scopes.

### Follow confirm

Replace:

```rust
follow_candidate_inner(...)
```

with:

```rust
FollowIngestService.follow_and_ingest(candidate, now)
```

Toast examples:

```text
✓ Followed Frieren · ingested 28 events
✓ Followed One Piece · no schedule events found
✓ Followed X · detail ingest failed, will retry
```

### Sync

`:sync` becomes:

```rust
RefreshService.refresh_due(50, now)
```

Toast examples:

```text
✓ Synced 42/50 due
synced 38/50, 12 failed
nothing due
```

## Phase 10: required tests

### Request budget tests

- typing does not call network
- Enter discovery with 6 searchable adapters makes max 6 adapter calls
- cached search query skips adapter calls
- candidates expose Type so the user can distinguish media kinds

### Follow ingest tests

- candidate follow persists canonical/source_ref
- follow calls only selected source ingest
- observation stored
- schedule events projected
- refresh state created
- detail ingest failure keeps follow but records retry state

### Refresh tests

- refresh only due refs
- respects budget limit
- calls matching adapter by source
- missing adapter skipped/recorded
- success updates `next_due_at` and `failure_count = 0`
- failure increments `failure_count` and backs off
- schedule projection updates existing event idempotently

### Adapter tests

For each primary source:

- search builds correct URL/request
- ingest builds correct URL/request
- raw payload fields correct:
  - source
  - endpoint
  - method
  - request_key
  - request_hash
  - response_hash
- parser works through generic service
- candidate searchable after ingest

### Startup test

Ensure app creation does not perform network.

Use a test adapter that panics if called. Constructing `App` should not call it
and should not panic.

## Phase 11: completion definition

The sources/search/ingest/library path is complete when all of the following
are true.

### Search

```text
typing = 0 requests
Enter discovery = max 6 searchable source requests
search cache suppresses repeated requests
results stored as source candidates with Type
```

### Follow

```text
follow candidate = 1 selected-source ingest request
canonical/source_ref/followed state stored
detail observation stored
schedule events projected
refresh state initialized
```

### Sync

```text
sync refreshes due followed refs only
manual sync max 50 requests
periodic sync max 5 requests
startup blocking max 0 requests
```

### Sources

```text
AniList/Jikan/Kitsu/TVMaze/MusicBrainz/iTunes discovery adapters implemented
candidate Type disambiguates media kind in the dropdown
```

### Store

```text
raw payloads preserved
source observations preserved
source candidates searchable
refresh state controls request timing
canonical schedule projection powers product serving layer
```

## Recommended immediate implementation order

1. Expose all enabled searchable adapters through `SourceRegistry::search_adapters()`.
2. Add request budget constants.
3. Enforce `source_search_cache` in `IngestSearchService`.
4. Add `FollowIngestService`.
5. Add `find_source_ref` / canonical lookup by source ref.
6. Add refresh-state TTL/backoff helper.
7. Wire TUI confirm-follow to `FollowIngestService`.
8. Implement `RefreshService`.
9. Replace `commands/sync.rs` with source-agnostic refresh service.
10. Add Jikan/Kitsu/TVMaze adapters.
11. Register all enabled discovery sources.
12. Add tests for request counts, Type-visible candidates, and startup zero-request behavior.
