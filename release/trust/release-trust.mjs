#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const OFFICIAL_REPO = "gregce/deadreckon";
const TARGETS = [
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "aarch64-unknown-linux-gnu",
  "x86_64-unknown-linux-gnu",
  "x86_64-pc-windows-msvc",
];
const REQUIRED_HOMEBREW_BINARIES = [
  "deadreckon",
  "dr-gate",
  "dr-capture",
  "dr-gate-evaluator-aarch64-unknown-linux-musl",
  "dr-gate-evaluator-x86_64-unknown-linux-musl",
];

const command = process.argv[2];
const args = parseArgs(process.argv.slice(3));

try {
  switch (command) {
    case "lane":
      writeJson(classifyRelease(args));
      break;
    case "validate":
      validateRelease(args);
      break;
    case "preflight":
      preflight(args);
      break;
    case "source-bundle-id": {
      const bundleBuildId = sourceBundleBuildId(args.root ?? ".");
      if (args.raw) {
        process.stdout.write(`${bundleBuildId}\n`);
      } else {
        writeJson({ schema_version: 1, bundle_build_id: bundleBuildId });
      }
      break;
    }
    case "bundle-proof":
      writeJson(buildBundleProof(args));
      break;
    case "verify-provider-proof":
      writeJson(validateProviderProof(args));
      break;
    case "sbom":
      writeSbom(args);
      break;
    case "checksums":
      writeChecksums(args);
      break;
    case "manifest":
      writeManifest(args);
      break;
    case "verify-manifest":
      verifyManifest(args);
      break;
    case "verify-homebrew":
      verifyHomebrew(args);
      break;
    default:
      throw new Error(
        "usage: release-trust.mjs <lane|validate|preflight|source-bundle-id|bundle-proof|verify-provider-proof|sbom|checksums|manifest|verify-manifest|verify-homebrew>",
      );
  }
} catch (error) {
  console.error(error.message);
  process.exit(1);
}

function parseArgs(values) {
  const parsed = {};
  for (let index = 0; index < values.length; index += 1) {
    const value = values[index];
    if (!value.startsWith("--")) {
      continue;
    }
    const withoutPrefix = value.slice(2);
    const equals = withoutPrefix.indexOf("=");
    if (equals !== -1) {
      parsed[withoutPrefix.slice(0, equals)] = withoutPrefix.slice(equals + 1);
      continue;
    }
    const next = values[index + 1];
    if (next && !next.startsWith("--")) {
      parsed[withoutPrefix] = next;
      index += 1;
    } else {
      parsed[withoutPrefix] = true;
    }
  }
  return parsed;
}

function option(name, envName, fallback = "") {
  return args[name] ?? process.env[envName] ?? fallback;
}

function classifyRelease(localArgs = args) {
  const ref = localArgs.ref ?? process.env.GITHUB_REF ?? "";
  const repo = localArgs.repo ?? process.env.GITHUB_REPOSITORY ?? "";
  const officialRepo = localArgs["official-repo"] ?? process.env.DEADRECKON_OFFICIAL_REPO ?? OFFICIAL_REPO;
  const isTag = ref.startsWith("refs/tags/");
  const tag = isTag ? ref.slice("refs/tags/".length) : (localArgs["ref-name"] ?? process.env.GITHUB_REF_NAME ?? "");
  const stable = /^v(\d+)\.(\d+)\.(\d+)$/.exec(tag);
  const rc = /^v(\d+)\.(\d+)\.(\d+)-rc\.(\d+)$/.exec(tag);
  let lane = "branch";
  let version = null;
  let rcNumber = null;
  if (isTag && stable) {
    lane = "stable";
    version = `${stable[1]}.${stable[2]}.${stable[3]}`;
  } else if (isTag && rc) {
    lane = "rc";
    // cargo-dist binds the git tag to the package version exactly, so an rc
    // release requires Cargo.toml to carry the full prerelease string
    // (e.g. 0.1.0-rc.1). Keep version in lockstep with the tag rather than
    // stripping the suffix, so `validate` and `dist host` agree.
    version = `${rc[1]}.${rc[2]}.${rc[3]}-rc.${rc[4]}`;
    rcNumber = Number.parseInt(rc[4], 10);
  } else if (isTag) {
    lane = "invalid_tag";
  }

  const official_repo = repo === officialRepo;
  const releaseLane = lane === "stable" || lane === "rc";
  const officialRelease = official_repo && releaseLane;
  // v0.8.x lane narrowing (operator decision, 2026-08-02): publish the
  // checksummed/attested GitHub payload and Homebrew formula now. npm waits on
  // trusted-publisher setup; Windows Authenticode waits on a certificate. Flip
  // either flag to false only after its corresponding trust material exists.
  const npmPublishingDeferred = true;
  const windowsSigningDeferred = true;
  return {
    schema_version: 1,
    lane,
    tag: releaseLane ? tag : null,
    version,
    rc_number: rcNumber,
    ref,
    repository: repo,
    official_repo,
    build_artifacts: releaseLane,
    publishes: officialRelease,
    publish_github_release: officialRelease,
    publish_homebrew: official_repo && lane === "stable",
    publish_npm: official_repo && lane === "stable" && !npmPublishingDeferred,
    release_notes_mode: lane === "rc" ? "prerelease" : lane === "stable" ? "stable" : "none",
    requires_macos_signing: officialRelease,
    requires_windows_signing: official_repo && lane === "stable" && !windowsSigningDeferred,
    requires_attestation: officialRelease,
    requires_homebrew_token: official_repo && lane === "stable",
    requires_npm_provenance: official_repo && lane === "stable" && !npmPublishingDeferred,
  };
}

function validateRelease(localArgs = args) {
  const lane = classifyRelease(localArgs);
  if (lane.lane === "invalid_tag") {
    throw new Error(`invalid release tag: ${lane.ref}`);
  }
  if (!lane.build_artifacts) {
    writeJson(lane);
    return;
  }

  const errors = [];
  const workspace = workspaceVersion();
  if (workspace !== lane.version) {
    errors.push(`tag ${lane.tag} does not match workspace version ${workspace}`);
  }
  if (lane.lane === "stable") {
    const npmVersion = wrapperPackageVersion();
    if (npmVersion !== lane.version) {
      errors.push(`tag ${lane.tag} does not match npm wrapper version ${npmVersion}`);
    }
    if (!localArgs["skip-changelog"] && !changelogHasVersion(lane.version, localArgs.changelog ?? "CHANGELOG.md")) {
      errors.push(`CHANGELOG.md must contain a release section for ${lane.version}`);
    }
    // Version equality is not enough: several commits can all identify as the
    // same pending patch release. Bind the real-provider evidence to the
    // deterministic build identity derived by crates/deadreckon/build.rs.
    try {
      validateProviderProof({
        root: localArgs.root ?? ".",
        version: lane.version,
        "known-good": localArgs["known-good"] ?? "release/known-good-providers.json",
      });
    } catch (error) {
      errors.push(error.message);
    }
  }
  if (errors.length > 0) {
    throw new Error(errors.join("\n"));
  }
  writeJson(lane);
}

function validateProviderProof(localArgs = args) {
  const root = path.resolve(localArgs.root ?? ".");
  const expectedVersion = required(localArgs.version, "--version");
  const knownGoodPath = path.resolve(localArgs["known-good"] ?? "release/known-good-providers.json");
  if (!fs.existsSync(knownGoodPath)) {
    throw new Error(`${knownGoodPath} is missing — re-run release/preflight-real.sh`);
  }

  let proof;
  try {
    proof = JSON.parse(fs.readFileSync(knownGoodPath, "utf8"));
  } catch (error) {
    throw new Error(`${knownGoodPath} is not valid JSON: ${error.message}`);
  }
  const errors = [];
  if (proof.schema_version !== 3) {
    errors.push(
      `${knownGoodPath} uses provider-proof schema ${proof.schema_version ?? "unknown"}, not 3 — re-run release/preflight-real.sh`,
    );
  }
  if (proof.deadreckon_version !== expectedVersion) {
    errors.push(
      `${knownGoodPath} proves ${proof.deadreckon_version ?? "no version"}, not ${expectedVersion} — re-run release/preflight-real.sh`,
    );
  }
  if (typeof proof.recorded_at !== "string" || Number.isNaN(Date.parse(proof.recorded_at))) {
    errors.push(`${knownGoodPath} has no valid recorded_at timestamp`);
  }
  if (typeof proof.source_commit !== "string" || !/^[a-f0-9]{40,64}$/.test(proof.source_commit)) {
    errors.push(`${knownGoodPath} has no exact source_commit`);
  }

  const expectedBundleBuildId = sourceBundleBuildId(root);
  const bundle = proof.bundle;
  if (!bundle || bundle.schema_version !== 1) {
    errors.push(`${knownGoodPath} has no supported exact bundle proof`);
  } else {
    if (bundle.package_version !== expectedVersion) {
      errors.push(
        `${knownGoodPath} bundle proves ${bundle.package_version ?? "no version"}, not ${expectedVersion}`,
      );
    }
    if (bundle.bundle_build_id !== expectedBundleBuildId) {
      errors.push(
        `${knownGoodPath} proves stale bundle ${bundle.bundle_build_id ?? "unknown"}; current source requires ${expectedBundleBuildId} — re-run release/preflight-real.sh`,
      );
    }
    for (const name of ["deadreckon", "dr-gate", "dr-capture"]) {
      const binary = bundle.binaries?.[name];
      if (
        !binary ||
        typeof binary.sha256 !== "string" ||
        !/^[a-f0-9]{64}$/.test(binary.sha256) ||
        !Number.isSafeInteger(binary.bytes) ||
        binary.bytes <= 0
      ) {
        errors.push(`${knownGoodPath} has no exact digest and size for ${name}`);
      }
    }
  }

  const providers = Array.isArray(proof.providers) ? proof.providers : [];
  const routes = new Set();
  for (const provider of providers) {
    if (!provider || typeof provider.route !== "string" || routes.has(provider.route)) {
      errors.push(`${knownGoodPath} contains a missing or duplicate provider route`);
      continue;
    }
    routes.add(provider.route);
    for (const field of [
      "binary_version",
      "reviewer_route",
      "reviewer_binary_version",
      "proof",
      "operator",
    ]) {
      if (typeof provider[field] !== "string" || provider[field].trim().length === 0) {
        errors.push(`${knownGoodPath} provider ${provider.route} has no ${field}`);
      }
    }
    if (typeof provider.run_id !== "string" || !/^[a-f0-9]{32}$/.test(provider.run_id)) {
      errors.push(`${knownGoodPath} provider ${provider.route} has no exact run_id`);
    }
  }
  const requiredRoutes = String(localArgs["required-routes"] ?? "cli:claude-code,cli:codex")
    .split(",")
    .map((route) => route.trim())
    .filter(Boolean);
  for (const route of requiredRoutes) {
    if (!routes.has(route)) {
      errors.push(`${knownGoodPath} has no successful real-provider proof for ${route}`);
    }
  }

  if (errors.length > 0) {
    throw new Error(errors.join("\n"));
  }
  return {
    schema_version: 1,
    valid: true,
    deadreckon_version: expectedVersion,
    bundle_build_id: expectedBundleBuildId,
    required_routes: requiredRoutes,
  };
}

function buildBundleProof(localArgs = args) {
  const root = path.resolve(localArgs.root ?? ".");
  const deadreckon = path.resolve(required(localArgs.deadreckon, "--deadreckon"));
  const extension = path.basename(deadreckon).toLowerCase().endsWith(".exe") ? ".exe" : "";
  const directory = path.dirname(deadreckon);
  const binaries = {
    deadreckon,
    "dr-gate": path.join(directory, `dr-gate${extension}`),
    "dr-capture": path.join(directory, `dr-capture${extension}`),
  };
  const expectedBundleBuildId = sourceBundleBuildId(root);
  const evidence = {};
  for (const [name, file] of Object.entries(binaries)) {
    const stat = fs.lstatSync(file);
    if (!stat.isFile() || stat.isSymbolicLink()) {
      throw new Error(`${file} must be a regular, non-symlinked release binary`);
    }
    const bytes = fs.readFileSync(file);
    const bundleBuildId = embeddedBundleBuildId(bytes, name);
    if (bundleBuildId !== expectedBundleBuildId) {
      throw new Error(
        `${name} belongs to ${bundleBuildId}, but current release source requires ${expectedBundleBuildId}`,
      );
    }
    evidence[name] = {
      sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
      bytes: stat.size,
    };
  }
  return {
    schema_version: 1,
    package_version: workspaceVersionAt(root),
    bundle_build_id: expectedBundleBuildId,
    binaries: evidence,
  };
}

function embeddedBundleBuildId(bytes, name) {
  const matches = [
    ...new Set(
      bytes
        .toString("latin1")
        .match(/deadreckon-bundle-build-id-sha256:[a-f0-9]{64}/g) ?? [],
    ),
  ];
  if (matches.length !== 1) {
    throw new Error(`${name} must embed exactly one DeadReckon bundle build identity`);
  }
  return matches[0];
}

function preflight(localArgs = args) {
  const lane = classifyRelease(localArgs);
  if (lane.lane === "invalid_tag") {
    throw new Error(`invalid release tag: ${lane.ref}`);
  }
  const missing = [];
  if (lane.requires_macos_signing) {
    for (const name of ["APPLE_CERT_P12", "APPLE_CERT_PWD", "APPLE_ID", "APPLE_TEAM_ID", "APPLE_APP_PWD"]) {
      if (!process.env[name]) {
        missing.push(name);
      }
    }
  }
  if (lane.requires_homebrew_token && !process.env.HOMEBREW_TAP_TOKEN) {
    missing.push("HOMEBREW_TAP_TOKEN");
  }
  if (lane.requires_npm_provenance && !truthy(process.env.NPM_TRUSTED_PUBLISHING) && !process.env.NPM_TOKEN) {
    missing.push("npm trusted publishing or NPM_TOKEN");
  }
  if (lane.requires_windows_signing) {
    for (const name of ["WINDOWS_CERT_PFX", "WINDOWS_CERT_PWD"]) {
      if (!process.env[name]) {
        missing.push(name);
      }
    }
  }
  const result = { ...lane, missing_trust_material: missing };
  if (missing.length > 0) {
    throw new Error(`missing official release trust material:\n- ${missing.join("\n- ")}`);
  }
  writeJson(result);
}

function writeSbom(localArgs = args) {
  const out = required(localArgs.out, "--out");
  const metadata = cargoMetadata();
  const created = new Date().toISOString();
  const packages = metadata.packages.map((pkg, index) => ({
    name: pkg.name,
    SPDXID: `SPDXRef-Package-${index + 1}-${safeId(pkg.name)}`,
    versionInfo: pkg.version,
    downloadLocation: pkg.source ?? "NOASSERTION",
    filesAnalyzed: false,
    licenseConcluded: "NOASSERTION",
    licenseDeclared: pkg.license ?? "NOASSERTION",
    copyrightText: "NOASSERTION",
  }));
  fs.writeFileSync(
    out,
    `${JSON.stringify(
      {
        spdxVersion: "SPDX-2.3",
        dataLicense: "CC0-1.0",
        SPDXID: "SPDXRef-DOCUMENT",
        name: `deadreckon-${option("ref-name", "GITHUB_REF_NAME", "local")}`,
        documentNamespace: `https://github.com/${OFFICIAL_REPO}/releases/sbom/${Date.now()}`,
        creationInfo: {
          created,
          creators: ["Tool: deadreckon-release-trust"],
        },
        packages,
      },
      null,
      2,
    )}\n`,
  );
}

function writeChecksums(localArgs = args) {
  const dir = required(localArgs.dir, "--dir");
  const out = required(localArgs.out, "--out");
  const lines = releaseFiles(dir, { includeChecksums: false, includeManifest: false })
    .map((entry) => `${sha256(path.join(dir, entry.relative))}  ${entry.name}`)
    .join("\n");
  fs.writeFileSync(out, lines.length > 0 ? `${lines}\n` : "");
}

function writeManifest(localArgs = args) {
  const dir = required(localArgs.dir, "--dir");
  const out = required(localArgs.out, "--out");
  const lane = classifyRelease(localArgs);
  const trust = readTrustStatus(dir);
  const files = releaseFiles(dir, { includeChecksums: true, includeManifest: false });
  const artifacts = files.map((entry) => artifactRecord(dir, entry, lane, trust));
  fs.writeFileSync(
    out,
    `${JSON.stringify(
      {
        schema_version: 1,
        lane: lane.lane,
        tag: lane.tag,
        version: lane.version,
        commit: localArgs.commit ?? process.env.GITHUB_SHA ?? null,
        generated_at: new Date().toISOString(),
        repository: lane.repository || OFFICIAL_REPO,
        cargo_dist_version: process.env.CARGO_DIST_VERSION ?? localArgs["cargo-dist-version"] ?? null,
        artifacts,
        package_managers: {
          homebrew: {
            publish: lane.publish_homebrew,
            tap: "gregce/homebrew-tap",
            checksum_verified: lane.publish_homebrew,
          },
          npm: {
            publish: lane.publish_npm,
            provenance: lane.requires_npm_provenance,
            trusted_publishing: truthy(process.env.NPM_TRUSTED_PUBLISHING),
          },
        },
        policies: {
          macos_signing_required: lane.requires_macos_signing,
          windows_signing_required: lane.requires_windows_signing,
          attestation_required: lane.requires_attestation,
        },
      },
      null,
      2,
    )}\n`,
  );
}

function verifyManifest(localArgs = args) {
  const dir = required(localArgs.dir, "--dir");
  const manifestPath = required(localArgs.manifest, "--manifest");
  const checksumsPath = required(localArgs.checksums, "--checksums");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  if (manifest.schema_version !== 1) {
    throw new Error(`unsupported release-manifest schema: ${manifest.schema_version}`);
  }
  const checksumMap = parseChecksums(checksumsPath);
  const manifestMap = new Map(manifest.artifacts.map((artifact) => [artifact.name, artifact]));
  const entries = releaseFiles(dir, { includeChecksums: true, includeManifest: false });
  const relativeByName = new Map(entries.map((entry) => [entry.name, entry.relative]));
  for (const entry of entries) {
    if (!manifestMap.has(entry.name)) {
      throw new Error(`release-manifest.json is missing ${entry.name}`);
    }
  }
  for (const [name, artifact] of manifestMap) {
    const relative = relativeByName.get(name);
    if (relative === undefined) {
      throw new Error(`manifest entry has no file: ${name}`);
    }
    const file = path.join(dir, relative);
    const digest = sha256(file);
    if (artifact.sha256 !== digest) {
      throw new Error(`manifest sha mismatch for ${name}`);
    }
    const bytes = fs.statSync(file).size;
    if (artifact.bytes !== bytes) {
      throw new Error(`manifest byte count mismatch for ${name}`);
    }
    assertInstallableArchiveLayout(name, file);
  }
  for (const [name, digest] of checksumMap) {
    const artifact = manifestMap.get(name);
    if (!artifact) {
      throw new Error(`SHA256SUMS entry has no manifest artifact: ${name}`);
    }
    if (artifact.sha256 !== digest) {
      throw new Error(`SHA256SUMS mismatch for ${name}`);
    }
  }
  const checksummedTrustTargets = new Set();
  for (const entry of entries) {
    if (artifactKind(entry.relative) !== "trust-status") {
      continue;
    }
    if (!checksumMap.has(entry.name)) {
      throw new Error(`trust status ${entry.name} is missing from SHA256SUMS`);
    }
    const value = JSON.parse(fs.readFileSync(path.join(dir, entry.relative), "utf8"));
    if (value.target) {
      checksummedTrustTargets.add(value.target);
    }
  }
  const trust = readTrustStatus(dir);
  for (const target of trust.keys()) {
    if (!checksummedTrustTargets.has(target)) {
      throw new Error(`trust status for target ${target} is not checksummed`);
    }
  }
  verifyArtifactTrustStatus(manifest, trust);
  writeJson({ ok: true, artifacts: manifest.artifacts.length });
}

function verifyArtifactTrustStatus(manifest, trust) {
  for (const artifact of manifest.artifacts) {
    const target = releaseTargetForName(artifact.name);
    if (artifact.target !== target) {
      throw new Error(
        `release-manifest target mismatch for ${artifact.name}: manifest ${displayValue(artifact.target)}, expected ${displayValue(target)}`,
      );
    }
    if (target === null) {
      continue;
    }
    const targetTrust = trust.get(target) ?? null;
    const signingRequired =
      (target.endsWith("apple-darwin") && manifest.policies?.macos_signing_required === true) ||
      (target.endsWith("windows-msvc") && manifest.policies?.windows_signing_required === true);
    if (signingRequired && targetTrust === null) {
      throw new Error(
        `release-manifest target ${target} requires signing but has no checksummed trust status`,
      );
    }
    const expected = {
      signed: Boolean(targetTrust?.signed),
      notarized: Boolean(targetTrust?.notarized),
      signature_kind: targetTrust?.signature_kind ?? null,
    };
    if (signingRequired) {
      if (expected.signed !== true) {
        throw new Error(
          `release signing policy for target ${target} requires trust field signed=true`,
        );
      }
      if (target.endsWith("apple-darwin") && expected.notarized !== true) {
        throw new Error(
          `release signing policy for target ${target} requires trust field notarized=true`,
        );
      }
      if (expected.signature_kind === null) {
        throw new Error(
          `release signing policy for target ${target} requires trust field signature_kind`,
        );
      }
    }
    for (const field of ["signed", "notarized", "signature_kind"]) {
      if (artifact[field] !== expected[field]) {
        throw new Error(
          `release-manifest trust mismatch for target ${target}, artifact ${artifact.name}, field ${field}: manifest ${displayValue(artifact[field])}, checksummed trust ${displayValue(expected[field])}`,
        );
      }
    }
  }
}

function displayValue(value) {
  return value === undefined ? "<missing>" : JSON.stringify(value);
}

// The cargo-dist shell installer resolves binaries through the archive's
// top-level directory name; "./"-prefixed members (from `tar -C dir .`)
// break it with mv ENOENT. rc.7 shipped that breakage from the macOS
// signing repack — fail closed if it ever reappears.
function assertInstallableArchiveLayout(name, file) {
  if (!(name.endsWith(".tar.xz") || name.endsWith(".tar.gz") || name.endsWith(".tgz"))) {
    return;
  }
  const listing = spawnSync("tar", ["-tf", file], { encoding: "utf8" });
  if (listing.status !== 0) {
    throw new Error(`tar -tf failed for ${name}: ${listing.stderr}`);
  }
  for (const member of listing.stdout.split("\n").filter(Boolean)) {
    if (member === "." || member === "./" || member.startsWith("./")) {
      throw new Error(
        `${name} contains './'-prefixed member ${member}; repack with explicit top-level names or the shell installer breaks`,
      );
    }
  }
}

function verifyHomebrew(localArgs = args) {
  const dir = required(localArgs.dir, "--dir");
  const checksums = parseChecksums(required(localArgs.checksums, "--checksums"));
  const formulae = releaseFiles(dir, { includeChecksums: true, includeManifest: false })
    .filter((entry) => entry.name.endsWith(".rb"))
    .map((entry) => entry.relative);
  const findChecksum = (basename) =>
    checksums.get(basename) ??
    [...checksums.entries()].find(([name]) => path.basename(name) === basename)?.[1];
  let verifiedUrls = 0;
  for (const formula of formulae) {
    const text = fs.readFileSync(path.join(dir, formula), "utf8");
    const installedBinaries = new Set(
      [...text.matchAll(/^\s*bin\.install\s+(.+)$/gm)].flatMap((match) =>
        [...match[1].matchAll(/"([^"]+)"/g)].map((name) => name[1]),
      ),
    );
    for (const binary of REQUIRED_HOMEBREW_BINARIES) {
      if (!installedBinaries.has(binary)) {
        throw new Error(`${formula} does not install required binary ${binary}`);
      }
    }
    // Multi-platform formulae carry one url per target inside platform blocks.
    // Homebrew does not infer a checksum from our release manifest, so every
    // URL must be immediately paired with the exact archive digest. Accepting
    // an unpinned URL here would make `brew install` download unverified bytes.
    const urls = [...text.matchAll(/url\s+"([^"]+)"/g)].map((match) => match[1]);
    const pinnedUrls = [
      ...text.matchAll(
        /^(\s*)url\s+"([^"]+)"\s*\r?\n\1sha256\s+"([0-9a-fA-F]{64})"\s*$/gm,
      ),
    ];
    if (urls.length === 0) {
      throw new Error(`${formula} must reference at least one url`);
    }
    if (pinnedUrls.length !== urls.length) {
      throw new Error(
        `${formula} must pin one sha256 immediately after each url (${pinnedUrls.length}/${urls.length} pinned)`,
      );
    }
    for (const [, , url, pinnedSha] of pinnedUrls) {
      const basename = path.basename(url);
      const expectedSha = findChecksum(basename);
      if (!expectedSha) {
        throw new Error(`${formula} references ${basename}, which is missing from SHA256SUMS`);
      }
      if (pinnedSha.toLowerCase() !== expectedSha.toLowerCase()) {
        throw new Error(
          `${formula} pins sha256 ${pinnedSha} for ${basename}, expected ${expectedSha}`,
        );
      }
      verifiedUrls += 1;
    }
  }
  writeJson({ ok: true, formulae: formulae.length, verified_urls: verifiedUrls });
}

function artifactRecord(dir, entry, lane, trust) {
  const absolute = path.join(dir, entry.relative);
  const target = releaseTargetForName(entry.name);
  const targetTrust = target ? trust.get(target) : null;
  return {
    name: entry.name,
    target,
    kind: artifactKind(entry.relative),
    sha256: sha256(absolute),
    bytes: fs.statSync(absolute).size,
    signed: Boolean(targetTrust?.signed),
    signature_kind: targetTrust?.signature_kind ?? null,
    notarized: Boolean(targetTrust?.notarized),
    attested: lane.requires_attestation,
    sbom: entry.name.endsWith(".spdx.json") ? entry.name : null,
  };
}

function releaseTargetForName(name) {
  return TARGETS.find((candidate) => name.includes(candidate)) ?? null;
}

function artifactKind(file) {
  if (file === "SHA256SUMS" || file.endsWith("/SHA256SUMS")) return "checksums";
  if (file.endsWith(".spdx.json")) return "sbom";
  if (isTrustStatusAsset(path.basename(file), file)) return "trust-status";
  if (file.endsWith(".rb")) return "homebrew-formula";
  if (file.endsWith(".sh") || file.endsWith(".ps1")) return "installer";
  if (file.endsWith(".zip") || file.endsWith(".tar.xz") || file.endsWith(".tar.gz") || file.endsWith(".tgz")) {
    return "archive";
  }
  if (file.endsWith(".json")) return "metadata";
  return "artifact";
}

function readTrustStatus(dir) {
  const result = new Map();
  const origins = new Map();
  walk(dir, (file) => {
    const relative = path.relative(dir, file).replaceAll(path.sep, "/");
    const name = path.basename(file);
    if (!isTrustStatusAsset(name, relative)) {
      return;
    }
    const contents = fs.readFileSync(file, "utf8");
    const value = JSON.parse(contents);
    if (!value.target) {
      return;
    }
    const namedTarget = trustStatusTargetFromName(name);
    if (namedTarget !== null && value.target !== namedTarget) {
      throw new Error(
        `trust status ${relative} names target ${value.target}, expected ${namedTarget}`,
      );
    }
    const digest = crypto.createHash("sha256").update(contents).digest("hex");
    const existing = origins.get(value.target);
    if (existing !== undefined && existing.digest !== digest) {
      throw new Error(
        `conflicting trust status for target ${value.target}: ${existing.relative} vs ${relative}`,
      );
    }
    if (existing === undefined) {
      origins.set(value.target, { digest, relative });
      result.set(value.target, value);
    }
  });
  return result;
}

function trustStatusTargetFromName(name) {
  for (const target of TARGETS) {
    if (name === `macos-${target}.json` || name === `windows-${target}.json`) {
      return target;
    }
  }
  return null;
}

function isTrustStatusAsset(name, relative) {
  const nestedTrustStatus =
    (relative.startsWith("trust/") || relative.includes("/trust/")) && name.endsWith(".json");
  const flatPublicTrustStatus = relative === name && trustStatusTargetFromName(name) !== null;
  return nestedTrustStatus || flatPublicTrustStatus;
}

// The shapes the publish step uploads. Anything else under the artifact
// tree (extracted per-target directories, loose binaries, READMEs) is a
// build intermediate, not a release asset — listing those in SHA256SUMS or
// the manifest would reference files users can never download.
function isReleaseAsset(name, relative) {
  if (isTrustStatusAsset(name, relative)) {
    return true;
  }
  return (
    name.endsWith(".tar.xz") ||
    name.endsWith(".tar.gz") ||
    name.endsWith(".tgz") ||
    name.endsWith(".zip") ||
    name.endsWith(".sha256") ||
    name === "sha256.sum" ||
    name === "deadreckon-installer.sh" ||
    name === "deadreckon-installer.ps1" ||
    name.endsWith(".rb") ||
    name === "SHA256SUMS" ||
    name === "release-manifest.json" ||
    name === "release-archive-members.json" ||
    name === "release.spdx.json"
  );
}

function releaseFiles(dir, options) {
  const entries = [];
  walk(dir, (file) => {
    const relative = path.relative(dir, file).replaceAll(path.sep, "/");
    if (relative === ".DS_Store" || relative.endsWith("/.DS_Store")) {
      return;
    }
    const name = relative.split("/").pop();
    if (!isReleaseAsset(name, relative)) {
      return;
    }
    if (!options.includeChecksums && name === "SHA256SUMS") {
      return;
    }
    if (!options.includeManifest && name === "release-manifest.json") {
      return;
    }
    entries.push({ name, relative });
  });
  entries.sort((a, b) => a.relative.localeCompare(b.relative));
  // Published release assets are flat, so every artifact is recorded by its
  // basename — users verify downloads with `shasum -a 256 -c SHA256SUMS`
  // next to flat files. The CI global-artifact layout nests some files
  // twice; identical duplicates collapse, divergent content fails closed.
  const byName = new Map();
  for (const entry of entries) {
    const existing = byName.get(entry.name);
    if (existing === undefined) {
      byName.set(entry.name, entry);
      continue;
    }
    const existingDigest = sha256(path.join(dir, existing.relative));
    const duplicateDigest = sha256(path.join(dir, entry.relative));
    if (existingDigest !== duplicateDigest) {
      throw new Error(
        `conflicting contents for release asset ${entry.name}: ${existing.relative} vs ${entry.relative}`,
      );
    }
  }
  return [...byName.values()].sort((a, b) => a.name.localeCompare(b.name));
}

function walk(dir, visitor) {
  if (!fs.existsSync(dir)) {
    return;
  }
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      walk(fullPath, visitor);
    } else {
      visitor(fullPath);
    }
  }
}

function parseChecksums(file) {
  const result = new Map();
  for (const line of fs.readFileSync(file, "utf8").split(/\r?\n/)) {
    if (!line.trim()) {
      continue;
    }
    const match = /^([a-f0-9]{64})\s+(.+)$/.exec(line.trim());
    if (!match) {
      throw new Error(`invalid SHA256SUMS line: ${line}`);
    }
    result.set(match[2], match[1]);
  }
  return result;
}

function cargoMetadata() {
  const output = spawnSync("cargo", ["metadata", "--format-version=1", "--locked"], {
    encoding: "utf8",
  });
  if (output.status === 0) {
    return JSON.parse(output.stdout);
  }
  return {
    packages: [
      {
        name: "deadreckon",
        version: workspaceVersion(),
        source: null,
        license: "MIT",
      },
    ],
  };
}

function workspaceVersion() {
  return workspaceVersionAt(".");
}

function workspaceVersionAt(root) {
  const text = fs.readFileSync(path.join(root, "Cargo.toml"), "utf8");
  const match = /\[workspace\.package\][\s\S]*?\nversion\s*=\s*"([^"]+)"/.exec(text);
  if (!match) {
    throw new Error("Cargo.toml [workspace.package] version not found");
  }
  return match[1];
}

// Keep this byte-for-byte equivalent to crates/deadreckon/build.rs. The ID is
// deliberately independent of the proof file itself, so an operator can prove
// a clean source commit, commit known-good-providers.json, and tag that result
// without invalidating the binary evidence.
function sourceBundleBuildId(rootValue) {
  const root = path.resolve(rootValue);
  const inputs = [path.join(root, "Cargo.toml"), path.join(root, "Cargo.lock")];
  const crates = path.join(root, "crates");
  const crateDirectories = fs
    .readdirSync(crates, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => path.join(crates, entry.name));
  for (const crateDirectory of crateDirectories) {
    for (const candidate of [
      path.join(crateDirectory, "Cargo.toml"),
      path.join(crateDirectory, "build.rs"),
    ]) {
      if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) {
        inputs.push(candidate);
      }
    }
    const source = path.join(crateDirectory, "src");
    if (fs.existsSync(source) && fs.statSync(source).isDirectory()) {
      collectBundleRustSources(source, inputs);
    }
  }
  // Rust PathBuf ordering compares one path component at a time. A plain
  // string sort puts `deadreckon-core` before `deadreckon/` because `-` sorts
  // before `/`, which silently produces a different digest.
  inputs.sort((left, right) => comparePathComponents(root, left, right));

  const digest = crypto.createHash("sha256");
  const decoder = new TextDecoder("utf-8", { fatal: true });
  for (const file of inputs) {
    const relative = normalizedRelative(root, file);
    digest.update(relative, "utf8");
    digest.update(Buffer.from([0]));
    let text;
    try {
      text = decoder.decode(fs.readFileSync(file));
    } catch (error) {
      throw new Error(`bundle build input must be UTF-8 (${file}): ${error.message}`);
    }
    digest.update(text.replaceAll("\r\n", "\n"), "utf8");
    digest.update(Buffer.from([0]));
  }
  return `deadreckon-bundle-build-id-sha256:${digest.digest("hex")}`;
}

function collectBundleRustSources(directory, inputs) {
  const entries = fs.readdirSync(directory, { withFileTypes: true });
  for (const entry of entries) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== "target") {
        collectBundleRustSources(file, inputs);
      }
    } else if (entry.isFile() && path.extname(entry.name) === ".rs") {
      inputs.push(file);
    }
  }
}

function normalizedRelative(root, file) {
  return path.relative(root, file).split(path.sep).join("/");
}

function comparePathComponents(root, left, right) {
  const leftComponents = normalizedRelative(root, left).split("/");
  const rightComponents = normalizedRelative(root, right).split("/");
  const common = Math.min(leftComponents.length, rightComponents.length);
  for (let index = 0; index < common; index += 1) {
    const leftComponent = leftComponents[index];
    const rightComponent = rightComponents[index];
    if (leftComponent < rightComponent) {
      return -1;
    }
    if (leftComponent > rightComponent) {
      return 1;
    }
  }
  return leftComponents.length - rightComponents.length;
}

function wrapperPackageVersion() {
  const packageJson = JSON.parse(fs.readFileSync("npm/deadreckon/package.json", "utf8"));
  return packageJson.version;
}

function changelogHasVersion(version, changelogPath) {
  if (!fs.existsSync(changelogPath)) {
    return false;
  }
  const escaped = version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`^##\\s+.*\\b${escaped}\\b`, "m").test(fs.readFileSync(changelogPath, "utf8"));
}

function safeId(value) {
  return value.replace(/[^A-Za-z0-9.-]/g, "-");
}

function sha256(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function required(value, name) {
  if (!value) {
    throw new Error(`${name} is required`);
  }
  return value;
}

function truthy(value) {
  return value === "1" || value === "true" || value === "TRUE" || value === "yes";
}

function writeJson(value) {
  const text = `${JSON.stringify(value, null, 2)}\n`;
  if (args.out) {
    fs.writeFileSync(args.out, text);
  }
  if (args["github-output"]) {
    const lines = Object.entries(value)
      .filter(([, entry]) => entry === null || ["string", "number", "boolean"].includes(typeof entry))
      .map(([key, entry]) => `${key}=${entry ?? ""}`);
    fs.appendFileSync(args["github-output"], `${lines.join("\n")}\n`);
  }
  process.stdout.write(text);
}
