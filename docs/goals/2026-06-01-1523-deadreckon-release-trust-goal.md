GOAL: Make DeadReckon official releases publish complete, signed, provenance-backed artifacts and fail closed when trust material is missing. The current cargo-dist pipeline can build archives, npm packages, Homebrew formulae, and guarded macOS notarization, but official releases still depend on operator discipline: tag shape is loose, signing can skip with warnings, checksums/provenance are not first-class, npm provenance is token-based, Windows signing is unresolved, and the runbook lacks exact secret wiring. Land a production-release hardening slice named Release Trust.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-06-01-1523-deadreckon-release-trust-rider.md` - phases, CI contract, manifest, signing.
- `/Users/gdc/deadreckon/.github/workflows/release.yml` - current cargo-dist, macOS signing, Homebrew, npm jobs.
- `/Users/gdc/deadreckon/.github/workflows/publish-npm.yml` - current token-based npm publishing.
- `/Users/gdc/deadreckon/dist-workspace.toml` - artifact matrix and installers.
- `/Users/gdc/deadreckon/docs/RELEASE.md` - public release runbook to harden.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-2343-deadreckon-distribute-rider.md` - distribution invariants.
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` and `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` - shipped posture and deferred work.
- Apple notarization, GitHub attestation, and npm provenance docs.

**Posture.** Production-release trust work. No runtime-state schema changes. No CLI redesign. No `git push`, tag push, or real package publish by the agent. Edits stay inside `/Users/gdc/deadreckon`. Forks/PRs stay usable without secrets; official RC/stable tags fail closed when release secrets or trust artifacts are missing.

**Release contract.**

- PRs and branches: plan only; no publishing, no signing secrets required.
- RC tags `vX.Y.Z-rc.N`: build artifacts, sign/notarize macOS, generate checksums, manifest, SBOM/provenance attestations, publish prerelease/draft, never stable package-manager channels.
- Stable tags `vX.Y.Z`: verify tag, versions, and CHANGELOG; full verify; build every target; require signing/provenance; publish GitHub Release, Homebrew, and npm.
- Official releases produce: archives/installers, `SHA256SUMS`, `release-manifest.json`, SBOM, GitHub attestations, npm provenance, Homebrew formula checksums, and verification commands.
- macOS signing/notarization is mandatory for official RC/stable releases. Windows signing is either implemented or stable Windows artifacts are blocked; never silently publish unsigned stable Windows binaries.

**Operator handoff.** Update `/Users/gdc/deadreckon/docs/RELEASE.md` with a secret-free checklist for exporting the Apple Developer ID Application certificate, base64 encoding it, creating an app-specific password, setting GitHub Actions secrets, running a local notarization smoke, and verifying CI artifacts. Do not commit certificates, passwords, team IDs, or local-only notes.

**Phases.** Eleven phases in the rider. Each: write depth tests first; implement the narrow workflow/doc/script change; keep fork dry-runs and official-tag fail-closed behavior separate; run focused tests plus release plan checks; update docs; commit locally when green. P11 updates AS-BUILT, RELEASE, V1-CANDIDATES if needed, and CHANGELOG.

**Verification.**

- Tests assert tag gating, version/changelog checks, and official-tag fail-closed signing/provenance behavior.
- `dist plan --output-format=json` or the repo's existing release-plan test remains green.
- `cargo test -p deadreckon release` and focused npm/Homebrew script tests pass.
- `cargo fmt --check`, `git diff --check`, and focused CI YAML checks are green.

**Stop when** official stable releases cannot publish unsigned or unattested artifacts, RC releases are safe to rehearse, docs include the Apple Developer ID wiring checklist, verification is green, and the work is committed locally.
