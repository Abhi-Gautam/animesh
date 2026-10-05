import type { Connection, Cursor, Detail, Feed, Key, Kind, Operation, RefreshTarget, Screen, SearchHit, SearchResults, Snapshot, ViewQuery } from "./types.js";
import { button, candidate, empty, factsRow, get, humanize, kindControls, node, page, relative, screens, sourceName } from "./ui.js";
import * as home from "./screens/home.js";
import * as discover from "./screens/discover.js";
import * as library from "./screens/library.js";
import * as schedule from "./screens/schedule.js";
import * as health from "./screens/health.js";
import * as detail from "./screens/detail.js";

const state: { screen: Screen; kind: Kind | null; feed: Feed; sort: "alphabetical" | "next_release"; cursor: Cursor | null; snapshot: Snapshot | null; selected: Detail | SearchHit | null; search: SearchResults | null } = { screen: "home", kind: null, feed: "anime_airing_this_week", sort: "next_release", cursor: null, snapshot: null, selected: null, search: null };
let viewGeneration = 0;
let searchGeneration = 0;
let mutationPending = false;
let toastTimer: ReturnType<typeof setTimeout> | undefined;
let boundaryTimer: ReturnType<typeof setTimeout> | undefined;
let selectedGeneration = 0;
let appliedStamp: Snapshot["stamp"] | null = null;
let engineConnected = false;

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!window.__TAURI__) throw { code: "unavailable", message: "Open the Animesh desktop app to connect to your local service." };
  return window.__TAURI__.core.invoke<T>(command, args);
}
function errorMessage(error: unknown): string { return typeof error === "object" && error !== null && "message" in error ? String(error.message) : String(error); }
function toast(message: string): void { const element = get("toast"); element.textContent = message; element.hidden = false; clearTimeout(toastTimer); toastTimer = setTimeout(() => element.hidden = true, 6000); }
function notice(message: string | null): void { get("notice-message").textContent = message ?? ""; get("notice").hidden = message === null; }
function showConnection(connected: boolean, message?: string): void {
  engineConnected = connected;
  const element = get("engine-state"); element.querySelector(".dot")?.classList.toggle("healthy", connected);
  const label = element.lastElementChild; if (label) label.textContent = connected ? "Local engine connected" : "Local engine disconnected";
  get("restart-service").hidden = connected;
  if (!connected) {
    if (state.screen === "health" && state.snapshot?.view.screen === "health") render(state.snapshot);
    notice(message ?? "The local Animesh service is not reachable. Your saved library has not been changed.");
  }
}
function query(): ViewQuery {
  switch (state.screen) {
    case "discover": return { screen: "discovery", feed: state.feed };
    case "library": return { screen: "library", kind: state.kind, state: "active", sort: state.sort, cursor: state.cursor };
    case "schedule": return { screen: "schedule", kind: state.kind, cursor: state.cursor };
    default: return { screen: state.screen === "health" ? "health" : "home" };
  }
}
function chooseKind(kind: Kind | null): void { state.kind = kind; firstPage(); }
function resetScroll(): void { get(state.screen).querySelector<HTMLElement>(".page-body")?.scrollTo(0, 0); }
function firstPage(): void { state.cursor = null; resetScroll(); void load(); }
function nextPage(cursor: Cursor): void { state.cursor = cursor; resetScroll(); void load(); }
function navigate(screen: Screen): void {
  if (location.hash !== `#${screen}`) { location.hash = screen; return; }
  state.screen = screen; state.cursor = null; state.selected = null; selectedGeneration++; get("detail-panel").hidden = true;
  for (const name of screens) { get(name).hidden = name !== screen; const link = document.querySelector(`nav a[href="#${name}"]`); if (name === screen) link?.setAttribute("aria-current", "page"); else link?.removeAttribute("aria-current"); }
  updateHeading();
  get("refresh").hidden = screen === "search";
  viewGeneration++; notice(null); resetScroll(); get("main").focus({ preventScroll: true });
  if (screen === "search") { renderSearch(); get<HTMLInputElement>("query").focus(); } else void load();
}
function greeting(): string { const hour = new Date().getHours(); return hour < 12 ? "Good morning" : hour < 18 ? "Good afternoon" : "Good evening"; }
const subtitles: Record<Screen, string> = { home: "", discover: "A bounded view of what is releasing now", search: "Find the exact anime or TV title you heard about", schedule: "Recently dropped and upcoming episodes, in your local time", library: "The titles you deliberately follow", health: "Your local engine, sources, and notifications" };
function updateHeading(): void {
  const screen = state.screen;
  get("page-title").textContent = screen === "home" ? greeting() : screen === "health" ? "System health" : humanize(screen);
  get("page-subtitle").textContent = screen === "home" ? new Intl.DateTimeFormat(undefined, { weekday: "long", day: "numeric", month: "long", year: "numeric" }).format(new Date()) : subtitles[screen];
}

async function load(): Promise<void> {
  if (state.screen === "search") return;
  updateHeading();
  const generation = ++viewGeneration; const requested = query(); const target = get(state.screen);
  if (!target.childElementCount) { const skeleton = node("div", "loading"); skeleton.setAttribute("aria-label", "Loading local data"); for (let i = 0; i < 3; i++) skeleton.append(node("div", "skeleton")); page(target, [skeleton], [], [node("span", "quiet", "Loading local data…")]); }
  try {
    const snapshot = await invoke<Snapshot>("view", { query: requested });
    if (generation !== viewGeneration) return;
    if (appliedStamp && appliedStamp.instance_id !== snapshot.stamp.instance_id) { state.cursor = null; state.search = null; state.selected = null; get("detail-panel").hidden = true; }
    appliedStamp = snapshot.stamp; state.snapshot = snapshot; showConnection(true); notice(snapshot.health.degraded.length ? snapshot.health.degraded.map(reason => health.remediation[reason] ?? humanize(reason)).join(" ") : null);
    render(snapshot); planBoundary(snapshot);
  } catch (error) {
    if (generation !== viewGeneration) return;
    const message = errorMessage(error); notice(message);
    if (typeof error === "object" && error !== null && "code" in error && ["unavailable", "protocol_mismatch"].includes(String(error.code))) showConnection(false, message);
    if (state.cursor) { state.cursor = null; void load(); return; }
    if (!state.snapshot || target.querySelector(".loading")) {
      const retry = button("Retry connection", () => void load(), "primary");
      const recovery = node("details"); recovery.append(node("summary", "", "Show recovery steps"), node("p", "", "Start the local service, then retry. If the service is already running, make sure the desktop app and daemon come from the same build."), node("pre", "", "animesh service start"));
      const panel = empty("Animesh service unavailable", message, [retry]); panel.append(recovery); page(target, [panel], [], [node("span", "quiet", "Local engine disconnected")]);
    }
  }
}
function render(snapshot: Snapshot): void {
  switch (snapshot.view.screen) {
    case "home": home.render(get("home"), snapshot, key => void openKey(key), navigate); break;
    case "discovery": discover.render(get("discover"), snapshot, feed => { state.feed = feed; resetScroll(); void load(); }, key => void openKey(key), item => void confirmMutation(item), () => void refresh()); break;
    case "library": library.render(get("library"), snapshot, state.kind, state.sort, (kind, sort) => { state.kind = kind; state.sort = sort; firstPage(); }, key => void openKey(key), nextPage, firstPage, state.cursor !== null); break;
    case "schedule": schedule.render(get("schedule"), snapshot, state.kind, chooseKind, key => void openKey(key), nextPage, firstPage, state.cursor !== null); break;
    case "health": {
      health.render(get("health"), snapshot, engineConnected, () => { void navigator.clipboard.writeText(JSON.stringify({ stamp: snapshot.stamp, health: snapshot.health }, null, 2)).then(() => toast("Diagnostics copied.")).catch(() => toast("Expand Diagnostic details to select and copy the text.")); });
      break;
    }
  }
}
function planBoundary(snapshot: Snapshot): void {
  clearTimeout(boundaryTimer);
  const data = snapshot.view;
  const rows = data.screen === "home" ? [...data.data.dropped, ...data.data.upcoming] : data.screen === "schedule" ? data.data.items : [];
  const now = Date.now() / 1000;
  const midnight = new Date(); midnight.setHours(24, 0, 0, 0);
  const deadlines = [midnight.getTime() / 1000, ...rows.flatMap(row => [row.scheduled_at, row.scheduled_at + 86400])].filter(value => value > now);
  if (deadlines.length) boundaryTimer = setTimeout(() => void load(), Math.min(2147483647, (Math.min(...deadlines) - now) * 1000 + 100));
}
async function openKey(key: Key, focus = true): Promise<void> {
  const generation = ++selectedGeneration;
  try { const snapshot = await invoke<Snapshot>("view", { query: { screen: "detail", key } }); if (generation !== selectedGeneration || snapshot.view.screen !== "detail") return; state.selected = snapshot.view.data; renderSelected(focus); }
  catch (error) { if (generation === selectedGeneration) toast(errorMessage(error)); }
}
function openCandidate(hit: SearchHit): void { selectedGeneration++; state.selected = hit; renderSelected(); }
function renderSelected(focus = true): void {
  const selected = state.selected; if (!selected) return;
  const followed = "facts" in selected ? selected.follow_state === "active" : selected.followed;
  detail.render(get("detail-content"), "facts" in selected ? selected : selected.candidate, followed, () => void confirmMutation(selected));
  const facts = "facts" in selected ? candidate(selected) : selected.candidate;
  get("detail-content").append(button("Open source page", () => { void invoke("open_source", { key: { source: facts.source, id: facts.source_id } }).catch(error => toast(errorMessage(error))); }));
  get("detail-panel").hidden = false; if (focus) get("close-detail").focus({ preventScroll: true });
}
function askConfirmation(title: string, source: string, dropping: boolean): Promise<boolean> {
  const dialog = get<HTMLDialogElement>("confirmation"); if (dialog.open) return Promise.resolve(false);
  get("confirm-title").textContent = `${dropping ? "Stop following" : "Follow"} ${title}?`;
  get("confirm-source").textContent = source;
  get("confirm-message").textContent = dropping ? "Schedule tracking and future notifications for this title will stop. Your other followed titles remain available." : "Animesh will track upcoming releases and schedule notifications when times are available.";
  const accept = get<HTMLButtonElement>("accept-confirm"); accept.textContent = dropping ? "Stop following" : "Follow"; accept.className = dropping ? "danger" : "primary";
  dialog.returnValue = "cancel"; const presented = performance.now();
  const guard = (event: KeyboardEvent): void => { if (event.key === "Enter" && performance.now() - presented < 250) event.preventDefault(); };
  dialog.addEventListener("keydown", guard); dialog.showModal(); get("cancel-confirm").focus();
  return new Promise(resolve => { dialog.addEventListener("close", () => { dialog.removeEventListener("keydown", guard); resolve(dialog.returnValue === "confirm"); }, { once: true }); });
}
async function confirmMutation(selected: Detail | SearchHit): Promise<void> {
  if (mutationPending) return;
  const facts = "facts" in selected ? candidate(selected) : selected.candidate;
  const dropping = "facts" in selected ? selected.follow_state === "active" : selected.followed;
  if (!await askConfirmation(facts.display_title, `${facts.source === "anilist" ? "Anime" : "TV"} · ${sourceName(facts.source)} · ${facts.format ?? "Unknown format"}${facts.season_year ? ` · ${facts.season_year}` : ""}`, dropping)) return;
  mutationPending = true;
  try {
    if (dropping && selected.media_id !== null) await invoke("drop_title", { mediaId: selected.media_id });
    else await invoke("follow_title", { key: { source: facts.source, id: facts.source_id } });
    toast(`${dropping ? "Stopped following" : "Following"} ${facts.display_title}.`);
    if (state.search) { for (const hit of state.search.items) { if (hit.candidate.source === facts.source && hit.candidate.source_id === facts.source_id) hit.followed = !dropping; } renderSearch(); }
    if (state.screen !== "search") await load();
    await openKey({ source: facts.source, id: facts.source_id });
  } catch (error) { toast(errorMessage(error)); }
  finally { mutationPending = false; }
}
async function refresh(): Promise<void> {
  const control = get<HTMLButtonElement>("refresh"); if (control.disabled) return;
  if (state.screen === "search") { get<HTMLFormElement>("search-form").requestSubmit(); return; }
  control.disabled = true; control.textContent = "Refreshing…";
  const target: RefreshTarget = state.screen === "discover" ? { kind: "discovery", feed: state.feed } : { kind: "library" };
  try { const operation = await invoke<Operation>("refresh", { target }); toast(operation.message ?? (operation.state === "completed" ? "Refresh completed." : `${humanize(operation.state)}. Refresh will resume automatically when possible.`)); await load(); }
  catch (error) { toast(errorMessage(error)); }
  finally { control.disabled = false; control.textContent = "Refresh"; }
}
function renderSearch(): void {
  get("search-controls").replaceChildren(kindControls(state.kind, kind => { state.kind = kind; renderSearch(); }));
  const results = get("search-results"); const data = state.search;
  if (!data) { results.replaceChildren(empty("Resolve a title", "Search AniList and TVmaze together. Check the year, format, and alternate title before following.")); get("search-status").textContent = "Search AniList and TVmaze"; return; }
  const query = get<HTMLInputElement>("query").value.trim().toLocaleLowerCase();
  const items = data.items.filter(hit => state.kind === null || (state.kind === "anime") === (hit.candidate.source === "anilist"));
  const exact = items.filter(hit => [hit.candidate.display_title, ...Object.values(hit.candidate.titles)].some(title => title?.toLocaleLowerCase() === query));
  const related = items.filter(hit => !exact.includes(hit));
  const content = node("div", "rows");
  for (const [title, matches] of [["Exact matches", exact], ["Related matches", related]] as const) {
    if (!matches.length) continue; content.append(node("h2", "day-heading", title));
    for (const hit of matches) content.append(factsRow(hit.candidate, hit.followed, () => openCandidate(hit), () => void confirmMutation(hit)));
  }
  if (!items.length) content.append(empty(data.issues.length ? "A source could not be reached" : "No matching titles", data.issues.length ? "Check source status or retry. Your followed titles and schedules remain available locally." : "Try the original title, an alternate title, or fewer words."));
  results.replaceChildren(content); get("search-status").textContent = `${items.length} matches${data.issues.length ? `. ${data.issues.map(issue => `${sourceName(issue.source)}: ${issue.message}`).join(" ")}` : " · AniList and TVmaze"}`;
}
async function search(event: SubmitEvent): Promise<void> {
  event.preventDefault(); const query = get<HTMLInputElement>("query").value.trim(); if (!query) return;
  const generation = ++searchGeneration; get("search-results").scrollTop = 0; get("search-status").textContent = "Searching AniList and TVmaze…";
  try { const results = await invoke<SearchResults>("search_titles", { query }); if (generation !== searchGeneration) return; state.search = results; renderSearch(); }
  catch (error) { if (generation === searchGeneration) get("search-status").textContent = errorMessage(error); }
}
function openPalette(): void {
  const dialog = get<HTMLDialogElement>("palette"); if (dialog.open || get<HTMLDialogElement>("confirmation").open) return; dialog.showModal(); get<HTMLInputElement>("command-query").value = ""; renderCommands(); get("command-query").focus();
}
function renderCommands(): void {
  const text = get<HTMLInputElement>("command-query").value.trim().toLocaleLowerCase(); const dialog = get<HTMLDialogElement>("palette");
  const choices = screens.filter(screen => screen.includes(text)).map(screen => button(humanize(screen), () => { dialog.close(); navigate(screen); }));
  if (text) choices.push(button(`Search titles for “${get<HTMLInputElement>("command-query").value.trim()}”`, paletteSearch));
  get("command-results").replaceChildren(...choices);
}
function paletteSearch(): void { const query = get<HTMLInputElement>("command-query").value.trim(); if (!query) return; get<HTMLDialogElement>("palette").close(); navigate("search"); get<HTMLInputElement>("query").value = query; get<HTMLFormElement>("search-form").requestSubmit(); }

get("global-search").addEventListener("click", openPalette);
get("refresh").addEventListener("click", () => void refresh());
get("restart-service").addEventListener("click", () => {
  const control = get<HTMLButtonElement>("restart-service"); control.disabled = true;
  void invoke<string>("start_service", { restart: true }).then(() => load()).catch(error => toast(errorMessage(error))).finally(() => control.disabled = false);
});
get("search-form").addEventListener("submit", event => void search(event as SubmitEvent));
get("query").addEventListener("input", () => { searchGeneration++; get("search-status").textContent = "Press Enter to search this title."; });
get("command-query").addEventListener("input", renderCommands);
get("command-query").addEventListener("keydown", event => { if (event.key === "Enter") { event.preventDefault(); paletteSearch(); } });
get("close-detail").addEventListener("click", () => { selectedGeneration++; state.selected = null; get("detail-panel").hidden = true; get("main").focus({ preventScroll: true }); });
const appearance = get<HTMLSelectElement>("theme");
try { appearance.value = localStorage.getItem("appearance") ?? "system"; } catch { /* Storage is optional; system theme works without it. */ }
function applyTheme(): void { if (appearance.value === "system") delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = appearance.value; }
applyTheme(); appearance.addEventListener("change", () => { applyTheme(); try { localStorage.setItem("appearance", appearance.value); } catch { /* No preference persistence available. */ } });
get("timezone").textContent = new Intl.DateTimeFormat(undefined, { timeZoneName: "short" }).formatToParts(new Date()).find(part => part.type === "timeZoneName")?.value ?? Intl.DateTimeFormat().resolvedOptions().timeZone;
const mac = navigator.platform.toLowerCase().includes("mac"); get("search-shortcut").textContent = mac ? "⌘ K" : "Ctrl K";
document.addEventListener("keydown", event => {
  if (get<HTMLDialogElement>("confirmation").open || get<HTMLDialogElement>("palette").open) return;
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") { event.preventDefault(); openPalette(); return; }
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "r") { event.preventDefault(); void refresh(); return; }
  if ((event.metaKey || event.ctrlKey) && /^[1-6]$/.test(event.key)) { event.preventDefault(); const screen = screens[Number(event.key) - 1]; if (screen) navigate(screen); return; }
  if (event.target instanceof HTMLInputElement || event.target instanceof HTMLSelectElement) return;
  if (event.key === "Escape") { selectedGeneration++; state.selected = null; get("detail-panel").hidden = true; get("main").focus({ preventScroll: true }); }
  if (state.selected) { const followed = "facts" in state.selected ? state.selected.follow_state === "active" : state.selected.followed; if ((event.key.toLowerCase() === "f" && !followed) || (event.key === "Delete" && followed)) { event.preventDefault(); void confirmMutation(state.selected); } }
});
window.addEventListener("hashchange", () => navigate(screens.includes(location.hash.slice(1) as Screen) ? location.hash.slice(1) as Screen : "home"));
setInterval(() => { if (document.hidden) return; for (const element of document.querySelectorAll<HTMLElement>("[data-countdown]")) { const timestamp = Number(element.dataset.countdown); element.textContent = relative(timestamp); element.className = timestamp < Date.now() / 1000 ? "pill dropped" : ""; } }, 60000);

async function connect(): Promise<void> {
  if (!window.__TAURI__) { navigate("home"); return; }
  await window.__TAURI__.event.listen<Connection>("connection", event => {
    const connection = event.payload; showConnection(connection.connected, connection.message ?? undefined);
    if (connection.connected) { state.cursor = null; if (connection.stamp && appliedStamp?.instance_id !== connection.stamp.instance_id) { state.search = null; state.selected = null; get("detail-panel").hidden = true; } void load(); }
  });
  await window.__TAURI__.event.listen("changed", () => { state.cursor = null; if (state.screen === "search" && state.search) { const previous = state.search; void invoke<SearchHit[]>("resolve_search", { candidates: previous.items.map(hit => hit.candidate) }).then(items => { if (state.search === previous) { state.search.items = items; renderSearch(); } }).catch(error => toast(errorMessage(error))); } else void load(); if (state.selected && "facts" in state.selected) void openKey(state.selected.facts.source_key, false); });
  navigate(screens.includes(location.hash.slice(1) as Screen) ? location.hash.slice(1) as Screen : "home");
}
void connect().catch(error => { notice(errorMessage(error)); navigate("home"); });
