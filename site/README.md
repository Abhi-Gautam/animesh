# Animesh website

Static HTML, CSS, and JavaScript. No framework, account system, or app backend.

Build: `python3 scripts/build-site.py`. Preview `target/site` with any static
HTTP server. The build validates every referenced asset and creates download
metadata from the latest public GitHub release. Use `--release-json FILE` to
build from saved GitHub release metadata without a network request.

The address is `https://animesh.syntropicsystems.dev/`, served by the
`animesh-website` Cloudflare Worker. Build with `python3 scripts/build-site.py`,
then deploy with `npx wrangler deploy` using the root `wrangler.jsonc`.
The Worker serves only static assets; downloads remain on GitHub.

Screenshots are real app captures. macOS captures use the maintainer’s library. Terminal images
render actual read-only command output in a terminal frame. Dates are sample
views from capture time, not a live airing schedule. The screenshot viewer
leaves the interface unchanged. Share images use the actual app capture.

No website analytics are enabled. The hosting provider has its own access logs.
Downloads are hosted by GitHub; searches inside the app contact AniList/TVmaze.
