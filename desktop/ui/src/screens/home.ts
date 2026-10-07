import type { Key, Snapshot } from "../types.js";
import { button, empty, fields, node, page, releaseRow, section, time } from "../ui.js";

export function render(target: HTMLElement, snapshot: Snapshot, open: (key: Key) => void, navigate: (screen: "search" | "schedule" | "health") => void): void {
  if (snapshot.view.screen !== "home") return;
  const data = snapshot.view.data;
  const overview = node("div", "overview"); const releases = node("div");
  if (!snapshot.health.active_follows) {
    const welcome = empty("Your next episode, without the checking", "Follow your first anime or TV show. Animesh keeps its schedule on this device and reminds you when the next episode releases.", [button("Find your first show", () => navigate("search"), "primary")]);
    const steps = node("ol", "getting-started");
    for (const [title, description] of [["Find a show", "Search by name. Check the year and source to pick the right title."], ["Follow it", "Choose Follow. Its next known episode appears on Home and Schedule."], ["Allow reminders", "On macOS, allow notifications when asked. On Linux, your desktop handles notifications."]]) {
      const step = node("li"); step.append(node("h3", "", title), node("p", "", description)); steps.append(step);
    }
    welcome.append(steps, node("p", "quiet", "Close the window whenever you like; the background service keeps tracking. Release times come from AniList and TVmaze and may change."), button("Check notification permission", () => navigate("health")));
    releases.append(welcome);
  } else {
    releases.append(section("Dropped recently", data.dropped.map(row => releaseRow(row, open, true)), "No episodes dropped in the past 24 hours."));
    releases.append(section("Up next", data.upcoming.map(row => releaseRow(row, open, true)), "Your library is intact; no upcoming episodes are currently known."));
  }
  const supporting = node("aside", "supporting"); const library = node("section"); library.append(node("h3", "", "Your library"), fields([["Anime", String(data.anime)], ["TV", String(data.tv)]]));
  const freshness = node("section"); freshness.append(node("h3", "", "Data freshness"), node("p", "quiet", snapshot.health.last_success_at ? `Last source refresh ${time(snapshot.health.last_success_at, true)}` : "No successful source refresh yet."));
  const notifications = node("section"); notifications.append(node("h3", "", "Notifications"), node("p", "quiet", snapshot.health.authorization === "authorized" ? `${snapshot.health.notifications.registered} registered with the system` : "Permission needs attention"), button(snapshot.health.degraded.length || snapshot.health.authorization !== "authorized" ? "Needs attention" : "All systems healthy", () => navigate("health")));
  supporting.append(library, freshness, notifications); overview.append(releases, supporting);
  page(target, [overview], [], [node("span", "quiet", `${snapshot.health.active_follows} active follows · ${data.upcoming.length} upcoming releases`), button("Open schedule", () => navigate("schedule"))]);
}
