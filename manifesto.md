# animesh — Product Manifesto

**animesh is a personal release radar for everything you follow — anime, TV, music — that knows what you subscribe to, verifies what's actually playable, and exposes that signal as three coequal surfaces: a TUI, push notifications, and an LLM-readable context.**

## The job-to-be-done

> animesh tells me the exact moment something I love becomes playable on a service I actually pay for, lets me one-tap into it, and exposes my taste graph to any LLM I want to ask "what should I watch tonight."

## Five non-negotiables

1. **Cross-media is the wedge, not anime alone.** The unit of follow is the franchise: an *artist* for music, a *show* for TV, a *show* for anime. Multi-season shows collapse into one entry — no duplicates per season. Music is coequal, not a sidebar.

2. **Subscriptions are the first signal.** What you pay for determines what we recommend, surface, probe, and notify on. animesh never tells you to watch something on a service you don't have.

3. **Verify-then-notify is the moat.** We do not trust the schedule. We probe the streamer at the expected moment and notify only when playback is actually possible. Crunchyroll-late is a solved problem.

4. **Canonical schema is the substrate.** Many noisy sources fan in (AniList, TMDB, TVMaze, MusicBrainz). Temperature-0 LLM canonicalization normalizes them into one graph: `Release × SourceRef × Engagement`. Many sinks fan out — TUI, notifier, LLM context, and eventually an embeddings rec engine.

5. **All-Rust, Mac-local, single-user, single-region.** No Python sidecar. No cloud. No multi-tenant. Marvel-tier engineering on a personal scale — built first for the author, OSS-shaped later.

## What animesh is NOT

- Not a recommender as the front door. Recommendation is a sink, not the wedge.
- Not a download manager. Sonarr/Radarr territory; animesh is for legal streaming.
- Not a tracker that "remembers what episode you're on." The streamer knows. We track engagement signal, not granular progress.
- Not multi-user, not cloud, not cross-platform. Single-user, Mac, local — by design.

## How animesh wins

By being the *only* product that combines:

- **Drop-minute precision** — probe-verified, not schedule-trusted.
- **Cross-media coverage** — anime + TV + music as peers.
- **Subscription-aware** — recommends only what you can actually watch.
- **One-tap launch** — deep-link into the player at the right title.
- **LLM-native context** — every release event is machine-readable, agent-callable.

Nothing in the market today does more than two of these. animesh does all five.

## The audience

One person, initially: the author. Mac-only, India-region, all-Rust. An OSS release is possible but a downstream decision — v0.5 through v1.0 are built single-user-first.

If others find it useful later, the canonical schema is OSS-shaped from day one.
