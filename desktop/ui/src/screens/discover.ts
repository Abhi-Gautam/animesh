import type { Detail, Feed, Key, Snapshot } from "../types.js";
import { button, candidate, compactTime, empty, episode, factsRow, humanize, node, page, time, titleFilter } from "../ui.js";

export const feeds: [Feed, string][] = [["anime_airing_this_week", "Anime · Airing this week"], ["anime_this_season", "Anime · This season"], ["tv_on_now", "TV · Current US grid"]];
const reasons: Record<Feed, string> = { anime_airing_this_week: "Anime releasing from the past 24 hours through the next seven days.", anime_this_season: "Anime in the current season.", tv_on_now: "Currently running English-language TV in the US broadcast and streaming schedule." };

export function render(target: HTMLElement, snapshot: Snapshot, chooseFeed: (feed: Feed) => void, open: (key: Key) => void, follow: (detail: Detail) => void, refresh: () => void): void {
  if (snapshot.view.screen !== "discovery") return;
  const data = snapshot.view.data; const controls = node("div", "controls");
  const collection = node("select"); collection.setAttribute("aria-label", "Discovery collection");
  for (const [key, label] of feeds) { const option = node("option", "", label); option.value = key; option.selected = data.key === key; collection.append(option); }
  collection.addEventListener("change", () => chooseFeed(collection.value as Feed)); controls.append(collection);
  const filter = titleFilter(target, "Filter discovery titles"); controls.append(filter);
  const description = node("p", "collection-description", reasons[data.key]);
  const rows = node("div", "rows"); const count = node("span", "quiet");
  const populate = (): void => {
    const items = data.items.filter(item => item.facts.display_title.toLocaleLowerCase().includes(filter.value.toLocaleLowerCase()));
    count.textContent = `${items.length} titles${filter.value ? ` of ${data.items.length}` : ""} · ${humanize(data.freshness)}${data.generated_at ? ` · Updated ${time(data.generated_at)}` : ""}`;
    const results = items.map(item => {
      const row = factsRow(candidate(item), item.follow_state === "active", () => open(item.facts.source_key), () => follow(item));
      const airing = item.facts.next_airing;
      if (airing) row.querySelector(".row-main")?.append(node("p", "quiet", `${episode(airing.episode, airing.season)} · ${compactTime(airing.airing_at)}`));
      return row;
    });
    rows.replaceChildren(...results);
    if (!items.length) rows.append(empty(data.generated_at ? "No titles match" : "Discovery snapshot unavailable", data.last_error ?? (data.generated_at ? "Try a different filter or collection." : "Load a bounded collection. Your followed titles remain separate."), data.generated_at ? [] : [button("Load discovery", refresh, "primary")]));
  };
  filter.addEventListener("input", populate); populate();
  const content = [rows];
  if (data.last_error) content.unshift(node("p", "explanation", data.last_error));
  if (data.freshness !== "fresh" && data.generated_at) content.unshift(node("p", "explanation", "This saved collection is out of date. It remains available while the source recovers."));
  page(target, content, [controls, description], [count]);
}
