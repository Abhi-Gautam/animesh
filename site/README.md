# Animesh website

Static HTML, CSS, and JavaScript. No framework, account system, or app backend.

Build: `python3 scripts/build-site.py`. Preview `target/site` with any static
HTTP server. The build validates every referenced asset and creates download
metadata from the latest public GitHub release. Use `--release-json FILE` to
build from saved GitHub release metadata without a network request.

The intended address is `https://animesh.syntropicsystems.dev/`. Hosting and DNS
are a separate final step; this repository does not change existing sites.
Cloudflare Pages build command: `python3 scripts/build-site.py`; output
directory: `target/site`. The same output can be hosted as Worker static assets.

Screenshots are real app captures. macOS captures use the maintainer’s library;
Linux captures come from the native WebKit release smoke test. Terminal images
render actual read-only command output in a terminal frame. Dates are sample
views from capture time, not a live airing schedule. The screenshot viewer
leaves the interface unchanged. Share images use the actual app capture.

No website analytics are enabled. The hosting provider has its own access logs.
Downloads are hosted by GitHub; searches inside the app contact AniList/TVmaze.
