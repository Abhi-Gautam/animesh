# Security

Report vulnerabilities privately through [GitHub private reporting](https://github.com/Abhi-Gautam/animesh/security/advisories/new). Do not post working exploits or user data in public issues.

Use the latest release and keep your OS and Linux WebKitGTK packages updated. Only download from this repository or the documented package managers. Verify release checksums and, when available, GitHub artifact attestations (`gh attestation verify FILE --repo Abhi-Gautam/animesh`). Mac public binaries must be Developer ID signed and notarized; do not disable Gatekeeper to run an unverified download.

Animesh has no account or cloud library. Searches and followed IDs contact AniList/TVmaze. The local database and backups contain private follow history; use OS disk encryption. Dropping a follow stops tracking and retains history; `animesh privacy forget media:N --yes` erases a title's local history. Your assistant provider may receive data when you deliberately use the CLI through an assistant.

Notification delivery depends on OS permissions, preview settings, power/session state, and source correctness. Library backups protect against application mistakes; keep an external encrypted backup for device loss.
