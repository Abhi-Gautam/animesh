import type { Cursor, Key, Kind, Release, Snapshot } from "../types.js";
import { button, empty, kindControls, node, page, pagination, releaseRow, section } from "../ui.js";

export function dayLabel(timestamp: number, now = new Date()): string {
  const date = new Date(timestamp * 1000); const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const tomorrow = new Date(today); tomorrow.setDate(tomorrow.getDate() + 1);
  if (date.toDateString() === today.toDateString()) return "Today";
  if (date.toDateString() === tomorrow.toDateString()) return "Tomorrow";
  return new Intl.DateTimeFormat(undefined, { weekday: "long", day: "numeric", month: "short" }).format(date);
}

export function render(target: HTMLElement, snapshot: Snapshot, kind: Kind | null, change: (kind: Kind | null) => void, open: (key: Key) => void, next: (cursor: Cursor) => void, first: () => void, notFirst: boolean): void {
  if (snapshot.view.screen !== "schedule") return;
  const data = snapshot.view.data; const controls = node("div", "controls"); controls.append(kindControls(kind, change), button("Today", first));
  const content: HTMLElement[] = [];
  const groups = new Map<string, Release[]>();
  for (const row of data.items) { const label = row.scheduled_at < Date.now() / 1000 ? "Dropped recently" : dayLabel(row.scheduled_at); const group = groups.get(label) ?? []; group.push(row); groups.set(label, group); }
  for (const [label, rows] of groups) content.push(section(label, rows.map(row => releaseRow(row, open)), ""));
  if (!data.items.length) content.push(empty("No upcoming releases", "Your library is intact; no upcoming episodes are currently known. Animesh only shows source-confirmed times."));
  page(target, content, [controls], pagination(`${data.items.length} releases shown · Next known episode per title`, data.next, next, first, notFirst));
}
