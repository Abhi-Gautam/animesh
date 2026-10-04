---
title: I just wanted to know when an episode dropped
description: A notification for new anime episodes, and why it needs one process that stays running.
published: 2026-08-10
project: Animesh
repository: https://github.com/Abhi-Gautam/animesh
sourceCommit: aaa616424d286409a9aa7035e3464ca53292efff
---

I watch a small set of anime at a time. Keeping up with them used to mean opening Crunchyroll and checking whether a new episode had appeared. If I wanted the exact time, I opened a countdown site as well.

Then I did it again later.

What I wanted was simpler: when an episode drops, tell me. I did not want a command I had to remember to run.

A command can start, fetch something, print it, and exit. A notification has to remain available, remember what I follow, survive a restart, and fire at the right time. That is why Animesh has a long-running process.

## One process owns the schedule

Animesh has a command-line client, a macOS menu bar, and notifications. If each one opened the database and called AniList independently, two refreshes could run at once, or one could read the database while another was writing to it.

The long-running process owns the database, AniList requests, refresh schedule, and notification decisions. The other parts send it requests.

![Animesh architecture showing the CLI and menu bar communicating through one daemon, which owns SQLite, AniList refreshes, scheduling, and native notification adapters.](/media/animesh/architecture.svg)

The command-line client sends each command to that process over a Unix socket and then exits. It does not open the database or call AniList itself.

```text
$ animesh --help
Personal release radar for anime

Commands:
  search   Search AniList for a title
  follow   Follow a title by its AniList id
  next     Show upcoming episodes. Local-only; never touches the network
  list     List everything you follow
  drop     Stop following a title
  refresh  Ask the app to refresh schedules now
  status   Show app health
  service  Run Animesh in the background, or stop it
```

`next` reads the stored schedule. It does not make a network request, so it still works when AniList is unavailable.

## The stored schedule has to remain usable

Animesh records the response it received, the schedule it derived from that response, and the notifications it wants to show. A failed or rate-limited response stays in the database instead of being discarded. The next refresh can use that record, including the time it should wait before trying again.

If the process stops while changing notifications, the next pass does not trust its previous record of what it registered. It reads the pending and delivered notifications from the operating system, compares them with the current schedule, and updates the difference. A rescheduled episode keeps its identifier but has a new air time, so comparing only the identifier would leave the old notification in place.

If the database cannot finish opening, the process stays running and answers a status request. The service manager does not have to restart it in a loop while the database is unavailable.

## macOS and Linux do not schedule notifications the same way

macOS can hold a notification request and deliver it later, even if Animesh is not running. The freedesktop notification interface used on Linux displays a notification when Animesh submits it. It does not keep a future notification for the application.

Animesh therefore asks the operating-system adapter whether it can hold a scheduled notification. On macOS, it registers the future request with macOS. On Linux, the process waits until the episode time and submits the notification then.

A notification change is saved only after the complete pass succeeds. Saving part of the pass would leave Animesh's record and the operating system's notifications disagreeing about what should happen next.

## What is not in the released version

Animesh 0.6.1, released on August 28, 2026, supports this AniList-backed anime workflow. It is available from GitHub and crates.io.

Television support and the desktop command center are on separate branches. They are not part of that release, so this article does not describe them as available features.

---

Checked against Animesh commit [`aaa6164`](https://github.com/Abhi-Gautam/animesh/tree/aaa616424d286409a9aa7035e3464ca53292efff), the current `master` commit. The relevant code is the [command-line client](https://github.com/Abhi-Gautam/animesh/blob/aaa616424d286409a9aa7035e3464ca53292efff/src/bin/animesh.rs), [daemon composition](https://github.com/Abhi-Gautam/animesh/blob/aaa616424d286409a9aa7035e3464ca53292efff/src/service.rs), [notification reconciler](https://github.com/Abhi-Gautam/animesh/blob/aaa616424d286409a9aa7035e3464ca53292efff/src/engine/reconciler.rs), and [library service](https://github.com/Abhi-Gautam/animesh/blob/aaa616424d286409a9aa7035e3464ca53292efff/src/library/service.rs).
