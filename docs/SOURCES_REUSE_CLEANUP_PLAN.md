# Sources/ingest reuse cleanup plan

This is the **cleanup-only** plan for the `sources/` + `ingest/` layers: turn the
current copy-paste consistency into compiler-enforced consistency. It is a
companion to `docs/SOURCES_E2E_IMPLEMENTATION_PLAN.md`, which delivered the
feature. That work landed and is green (201 tests); the architecture is correct
and uniformly applied. What remains is the extraction pass.

**No architecture change. No logical/async change** (those are tracked
separately). Every phase below is independently shippable and behavior-preserving
*except* the two flagged convergences in Phase 3.

## Why this is needed

The adapter pattern is applied identically across all six sources — but the
consistency was achieved by copy-paste, not extraction. Concrete duplication
(verified line anchors):

- `get_json` ×4 — `jikan.rs:222`, `kitsu.rs:192`, `tvmaze.rs:209`, `musicbrainz.rs:200`
- `url_with_path` ×4, `url_with_params` ×4 — same four files
- `raw_payload` ×5 — those four + `anilist.rs:219`
- `push_image` ×2, `image_obs` ×2, `push_alias` ×3, `push_title_alias` ×1, `date_event` ×2
- `follow.rs` ≈ `refresh.rs` — same `ingest → match → parse_fetch → match → next_event_at → record`
  ladder; the next-event computation is byte-identical (`follow.rs:165` ≡ `refresh.rs:114`)
- `failure_backoff` divergence — `follow.rs` hardcodes `failure_backoff(1)` at 5 sites;
  `refresh.rs` uses the real count

This violates the project's own rule (`CLAUDE.md`: *"factor one out when the second
adapter needs it"*). The trait was factored out; the plumbing was not. Copy-paste
consistency is the fragile kind: `musicbrainz`'s `get_json` already diverges (it is
the only one with a `User-Agent` header), so the four "identical" copies are no
longer identical. Adding source N+1 today costs ~150 boilerplate lines; the
source-specific parser is only ~40% of that.

## Gate

`cargo test` (201 green) after **every** phase. The assertion-heavy parser/service
tests are the safety net, so each change below is pinned to keep the exact strings
and hashes those tests check.

---

## Phase 1 — `src/sources/http.rs` (shared transport)

**Change.** New module; delete the per-adapter copies.

```rust
pub async fn get_json(source: &str, client: &Client, url: &str, headers: &[(&str, &str)]) -> Result<String>;
pub fn url_with_path(source: &str, base_url: &str, path: &str) -> Result<Url>;
pub fn url_with_params(source: &str, base_url: &str, path: &str, params: &[(&str, String)]) -> Result<Url>;
pub fn raw_payload(
    source: &str, endpoint: &str, request_key: &str, method: HttpMethod,
    request_json: Option<String>, response_json: String, now: i64,
) -> RawSourcePayload;
```

Deletes `get_json` ×4, `url_with_path`/`url_with_params` ×4 each, `raw_payload` ×5
(incl. `anilist.rs:219`). **~-140 lines.**

**Why free functions, not a base struct or a blanket trait default.**

- A **base struct** (adapters hold an `HttpClient` helper) forces every call through
  `self.http.get(...)` plus an extra field, and still doesn't remove `raw_payload`
  (a pure function of its args, not of client state). More ceremony than the
  problem warrants.
- A **blanket `trait` default method** couples the HTTP shape to `SourceAdapter`,
  but AniList's transport is GraphQL-POST (`AniListClient::raw_query`,
  `anilist.rs:42`) — it doesn't fit the GET shape, so a default would be wrong for
  AniList or need overriding, defeating the point.
- **Free functions** are the minimum that kills the duplication: pure, take the
  source name for error context, no new adapter state. What varies (URL shape,
  GraphQL vs GET) stays in the adapter; what is identical (issue request, check
  status, build payload) moves out. That is the correct seam.

**Why `headers: &[(&str, &str)]` and not a builder.** Only `musicbrainz` needs a
header today (`User-Agent`, `musicbrainz.rs:204`). A builder/options-struct is
speculative generality for one caller; a slice is `&[]` for five adapters and
`&[("User-Agent", UA)]` for one. Promote to a struct only when a third axis
(timeout, auth) actually appears.

**Why one `raw_payload` taking `method` + `request_json`, not GET/POST variants.**
The only differences across the 5 copies are method, whether `request_json` is
`Some`, and the hash input. The rule
`stable_hash(request_json.as_deref().unwrap_or(request_key))` already unifies both
cases (GET passes `None` → hashes the key, identical to today). One function with
the union of params is strictly less code, with no branch a caller can get wrong.

**Pinned invariants:** id format `raw:{source}:{endpoint}:{hash}:{hash}`; the hash
input rule above; error strings `"{source} HTTP {status}: {body}"` and
`"read {source} response body"`. Keeps `anilist.rs:652` (asserts `"429"` + body) and
each adapter's `raw(body)` test green.

---

## Phase 2 — `src/ingest/obs.rs` (observation builders)

**Change.** Move the builders into one module:

```rust
pub fn push_alias(out: &mut Vec<AliasObservation>, text: &str, locale: Option<&str>, kind: &str, conf: f64);
pub fn push_image(out: &mut Vec<ImageObservation>, kind: &str, url: Option<&str>);
pub fn date_event(id: &str, kind: &str, raw_date: &str, observed_at: i64) -> Result<ReleaseEventObservation>;
```

Deletes `push_alias` ×3 + `push_title_alias` ×1, `push_image` ×2 + `image_obs` ×2,
`date_event` ×2. **~-120 lines.**

**Why keep the `&mut Vec` push-style, not return `Option<T>` + `.extend`.** Returning
`Option` is arguably cleaner, but every call site is `push_x(&mut vec, ...)` in
imperative builder blocks (e.g. `anilist.rs:370-416`). Keeping the signature makes
the move a pure relocation — zero call-site logic change, lowest risk against the
parser tests. The functional refactor is a separate, optional taste change; do not
bundle it into a dedup.

**Why functions, not methods/`From` impls on the observation structs.** `push_alias`
skips blanks and assigns confidence — that is *adapter-mapping policy*, not
intrinsic to `AliasObservation`. A constructor on the type would bake one adapter's
confidence conventions into the shared struct. Free helpers keep the struct a dumb
data carrier (which is what `observation.rs` is) and the mapping decisions in one
ingest-side place.

**Why jikan's `date_event` survives and tvmaze's `episode_to_event` is NOT folded
in.** jikan's parses RFC3339 *then* falls back to `%Y-%m-%d` (`jikan.rs:421`);
kitsu's only does the date form (`kitsu.rs:355`) — jikan's is a strict superset and
absorbs kitsu with no loss. tvmaze's `episode_to_event` (`tvmaze.rs:354`) carries
precision + timezone-offset logic that only it uses; folding it in would add
single-caller branches — the over-abstraction failure mode. It stays put.

---

## Phase 3 — shared detail-ingest core (the biggest dedup)

**Change.** New `src/ingest/detail.rs`:

```rust
pub struct DetailOutcome {
    pub detail_ingested: bool,
    pub projected_events: usize,
    pub next_due_at: Option<i64>,
    pub warning: Option<String>,
}

pub enum CanonicalSource<'a> {
    Known(&'a CanonicalId), // follow: from the candidate (follow.rs:39)
    LookupByRef,            // refresh: resolve after parse (refresh.rs:103); None => recorded failure
}

pub async fn ingest_detail(
    library: &Library, adapter: &dyn SourceAdapter,
    source: &str, source_id: &str, canonical: CanonicalSource<'_>, now: i64,
) -> Result<DetailOutcome>;
```

`ingest_detail` owns the whole ladder: ingest → 3-way match → store-raw-on-parse-fail
→ `record_parse_error` → resolve canonical → `next_event_at` min →
success/failure record.

- `follow.rs` shrinks to: persist follow intent → `ingest_detail(Known(&id))` → map
  `DetailOutcome` → `FollowIngestReport`.
- `refresh.rs` shrinks to: loop due states → `ingest_detail(LookupByRef)` → fold
  into `RefreshReport` counters.

`follow.rs` ~305→~120, `refresh.rs` ~153→~90, `+detail.rs` ~120. **~-130 net.**

**Why a free function returning an outcome, not a method on `Library`.**
`ingest_detail` orchestrates `adapter` (network) + `Library` (persistence).
`Library` is the mutation chokepoint but must not know about
`SourceAdapter`/network — putting orchestration inside it would drag the source
layer into the store-facing facade and break the layering rule. Orchestration
belongs in `ingest/`, calling *into* `Library` — exactly where the two services
already live.

**Why the `CanonicalSource` enum, not two separate functions.** The only real
divergence between follow and refresh is canonical-id provenance: follow knows it
up front (`follow.rs:39`); refresh looks it up after a successful parse and treats
"no source_ref" as a failure (`refresh.rs:103`). Everything else — the entire match
ladder — is identical. Two functions would re-duplicate ~80% to vary 20%. The enum
isolates the 20% to one `match` inside the core; the shared 80% is written once.

**Why report shapes stay separate (no unified report).** `FollowIngestReport`
carries `outcome`/`candidate`; `RefreshReport` carries batch counters. They serve
different callers (one follow vs N refreshes). One struct would give each caller
fields it doesn't use. The core returns the neutral `DetailOutcome`; each service
maps it to its own report. Shared mechanism, caller-specific presentation.

**Pinned invariants:** exact warning strings (`"{source} detail ingest: ..."`,
`"{source} parser produced no observation for ..."`) and `failure_count` semantics
— `follow.rs` tests assert these substrings (`follow.rs:292`/`303`).

### Phase 3a (convergence, needs sign-off) — move `failure_backoff` into `Library`

Move backoff computation into `Library::record_source_ingest_failure`.

**Why it's right.** That method already derives the true `failure_count` internally
(`library/mod.rs:303-307`), but the *caller* passes a separately-computed backoff —
which is exactly why `follow` (always `failure_backoff(1)`) and `refresh` (real
count) diverge for the same event. Moving backoff to where the count lives makes
divergence impossible and deletes all 7 `failure_backoff` call sites in the
services. Layering holds: `budget::failure_backoff` is pure policy (no rusqlite),
and `Library` already owns scheduling for the success path.

**Cost.** `record_source_ingest_failure` loses its `next_due_at` param (one-time
signature churn).

### Phase 3b (convergence, needs sign-off) — uniform id-mismatch check

Apply the candidate/observation id-mismatch check in both paths, inside the core.

**Why it's right.** Only `follow` validates that the returned observation matches
what was requested (`follow.rs:155`, a hard `Err`). A source returning the wrong id
is an anomaly `refresh` should catch too — and in a *batch* refresh it should be a
recorded failure, not a hard error that aborts the run. Folding it into the core
(recorded failure in both) makes the check uniform and batch-safe.

**Cost.** Follow's mismatch changes from hard-error → recorded-failure (behavior
change — hence sign-off).

---

## Phase 4 — kill the dead/asymmetric surface

- **`enrichment_scopes()`** — removed from the `SourceAdapter` trait during unified discovery cleanup. Re-add only with a concrete caller when enrichment routing actually exists.
- **`SourceRequest`** (`request.rs:17`) — delete. `#[allow(dead_code)]`, unused; the
  flow uses `RawSourcePayload` directly. (grep-confirm first.)
- **`itunes` / `musicbrainz` asymmetry** — a roadmap decision, not a code-quality
  one. `itunes` is parser-only (no adapter); `musicbrainz` has a full adapter but is
  unregistered. Either finish/register them now, or leave parked with a one-line doc
  comment on `production()` noting why they're excluded. The plan does not pick.

---

## Order and rationale

1 → 2 → 3 → 4. **Leaf-first:** Phases 1/2 shrink the adapters with zero behavior
change and de-risk Phase 3 (the services get simpler once helpers are shared).
Phase 3 is last among the mechanical work because it carries the two convergences.
Phase 4 is independent — do it whenever.

## Net effect

| Phase | Risk | Lines | Behavior change |
|-------|------|-------|-----------------|
| 1 `http.rs` | low | ~-140 | none |
| 2 `obs.rs` | low | ~-120 | none |
| 3 `detail.rs` | medium | ~-130 | 3a + 3b (sign-off) |
| 4 dead surface | low | ~-40 | none (remove unused) |

**~-430 lines**, and source N+1 drops from ~150 boilerplate lines to ~40. The win
that matters: consistency stops being maintained by hand (which `musicbrainz`'s lone
`User-Agent` already broke) and becomes compiler-enforced — change `get_json` once,
every adapter gets it.

## Open decisions before starting

1. **Phase 3a + 3b** — approve both convergences? 3a fixes the backoff divergence;
   3b makes the id-check uniform but changes follow's mismatch to a recorded failure.
2. **itunes/musicbrainz** — register/finish now, or leave parked with an explicit
   marker?
