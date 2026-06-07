# Manual QA — animesh TUI

Run this checklist before tagging a release. `cargo test` verifies code
correctness; this verifies *feature* correctness. The two are different
problems — automated tests caught zero of the v0.4 onboarding bugs
because the bugs were "the wire was never connected." Press the keys
yourself; read the screen.

**Setup.** Use a throwaway DB so you don't trash your real library:

```sh
ANIMESH_DB_PATH=/tmp/animesh-qa.sqlite cargo run
```

Delete `/tmp/animesh-qa.sqlite` between sections to reset state.

---

## Section A — First-run onboarding

Goal: a person who has never used animesh can follow their first show
in under 30 seconds without reading the README.

| # | Action | Expected |
|---|--------|----------|
| A1 | Delete `/tmp/animesh-qa.sqlite`. Launch animesh. | Centered welcome card with the title "welcome to animesh", three keys (`a` / `:` / `?`), and dim footer text about the three-pane view. No empty Today/Late/Backlog columns. |
| A2 | Press `j`, `k`, `w`, `tab`. | All no-ops. No toast, no panic. Cursor doesn't move (nothing to move). |
| A3 | Press `?`. | Help overlay opens with the full keymap, including `:`, `/`, `a` rows. |
| A4 | Press `Esc`. | Help closes; welcome card returns. |
| A5 | Press `:`. | Command palette opens. Footer flips to ` COMMAND ` mode pill. Suggestion list shows all verbs starting with `watched`. |
| A6 | Type `follow 21`. Press Enter. | Toast: `✓ Followed One Piece`. Welcome card disappears; three panes appear with One Piece in one of them. Footer back to ` NORMAL `. |
| A7 | Press `a`. | Follow overlay opens. Prompt `search ›` is empty. Footer pill ` FOLLOW `. |
| A8 | Type `frieren`, press Enter. | Within ~1s, results list appears (top hit "Frieren: Beyond Journey's End"). Footer hint changes to "Enter follow • ↑↓/jk select • Esc cancel". |
| A9 | Press `Enter` on the top hit. | Toast: `✓ Followed Frieren...`. Overlay closes. Library now has 2 shows. |

**Pass:** all of the above. **Fail:** any toast missing, any overlay
staying open after Enter, any state not updating.

---

## Section B — Command mode (`:`)

The mode that didn't work in v0.4. Verify every command both via the
canonical name and via an alias.

Pre-seed: have at least one follow (e.g. via `:follow 21`). Cursor in
the Today pane on that show.

| # | Action | Expected |
|---|--------|----------|
| B1 | `:` then Esc. | Overlay closes, no state change, no toast. |
| B2 | `:watched` Enter. | Toast: `✓ Marked <title> — episode 1 watched`. The status line at top shows updated counts. |
| B3 | `:w` Enter. (alias) | Same as B2 but episode 2. |
| B4 | `:nope` Enter. | Toast: `unknown command: nope`. No state change. Overlay closes. |
| B5 | `:` (empty), then ↓ ↓ Enter. | Runs the third suggestion (`drop`). Toast: `✗ Dropped <title> — undo with :follow <id>`. Show disappears from panes; if it was the last show, welcome card returns. |
| B6 | `:` then type `wat`, press Tab. | Query autocompletes to `watched`. |
| B7 | `:follow` (no arg) Enter. | Toast: `:follow needs <anilist-id>`. |
| B8 | `:follow frieren` Enter. | Toast: `:follow: 'frieren' is not a numeric AniList id`. |
| B9 | `:sync` Enter. | Toast: `✓ Synced N/N` after a couple of seconds. Status line stable. |
| B10 | `:doctor` Enter. | Toast: `following N shows`. |
| B11 | `:help` Enter. | Help overlay opens. |
| B12 | `:q` Enter. | Process exits cleanly; terminal restored (no scrambled output, cursor visible). |

**Critical regression check:** B2 and B3 must produce identical state
changes. If they diverge, the registry isn't the single source of truth.

---

## Section C — Search / jump mode (`/`)

Pre-seed: 3+ follows across multiple panes.

| # | Action | Expected |
|---|--------|----------|
| C1 | `/` | Overlay opens. Prompt `/`. Footer ` SEARCH `. |
| C2 | Type a partial title. | List filters live; no network call. |
| C3 | ↓ to select another hit. | Selection arrow moves. |
| C4 | Enter. | Overlay closes. Focused pane switches to wherever the selected show lives, with its row highlighted. |
| C5 | `/zzzzz` Enter. | Overlay closes silently (no hit, no jump). No error toast. |
| C6 | `/` then Esc. | Overlay closes, focused pane unchanged. |

---

## Section D — Picker mode (`a`)

Pre-seed: any state. Requires network.

| # | Action | Expected |
|---|--------|----------|
| D1 | `a` | Overlay `a · follow new show`. Empty query prompt. |
| D2 | Enter on empty query. | Inline red text "type a query first". Overlay stays open. |
| D3 | `frieren` Enter. | Within ~1s, list of ≥1 AniList result with `#<id>` and status. Footer flips to "Enter follow • ↑↓/jk select". |
| D4 | Type a letter while in Picking. | Returns to AwaitingQuery; results clear (escape-hatch to refine). |
| D5 | After D3, `↓↓` Enter. | Toast: `✓ Followed <title>` or `already following <title>`. Library updates. |
| D6 | `a` then `asdfqwrxyz` Enter. | Inline error: "no matches on AniList". Stay in overlay. |
| D7 | While query-typing, Esc. | Overlay closes, no toast. |

---

## Section E — Action keys & invariants

| # | Action | Expected |
|---|--------|----------|
| E1 | `w` on a Today/Late show. | Same as `:watched`. |
| E2 | `g` on a show with no streaming link. | Toast: `no streaming link cached for <title> — try ':sync'`. |
| E3 | `g` on a show with a streaming link (set via `:sync` after follow). | Browser opens; toast `↗ Opening <title>`. |
| E4 | `d` on a show. | Same as `:drop`. |
| E5 | Drop the last show. | Welcome card returns; we're back in first-run mode (state-derived, no flag). |
| E6 | Resize terminal narrow (<60 cols). | No panic. Layout collapses gracefully. |
| E7 | Resize very small (<20 rows). | Overlays clamp; no panic. |
| E8 | Ctrl-C from any overlay. | Quits cleanly. |

---

## Section F — Recovery

| # | Action | Expected |
|---|--------|----------|
| F1 | Kill animesh with SIGTERM mid-render. | Terminal restored on relaunch (panic-hook guard). |
| F2 | Disconnect network, then `:sync`. | Toast within ~5s: `sync failed: ...`. UI stays responsive. |
| F3 | Disconnect network, `a` query Enter. | Inline error `AniList: ...`. Overlay stays open. |

---

## Sign-off

Once every row passes, the release is ready to tag. If any single row
fails, file an issue with the row id (e.g. "QA B3 fail: alias :w didn't
increment"). Don't ship until red rows are green.
