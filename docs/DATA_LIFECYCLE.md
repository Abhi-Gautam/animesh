# animesh data lifecycle, storage, and request budget

This document defines when animesh goes online, how many requests it may make,
what gets stored, and which local tables power the product. It is the contract
for source adapters, ingestion, sync, and the TUI.

animesh is local-first. The TUI renders from SQLite. Network requests exist to
update the local source lake and followed schedule; they are never required for
ordinary reads.

## 1. Core contract

```text
TUI reads local SQLite.
Network writes source evidence into SQLite.
Projection turns evidence into serving state.
```

No UI typing path should perform network requests. Network work is explicit,
budgeted, and routed through source adapters.

## 2. Source classes

Not every source has the same job. Sources are classified by when they may be
queried.

### 2.1 Primary search sources

Primary search sources are allowed to run during explicit user discovery search
when the user presses Enter in the follow/search bar.

Initial anime-first primary search sources:

```text
AniList
Jikan
Kitsu
TVMaze
```

Therefore:

```text
Enter search = max 4 requests
```

One request per enabled primary search source.

### 2.2 Secondary enrichment sources

Secondary sources do **not** participate in broad discovery search by default.
They only enrich known/followed entities during follow-time ingest, source-ref
linking, canonical resolution, or sync.

Initial secondary enrichment sources:

```text
MusicBrainz
iTunes
```

Rationale:

- They are valuable for cross-media enrichment.
- They can expand known entities once the app has intent/context.
- They should not add request cost or noisy results to anime-first search.

Later, when the app has a media mode/search scope, they can become primary for
that mode:

```text
music mode/search scope → MusicBrainz + iTunes primary
anime mode/search scope → AniList + Jikan + Kitsu + TVMaze primary
```

### 2.3 Source adapter port

Each source exposes exactly two online operations:

```rust
search(query, limit, now) -> Vec<RawSourcePayload>
ingest(source_id, now) -> Option<RawSourcePayload>
```

- `search` is for explicit discovery only.
- `ingest` is for known source IDs during follow/sync/enrichment.
- Sources do not mutate the database directly.
- Sources own HTTP details, endpoint shapes, request construction, rate limits,
  and parser selection.

## 3. Request triggers

### 3.1 Typing in follow/search bar

```text
requests = 0
```

Flow:

```text
key press
→ Library.search_source_candidates(query)
→ source_candidate_fts
→ render local candidates
```

This is always offline/local-only.

### 3.2 Pressing Enter in follow/search bar

```text
requests = number of enabled primary search sources
initial max = 4
```

Flow:

```text
Enter
→ Discovery/IngestSearchService
→ SourceRegistry.primary_search_sources()
→ source.search(query, limit, now)
→ store raw_source_payload
→ parse SourceObservation
→ store source observations
→ upsert source_candidate/source_candidate_fts
→ re-run local source_candidate_fts search
→ render updated candidates
```

Search does **not** follow anything and does **not** create canonical entities.
It only expands local candidate memory.

### 3.3 Selecting/following a candidate

Default:

```text
requests = 1
```

One detail ingest for the selected candidate's source.

Flow:

```text
selected SourceCandidateResult
→ Library.follow_source_candidate(candidate)
→ selected_source.ingest(source_id, now)
→ store raw_source_payload
→ parse SourceObservation
→ store source observations
→ project source events/facts into canonical serving tables
→ update source_ref_refresh_state
→ reload TUI from local state
```

No all-source fan-out on follow by default. Cross-source enrichment can be a
separate budgeted stage later.

### 3.4 Startup

Startup should not block on network.

```text
startup_blocking_requests = 0
```

After UI load, an optional background refresh may run:

```text
startup_background_requests = min(due_followed_source_refs, 10)
```

### 3.5 Periodic background sync

Default:

```text
background_sync_interval = 30 minutes
background_sync_max_requests_per_tick = 5
```

This means background refresh is bounded and polite:

```text
max background requests/hour = 10
```

### 3.6 Manual sync

Manual sync is still bounded.

```text
manual_sync_requests = min(due_followed_source_refs, 50)
```

If 173 followed refs are due, manual sync refreshes 50 and reports the rest as
pending/due.

## 4. Request budget formulas

Let:

```text
F = number of followed canonical entities
R = number of source_refs attached to followed entities
P = number of enabled primary search sources
D = number of due followed source_refs
```

Then:

| Action | Requests |
|---|---:|
| Typing | 0 |
| Enter discovery search | P, initial max 4 |
| Follow selected candidate | 1 |
| Startup blocking | 0 |
| Startup background refresh | min(D, 10) |
| Periodic background refresh | min(D, 5) per 30m tick |
| Manual sync | min(D, 50) |

## 5. Search query cache

Repeated Enter searches should not repeatedly hit sources for identical recent
queries.

Add a cache table:

```sql
source_search_cache (
    source TEXT NOT NULL,
    query_key TEXT NOT NULL,
    last_success_at INTEGER,
    next_due_at INTEGER,
    PRIMARY KEY (source, query_key)
)
```

Initial search cache TTL:

```text
24 hours
```

Behavior:

```text
same source + same normalized query within 24h → 0 requests for that source
```

A later force-refresh command can bypass this.

## 6. Refresh state

Each followed source ref needs refresh state separate from source identity.

Proposed table:

```sql
source_ref_refresh_state (
    source TEXT NOT NULL,
    source_id TEXT NOT NULL,
    last_attempt_at INTEGER,
    last_success_at INTEGER,
    last_error TEXT,
    next_due_at INTEGER,
    failure_count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (source, source_id)
)
```

`source_ref` remains identity/linkage. Refresh state owns scheduling and failure
backoff.

## 7. Refresh TTL rules

TTL depends on source/entity state and next event proximity.

| State | TTL |
|---|---:|
| Active/upcoming/releasing | 6 hours |
| Next event within 48 hours | 1 hour |
| Finished/stable/released | 30 days |
| Music artist | 7 days |
| Failed refresh | exponential backoff |

Failure backoff:

```text
next_due_at = now + min(24h, 15m * 2^failure_count)
```

Examples:

```text
failure 1 → 15m
failure 2 → 30m
failure 3 → 60m
failure 4 → 120m
cap → 24h
```

## 8. Storage moments

### 8.1 Search-time storage

When an Enter search returns source results, store:

```text
raw_source_payload
source_observation
source_alias_observation
external_id_observation
link_observation
image_observation
release_event_observation, if present
source_candidate
source_search_cache
```

Search-time storage does not create:

```text
canonical_release
source_ref
followed_at
```

### 8.2 Follow-time storage

When following a candidate, store/update:

```text
canonical_release
source_ref
followed_at
```

Then selected-source detail ingest stores:

```text
raw_source_payload
source_observation
child source observation tables
source_candidate update
```

Then projection stores:

```text
canonical_schedule_event
canonical serving summary / next-event projection
source_ref_refresh_state
```

### 8.3 Sync-time storage

When refreshing due followed refs, store/update:

```text
raw_source_payload
source_observation
child source observation tables
source_candidate update
canonical_schedule_event projection
source_ref_refresh_state
```

Sync must be idempotent.

## 9. Serving projection

Raw source observations are evidence, not the serving model. The TUI should not
recompute product state from raw observations on every render.

Missing base table:

```sql
canonical_schedule_event (
    id TEXT PRIMARY KEY,
    canonical_id TEXT NOT NULL,
    source TEXT NOT NULL,
    source_event_id TEXT NOT NULL,
    event_kind TEXT NOT NULL,
    title TEXT,
    season INTEGER,
    episode INTEGER,
    local_date TEXT,
    local_time TEXT,
    source_timezone TEXT,
    scheduled_at INTEGER,
    precision TEXT NOT NULL,
    confidence REAL NOT NULL,
    observed_at INTEGER NOT NULL,
    superseded_at INTEGER
)
```

This projection is populated from `release_event_observation` for followed
canonical entities. TUI panes should eventually read from this projection rather
than legacy `metadata_cache` next-episode fields.

A simpler first projection may be added if needed:

```text
canonical_next_event
```

But the full event table better supports history, graphing, and schedule diffs.

## 10. Source-specific request counts

### 10.1 AniList

Search:

```text
1 GraphQL request per Enter search
```

Follow/detail ingest:

```text
1 GraphQL request by media id
```

Refresh:

```text
1 GraphQL request per due followed AniList source_ref
```

### 10.2 Jikan

Search:

```text
1 GET /v4/anime?q=...&limit=...
```

Follow/detail ingest:

```text
1 GET /v4/anime/{id}/full
```

Refresh:

```text
1 GET /v4/anime/{id}/full per due ref
```

Budget rule:

```text
max_jikan_requests_per_second = 1
```

### 10.3 Kitsu

Search:

```text
1 GET /edge/anime?filter[text]=...
```

Follow/detail ingest:

```text
1 GET /edge/anime/{id}
```

Refresh:

```text
1 GET /edge/anime/{id} per due ref
```

### 10.4 TVMaze

Search:

```text
1 GET /search/shows?q=...
```

Follow/detail ingest:

```text
1 GET /shows/{id}?embed=episodes
```

Refresh:

```text
1 GET /shows/{id}?embed=episodes per due ref
```

TVMaze detail ingest is especially valuable because one request can return all
known episode events.

### 10.5 MusicBrainz

Default class:

```text
secondary enrichment source
```

Not included in anime-first Enter search.

When enabled for music/cross-media enrichment:

Artist search:

```text
1 GET /ws/2/artist?query=...
```

Artist detail ingest:

```text
1 GET /ws/2/artist/{mbid}?inc=aliases+tags+url-rels
```

Release-group enrichment later:

```text
+1 GET /ws/2/release-group?artist={mbid}
```

Budget rule:

```text
MusicBrainz requires a meaningful User-Agent and <= 1 request/sec.
```

### 10.6 iTunes

Default class:

```text
secondary enrichment source
```

Not included in anime-first Enter search.

When enabled for music/film/cross-media scope:

Search:

```text
1 GET /search?term=...
```

Detail ingest:

```text
1 GET /lookup?id=...
```

Source ID mapping:

```text
artist:159260351 → lookup?id=159260351
track:123 → lookup?id=123
collection:456 → lookup?id=456
```

## 11. End-to-end flows

### 11.1 Search E2E

```text
User types query
  requests: 0
  reads: source_candidate_fts

User presses Enter
  requests: up to enabled primary source count, initial max 4
  writes:
    raw_source_payload
    source_observation
    source_candidate
    source_search_cache
  reads:
    source_candidate_fts
```

### 11.2 Follow E2E

```text
User picks candidate
  writes:
    canonical_release
    source_ref
    followed_at

  requests:
    1 detail ingest from selected candidate's source

  writes:
    raw_source_payload
    source_observation
    release_event_observation
    canonical_schedule_event
    source_ref_refresh_state

  reads:
    local serving tables
```

### 11.3 Startup E2E

```text
Open app
  blocking requests: 0
  reads:
    canonical_release
    source_ref
    canonical_schedule_event / serving summary
    engagement

Background startup refresh
  requests:
    min(due_refs, 10)
```

### 11.4 Periodic sync E2E

```text
Every 30 minutes
  determine due followed source refs
  requests:
    min(due_refs, 5)
  writes:
    raw payloads
    observations
    projection
    refresh state
```

### 11.5 Manual sync E2E

```text
User runs :sync
  determine due followed source refs
  requests:
    min(due_refs, 50)
  writes:
    raw payloads
    observations
    projection
    refresh state
```

## 12. Initial defaults

```text
Typing:
  0 requests

Enter search:
  max 4 requests
  primary sources: AniList, Jikan, Kitsu, TVMaze
  secondary sources: MusicBrainz, iTunes
  search cache TTL: 24h

Follow:
  max 1 request
  source: selected candidate's source only

Startup:
  blocking requests: 0
  background refresh max: 10

Periodic:
  interval: 30 minutes
  max requests per tick: 5

Manual sync:
  max requests: 50

Active/upcoming TTL:
  6h

Near-event TTL:
  1h

Finished TTL:
  30d

Music artist TTL:
  7d

Failure backoff:
  15m * 2^failure_count, cap 24h
```

## 13. Implementation order

Do not add more source adapters until this base is implemented.

Recommended order:

1. Add migrations for:
   - `source_search_cache`
   - `source_ref_refresh_state`
   - `canonical_schedule_event`
2. Add request budget structs/constants.
3. Add follow-time detail ingest using `SourceAdapter::ingest`.
4. Add schedule projection from source observations to canonical events.
5. Replace legacy `metadata_cache` serving behavior with projection-backed reads.
6. Add refresh service for due followed source refs.
7. Then add remaining primary source adapters:
   - Jikan
   - Kitsu
   - TVMaze
8. Add secondary enrichment adapters when enrichment flows exist:
   - MusicBrainz
   - iTunes

## 14. Non-goals for this phase

```text
No availability / where-to-watch resolution.
No all-source fan-out on follow.
No unbounded startup sync.
No network while typing.
No MusicBrainz/iTunes in anime-first broad search by default.
```
