import type { Key, Snapshot } from "../types.js";
import { button, empty, fields, node, page, releaseRow, section, time } from "../ui.js";

export function render(target: HTMLElement, snapshot: Snapshot, open: (key: Key) => void, navigate: (screen: "search" | "discover" | "schedule" | "health") => void): void {
  if (snapshot.view.screen !== "home") return;
  const data = snapshot.view.data;
  const overview = node("div", "overview"); const releases = node("div");
  if (!snapshot.health.active_follows) {
    releases.append(empty("Nothing followed yet", "Find a title you care about. Animesh will keep its release schedule here.", [button("Search for a title", () => navigate("search"), "primary"), button("Explore what is airing", () => navigate("discover"))]));
  } else {
    releases.append(section("Dropped recently", data.dropped.map(row => releaseRow(row, open, true)), "No episodes dropped in the past 24 hours."));
    releases.append(section("Up next", data.upcoming.map(row => releaseRow(row, open, true)), "Your library is intact; no upcoming episodes are currently known."));
  }
  const supporting = node("aside", "supporting"); const library = node("section"); library.append(node("h3", "", "Your library"), fields([["Anime", String(data.anime)], ["TV", String(data.tv)]]));
  const freshness = node("section"); freshness.append(node("h3", "", "Data freshness"), node("p", "quiet", snapshot.health.last_success_at ? `Last source refresh ${time(snapshot.health.last_success_at, true)}` : "No successful source refresh yet."));
  const notifications = node("section"); notifications.append(node("h3", "", "Notifications"), node("p", "quiet", snapshot.health.authorization === "authorized" ? `${snapshot.health.notifications.registered} registered with the system` : "Permission needs attention"), button(snapshot.health.degraded.length ? "Needs attention" : "All systems healthy", () => navigate("health")));
  supporting.append(library, freshness, notifications); overview.append(releases, supporting);
  page(target, [overview], [], [node("span", "quiet", `${snapshot.health.active_follows} active follows · ${data.upcoming.length} upcoming releases`), button("Open schedule", () => navigate("schedule"))]);
}
