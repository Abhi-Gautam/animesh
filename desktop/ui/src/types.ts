export type Source = "anilist" | "tvmaze";
export type Kind = "anime" | "tv";
export type Freshness = "fresh" | "stale" | "backing_off";
export type Screen = "home" | "search" | "schedule" | "library" | "health";
export interface Key { source: Source; id: number }
export interface Stamp { instance_id: string; revision: number }
export interface Cursor extends Stamp { offset: number; scope: string }
export interface Titles { english: string | null; romaji: string | null; native: string | null }
export interface NextAiring { episode: number; season: number | null; airing_at: number }
export interface Facts { source_key: Key; display_title: string; titles: Titles; status: string; format_raw: string | null; episode_count: number | null; season_year: number | null; next_airing: NextAiring | null }
export interface Detail { media_id: number; facts: Facts; follow_state: "active" | "dropped" | null; last_success_at: number | null; freshness: Freshness; schedule_revision: number | null; notification_state: string | null }
export interface Release { media_id: number; source: Source; source_id: number; display_title: string; episode: number | null; season: number | null; scheduled_at: number; schedule_revision: number; freshness: Freshness; aired: boolean; last_success_at: number | null }
export interface Candidate { source: Source; source_id: number; display_title: string; titles: Titles; status: string; format: string | null; episode_count: number | null; season_year: number | null }
export interface SearchHit { candidate: Candidate; media_id: number | null; followed: boolean }
export interface SearchResults { items: SearchHit[]; issues: { source: Source; message: string }[] }
export interface Health { process_version: string; schema_version: number; bootstrap: string; database_ready: boolean; active_follows: number; started_at: number; last_success_at: number | null; authorization: string; degraded: string[]; refresh: { due: number; stale: number; backing_off: number; failed: number }; notifications: { desired: number; registered: number; failed: number; deferred_capacity: number }; source_blocked_until: number | null }
export interface Operation { id: string; target: RefreshTarget; state: "queued" | "running" | "completed" | "throttled" | "failed"; updated_at: number; message: string | null }
export type RefreshTarget = { kind: "library" };
export type ViewQuery = { screen: "home" | "health" } | { screen: "schedule"; kind: Kind | null; cursor: Cursor | null } | { screen: "library"; kind: Kind | null; state: "active" | "dropped"; sort: "alphabetical" | "next_release"; cursor: Cursor | null } | { screen: "detail"; key: Key };
export type ViewData =
  | { screen: "home"; data: { dropped: Release[]; upcoming: Release[]; anime: number; tv: number } }
  | { screen: "library"; data: { items: Detail[]; next: Cursor | null } }
  | { screen: "schedule"; data: { items: Release[]; next: Cursor | null } }
  | { screen: "detail"; data: Detail }
  | { screen: "health"; data: { sources: { source: Source; blocked_until: number | null; last_success_at: number | null }[]; operations: Operation[] } };
export interface Snapshot { stamp: Stamp; generated_at: number; health: Health; view: ViewData }
export interface Connection { connected: boolean; message: string | null; stamp: Stamp | null }

declare global {
  interface Window {
    __TAURI__: {
      core: { invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> };
      event: { listen<T>(event: string, callback: (event: { payload: T }) => void): Promise<() => void> };
    };
  }
}
