import type { Candidate, Cursor, Detail, Key, NextAiring, Release, Screen, Source } from "./types.js";

export const screens: Screen[] = ["home", "discover", "search", "schedule", "library", "health"];
export const sourceName = (source: Source): string => source === "anilist" ? "AniList" : "TVmaze";
export const kindName = (source: Source): string => source === "anilist" ? "Anime" : "TV";
export const humanize = (value: string): string => value.replaceAll("_", " ").replace(/^./, c => c.toUpperCase());
export function node<K extends keyof HTMLElementTagNameMap>(tag: K, className = "", text?: string): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text !== undefined) element.textContent = text;
  return element;
}
export function button(text: string, action: () => void, className = ""): HTMLButtonElement {
  const element = node("button", className, text);
  element.type = "button";
  element.addEventListener("click", action);
  return element;
}
export function get<T extends HTMLElement>(id: string): T { return document.getElementById(id) as T; }
export function page(target: HTMLElement, content: HTMLElement[], controls: HTMLElement[] = [], footer: HTMLElement[] = []): void {
  const previous = target.querySelector<HTMLElement>(".page-body");
  const active = document.activeElement;
  const focused = active instanceof HTMLInputElement && target.contains(active) ? active : null;
  const top = previous?.scrollTop ?? 0;
  const header = node("div", "page-controls"); header.append(...controls);
  const body = node("div", "page-body"); body.tabIndex = 0;
  body.setAttribute("role", "region"); body.setAttribute("aria-label", `${target.getAttribute("aria-label")} content`); body.append(...content);
  const bottom = node("footer", "page-footer"); bottom.append(...footer);
  target.replaceChildren(header, body, bottom); body.scrollTop = top;
  if (focused?.name) {
    const replacement = target.querySelector<HTMLInputElement>(`input[name="${focused.name}"]`);
    replacement?.focus({ preventScroll: true });
    if (replacement && focused.selectionStart !== null) replacement.setSelectionRange(focused.selectionStart, focused.selectionEnd);
  }
}
export function titleFilter(target: HTMLElement, label: string): HTMLInputElement {
  const input = node("input"); input.type = "search"; input.name = "title-filter";
  input.placeholder = "Filter titles"; input.setAttribute("aria-label", label);
  input.value = target.querySelector<HTMLInputElement>('input[name="title-filter"]')?.value ?? "";
  return input;
}
export function pagination(text: string, cursor: Cursor | null, next: (cursor: Cursor) => void, first: () => void, notFirst: boolean): HTMLElement[] {
  const items: HTMLElement[] = [node("span", "quiet", text)];
  if (notFirst) items.push(button("First page", first));
  if (cursor) items.push(button("Next page", () => next(cursor)));
  return items;
}
export function avatar(title: string): HTMLElement {
  const initials = title.replace(/[^\p{L}\p{N}\s]/gu, " ").split(/\s+/).filter(Boolean).slice(0, 2).map(word => Array.from(word)[0]).join("").toUpperCase();
  const element = node("span", "avatar", initials || "•");
  element.setAttribute("aria-hidden", "true");
  return element;
}
export function time(timestamp: number, full = false): string {
  return new Intl.DateTimeFormat(undefined, full ? { weekday: "short", day: "numeric", month: "short", year: "numeric", hour: "2-digit", minute: "2-digit", timeZoneName: "short" } : { hour: "2-digit", minute: "2-digit" }).format(new Date(timestamp * 1000));
}
export function relative(timestamp: number, now = Date.now() / 1000): string {
  const seconds = timestamp - now;
  const minutes = Math.max(1, Math.round(Math.abs(seconds) / 60));
  const amount = minutes < 60 ? `${minutes}m` : minutes < 1440 ? `${Math.floor(minutes / 60)}h${minutes % 60 ? ` ${minutes % 60}m` : ""}` : `${Math.floor(minutes / 1440)}d${Math.floor(minutes % 1440 / 60) ? ` ${Math.floor(minutes % 1440 / 60)}h` : ""}`;
  return Math.abs(seconds) < 30 ? "Dropping now" : seconds < 0 ? `Dropped ${amount} ago` : `In ${amount}`;
}
export function episode(episode: number | null, season: number | null): string { return episode === null ? "Next episode" : season === null ? `Episode ${episode}` : `S${season}E${episode}`; }
export function nextText(next: NextAiring | null): string { return next ? `${episode(next.episode, next.season)} · ${time(next.airing_at, true)}` : "No upcoming episode known"; }
export function compactTime(timestamp: number): string {
  const date = new Date(timestamp * 1000); const now = new Date();
  const day = date.toDateString() === now.toDateString() ? "Today" : new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", ...(date.getFullYear() !== now.getFullYear() ? { year: "numeric" } : {}) }).format(date);
  return `${day}, ${time(timestamp)}`;
}
export function titleButton(title: string, key: Key, open: (key: Key) => void): HTMLButtonElement { return button(title, () => open(key), "title-button"); }
export function releaseRow(release: Release, open: (key: Key) => void, showDate = false): HTMLElement {
  const row = node("div", "release-row");
  const main = node("div", "row-main");
  main.append(titleButton(release.display_title, { source: release.source, id: release.source_id }, open));
  const meta = node("div", "row-meta", `${episode(release.episode, release.season)} · ${kindName(release.source)} · ${sourceName(release.source)}`);
  if (release.freshness !== "fresh") meta.append(node("span", "pill warning", humanize(release.freshness)));
  if (release.schedule_revision > 1) meta.append(node("span", "quiet", "Schedule revised"));
  main.append(meta);
  const exact = node("time", "release-time", showDate ? compactTime(release.scheduled_at) : time(release.scheduled_at));
  exact.dateTime = new Date(release.scheduled_at * 1000).toISOString(); exact.title = time(release.scheduled_at, true);
  const timing = node("div", "timing");
  const countdown = node("span", release.scheduled_at < Date.now() / 1000 ? "pill dropped" : "", relative(release.scheduled_at));
  countdown.dataset.countdown = String(release.scheduled_at);
  timing.append(countdown);
  row.append(avatar(release.display_title), main, exact, timing);
  return row;
}
export function section(title: string, rows: HTMLElement[], emptyText: string): HTMLElement {
  const element = node("section", "group"); element.append(node("h2", "section-heading", title));
  const body = node("div", "rows"); body.append(...rows);
  element.append(rows.length ? body : node("p", "quiet", emptyText)); return element;
}
export function factsRow(candidate: Candidate, followed: boolean, open: () => void, follow: () => void): HTMLElement {
  const row = node("div", "media-row"); const main = node("div", "row-main");
  main.append(button(candidate.display_title, open, "title-button"));
  const alternate = candidate.titles.native ?? candidate.titles.romaji;
  if (alternate && alternate !== candidate.display_title) main.append(node("p", "quiet", alternate));
  main.append(node("div", "row-meta", [kindName(candidate.source), candidate.format, candidate.season_year, humanize(candidate.status), candidate.episode_count ? `${candidate.episode_count} episodes` : null, sourceName(candidate.source)].filter(v => v !== null).join(" · ")));
  row.append(avatar(candidate.display_title), main, followed ? node("span", "following", "✓ Following") : button("Follow", follow));
  return row;
}
export function candidate(detail: Detail): Candidate { const f = detail.facts; return { source: f.source_key.source, source_id: f.source_key.id, display_title: f.display_title, titles: f.titles, status: f.status, format: f.format_raw, episode_count: f.episode_count, season_year: f.season_year }; }
export function empty(title: string, message: string, actions: HTMLElement[] = []): HTMLElement {
  const panel = node("div", "empty"); panel.append(node("h2", "", title), node("p", "", message));
  if (actions.length) { const group = node("div", "actions"); group.append(...actions); panel.append(group); } return panel;
}
export function fields(values: [string, string][]): HTMLElement {
  const dl = node("dl"); for (const [label, value] of values) { const row = node("div"); row.append(node("dt", "", label), node("dd", "", value)); dl.append(row); } return dl;
}
export function kindControls(current: "anime" | "tv" | null, change: (kind: "anime" | "tv" | null) => void): HTMLElement {
  const group = node("div", "segmented"); group.setAttribute("aria-label", "Media kind");
  for (const [value, label] of [[null, "All"], ["anime", "Anime"], ["tv", "TV"]] as const) { const tab = button(label, () => change(value)); tab.setAttribute("aria-pressed", String(current === value)); group.append(tab); } return group;
}
