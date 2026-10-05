import type { Candidate, Detail } from "../types.js";
import { avatar, button, candidate, fields, humanize, kindName, nextText, node, sourceName, time } from "../ui.js";

export function render(target: HTMLElement, selected: Detail | Candidate, followed: boolean, mutate: () => void): void {
  const detail = "facts" in selected ? selected : null; const facts = detail ? candidate(detail) : selected as Candidate;
  const alternate = facts.titles.native ?? facts.titles.romaji;
  const heading = node("div", "detail-heading");
  const title = node("div", "detail-title");
  title.append(node("h2", "", facts.display_title));
  if (alternate && alternate !== facts.display_title) title.append(node("p", "subtitle", alternate));
  heading.append(avatar(facts.display_title), title);
  target.replaceChildren(heading);
  const actions = node("div", "actions"); actions.append(button(followed ? "Stop following" : "Follow title", mutate, followed ? "danger" : "primary")); target.append(actions);
  target.append(fields([["Type", kindName(facts.source)], ["Format", facts.format ?? "Unknown"], ["Status", humanize(facts.status)], ["Year", facts.season_year ? String(facts.season_year) : "Unknown"], ["Episodes", facts.episode_count ? String(facts.episode_count) : "Not announced"], ["Source", sourceName(facts.source)], ["Follow state", followed ? "Following" : "Not followed"]]));
  const schedule = node("section", "schedule-card"); schedule.append(node("h3", "", "Next known release"), node("p", "", detail ? nextText(detail.facts.next_airing) : "A schedule is confirmed when you follow this title."));
  if (detail) schedule.append(node("p", "quiet", `Notifications: ${detail.notification_state ? humanize(detail.notification_state) : "No registration for a next episode"}`));
  target.append(schedule);
  if (detail) target.append(node("p", "quiet", detail.last_success_at ? `Last source refresh: ${time(detail.last_success_at, true)}. ${humanize(detail.freshness)}.` : "No successful source refresh recorded."));
  target.append(node("p", "explanation", "Only the next projected episode is known. Future episodes are shown when the source provides a confirmed time."));
  const why = node("details"); why.append(node("summary", "", "Why this data?"), node("p", "", `${sourceName(facts.source)} supplies this title’s identity and schedule. ${facts.source}:${facts.source_id}. Following is your explicit choice.`)); target.append(why);
}
