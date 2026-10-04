import type { Cursor, Key, Kind, Snapshot } from "../types.js";
import { avatar, button, compactTime, empty, episode, humanize, kindControls, node, page, pagination, sourceName, titleFilter } from "../ui.js";

export function render(target: HTMLElement, snapshot: Snapshot, kind: Kind | null, sort: "alphabetical" | "next_release", change: (kind: Kind | null, sort: "alphabetical" | "next_release") => void, open: (key: Key) => void, next: (cursor: Cursor) => void, first: () => void, notFirst: boolean): void {
  if (snapshot.view.screen !== "library") return;
  const data = snapshot.view.data; const controls = node("div", "controls");
  controls.append(kindControls(kind, value => change(value, sort)));
  const ordering = node("select"); ordering.setAttribute("aria-label", "Library sort order");
  for (const [value, label] of [["next_release", "Next release"], ["alphabetical", "Alphabetical"]] as const) { const option = node("option", "", label); option.value = value; option.selected = value === sort; ordering.append(option); }
  ordering.addEventListener("change", () => change(kind, ordering.value as typeof sort)); controls.append(ordering);
  const filter = titleFilter(target, "Filter library titles"); controls.append(filter);
  const rows = node("div", "rows");
  const count = node("span", "quiet");
  const populate = (): void => {
    const items = data.items.filter(item => item.facts.display_title.toLocaleLowerCase().includes(filter.value.toLocaleLowerCase()));
    count.textContent = kind !== null || filter.value || notFirst || data.next ? `${items.length} shown · ${snapshot.health.active_follows} active follows total` : `${snapshot.health.active_follows} active follows`;
    rows.replaceChildren(...items.map(item => {
      const facts = item.facts; const row = button("", () => open(facts.source_key), "media-row library-row");
      const main = node("span", "row-main");
      main.append(node("span", "row-title", facts.display_title));
      const meta = node("span", "row-meta", `${facts.source_key.source === "anilist" ? "Anime" : "TV"} · ${sourceName(facts.source_key.source)}`);
      if (item.freshness !== "fresh") meta.append(node("span", "pill warning", humanize(item.freshness)));
      main.append(meta);
      const airing = facts.next_airing; const next = node("span", "row-next");
      if (airing) next.append(node("span", "", episode(airing.episode, airing.season)), node("span", "", compactTime(airing.airing_at)));
      else next.textContent = "No upcoming episode known";
      row.append(avatar(facts.display_title), main, next); return row;
    }));
    if (!rows.childElementCount) rows.append(empty(data.items.length ? "No titles match" : "No followed titles here", data.items.length ? "Try a different title filter." : "Search for a title or explore a discovery collection."));
  };
  filter.addEventListener("input", populate); populate();
  const footer = pagination("", data.next, next, first, notFirst); footer[0] = count;
  page(target, [rows], [controls], footer);
}
