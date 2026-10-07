---
name: animesh
description: Help with the user's anime and TV library through the local Animesh CLI. Use for upcoming or just-released episodes, viewing and managing follows, planning what to watch, recommendations informed by their library, and Animesh setup or troubleshooting.
license: MIT
compatibility: Requires the animesh CLI on PATH on macOS or Linux. Library commands need its local background service; search and follow contact AniList or TVmaze. Skill installation and service management work without the daemon.
---

# Animesh

Animesh keeps the user's followed anime and TV shows and their release schedules
on this device. Use the CLI to access this library. The desktop app and CLI share
the same data; no account or cloud library is involved.

## Help the user

- **What's next or what dropped:** answer for the user's followed shows, with
  episode numbers and release times in their timezone.
- **Tonight or this week:** group relevant releases by local date and help the
  user choose what to make time for. Animesh reports release times, not whether
  the user has watched an episode or where it is available to stream.
- **Manage their library:** find the right anime or TV title, follow it, or stop
  following it. Resolve ambiguous titles or seasons before making a change.
- **Suggest something:** read their library first, connect suggestions to shows
  they follow and preferences they have expressed, and explain the fit. This is
  assistant reasoning informed by Animesh data. The CLI has no recommendation
  engine, ratings, watch progress, or genre/synopsis fields. Do not invent scores
  or treat a follow as proof they liked or finished a show. An empty library is
  a reason to ask what they enjoy. Verify unfamiliar title details using available
  sources; check candidates with `search` before offering to follow them.
- **Explain and troubleshoot:** help with missing releases, stale schedules,
  notification problems, the background service, and skill installation.
- **Recurring help when requested:** if the host agent supports scheduled tasks,
  use that facility for an explicitly requested release briefing or recommendation
  check. The skill itself does not run in the background or create an automation.
  Read the current library on each run. For a release monitor, notify on relevant
  new releases or changes and stay quiet when nothing actionable has changed.

Suggesting a show does not authorize following it. A clear request to follow or
drop a resolved title is sufficient authorization; do not ask for the same
approval again. Ask when the target or intended action remains ambiguous.

## Read the command results

Use `animesh --json ...` for data and actions. Successful output is one JSON
line, for example `{"ok":true,"kind":"upcoming","data":[...]}`. Failures have
`{"ok":false,"error":{"code":"unavailable","message":"...","retry_after_secs":null}}`.
Read fields by name and branch on `ok` and the stable error `code`, not message
wording. Service and skill commands return `data.message` inside the same envelope.

Exit codes: `0` success; `1` invalid input (fix the arguments); `2` needs
intervention (explain the required action); `3` temporary failure (retry at most
once after `retry_after_secs` when supplied, then report it).

## Choose the command

| Need | Command | Meaning |
| --- | --- | --- |
| Next release | `animesh --json next -n 1` | Future only, soonest first. |
| Upcoming releases | `animesh --json next -n 500` | Up to 500 next known releases, one per title; filter to the requested local date range. |
| Just released | `animesh --json next --dropped -n 500` | Releases within the last 24 hours, newest first. |
| Followed library and taste context | `animesh --json list` | Followed titles, including those with no upcoming episode. |
| Find an anime | `animesh --json search "TITLE"` | AniList candidates, without changing the library. |
| Find a TV show | `animesh --json search --tv "TITLE"` | TVmaze candidates, without changing the library. |
| Follow a resolved title | `animesh --json follow SOURCE:ID` | Use `anilist:N` or `tvmaze:N` from search. |
| Stop following | `animesh --json drop media:N` | Use the local ID from list; source tokens are also accepted. |
| Engine and notification health | `animesh --json status` | Version, follow count, refresh times, notification counts, degraded reasons. |
| Refresh stale schedules | `animesh --json refresh` | Requests a refresh; does not prove it has completed. |
| Background service registration | `animesh --json service status` | Whether the system manages the background process. |
| Skill installation | `animesh --json skill status` | Where the skill is installed and whether it matches this build. |

### Schedule and library interpretation

`next` and `list` read local data without contacting a source. `next` defaults to
50 rows and has a maximum of 500; it is the next known episode for each title,
not an exhaustive calendar of every future episode. Say when the limit or that
coverage affects an answer. For a specific followed show, find it in `list` and
use its `upcoming` value instead of assuming it appears in a short `next` result.

`scheduled_at` is a Unix timestamp in UTC. Convert it to the user's timezone
before filtering into today, tonight, or this week. Use `season` for TV episode
labels such as S2E4; when season is null, say Episode 4. Episode numbers may be
null: say next release rather than inventing one.

`freshness` is `fresh`, `stale`, or `backing_off`. Qualify stale times and explain
that schedules can change. An empty upcoming list means no upcoming release is
known, not that the library is empty. `list` rows carry `media_id`, `source`,
`source_id`, `display_title`, `state`, and nullable `upcoming`.

A missing title in the 24-hour dropped window does not establish that an older
episode never aired. Release timestamps do not guarantee availability in the
user's region or streaming subscription.

### Search and follow identities

Source IDs and local media IDs are separate. AniList 21 and TVmaze 21 are
different titles. Never pass a bare number or invent an ID.

Search before following. Inspect the candidate's title, alternate `titles`,
year, format, and source to match what the user means. Use TVmaze for live-action
TV; a unique AniList result does not establish that it is the intended TV show.
Route the follow using the result's `source` and `source_id`.

For dropping, read `list` and match the user's title to `media_id`, then pass
`media:N` (or its verified source token). Report the actual returned `outcome`:
`newly_followed`, `reactivated`, or `already_active`. Do not claim a new follow
when the title was already active.

## Setup and recovery

- **CLI missing:** check whether it is installed but absent from PATH. When the
  user requests installation, use the package matching their machine from
  https://github.com/Abhi-Gautam/animesh/releases/latest and the instructions at
  https://github.com/Abhi-Gautam/animesh#install. Then run
  `animesh --json skill install` and verify with `skill status`. The desktop app
  offers the same installation in onboarding and Health → Your AI assistant.
  Reload skills or open a new agent session if it is not discovered. A schedule
  question alone is not a request to install software.
- **`unavailable`:** read `service status`. If the user requested setup or
  recovery, run `animesh --json service start`, then check `status`; otherwise
  explain that starting the background service is the next step.
- **Stale data:** when the user needs a current airtime, request `refresh` once
  and re-read the local result. Report `started` or `already_running` accurately;
  if freshness remains stale, say so. Do not poll or repeatedly refresh.
- **`source_rate_limited`:** respect `retry_after_secs`, retry once, then stop.
  Use local `list` and `next` where helpful.
- **`source_unavailable`:** use the local schedule and explain its freshness.
- **Notification problems:** inspect `status` before suggesting a fix. Explain
  the reported permission or capacity problem; use the desktop Health page for
  diagnostics. Do not promise a banner merely because a release is scheduled.
- **Version or database errors:** explain the reported failure. Do not delete
  the library, edit SQLite directly, or repeatedly restart the service.
- **Edited skill:** normal installation keeps it intact. Do not use
  `skill install --force` unless the user authorizes replacing those edits.

Keep answers tied to the user's actual question. Use the smallest useful set of
commands, distinguish observed library data from suggestions, and report what
changed only after the command succeeds.
