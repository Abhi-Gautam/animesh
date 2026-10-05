const views = {
  search: ["mac-search.png", "Animesh Search has a title search field, anime and TV filters, and source guidance.", "Find a title by name, then check its year, format, and source before following."],
  health: ["mac-health.png", "Animesh Health shows the local engine, source freshness, notification status, and storage checks.", "Understand source freshness, notification permission, and the background service from one page."],
  schedule: ["mac-schedule.png", "Animesh Schedule groups upcoming anime and TV episodes by local date, with filters above and a summary below.", "Look ahead by date. Filters and the summary stay visible while the list scrolls."],
  library: ["mac-library.png", "Animesh Library showing followed titles, an anime and TV filter, sorting, and each title’s next known episode.", "Follow deliberately. Keep your anime and TV together, with the next known episode alongside each title."],
  discover: ["mac-discover.png", "Animesh Discover showing an airing anime collection with title details and Follow controls.", "Explore a bounded collection of airing titles, then choose what you want to follow."],
  linux: ["linux-discover.png", "Animesh running natively on Linux, showing the Discover page with airing anime and Follow controls.", "The same release radar on Linux. Actual Ubuntu desktop capture; appearance follows your system."],
};
for (const control of document.querySelectorAll("[data-view]")) {
  control.addEventListener("click", () => {
    const [file, alt, caption] = views[control.dataset.view];
    for (const sibling of document.querySelectorAll("[data-view]")) sibling.setAttribute("aria-pressed", String(sibling === control));
    const image = document.getElementById("view-image"); image.src = `assets/screenshots/${file}`; image.alt = alt; document.getElementById("view-full").href = image.src;
    document.getElementById("view-frame").classList.toggle("linux-shot", control.dataset.view === "linux");
    document.getElementById("view-caption").textContent = caption;
  });
}
