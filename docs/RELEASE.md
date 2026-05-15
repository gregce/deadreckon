# Release Setup

deadreckon releases are built by GitHub Actions through `cargo-dist`.
Pushing a version tag starts the release workflow; ordinary pushes and pull
requests run the `dist plan` check without publishing anything.

## Required Apple Secrets

macOS artifacts are signed and notarized when the repository has these GitHub
Actions secrets configured:

| Secret | Purpose |
| --- | --- |
| `APPLE_CERT_P12` | Base64-encoded Developer ID Application `.p12` certificate export. |
| `APPLE_CERT_PWD` | Password used when exporting the `.p12` certificate. |
| `APPLE_ID` | Apple Developer account email used by `notarytool`. |
| `APPLE_TEAM_ID` | Apple Developer Team ID. |
| `APPLE_APP_PWD` | App-specific password for the Apple ID used by `notarytool`. |

If `APPLE_CERT_P12` is not present, the workflow skips macOS signing and
notarization with a warning. That keeps forks and dry-runs usable, but release
notes for any unsigned macOS artifact must call out that it is unsigned.

## Release Flow

1. Confirm `cargo build --release`, `cargo test --workspace`, `cargo clippy
   --workspace -- -D warnings`, and `cargo fmt --check` are green.
2. Confirm the Apple secrets above are configured before creating a public
   macOS release.
3. Create the release tag locally.
4. Push the tag to GitHub. The release workflow builds five target artifacts,
   signs and notarizes the two macOS binaries when secrets are present, builds
   shell and PowerShell installers, and publishes the GitHub release.

The agent should not push tags. The first real release remains an operator
action.
