# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

animesh is a personal release radar — anime first, but the substrate is cross-media (anime + TV + music). Single-user, Mac-local, all-Rust, SQLite-backed.

## Architecture

### Product architecture north star

animesh is not fundamentally a TUI app. It is a local-first personal data intelligence engine: external world → ingestion → durable evidence → normalized observations → canonical user graph → schedules/events/relationships → serving read models → many surfaces. The TUI is only one serving surface; future surfaces include daemon notifications, timelines, graph visualizations, recommendations, APIs, and dashboards.

Think in data-platform layers:

- Bronze: raw external evidence
- Silver: normalized source facts
- Gold: canonical user graph and projected facts 
- Serving: Library read models consumed by TUI/daemon/API/visualizations

Core engine capabilities are: discover → follow → ingest → normalize → canonicalize → project schedules/events → sync/refresh → resolve read models → user feedback → health/explainability.

## Project conventions

- **Marvel-tier bar.** Reject "good enough" defaults — Postgres-grade durability + Task-Manager-grade efficiency. The user will flag laziness.
- **Active development.** We are building right now no need to preserve any old behavior. Cleaner architecture wins.
- **Reuse before building.** Before adding a primitive, deep-dive what exists and either justify why it can't be extended or extend it. Recommend, then ask.
