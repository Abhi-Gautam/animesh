import type { Snapshot } from "../types.js";
import { button, fields, humanize, node, page, sourceName, time } from "../ui.js";

export const remediation: Record<string, string> = {
  notifications_denied: "Enable Animesh notifications in your system’s notification settings.",
  notification_capacity_exceeded: "The system has limited notification capacity. Animesh registers the nearest releases first.",
  source_rate_limited: "A source asked Animesh to wait. Existing local schedules remain available, and refresh resumes automatically.",
  schema_too_new: "Install the newer Animesh version that created this library.",
  database_corrupt: "The library failed its integrity check. Recover a backup before restarting the service.",
  database_read_only: "The library is not writable. Check its folder permissions.",
  migration_failed: "A database upgrade failed and was rolled back. Copy diagnostics for the failing step.",
};
const date = (value: number | null): string => value === null ? "Not yet" : time(value, true);
function block(title: string, values: [string, string][]): HTMLElement { const section = node("section", "health-block"); section.append(node("h2", "", title), fields(values)); return section; }
export function render(target: HTMLElement, snapshot: Snapshot, connected: boolean, copy: () => void): void {
  if (snapshot.view.screen !== "health") return;
  const h = snapshot.health; const grid = node("div", "health-grid");
  const status = !connected ? "Engine disconnected · showing last retrieved information" : h.degraded.length ? "Some systems need attention" : "All systems healthy";
  grid.append(block("Engine", [["Daemon", connected ? humanize(h.bootstrap) : "Disconnected"], ["Version", h.process_version], ["Database", h.database_ready ? "Ready" : "Needs recovery"], ["Started", date(h.started_at)]]));
  const sources = block("Sources", [["Last source success", date(h.last_success_at)], ["Titles due", String(h.refresh.due)], ["Stale titles", String(h.refresh.stale)], ["Backing off", String(h.refresh.backing_off)]]);
  for (const source of snapshot.view.data.sources) sources.append(node("p", "quiet", `${sourceName(source.source)}: ${source.blocked_until ? `retry after ${date(source.blocked_until)}` : "Available"}. Last success: ${date(source.last_success_at)}.`)); grid.append(sources);
  grid.append(block("Notifications", [["Permission", humanize(h.authorization)], ["Desired", String(h.notifications.desired)], ["Registered", String(h.notifications.registered)], ["Failed", String(h.notifications.failed)], ["Deferred by capacity", String(h.notifications.deferred_capacity)]]));
  const storage = block("Storage", [["Schema", String(h.schema_version)], ["Database checks", h.database_ready ? "Passed at startup" : "Failed at startup"]]);
  const diagnostics = node("details"); diagnostics.append(node("summary", "", "Diagnostic details"), node("pre", "", JSON.stringify({ stamp: snapshot.stamp, health: h }, null, 2))); storage.append(diagnostics); grid.append(storage);
  const content: HTMLElement[] = [grid];
  for (const reason of h.degraded) content.push(node("p", "explanation", remediation[reason] ?? humanize(reason)));
  const operations = node("section", "group"); operations.append(node("h2", "section-heading", "Recent refreshes"));
  for (const operation of snapshot.view.data.operations) operations.append(node("p", "explanation", `${operation.target.kind === "library" ? "Library" : humanize(operation.target.feed)} · ${humanize(operation.state)} · ${date(operation.updated_at)}${operation.message ? `\n${operation.message}` : ""}`));
  if (!snapshot.view.data.operations.length) operations.append(node("p", "quiet", "No manual refreshes yet."));
  content.push(operations); page(target, content, [], [node("span", "quiet", status), button("Copy diagnostics", copy)]);
}
