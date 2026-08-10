---
title: I just wanted to know when an episode dropped
description: How a manual anime countdown habit became a notification pipeline, a local daemon, and the beginning of a personal data system.
published: 2026-08-10
project: Animesh
repository: https://github.com/Abhi-Gautam/animesh
sourceCommit: 3daa7006b613e08e38ce7160efed5c776f738273
---

I watch a small set of anime at a time. Keeping up with them used to mean opening Crunchyroll, scrolling around, and checking whether a new episode had appeared. If I cared about the exact time, I would open one of the countdown sites and look it up there.

Then I would do it again later.

The problem was not that release information was unavailable. It was that I had to keep pulling it from several places. What I wanted was much simpler: when an episode drops, tell me.

> I did not want a CLI that I had to remember to run. I wanted a notification pipeline.

That requirement changed the shape of the project. A command can start, fetch something, print it, and disappear. A notification system has to remain alive, remember what I follow, survive restarts, refresh incomplete information, and deliver something at the right time without me asking.

That is how a small anime utility pulled me into daemon architecture, service managers, Unix sockets, native notification systems, and the same questions I found while reading how software such as Tailscale and Docker structures long-running local processes.

## One process has to own the truth

Animesh has several visible surfaces: a CLI, a macOS menu bar, and native notifications. Letting each surface open the database and fetch AniList independently would look simpler at first. It would also create several owners of the same state.

Two commands could refresh at once. The menu could read halfway through a write. Notification policy would slowly split across every surface. None of those failures needs scale; they can happen on one laptop.

So one long-lived process owns the database, the AniList client, refresh scheduling, and notification policy. Everything else is a client or an adapter.

![Animesh architecture showing the CLI and menu bar communicating through one daemon, which owns SQLite, AniList refreshes, scheduling, and native notification adapters.](/media/animesh/architecture.svg "One owner, several surfaces. D2 and ELK compute the geometry; narrow screens scroll instead of shrinking labels into unreadable text.")

The CLI is intentionally thin. It parses one command, sends one versioned request over a user-private Unix socket, prints one response, and exits. It never opens SQLite and it never constructs an AniList client.

```text
$ animesh --help
Personal release radar for anime

Commands:
  search   Search AniList for a title
  follow   Follow a title by its AniList id
  next     Show upcoming episodes. Local-only; never touches the network
  list     List everything you follow
  refresh  Ask the app to refresh schedules now
  status   Show app health
```

The important promise is attached to `next`: it is local-only.

## “Local-first” has to survive failure

Putting SQLite on a laptop is not enough to make a system local-first. The useful test is what still works when everything around the database becomes unreliable.

- **AniList is unavailable:** `animesh next` still reads the last known schedule locally.
- **The source rate-limits a refresh:** the throttle is persisted, so restarting the daemon does not conveniently forget it.
- **The database cannot finish booting:** the daemon stays reachable and serves health instead of disappearing into a service-manager crash loop.
- **The process dies during notification work:** the next pass reads what the operating system actually holds and converges again.

The local database is therefore not a cache in front of AniList. It is the record Animesh serves. AniList is one source that supplies new observations.

## The notification needed a data pipeline

A remote API response is not yet something a person—or an AI agent—can safely reason from. It can be late, malformed, contradictory, incomplete, or valid but older than a concurrent response.

Animesh keeps three concerns separate:

1. **What arrived:** the fetch occurrence, body, outcome, timing, and rate-limit evidence.
2. **What it means:** normalized media, observations, release events, and follow state.
3. **What the product uses:** upcoming releases, health, refresh decisions, and notification intent.

This is a small local version of the bronze, silver, and gold separation used in larger data platforms. The names matter less than the boundary: failed responses remain evidence instead of disappearing because they produced no projection.

That separation also matters when an episode is rescheduled. An identifier can remain the same while its airing time changes. Animesh compares the canonical desired request with the request held by the operating system; matching only the identifier would quietly accept a stale trigger.

## Notifications are reconciliation, not delivery

macOS and Linux expose different semantics. macOS can hold a future notification request and fire it even if Animesh is not running. The freedesktop interface used on Linux displays a notification when it is submitted; it does not hold a future schedule for the application.

A useful abstraction cannot pretend those systems behave the same. The notification surface tells the reconciler whether it can hold a schedule. On macOS, Animesh can register future work with the OS. On Linux, the daemon waits until airtime and submits then.

The reconciler follows an observe–converge–record loop:

1. **Observe:** read authorization, pending requests, delivered requests, and the desired local plan.
2. **Converge:** add missing requests, replace stale revisions, and remove requests Animesh no longer wants.
3. **Record:** commit the complete pass as one transaction. A partial pass is worse than no pass.

The OS is authoritative about what it currently holds. SQLite is authoritative about what Animesh wants. Reconciliation is the function that makes those two facts agree again after a crash, a reschedule, a permission change, or an upgrade.

## The schedule is the wedge

The notification solved the immediate annoyance. The more important thing accumulating underneath is a structured, local history of what I follow, watch, miss, drop, and return to.

That record has two readers. The first is me asking a direct question such as “what drops tonight?” The second is an AI system that should understand my taste before recommending anything.

Most personalized AI experiences start by asking the same questions again or inferring taste from a short conversation. A user-owned data system can provide durable context instead: what I actually followed, where I stopped, which genres persist, how my taste changes, and eventually the same kind of history for television, films, and music.

Anime is the first source because it created the real need. It does not have to be the boundary of the system.

## Where this goes next

The current system establishes the difficult base: one owner process, durable local evidence, normalized observations, read models, source-aware refresh policy, and notification reconciliation across two operating systems.

The next layers are not “add AI” as a feature. They are better personal data: backlog and history, richer taste signals, more media sources, and interfaces that can answer useful questions without exporting the underlying record to somebody else’s account.

The first version started because I was tired of refreshing a webpage. The reason to keep building it is that a notification pipeline can become something more useful: a private, structured memory of what I like.

---

This article was checked against Animesh commit [`3daa700`](https://github.com/Abhi-Gautam/animesh/tree/3daa7006b613e08e38ce7160efed5c776f738273). The relevant implementation trails are the [daemon composition root](https://github.com/Abhi-Gautam/animesh/blob/3daa7006b613e08e38ce7160efed5c776f738273/src/bin/app.rs), [library boundaries](https://github.com/Abhi-Gautam/animesh/blob/3daa7006b613e08e38ce7160efed5c776f738273/src/library/service.rs), [notification reconciler](https://github.com/Abhi-Gautam/animesh/blob/3daa7006b613e08e38ce7160efed5c776f738273/src/engine/reconciler.rs), and [SQLite connection model](https://github.com/Abhi-Gautam/animesh/blob/3daa7006b613e08e38ce7160efed5c776f738273/src/store/connection.rs).
