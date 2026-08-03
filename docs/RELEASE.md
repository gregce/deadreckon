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
| Stable | `vX.Y.Z` | Requires tag/version/changelog agreement, full verify, enabled official-repo trust material, release artifacts, checksums, manifest, SBOM, GitHub attestations, and Homebrew integrity. npm publication and Windows Authenticode may be explicitly deferred by the checked-in release policy. |

Windows artifacts require Authenticode signing whenever
`windowsSigningDeferred` is false. The initial `v0.8.0` lane deliberately keeps
that flag true until a certificate is configured, so its Windows archive is
unsigned but remains covered by checksums and GitHub attestations. This is an
explicit release-scope decision, not a missing-secret fallback.

## Official Artifact Contract

Every official RC or stable release must produce:

- cargo-dist archives and shell/PowerShell installers. Every host archive must
  contain the host-native `deadreckon`, `dr-gate`, and `dr-capture` binaries
  plus both static Linux evaluator sidecars:
  `dr-gate-evaluator-aarch64-unknown-linux-musl` and
  `dr-gate-evaluator-x86_64-unknown-linux-musl`;
- the Homebrew formula artifact;
- npm wrapper and platform packages when npm publication is enabled for the
  stable lane;
- `SHA256SUMS`;
- `release-manifest.json`;
- `release-archive-members.json`, which records every final archive member and
  the common evaluator-sidecar digests;
- `release.spdx.json`;
- GitHub artifact attestations;
- npm provenance whenever npm packages are published;
- release notes or runbook commands that show verification steps.

`cargo-dist` plans and builds artifacts but never creates or updates the GitHub
Release. After the trust bundle and attestations succeed, one final write-scoped
job publishes the release with `gh` and includes the checksum and attestation
verification commands in the release notes.

Users should be able to verify a downloaded artifact with:

```sh
shasum -a 256 -c SHA256SUMS
gh attestation verify <artifact> --repo gregce/deadreckon
```

Release CI builds the two evaluator sidecars as static musl ELF binaries for
Linux arm64 and x86-64. It validates their architecture and lack of a dynamic
interpreter, then inserts the identical pair into every host archive before
signing, checksums, manifests, attestations, package-manager preparation, and
upload. `release/evaluator-sidecars.mjs` fails closed when a helper is missing,
duplicated, unsafe to extract, dynamically linked, for the wrong architecture,
or differs between archives.

For macOS artifacts, CI extracts the complete assembled archive, signs the
host-native `deadreckon`, `dr-gate`, and `dr-capture` binaries, verifies each
with `codesign --strict`, submits the complete payload with `notarytool`, then
repacks the archive that will be checksummed and uploaded. The Linux evaluator
sidecars are not Apple-signed executables, but they are members of the
notarized, checksummed and attested payload.

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

Official stable releases fail if Windows signing is enabled and signing
material is absent. Windows Authenticode is explicitly deferred for `v0.8.0`.

| Secret | Purpose |
| --- | --- |
| `WINDOWS_CERT_PFX` | Base64-encoded Authenticode code-signing `.pfx` export. |
| `WINDOWS_CERT_PWD` | Password for the `.pfx` export. |

The Windows job decodes the `.pfx`, extracts the complete assembled cargo-dist
Windows zip, signs `deadreckon.exe`, `dr-gate.exe`, and `dr-capture.exe` with
`signtool sign`, verifies each with `signtool verify`, then repacks the zip and
records Authenticode status in `release-manifest.json`.

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
rights to `gregce/homebrew-tap`. The workflow verifies that the generated formula
checksum matches `SHA256SUMS` before committing the formula.

When enabled, npm stable publication prefers npm trusted publishing. Configure trusted
publishing in npm for `deadreckon` and each platform package, then set the
repository variable `NPM_TRUSTED_PUBLISHING=true`. If trusted publishing is not
available yet, set `NPM_TOKEN`; the workflow still publishes every package with
`npm publish --provenance`. Platform packages publish first and the wrapper
publishes last. The npm workflow validates the tag through the release trust
gate and refuses manual dispatches that are not official stable release tags.

The release workflows require `id-token: write` for GitHub artifact
attestations and npm provenance.

## Stable operator checklist

Everything the operator confirms before the narrowed `v0.8.0` stable release:

1. Work from a clean revision whose branch CI and release-plan workflow pass.
2. Confirm the `gregce/homebrew-tap` repository is reachable and
   `HOMEBREW_TAP_TOKEN` still has push rights.
3. Confirm the checked-in release policy still deliberately defers npm
   publication and Windows Authenticode. The Windows archive will be unsigned
   but checksummed and attested; no npm package is published by this tag.
4. Set the Rust workspace, the wrapper in `npm/deadreckon/package.json` and all
   npm optional dependency pins to the exact stable version, update
   `Cargo.lock`, and confirm `CHANGELOG.md` has the matching release section.
5. Run `make build`, then `release/preflight-real.sh`, and commit the refreshed
   `release/known-good-providers.json`. The recorded `deadreckon_version` must
   equal the stable tag version.
6. Run the focused checks and the complete verification suite, then validate
   the exact stable ref through `release/trust/release-trust.mjs`.
7. Rehearse the release through an RC and verify its archives, installers,
   checksums, manifest, archive-member inventory, SBOM, signing/notarization
   evidence and attestations.
8. Create and push the stable tag only after the operator explicitly approves
   that exact tag. Automation never invents or advances a release tag.

To widen a later release, configure npm trusted publishing (or `NPM_TOKEN`) and
the two Windows signing secrets, then set `npmPublishingDeferred` and
`windowsSigningDeferred` to false in `release/trust/release-trust.mjs`. The
existing fail-closed preflight and publication jobs become mandatory again.

## Operator Release Flow

1. Confirm the target tag is valid:
   - stable: `vX.Y.Z`;
   - RC: `vX.Y.Z-rc.N`.
2. Confirm version agreement:
   - for an RC, `Cargo.toml` and `Cargo.lock` carry the full prerelease version
     (`X.Y.Z-rc.N`), while the unpublished npm wrapper and its five pins carry
     the future stable base version (`X.Y.Z`);
   - for stable, the Rust workspace, lockfile, npm wrapper and all five pins
     carry exactly `X.Y.Z`.
3. For stable releases, confirm `CHANGELOG.md` has a section for `X.Y.Z`.
4. Confirm Apple and Homebrew trust material are configured. Confirm npm and
   Windows are either enabled with their trust material or explicitly deferred
   by the checked-in policy.
5. Run focused local checks:

   ```sh
   cargo test -p deadreckon --test release_plan
   cargo test -p deadreckon --test npm_wrapper
   cargo fmt --all -- --check
   git diff --check
   ```

6. If `cargo-dist` is installed locally, also run:

   ```sh
   dist plan --output-format=json
   ```

7. For a stable cut, run the real-provider proof harness (operator-only;
   it refuses under CI and consumes a few real provider turns per route).
   Subscription CLI routes record no metered DeadReckon spend, but still use
   provider quota and wall-clock time:

   ```sh
   make build
   release/preflight-real.sh            # cli:claude-code cli:codex
   release/preflight-real.sh cli:gemini # extend by argument
   ```

   For each route the harness proves one verified delivery and one
   identity-bound cancellation with no surviving provider process. On success
   it records the probed binary versions in
   `release/known-good-providers.json` (schema_version 1); commit that file
   so the release notes can reference known-good CLI versions.
8. Create and push an RC tag first. Review the GitHub Actions run, artifacts,
   `SHA256SUMS`, `release-manifest.json`, `release-archive-members.json`,
   `release.spdx.json`, macOS signing evidence, and attestations. Confirm every
   archive contains exactly one host-native `deadreckon`, `dr-gate`, and
   `dr-capture`, plus exactly one copy of each static evaluator sidecar.
9. After the RC rehearsal is clean, create and push the stable tag.

Every release tag remains an operator action.

### Windows smoke (operator checklist, when Authenticode is enabled)

On a Windows machine or VM, before announcing a stable release:

1. Download the signed `deadreckon-x86_64-pc-windows-msvc.zip` from the
   release page and run `signtool verify /pa deadreckon.exe` after unzip.
2. Run `deadreckon.exe --version` and confirm it matches the tag.
3. Run `deadreckon.exe doctor` and confirm no failed checks.
4. Run `deadreckon.exe try --sandbox none` in a scratch git repo.
5. Record the result in `release/known-good-providers.json` under route
   `windows-smoke` with the Windows build used as `binary_version`.

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
`SHA256SUMS` attached. The wrapper refuses a completed installation unless all
three host-native binaries and both evaluator sidecars are executable beside
`deadreckon`. For a pinned rehearsal:

```sh
curl -fsSL https://deadreckon.sh/install.sh | DEADRECKON_TAG=<tag-under-test> sh
```
