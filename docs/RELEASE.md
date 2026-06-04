# Release Setup

DeadReckon releases are built by GitHub Actions through `cargo-dist` plus the
release trust helpers under `release/trust/`. The agent may edit and test the
release pipeline, but creating tags, pushing tags, publishing packages, and
pushing the Homebrew tap remain operator actions.

## Release Lanes

| Lane | Trigger | Behavior |
| --- | --- | --- |
| Branch or PR | non-tag push or pull request | Runs `dist plan --output-format=json` and static release-policy checks only. No private secrets are required and nothing publishes. |
| RC | `vX.Y.Z-rc.N` | Builds release artifacts, requires official-repo macOS signing/notarization, generates `SHA256SUMS`, `release-manifest.json`, `release.spdx.json`, and GitHub attestations, then publishes a GitHub prerelease. It does not publish npm or Homebrew stable channels. |
| Stable | `vX.Y.Z` | Requires tag/version/changelog agreement, full verify, official-repo trust material, release artifacts, checksums, manifest, SBOM, GitHub attestations, npm provenance, and Homebrew integrity. |

Stable Windows artifacts require Authenticode signing. If the Windows signing
secrets are absent, the stable release fails before any unsigned Windows artifact
can upload. RC and dry-run builds may still exercise the Windows target without
stable publication.

## Official Artifact Contract

Every official RC or stable release must produce:

- cargo-dist archives and shell/PowerShell installers;
- the Homebrew formula artifact;
- npm wrapper and platform packages when the lane is stable;
- `SHA256SUMS`;
- `release-manifest.json`;
- `release.spdx.json`;
- GitHub artifact attestations;
- npm provenance for published npm packages;
- release notes or runbook commands that show verification steps.

The workflow uploads the trust bundle explicitly after `cargo-dist` creates the
GitHub Release, then appends the checksum and attestation verification commands
to the release notes.

Users should be able to verify a downloaded artifact with:

```sh
shasum -a 256 -c SHA256SUMS
gh attestation verify <artifact> --repo gregce/deadreckon
```

For macOS artifacts, CI extracts the cargo-dist archive, signs the packaged
`deadreckon` binary, verifies it with `codesign`, submits it with `notarytool`,
then repacks the archive that will be uploaded.

## Required Apple Secrets

Official RC and stable releases fail if any macOS signing secret is absent.
Forks, PRs, and branch dry-runs do not require these secrets.

| Secret | Purpose |
| --- | --- |
| `APPLE_CERT_P12` | Base64-encoded Developer ID Application `.p12` certificate export. |
| `APPLE_CERT_PWD` | Password used when exporting the `.p12` certificate. |
| `APPLE_ID` | Apple Developer account email used by `notarytool`. |
| `APPLE_TEAM_ID` | Apple Developer Team ID. |
| `APPLE_APP_PWD` | App-specific password for the Apple ID used by `notarytool`. |

## Required Windows Secrets

Official stable releases fail if Windows signing material is absent.

| Secret | Purpose |
| --- | --- |
| `WINDOWS_CERT_PFX` | Base64-encoded Authenticode code-signing `.pfx` export. |
| `WINDOWS_CERT_PWD` | Password for the `.pfx` export. |

The Windows job decodes the `.pfx`, extracts the cargo-dist Windows zip, signs
`deadreckon.exe` with `signtool sign`, verifies it with `signtool verify`, then
repacks the zip and records Authenticode status in `release-manifest.json`.

## Apple Developer ID Checklist

1. Confirm the Apple account is enrolled in the Apple Developer Program.
2. In Keychain Access, create a certificate signing request for code signing.
3. In the Apple Developer portal, create a `Developer ID Application`
   certificate for macOS distribution.
4. Download and install the certificate into the login keychain.
5. Export the certificate and private key as a password-protected `.p12`.
6. Base64 encode the `.p12` without line wrapping.

   ```sh
   base64 -i Certificates.p12 | tr -d '\n'
   ```

7. Store the encoded value as the GitHub Actions secret `APPLE_CERT_P12`.
8. Store the `.p12` export password as `APPLE_CERT_PWD`.
9. Create an Apple app-specific password for the Apple ID used by CI.
10. Store the Apple ID email as `APPLE_ID`.
11. Store the Team ID as `APPLE_TEAM_ID`.
12. Store the app-specific password as `APPLE_APP_PWD`.
13. Confirm no `.p12`, `.pfx`, `.cer`, `.pem`, signing request, password, or Team ID
    material is committed to git.
14. Run a local smoke on a Mac before the first RC:

    ```sh
    cargo build --release -p deadreckon
    codesign --sign "Developer ID Application" --options runtime --timestamp target/release/deadreckon
    codesign --verify --verbose target/release/deadreckon
    ditto -c -k --keepParent target/release/deadreckon /tmp/deadreckon.zip
    xcrun notarytool submit /tmp/deadreckon.zip \
      --apple-id "$APPLE_ID" \
      --team-id "$APPLE_TEAM_ID" \
      --password "$APPLE_APP_PWD" \
      --wait
    ```

15. After CI hardening is merged, run an RC tag and confirm the workflow verifies
    the extracted packaged macOS archives, not only a loose local binary.

## Package Manager Trust

Homebrew stable publication requires `HOMEBREW_TAP_TOKEN`, a token with push
rights to `gdc/homebrew-tap`. The workflow verifies that the generated formula
checksum matches `SHA256SUMS` before committing the formula.

npm stable publication prefers npm trusted publishing. Configure trusted
publishing in npm for `deadreckon` and each platform package, then set the
repository variable `NPM_TRUSTED_PUBLISHING=true`. If trusted publishing is not
available yet, set `NPM_TOKEN`; the workflow still publishes every package with
`npm publish --provenance`. Platform packages publish first and the wrapper
publishes last. The npm workflow validates the tag through the release trust
gate and refuses manual dispatches that are not official stable release tags.

The release workflows require `id-token: write` for GitHub artifact
attestations and npm provenance.

## Operator Release Flow

1. Confirm the target tag is valid:
   - stable: `vX.Y.Z`;
   - RC: `vX.Y.Z-rc.N`.
2. Confirm `Cargo.toml` workspace version and `npm/deadreckon/package.json`
   match the tag base version.
3. For stable releases, confirm `CHANGELOG.md` has a section for `X.Y.Z`.
4. Confirm Apple, Homebrew, and npm trust material are configured.
5. Confirm Windows Authenticode signing secrets are configured for stable tags.
6. Run focused local checks:

   ```sh
   cargo test -p deadreckon --test release_plan
   cargo test -p deadreckon --test npm_wrapper
   cargo fmt --check
   git diff --check
   ```

7. If `cargo-dist` is installed locally, also run:

   ```sh
   dist plan --output-format=json
   ```

8. Create and push an RC tag first. Review the GitHub Actions run, artifacts,
   `SHA256SUMS`, `release-manifest.json`, `release.spdx.json`, macOS signing
   evidence, and attestations.
9. After the RC rehearsal is clean, create and push the stable tag.

The first real release remains an operator action.

## Site Installer

The website can host a thin bootstrap script at `https://deadreckon.sh/install.sh`.
That script downloads `deadreckon-installer.sh` from the GitHub Release selected
by `DEADRECKON_TAG` and then delegates to the cargo-dist installer. For the first
RC, the intended public command is:

```sh
curl -fsSL https://deadreckon.sh/install.sh | sh
deadreckon try
deadreckon doctor
```

Before announcing the command, confirm that the site script's default tag points
at the published RC and that the release has `deadreckon-installer.sh` plus
`SHA256SUMS` attached. For a pinned rehearsal:

```sh
curl -fsSL https://deadreckon.sh/install.sh | DEADRECKON_TAG=v0.1.0-rc.1 sh
```
