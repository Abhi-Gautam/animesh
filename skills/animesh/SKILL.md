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

Three different IDs, and mixing them is the most common failure:

- **`source` + `source_id`** — the public identity of a title. `source` is
  `anilist` (anime) or `tvmaze` (TV). `source_id` is that source's integer.
  AniList 21 and TVmaze 21 are different shows. Follow with the token search
  prints (`anilist:21` or `tvmaze:82`), or `follow --tv SOURCE_ID` when
  `source` is `tvmaze`.
- **`media_id`** — a small local row ID, unique to this user's library.
  `drop` takes this one.

Never guess an ID. Get `source` and `source_id` from `search`, and `media_id`
from `list` or `next`. A bare integer is AniList. Never pass a TVmaze id as
a bare number.

## Commands

### `animesh --json next [-n LIMIT]`

Upcoming episodes, soonest first. Local only — never touches the network, so it
is cheap and safe to call often. This is the right answer to almost every
"what's airing" question.

```
$ animesh --json next -n 1
{"data":[{"aired":false,"display_title":"Re:ZERO -Starting Life in Another World- Season 4",
  "episode":14,"event_uuid":"8eb1bce4-...","freshness":"fresh",
  "last_success_at":1787572804,"media_id":4,"release_event_id":11,
  "schedule_revision":1,"scheduled_at":1787749200,"source":"anilist",
  "source_id":189046}],
 "kind":"upcoming","ok":true}
```

(Wrapped here for reading; the real output is one line.)

`scheduled_at` is a Unix timestamp in UTC. Convert it to the user's local
timezone before saying a time out loud. `freshness` is `fresh`, `stale`, or
`unknown` — anything other than `fresh` means the schedule has not been
confirmed recently, so hedge. `aired: true` means the airtime has passed.

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

### `animesh --json search --tv ["QUERY"]`

Searches TVmaze for TV. With a query, title search. With no query, currently
airing English-language US broadcasts and streams — this is the answer to
"what's on TV", not a lookup you have to invent a query for. Candidates have
`source: "tvmaze"`. Follow those with `follow --tv SOURCE_ID`. Never pass a
TVmaze `source_id` to `follow` without `--tv`.

### `animesh --json follow ID`

Starts following. `ID` is `anilist:N`, `tvmaze:N`, or a bare AniList number.
`follow --tv N` is the same as `follow tvmaze:N`. Returns `outcome`:
`newly_followed`, `reactivated`, or `already_active`, plus `source` and
`source_id`. If `source` is not the catalog you meant, you followed the wrong
show — `drop` it.

Always `search` first. Route by the candidate's `source` field, not by whether
the user said "TV": if `source` is `tvmaze`, follow `tvmaze:SOURCE_ID` (or
`--tv SOURCE_ID`). If AniList returns nothing or a bad fit for a live-action
title, search `--tv` before following. Confirm the match every time — a unique
AniList hit is not proof it is the right show. AniList and TVmaze ids overlap.

### `animesh --json drop MEDIA_ID`

Stops following, by **local media** ID from `list`. This is destructive from the
user's point of view — confirm before calling it, and never infer the ID from a
title without checking `list` first.

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
   `search --tv` with no query.
2. Convert every `scheduled_at` to the user's local timezone. A UTC timestamp
   read aloud is a wrong answer.
3. Never invent an ID. Every ID comes from a command you just ran.
4. Confirm before `follow` and before `drop`. Both change a durable record the
   user cares about.
5. Report `outcome` and `disposition` honestly instead of assuming the happy
   path.
6. When recommending anything, read `list` first. It is the record of what this
   person actually chose to keep up with.
