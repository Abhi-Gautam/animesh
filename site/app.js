const views = {
  home: ["mac-home.png", "Animesh Home showing recent and upcoming episodes from followed anime and TV shows.", "What dropped. What’s next."],
  schedule: ["mac-schedule.png", "Animesh Schedule showing upcoming anime and TV episodes by date.", "Your upcoming episodes, by date."],
  library: ["mac-library.png", "Animesh Library showing followed anime and TV shows and their next episodes.", "The shows you follow."],
  linux: ["linux-home.png", "Animesh Home running on Ubuntu, showing how to search for and follow a first title.", "On Linux, too."],
  menubar: ["mac-menubar.png", "Animesh’s Mac menu bar showing upcoming releases and an Open Animesh button.", "A quick look from the Mac menu bar."],
  terminal: ["cli-next.png", "Actual animesh next terminal output listing upcoming episodes and release times.", "The same library, from your terminal."],
};
for (const control of document.querySelectorAll("[data-view]")) {
  control.addEventListener("click", () => {
    const [file, alt, caption] = views[control.dataset.view];
    for (const sibling of document.querySelectorAll("[data-view]")) sibling.setAttribute("aria-pressed", String(sibling === control));
    const image = document.getElementById("view-image");
    image.src = `assets/screenshots/${file}`;
    image.alt = alt;
    image.dataset.view = control.dataset.view;
    document.getElementById("view-full").href = image.src;
    document.getElementById("view-caption").textContent = caption;
    document.getElementById("terminal-search").hidden = control.dataset.view !== "terminal";
  });
}

const copySetup = document.getElementById("copy-setup-prompt");
copySetup.hidden = false;
copySetup.addEventListener("click", async () => {
  const prompt = document.getElementById("agent-setup-prompt");
  const status = document.getElementById("setup-copy-status");
  try {
    await navigator.clipboard.writeText(prompt.value);
    status.textContent = "Copied. Paste it into your AI assistant.";
  } catch {
    prompt.focus();
    prompt.select();
    status.textContent = "Select and copy the prompt, then paste it into your AI assistant.";
  }
});
