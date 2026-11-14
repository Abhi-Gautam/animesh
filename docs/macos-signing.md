# Mac release signing

Mac downloads ship ad-hoc signed and not notarized. The packaging runner
verifies the ad-hoc seal before packaging, and the install text tells users to
approve the app once in Privacy & Security or install with Homebrew.

Developer ID releases are optional and prepared below. Enabling them requires an
Apple Developer Program membership, a Developer ID Application certificate with
its private key, and a team App Store Connect API key permitted to use
notarization.

In the protected `release` environment in Actions settings, add these **secrets**:

- `APPLE_CERTIFICATE_P12`: base64-encoded certificate and private key export.
- `APPLE_CERTIFICATE_PASSWORD`: the password protecting that export.
- `APPLE_NOTARY_KEY_P8`: the complete contents of the team API key’s `.p8` file.
- `APPLE_NOTARY_KEY_ID`: its key ID.
- `APPLE_NOTARY_ISSUER_ID`: its issuer ID.

Add the **variable** `APPLE_SIGNING_IDENTITY`, using the exact
`Developer ID Application: … (…)` certificate identity. That variable is the
switch: once it is set, every secret above is required and a missing or rejected
credential fails the release instead of falling back to ad-hoc signing.
Do not put credentials in Git, issue comments, or chat. Certificate purchase,
account enrollment, and acceptance of Apple agreements are maintainer steps.

Compilation and dependency scripts run on unprivileged builders. Separate clean packaging runners download bounded, path-validated archives and import the certificate into an ephemeral keychain; no compilation or package scripts run with signing credentials. The
CLI, nested desktop app, and outer app are signed inside-out with hardened
runtime and secure timestamps. The app is submitted to Apple, stapled, and
checked with Gatekeeper before packaging; the disk image is then signed,
notarized, stapled, and assessed too. Final checksums are written after stapling.
The tarball contains the already stapled app. A missing credential, mismatched
certificate, rejection, or timeout fails the build rather than falling back to
an unsigned download. Cleanup restores the runner’s keychain search list and
removes temporary credentials even when a build fails.

Local source builds and CI smoke continue to use ad-hoc signatures. Public Mac packaging refuses that mode.
Do not describe a release as notarized until both Mac architecture jobs pass
Apple acceptance, ticket validation, and Gatekeeper assessment.

See [Apple’s notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow).

Crates.io publishing uses short-lived OIDC credentials. Configure a trusted publisher for `Abhi-Gautam/animesh`, workflow `release.yml`, environment `release`, in the crate's settings. Revoke the old registry token after this is configured; do not add a long-lived registry token to Actions. Every release tag must point to a commit on protected `master`. The protected release environment requires owner approval before credentials are released.
