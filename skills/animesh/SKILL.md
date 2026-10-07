---
name: animesh
description: Read and edit the user's personal anime and TV release radar through the local `animesh` CLI — what they follow, what episode airs next and when, and adding or removing follows. Use whenever the user asks what is airing today, tonight, this week, or next; what they are watching or following; when a specific show's next episode drops; or asks to start or stop following a series. Also use to answer "what is this person into" before recommending anything, since the follow list is a durable record of their taste. Requires the animesh CLI on PATH.
license: MIT
compatibility: Requires the `animesh` CLI on PATH (macOS or Linux) with its background daemon running. Reads and writes a local SQLite library; `search` and `follow` reach AniList (anime) or TVmaze (TV) over the network.
---

# animesh

`animesh` is a local-first release radar for anime and TV. It holds one person's
library on their machine: the shows they follow, the schedule for each, and what
airs next.
Nothing is in the cloud and there is no account, so this CLI is the only way to
read it — and it is a far better source of what this person actually watches
than anything you can infer from conversation.

## Always pass `--json`

Every command accepts a global `--json` flag. Use it every time. Without it you
get output formatted for a human terminal that you will parse wrong.

Success is one line:

```json
{"data":[ ... ],"kind":"upcoming","ok":true}
```

Failure is one line, on stdout, with a stable `code`:

```json
{"error":{"code":"unavailable","message":"Animesh is not running.","retry_after_secs":null},"ok":false}
```

Keys are emitted in sorted order, so read fields by name — never by position.

`kind` names the reply. `code` is stable and safe to branch on; `message` is
prose for a person and may change between releases. Never branch on `message`.

## Exit codes

| Code | Meaning | What to do |
| --- | --- | --- |
| 0 | Success | Continue. |
| 1 | Bad input | Fix the arguments. Do not retry unchanged. |
| 2 | Needs intervention | Tell the user; they have to act. Do not retry. |
| 3 | Temporary | Retry once after `retry_after_secs`, then stop. |

## The ID types — read this before calling anything

Two tokens, printed on every row. A bare integer is not an identity.

- **`anilist:N` / `tvmaze:N`** — the public identity of a title. AniList 21
  and TVmaze 21 are different shows. `follow` takes this token (or
  `follow --tv N` as the same as `tvmaze:N`). `drop` accepts it too.
- **`media:N`** — a local row ID, unique to this user's library. `drop`
  also accepts this. JSON still carries numeric `media_id`, `source`, and
  `source_id`; the tokens are those fields joined, not a third number space.

Never guess an ID. Get the source token from `search`. Get `media:N` from
`list` or `next`. Never pass a bare number to `follow` or `drop`.

## Commands

### `animesh --json next [-n LIMIT]`

**Future** episodes, soonest first. `next -n 1` is the actual next airing,
never something that already dropped. Local only — never touches the network.

```
$ animesh --json next -n 1
{"data":[{"aired":false,"display_title":"Re:ZERO -Starting Life in Another World- Season 4",
  "episode":14,"event_uuid":"8eb1bce4-...","freshness":"fresh",
  "last_success_at":1787572804,"media_id":4,"release_event_id":11,
  "schedule_revision":1,"scheduled_at":1787749200,"season":null,"source":"anilist",
  "source_id":189046}],
 "kind":"upcoming","ok":true}
```

(Wrapped here for reading; the real output is one line.)

`scheduled_at` is a Unix timestamp in UTC. Convert it to the user's local
timezone before saying a time out loud. `freshness` is `fresh`, `stale`, or
`backing_off` — anything other than `fresh` means hedge. `season` is set for
TVmaze (say "S18E4"); AniList is `null` (say "Episode 1176").

### `animesh --json next --dropped`

Just-aired episodes from the last 24 hours, newest first. Use this for "did
it drop?", not `next -n 1`.

### `animesh --json list`

Everything the user follows, whether or not an episode is scheduled. Use this,
not `next`, when asked what they watch or what they are into — a finished or
between-seasons show has no upcoming episode but is still part of their taste.

Each row carries `media_id`, `source`, `source_id`, `display_title`, `state`,
and an `upcoming` object that is `null` when nothing is scheduled.

### `animesh --json search "QUERY"`

Searches AniList for anime. Returns candidates with `source` (`anilist`),
`source_id`, `display_title`, `titles`, `status`, `format`, `episode_count`,
`season_year`. Reaches the network.

This is a lookup step, not an answer. Run it to turn a title into a
`source_id` before following.

### `animesh --json search --tv "QUERY"`

Searches TVmaze by title. A query is required. Candidates have
`source: "tvmaze"`. Follow with `follow tvmaze:SOURCE_ID`.
Search results are transient; the library changes only when the user follows a title.

### `animesh --json follow ID`

Starts following. `ID` is `anilist:N` or `tvmaze:N`. A bare number is rejected.
`follow --tv N` is the same as `follow tvmaze:N`. Returns `outcome`:
`newly_followed`, `reactivated`, or `already_active`, plus `source` and
`source_id`. If `source` is not the catalog you meant, you followed the wrong
show — `drop` it.

Always `search` first. Route by the candidate's `source` field: if `source`
is `tvmaze`, follow `tvmaze:SOURCE_ID`. If AniList returns nothing or a bad
fit for a live-action title, search `--tv` before following. Confirm the
match every time — a unique AniList hit is not proof it is the right show.

### `animesh --json drop ID`

Stops following. `ID` is `media:N` from `list`/`next`, or `anilist:N` /
`tvmaze:N`. Confirm before calling it. A bare number is rejected.

### `animesh --json status`

Daemon health: `process_version`, `active_follows`, `earliest_upcoming`,
`last_success_at`, notification counts, and a `degraded` array. Run this when a
command failed and you need to tell the user why.

### `animesh --json refresh`

Asks the daemon to pull schedules now. Rarely needed — it refreshes on its own.
Call it at most once per conversation, only if `freshness` is not `fresh` and
the user is asking about a specific airtime. Returns `disposition`:
`started` or `already_running`. Never poll it in a loop.

### `animesh --json service status`

Whether the background daemon is registered with the system.

## When something fails

- **`unavailable`** — the daemon is not running. Tell the user to run
  `animesh service start`. Do not run it for them without asking.
- **`source_rate_limited`** — AniList or TVmaze is throttling. Wait
  `retry_after_secs`, retry once, then stop and say so.
- **`source_unavailable`** — the network or a source is down. `next` and `list`
  still work; use them and say the data may be stale.
- **command not found** — Animesh is not installed. It is at
  https://github.com/Abhi-Gautam/animesh. Do not attempt to install it yourself.

## Working rules

1. Prefer `next` and `list`. They are local, instant, and never fail from the
   network. For "what's on TV right now" that is not already in the library,
   `search --tv "TITLE"`. Ask for a title if none was given.
2. Convert every `scheduled_at` to the user's local timezone. A UTC timestamp
   read aloud is a wrong answer.
3. Never invent an ID. Every ID comes from a command you just ran.
4. Confirm before `follow` and before `drop`. Both change a durable record the
   user cares about.
5. Report `outcome` and `disposition` honestly instead of assuming the happy
   path.
6. When recommending anything, read `list` first. It is the record of what this
   person actually chose to keep up with.
