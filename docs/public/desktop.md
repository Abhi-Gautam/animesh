---
title: Animesh now has a desktop app
description: Anime and TV release reminders on macOS and Linux, with a desktop window, a Mac menu bar, and a CLI sharing one library.
published: 2026-10-05
project: Animesh
repository: https://github.com/Abhi-Gautam/animesh
sourceCommit: 16bbb7e4c1a61587fa117f504902bc74d791bc76
---

Animesh 0.7.0 adds a desktop app for macOS and Linux, and TV shows alongside anime. Search for a show, follow it, and get a reminder when its next known episode releases.

[Screenshots and downloads are on the Animesh site](https://animesh.syntropicsystems.dev/).

The [first article](/writing/animesh/) explained why a notification app needs a process that stays running. That part has not changed. The desktop window gives the same library a more useful way to browse.

## One library, several ways to use it

The window has Home, Discover, Search, Schedule, Library, and Health. Home shows what recently dropped and what is next. Schedule groups episodes by date. Health shows whether the background service, sources, and notifications need attention.

![Animesh Home on macOS, showing recently released episodes, upcoming shows, and a summary of 16 followed titles.](https://animesh.syntropicsystems.dev/assets/screenshots/mac-home.png)

The window does not open SQLite or fetch schedules itself. It sends requests to the existing engine over the local socket. The CLI uses that same engine, and the Mac menu bar reads the same followed titles. Following a show in one place makes it available in the others.

Anime schedules come from AniList; TV schedules come from TVmaze. Times appear in the local timezone. A source release time does not guarantee that an episode is available on a particular streaming service or in a particular region.

## Scroll the list, keep the controls

An early desktop layout scrolled the whole page. The filters disappeared with the titles, and the footer could disappear too. Nested scrolling made it harder to know which part of the window was moving.

The pages now reuse a layout with controls at the top, the list in the middle, and a summary at the bottom. Only the middle scrolls. On Library, for example, the count of active follows stays visible while browsing the titles.

![Animesh Library with its filters above the list and its active-follow summary below.](https://animesh.syntropicsystems.dev/assets/screenshots/mac-library.png)

## Available on both systems

The release includes Apple Silicon and Intel Mac disk images, plus x86_64 and ARM64 Linux packages and tarballs. Each distribution bundles the desktop app, engine, and CLI together. The release builds and Linux desktop smoke checks run on the target architectures.

The current Mac downloads are not notarized. The installation guide explains the approval macOS may require; Homebrew is another option. Signing and notarization support is prepared, but it still needs Developer ID credentials.

The new first-use and text-size improvements merged after 0.7.0 are for the next app release. The site currently downloads 0.7.0.

[Get Animesh](https://animesh.syntropicsystems.dev/) · [Release notes](https://github.com/Abhi-Gautam/animesh/releases/tag/v0.7.0) · [Source](https://github.com/Abhi-Gautam/animesh)
