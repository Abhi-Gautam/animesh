# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

animesh is a personal release radar — anime first, but the substrate is cross-media (anime + TV + music). Single-user, Mac-local, all-Rust, SQLite-backed. The product story lives in `manifesto.md`; read it before making architectural decisions.

## Architecture

### The Library facade is the only mutation chokepoint

`src/library/mod.rs` is the *single* place that answers "is this followed?" and the only thing above `store/` allowed to mutate state. TUI handlers, the sync loop, and CLI shims are thin marshalling — they parse input, call `Library`, render output. Everything below `Library` (sources, store) does only what `Library` asks.

When adding a feature, the question is almost never "where does this code go" — it's "what's the smallest `Library` primitive that lets the caller express this." Add the primitive; the caller becomes trivial.

### Layering rules 

- `store/` is the **only** module that imports `rusqlite`. Driver swap (rusqlite → turso) is meant to be a one-module rewrite.
- `sources/` is the **only** place `reqwest` appears. Each source owns its own rate-limiting + retry. No shared `Source` trait yet — factor one out when the second adapter needs it.
- `Library` does **not** canonicalize titles. The canonicalization pipeline (`ingest/` + `search/`) decides that.

### Error discipline

`errors.rs` defines three exit kinds: `User=1`, `Durable=2`, `Network=3`. Wrap intentional user/network errors in `UserError` / `NetworkError`;

## Project conventions

- **Marvel-tier bar.** Reject "good enough" defaults — Postgres-grade durability + Task-Manager-grade efficiency. The user will flag laziness.
- **Active development.** We are building right now no need to preserve any old behavior. Cleaner architecture wins.
- **Reuse before building.** Before adding a primitive, deep-dive what exists and either justify why it can't be extended or extend it. Recommend, then ask.
