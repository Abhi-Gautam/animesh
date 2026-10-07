# Mac release signing

The current v0.7.1 downloads are ad-hoc signed and not notarized. This workflow
prepares future Developer ID releases; enabling it requires an Apple Developer
Program membership, a Developer ID Application certificate with its private
key, and a team App Store Connect API key permitted to use notarization.

In the repository’s Actions settings, add these **secrets**:

- `APPLE_CERTIFICATE_P12`: base64-encoded certificate and private key export.
- `APPLE_CERTIFICATE_PASSWORD`: the password protecting that export.
- `APPLE_NOTARY_KEY_P8`: the complete contents of the team API key’s `.p8` file.
- `APPLE_NOTARY_KEY_ID`: its key ID.
- `APPLE_NOTARY_ISSUER_ID`: its issuer ID.

Add the **variable** `APPLE_SIGNING_IDENTITY`, using the exact
`Developer ID Application: … (…)` certificate identity. Set the variable
`MACOS_SIGNING_ENABLED` to `true` only after all credentials are configured.
Do not put credentials in Git, issue comments, or chat. Certificate purchase,
account enrollment, and acceptance of Apple agreements are maintainer steps.

Both native Mac jobs import the certificate into an ephemeral keychain. The
CLI, nested desktop app, and outer app are signed inside-out with hardened
runtime and secure timestamps. The app is submitted to Apple, stapled, and
checked with Gatekeeper before packaging; the disk image is then signed,
notarized, stapled, and assessed too. Final checksums are written after stapling.
The tarball contains the already stapled app. A missing credential, mismatched
certificate, rejection, or timeout fails the build rather than falling back to
an unsigned download. Cleanup restores the runner’s keychain search list and
removes temporary credentials even when a build fails.

With signing disabled, source builds and CI smoke continue to use ad-hoc
signatures. Their install instructions remain explicit about notarization.
Do not describe a release as notarized until both Mac architecture jobs pass
Apple acceptance, ticket validation, and Gatekeeper assessment.

See [Apple’s notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow).
