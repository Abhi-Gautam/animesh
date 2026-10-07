import type { SkillStatus } from "./types.js";
import { button, node, replaceContent } from "./ui.js";

export interface SkillSetup {
  status: SkillStatus | null;
  pending: "checking" | "installing" | null;
  error: string | null;
  install: () => void;
  check: () => void;
}

export function render(target: HTMLElement, setup: SkillSetup, onboarding: boolean): void {
  const locations = setup.status?.locations ?? [];
  const installed = locations.length > 0 && locations.every(location => location.state !== "absent");
  const edited = locations.some(location => location.state === "edited");
  const content: HTMLElement[] = [node(onboarding ? "h3" : "h2", "", "Your AI assistant")];
  content.push(node("p", "quiet", "Let your assistant check what airs next, manage your follows, and read your library before recommending a show."));
  if (setup.pending) {
    content.push(node("p", "quiet", setup.pending === "installing" ? "Installing the Animesh skill…" : "Checking skill installation…"));
  } else if (setup.status) {
    const message = installed ? edited ? "The installed skill differs from this app version." : "Animesh skill installed." : locations.some(location => location.state !== "absent") ? "Skill installation is incomplete." : "The Animesh skill is not installed.";
    content.push(node("p", "quiet", message));
  }
  if (setup.error) {
    const error = node("p", "explanation", setup.error);
    error.setAttribute("role", "alert"); content.push(error);
  }
  if (!installed && !edited) {
    const install = button(setup.pending === "installing" ? "Installing…" : "Install Animesh skill", setup.install, "primary");
    install.disabled = setup.pending !== null || setup.status === null;
    content.push(install);
  }
  if (setup.status === null || setup.error || edited || installed) {
    const check = button("Check installation", setup.check);
    check.disabled = setup.pending !== null; content.push(check);
  }
  content.push(node("p", "quiet", installed ? "Open a new chat or reload skills in your assistant, then ask what is airing this week." : onboarding ? "Optional. You can install it later from Health → Your AI assistant." : "Install once for compatible AI assistants on this device."));
  if (edited) content.push(node("p", "quiet", "The existing file has been kept. It may be customized or from another version. To replace it, review the file and run animesh skill install --force."));
  if (locations.length) {
    const details = node("details"); details.append(node("summary", "", "Installation details"));
    for (const location of locations) details.append(node("pre", "", `${location.path}\n${location.state === "absent" ? "Not installed" : location.state === "edited" ? "Different from this app version" : "Installed"}`));
    content.push(details);
  }
  target.setAttribute("aria-busy", String(setup.pending !== null));
  replaceContent(target, content);
}
