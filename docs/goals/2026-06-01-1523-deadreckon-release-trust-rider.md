# DeadReckon - Release Trust Rider (signed artifacts, provenance, fail-closed publishing)

This rider holds the prescriptive constraints for the goal at `/Users/gdc/deadreckon/docs/goals/2026-06-01-1523-deadreckon-release-trust-goal.md`. It supersedes nothing in the 2026-05-11 distribution rider; that rider made DeadReckon installable and self-updatable. This rider hardens the official release path so published artifacts are complete, signed where the platform supports it, provenance-backed, and refused when trust material is missing.

**All paths absolute.** Source root is `/Users/gdc/deadreckon`. Runtime state under `/Users/gdc/.deadreckon` is not part of this goal. Release artifacts are created by CI under `target/distrib` and by local tests under tempdirs only.

## Posture (decided - do not redesign)

- **Maturity stays production-release track.** This is release trust hardening, not a new install channel or CLI UX redesign.
- **No DeadReckon durable runtime schema changes.** Do not change `PipelineState`, plans, campaigns, chains, library artifacts, run markers, or install receipts unless an existing release script already owns that file.
- **Official releases fail closed.** On the canonical repository, RC and stable tags must not publish if required signing/provenance/checksum steps fail or required secrets are absent.
- **Forks and PRs remain usable.** Pull requests, branches, and forked repos can run `dist plan` and non-publishing checks without Apple/npm/Homebrew/Windows secrets.
- **No agent-driven publishing.** The agent must not run `git tag`, `git push`, `gh release upload`, `npm publish`, or Homebrew tap pushes outside tests/dry-runs.
- **No new package manager channel.** Stay with cargo-dist installers, GitHub Releases, Homebrew, npm wrapper/platform packages, and the existing update channel model.
- **No secret material in git.** Docs may name secret keys and commands, but not actual values, certificates, passwords, or team identifiers.
- **Edits stay inside `/Users/gdc/deadreckon`.**

## Current baseline and gaps

Current files to inspect:

- `/Users/gdc/deadreckon/.github/workflows/release.yml`
  - Builds five cargo-dist targets.
  - Runs macOS codesign/notarytool before `dist build`.
  - Skips macOS signing with warnings if Apple secrets are missing.
  - Publishes GitHub Release, Homebrew formula, and npm packages on any tag.
- `/Users/gdc/deadreckon/.github/workflows/publish-npm.yml`
  - Uses `NPM_TOKEN`.
  - Repackages GitHub release artifacts into wrapper/platform packages.
  - Does not currently require npm trusted publishing or provenance.
- `/Users/gdc/deadreckon/dist-workspace.toml`
  - Declares the target matrix and cargo-dist installers.
- `/Users/gdc/deadreckon/docs/RELEASE.md`
  - Names Apple/npm/Homebrew secrets and says the operator pushes a tag.
  - Does not yet define RC vs stable behavior, fail-closed policy, provenance, or exact Apple Developer ID setup steps.

Important gap: signing before `dist build` is suspicious. If `dist build` rebuilds the binary after the manual signing step, the uploaded artifact may not be the signed binary. The implementation must prove the artifact inside each macOS archive is signed after packaging, or move signing to a position that signs the exact artifact contents.

## External primary references

Use current official docs while implementing:

- Apple Developer ID overview: `https://developer.apple.com/developer-id/`
- Apple notarization overview: `https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution`
- Apple command-line notarization workflow: `https://developer.apple.com/documentation/security/notarizing_macos_software_before_distribution/customizing_the_notarization_workflow`
- GitHub artifact attestations: `https://docs.github.com/en/actions/concepts/security/artifact-attestations`
- GitHub `actions/attest`: `https://github.com/actions/attest`
- npm trusted publishing: `https://docs.npmjs.com/trusted-publishers`
- npm provenance: `https://docs.npmjs.com/generating-provenance-statements`

Do not quote large blocks from those docs. Extract only command requirements and policy facts.

## Release lanes

### Branch and pull request

Purpose: cheap confidence and workflow validity.

Required:

- `dist plan --output-format=json`
- release workflow static checks;
- release script unit tests;
- no publishing jobs;
- no signing secret requirement;
- no GitHub Release mutation.

### RC tag

Tag shape: `vX.Y.Z-rc.N`

Purpose: rehearse the real release path without stable package-manager publication.

Required:

- validate tag shape;
- validate workspace version is `X.Y.Z` or a documented prerelease-compatible version policy;
- run full build/test/clippy/fmt or the release verification job;
- build all target artifacts;
- require macOS signing/notarization on the official repo;
- generate `SHA256SUMS`, `release-manifest.json`, SBOM, and GitHub attestations;
- publish a GitHub prerelease or draft only;
- do not update stable Homebrew tap;
- do not publish stable npm tags; if npm prerelease is implemented, publish under `next` only and document it.

### Stable tag

Tag shape: `vX.Y.Z`

Purpose: public stable release.

Required:

- validate tag shape;
- validate workspace/package versions exactly match `X.Y.Z`;
- validate `CHANGELOG.md` has a release section for `X.Y.Z`;
- run full release verification;
- build all target artifacts;
- require macOS signing/notarization on official repo;
- require Windows signing or block stable Windows artifact publication;
- generate checksums, manifest, SBOM, and GitHub attestations;
- publish GitHub Release;
- publish Homebrew formula only after GitHub Release artifacts exist and checksums match;
- publish npm packages with provenance or trusted publishing;
- run install smoke commands against the published artifacts where practical.

## Artifact contract

Every official RC/stable release must have:

- cargo-dist local artifacts for all configured targets;
- shell and PowerShell installers;
- Homebrew formula artifact;
- npm wrapper and platform package artifacts;
- `SHA256SUMS`;
- `release-manifest.json`;
- SBOM file, SPDX JSON or CycloneDX JSON;
- GitHub artifact attestations for release artifacts;
- npm provenance for published packages;
- release notes that identify signed/notarized platforms and any intentionally withheld platform.

## `release-manifest.json` schema

Generate this file in CI and attach it to the GitHub Release.

```json
{
  "schema_version": 1,
  "tag": "v0.1.0",
  "commit": "<git sha>",
  "generated_at": "2026-06-01T00:00:00Z",
  "repository": "gdc/deadreckon",
  "cargo_dist_version": "0.31.0",
  "artifacts": [
    {
      "name": "deadreckon-aarch64-apple-darwin.tar.xz",
      "target": "aarch64-apple-darwin",
      "kind": "archive",
      "sha256": "<hex>",
      "bytes": 123,
      "signed": true,
      "signature_kind": "apple-developer-id",
      "notarized": true,
      "attested": true,
      "sbom": "deadreckon.spdx.json"
    }
  ],
  "package_managers": {
    "homebrew": {
      "published": true,
      "tap": "gdc/homebrew-tap",
      "formula": "deadreckon.rb"
    },
    "npm": {
      "published": true,
      "provenance": true,
      "packages": [
        "deadreckon",
        "deadreckon-darwin-arm64"
      ]
    }
  }
}
```

The exact artifact names should match cargo-dist output at HEAD. The schema should be tested as data, not hand-waved in release notes.

## Secret model

### Required for official stable and RC macOS release

- `APPLE_CERT_P12`: base64-encoded Developer ID Application `.p12`.
- `APPLE_CERT_PWD`: password used when exporting the `.p12`.
- `APPLE_ID`: Apple Developer account email for notarytool.
- `APPLE_TEAM_ID`: Apple Developer Team ID.
- `APPLE_APP_PWD`: app-specific password for notarytool.

### Required for official package-manager publishing

- Homebrew: `HOMEBREW_TAP_TOKEN`, unless a GitHub App or fine-grained token flow replaces it.
- npm: either trusted publishing/OIDC configured for each package, or `NPM_TOKEN` plus `npm publish --provenance` if trusted publishing cannot be completed in this slice.

### Required for stable Windows artifacts

Choose one before stable Windows publication:

- Azure Trusted Signing with the required OIDC/tenant/account/profile secrets; or
- traditional code-signing certificate and `signtool`; or
- block Windows stable artifacts and document Windows as RC/unsigned-only until a separate signing path lands.

Do not silently publish unsigned stable Windows binaries.

## Apple Developer ID operator checklist

This section is safe for `/Users/gdc/deadreckon/docs/RELEASE.md` because it names secret slots but not values.

1. Confirm Apple Developer Program membership is active.
2. In Xcode, sign in with the Apple ID that owns the team.
3. Create or download a **Developer ID Application** certificate, not an Apple Development, Mac App Distribution, or ad hoc certificate.
4. Open Keychain Access and find the identity named like `Developer ID Application: <Name> (<TEAMID>)`.
5. Export the certificate plus private key as `.p12`.
6. Use a strong export password. This becomes `APPLE_CERT_PWD`.
7. Base64-encode the `.p12` without committing it:

   ```sh
   base64 -i developer-id.p12 | pbcopy
   ```

   On Linux:

   ```sh
   base64 -w 0 developer-id.p12
   ```

8. Add the copied value as GitHub Actions secret `APPLE_CERT_P12`.
9. Add the `.p12` export password as `APPLE_CERT_PWD`.
10. Find the Team ID in Apple Developer account membership details and add it as `APPLE_TEAM_ID`.
11. Add the Apple account email as `APPLE_ID`.
12. Create an app-specific password for the Apple ID and add it as `APPLE_APP_PWD`.
13. Locally verify before trusting CI:

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

14. Run an RC tag after CI hardening lands. Confirm CI extracts the macOS release archives and verifies the packaged binary with `codesign --verify` and a notarization assessment.

Notes:

- Use `notarytool`, not `altool`.
- Use hardened runtime and timestamp when signing.
- For a raw CLI binary, do not promise stapling unless the packaged artifact supports it. Gatekeeper can use Apple's online notarization ticket.
- Store `.p12` only in the keychain and GitHub Actions secret store. Do not leave it in the repo or a shared directory.

## CI design

### Workflow permissions

Publishing and attestation jobs need explicit permissions:

```yaml
permissions:
  contents: write
  id-token: write
  attestations: write
  artifact-metadata: write
```

Keep minimum permissions on non-publishing jobs. If a job only runs plan checks, it should not have write permissions.

### Tag gate job

Add a first-class job that emits normalized release lane metadata:

```json
{
  "lane": "stable",
  "tag": "v0.1.0",
  "version": "0.1.0",
  "official_repo": true,
  "publishes": true,
  "requires_macos_signing": true,
  "requires_windows_signing": true,
  "requires_attestation": true
}
```

Every publish/signing job should read this metadata instead of reimplementing `startsWith(github.ref, 'refs/tags/')` differently.

### Secret preflight

On official RC/stable tags, fail early if required secrets are missing. On forks and non-tag builds, report skipped trust checks without failing.

Do not let the macOS signing step `exit 0` on official tags when Apple secrets are absent.

### Mac artifact signing and verification

The workflow must prove the uploaded artifact contains a signed binary:

1. Build/package artifact.
2. Sign the exact binary that will be archived, or sign via cargo-dist's supported signing hook if available.
3. Notarize an accepted zip/package containing that binary.
4. Archive/upload the signed binary.
5. Download or inspect `target/distrib` artifact.
6. Extract archive.
7. Run `codesign --verify --verbose <extracted deadreckon>`.
8. Run an assessment command where reliable for command-line tools.
9. Record signed/notarized status in `release-manifest.json`.

If cargo-dist rebuilds after manual signing, the phase must fail until the signing position is corrected.

### Checksums and manifest

Generate `SHA256SUMS` from exactly the files uploaded to the release. Generate `release-manifest.json` from the same file list. Test that every release artifact has a manifest entry and every manifest entry has a matching file.

### Attestations

Use `actions/attest@v4` or current GitHub-recommended equivalent for release artifacts. Attest the files in `target/distrib`, including `SHA256SUMS`, `release-manifest.json`, installers, and archives. Add release notes showing users how to run:

```sh
gh attestation verify <artifact> --repo gdc/deadreckon
```

### npm provenance

Preferred path: configure npm trusted publishing for `deadreckon` and each platform package, then remove long-lived token dependency for official publishing. If that is too much for this slice, require `npm publish --provenance` and document token rotation, then log trusted publishing as a V1 or immediate follow-up.

The workflow must publish platform packages first, then wrapper last, as it does today.

### Homebrew

Homebrew formula publication must happen only after GitHub Release artifacts and checksums exist. The formula checksum must match the uploaded artifact. The tap commit should identify the release tag and should be made by the release bot identity.

## Phases (eleven)

Each phase: write the named depth tests first and watch them fail; implement; run focused tests; run `cargo fmt --check` and `git diff --check`; commit locally at sensible boundaries. Use local script tests and YAML/static checks whenever a live external publish would otherwise be required.

### P1 - Release trust inventory and test harness

- Inventory current release jobs, scripts, artifacts, and docs.
- Add local fixtures for branch, fork, RC tag, stable tag, missing secrets, and present secrets.
- Add a test helper that evaluates release lane metadata without invoking GitHub.

Depth tests:

- `release_lane_classifies_branch_rc_and_stable_tags`
- `official_release_requires_trust_material`
- `fork_release_plan_does_not_require_private_secrets`

### P2 - Tag, version, and changelog gate

- Add a deterministic tag gate script or Rust testable helper.
- Accept only `vX.Y.Z` and `vX.Y.Z-rc.N` for publish lanes.
- Verify stable tag matches workspace package versions and npm package versions.
- Verify stable tag has a `CHANGELOG.md` section.

Depth tests:

- `stable_tag_must_match_workspace_version`
- `rc_tag_uses_prerelease_lane_without_stable_publish`
- `stable_tag_requires_changelog_entry`
- `invalid_tag_never_publishes`

### P3 - Fail-closed official release preflight

- Replace warning-only secret skips with lane-aware behavior.
- Official RC/stable: missing Apple signing secrets fail before artifact publication.
- Fork/branch/PR: missing secrets produce non-fatal skipped status.
- Add a single preflight summary that names missing secret keys without printing values.

Depth tests:

- `official_stable_missing_apple_secret_fails`
- `official_rc_missing_apple_secret_fails`
- `fork_tag_missing_apple_secret_skips_without_publish`
- `preflight_never_logs_secret_values`

### P4 - macOS signing and notarization correctness

- Prove whether current signing position signs the actual release artifact.
- If not, move signing to a supported cargo-dist hook or post-build artifact step.
- Verify both `aarch64-apple-darwin` and `x86_64-apple-darwin` release artifacts after extraction.
- Capture signed/notarized booleans into the manifest.

Depth tests:

- `macos_release_artifact_contains_codesigned_binary`
- `macos_artifact_manifest_records_notarization`
- `macos_signing_step_fails_closed_on_official_tags`
- `macos_signing_step_skips_only_on_non_official_dry_runs`

### P5 - Checksums and release manifest

- Generate `SHA256SUMS` from final upload files.
- Generate `release-manifest.json`.
- Assert every artifact is listed once and every listed checksum matches bytes on disk.
- Include release lane, commit, cargo-dist version, target, kind, size, signing, attestation, and package-manager status.

Depth tests:

- `release_manifest_covers_every_distrib_artifact`
- `release_manifest_checksums_match_sha256sums`
- `release_manifest_records_signing_and_attestation_status`
- `release_manifest_schema_is_stable`

### P6 - SBOM and GitHub attestations

- Generate an SBOM for the release. Prefer a maintained Rust-compatible generator that can run in CI without service credentials.
- Add GitHub artifact attestations for release outputs.
- Add a verification note to release docs.
- Keep branch/fork behavior safe.

Depth tests:

- `release_sbom_is_generated_for_official_tags`
- `attestation_job_has_oidc_and_attestation_permissions`
- `attestation_subjects_match_release_manifest_artifacts`
- `release_docs_include_gh_attestation_verify_command`

### P7 - npm provenance and trusted publishing path

- Prefer npm trusted publishing/OIDC for the wrapper and platform packages.
- If trusted publishing cannot be fully configured in code alone, add `npm publish --provenance`, document the operator-side trusted-publisher setup, and log the remaining migration.
- Ensure wrapper publish remains last.
- Ensure npm package versions match the release tag.

Depth tests:

- `npm_publish_uses_provenance_or_trusted_publishing`
- `npm_wrapper_publishes_after_platform_packages`
- `npm_package_versions_match_stable_tag`
- `npm_rc_does_not_publish_stable_latest`

### P8 - Windows signing decision gate

- Add a stable-release gate for Windows signing.
- If a signing provider is configured, sign the Windows executable and verify it before artifact upload.
- If not configured, block stable Windows artifacts or mark Windows stable publication as unavailable before upload.
- Do not leave "unsigned but stable" as the default.

Depth tests:

- `stable_windows_artifact_requires_signing_or_is_withheld`
- `windows_unsigned_artifact_can_exist_only_on_rc_or_dry_run`
- `windows_signing_policy_is_documented_in_release_manifest`

### P9 - Homebrew formula integrity

- Ensure formula checksums match GitHub Release artifacts.
- Ensure formula publish waits for GitHub Release publication.
- Ensure tap commit message and bot identity are stable.
- Keep missing `HOMEBREW_TAP_TOKEN` fail-closed for official stable releases.

Depth tests:

- `homebrew_formula_sha_matches_release_artifact`
- `homebrew_publish_requires_release_artifacts`
- `official_stable_missing_homebrew_token_fails`
- `rc_release_does_not_publish_stable_homebrew_formula`

### P10 - End-to-end release rehearsal

- Add a no-publish release rehearsal command or documented workflow dispatch path.
- Rehearse RC behavior without mutating package managers.
- Validate install commands from generated artifacts where practical.
- Confirm the generated release notes explain checksums, signing, notarization, and attestation verification.

Depth tests:

- `release_rehearsal_builds_manifest_without_publish`
- `rc_release_notes_mark_prerelease_and_trust_status`
- `stable_release_notes_include_install_and_verify_commands`
- `published_artifact_smoke_uses_release_archive_not_target_binary`

### P11 - Docs, architecture, and changelog

- Update `/Users/gdc/deadreckon/docs/RELEASE.md` with:
  - branch/RC/stable lanes;
  - fail-closed policy;
  - exact Apple Developer ID operator checklist;
  - npm trusted publishing/provenance setup;
  - Homebrew token setup;
  - Windows signing policy;
  - artifact verification commands.
- Update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` release packaging section.
- Update `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` for any deferred release trust items.
- Append a `Release trust (production release)` section to `/Users/gdc/deadreckon/CHANGELOG.md`.

No depth test required for P11, but docs must cite the tests or scripts that enforce the release contract.

## Verification commands

Run before final commit:

```sh
cargo test -p deadreckon release
cargo test -p deadreckon npm
cargo test -p deadreckon update
dist plan --output-format=json
cargo fmt --check
git diff --check
```

If `dist` is unavailable locally, document that and rely on the existing release-plan test plus CI static checks. Do not fake a passing dist plan.

## Stop conditions

Stop only when all are true:

- Official stable releases cannot publish if macOS signing/notarization is missing.
- Windows stable artifact policy is fail-closed: signed or withheld.
- Checksums, manifest, SBOM, and attestations are first-class release artifacts.
- npm packages publish with provenance or the docs and V1 log clearly identify the remaining trusted-publishing operator step.
- Homebrew formula integrity is checked against release artifacts.
- `docs/RELEASE.md` includes a precise Apple Developer ID setup checklist without secrets.
- Forks/PRs can still run release-plan checks without private secrets.
- Verification is green or any unavailable external command is documented with the exact reason.
- Work is committed locally and not pushed.
