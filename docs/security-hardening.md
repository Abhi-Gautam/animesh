# Security remediation — 5 November 2026

This change addresses the source and repository findings from the 8 October
audit of v0.7.2. Existing downloads are not retroactively patched. The changes
must pass required checks, merge, and ship before installed users receive them.

## Audit findings

| Finding | Remediation |
| --- | --- |
| S01: release chain | Read-only default workflow tokens; actions pinned to commits; protected `master` and immutable release tags; protected owner-approved release environment; secret scanning and push protection enabled; dependency/security checks required; artifact provenance; short-lived crates.io publishing credentials. Dependency compilation runs separately from Mac signing and crate publishing credentials. |
| S02: dependencies | Updated both locked dependency graphs, including production Rustls and test HTTP/2 dependencies. Linux GLib 0.18 receives the compatible upstream iterator fix; the security gate verifies its exact source hashes and still scans its registry identity for new advisories. |
| S03: terminal text | Human-readable titles and errors escape control characters, line separators, and bidirectional overrides. The assistant skill treats all provider text as untrusted data. |
| S04: retention/recovery | Raw evidence has a 32 MiB / 30-day budget; obsolete observations/events are pruned without removing current facts. SQLite growth is limited to 256 MiB for new libraries; existing larger files cannot grow further. Consistent private backups and verified offline restore preserve original recovery bytes. Backup failures are visible. |
| S05: backoff | Provider retry delays are bounded to one day; migration heals unreasonable old persisted deadlines. |
| S06: failed responses | Non-success HTTP responses cannot commit provider-shaped facts; narrowly defined not-found responses remain supported. Failed refreshes preserve current facts. |
| S07: large libraries | CLI reads page through revision-bound cursors. Reply serialization is bounded and oversized legacy requests receive a structured error. Maximum-length titles are tested across 501 follows. |
| S08: release identity | Public Mac packaging requires Developer ID signing, notarization, stapling, and Gatekeeper assessment for the app and disk image. Missing credentials fail closed. Local development signatures remain supported. Public packages receive GitHub provenance. |

Additional changes disable provider redirects, enforce production HTTPS, check
private path ownership and links, make sensitive writes atomic, bound writer and
IPC capacity, keep shutdown responsive, preserve edited assistant skills on
uninstall, stage upgrades with rollback, remove title-bearing copied diagnostics,
and offer private notification banners and explicit history erasure. The website
source adds host-specific HSTS.

## Verification

Required CI covers native macOS Apple Silicon/Intel and Ubuntu 24.04 AMD64/ARM64:
workspace tests, strict lint/format checks, desktop bridge tests, frontend build,
default release builds, isolated CLI recovery/privacy checks, and installer and
signing failure tests. A separate job exercises the bundled Mac daemon and
singleton behavior. The release pipeline also installs the Linux package and
exercises its WebKit desktop before publication.

Regression coverage includes hostile provider text/status/redirect/backoff,
over-limit IPC responses, retention integrity, full-disk transaction rollback,
corrupt-original recovery, corrupt-backup rejection, private path links, saturated
wait subscriptions, installer rollback, and hostile release archives. An
exhaustive small-state mutation test checks all subsets of four follows in both
deletion orders, including rollback, repeated erasure, and repeated retention.
End-to-end tests use temporary roots, explicit harness binaries, and local mock
providers; they never open a maintainer's real library.

Both Rust lockfiles and both npm lockfiles are checked for advisories. The GLib
fixed-advisory exception and the build-only `proc-macro-error` maintenance notice
are scoped and expire on 1 February 2027. Replace them with a supported compatible
GTK stack when available; do not broaden or silently renew the exceptions.

## Remaining release and account work

- Configure the Apple Developer ID and notarization credentials in the protected
  `release` environment as documented in [Mac signing](macos-signing.md). A real
  Apple acceptance and installed signed release have not been exercised without
  those credentials. Old v0.7.2 Mac downloads remain ad-hoc signed.
- Configure the crate's trusted publisher for this repository, `release.yml`, and
  environment `release`. The old Actions registry secret has been deleted; revoke
  the original token in the crates.io account as well.
- Authenticate the pinned Cloudflare CLI and deploy the website to activate HSTS.
  The source change alone does not change live response headers.
- Merge the reviewed change and publish a new version through the protected
  pipeline. Confirm signatures, provenance, package installation, and native
  notification behavior on the resulting release. Do not reuse the v0.7.2 tag.

Repository security controls were applied live. Dependabot alerts on the old
default-branch lockfiles remain until this patch merges; do not dismiss unresolved
findings merely because a branch contains the fix.

## Practical limits

OS WebKit/GTK packages, native notification delivery, account MFA, disk encryption,
and external backups remain outside Cargo/npm coverage. Keep supported operating
systems and native packages current. Same-user malicious programs can ordinarily
read the user's files. Erasure covers app-managed data and snapshots, not external
backups, filesystem snapshots, OS-retained notifications, or existing assistant
chats. Signing establishes release identity; it does not prove the absence of
bugs. Continuous advisory checks and failure testing remain necessary.
