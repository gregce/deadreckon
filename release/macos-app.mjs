#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const APP_TARGET = "universal-apple-darwin-app";
const APP_ARCHIVE = "deadreckon-mac.zip";
const APP_TRUST_STATUS = "macos-universal-apple-darwin-app.json";
const MAX_CAPTURE_BYTES = 128 * 1024 * 1024;
const ARCHIVES = {
  arm64: "deadreckon-aarch64-apple-darwin.tar.xz",
  x86_64: "deadreckon-x86_64-apple-darwin.tar.xz",
};
const RESOURCE_PATHS = {
  arm64: {
    cli: "Resources/bin/deadreckon_darwin_arm64",
    gate: "Resources/bin/dr-gate",
  },
  x86_64: {
    cli: "Resources/bin/deadreckon_darwin_x86_64",
    gate: "Resources/libexec/deadreckon/dr-gate",
  },
};

const command = process.argv[2];
const args = parseArgs(process.argv.slice(3));

try {
  let result;
  switch (command) {
    case "hydrate":
      result = hydrateResources(args);
      break;
    case "verify-resources":
      result = verifyResources(args);
      break;
    case "verify-bundle":
      result = verifyBundle(args);
      break;
    case "trust":
      result = writeTrustStatus(args);
      break;
    default:
      throw new Error(
        "usage: macos-app.mjs <hydrate|verify-resources|verify-bundle|trust>",
      );
  }
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
} catch (error) {
  console.error(error.message);
  process.exit(1);
}

function hydrateResources(localArgs) {
  const distrib = path.resolve(required(localArgs.dir, "--dir"));
  const appRoot = path.resolve(required(localArgs["app-root"], "--app-root"));
  const releaseVersion = releaseVersionArg(localArgs.version);
  const sourceCommit = commitArg(localArgs.commit);
  const manifestPath = path.join(appRoot, "Resources/bin/manifest.json");
  const sourceArchives = {};
  const sha256 = {};
  const gateSha256 = {};
  const hydrated = {};

  // Resolve and validate every required release archive before mutating the app
  // resource tree. A missing or malformed second architecture must not leave a
  // plausible-looking partial bundle behind.
  for (const [arch, archiveName] of Object.entries(ARCHIVES)) {
    const archive = findUniqueAsset(distrib, archiveName);
    const target = archiveName.slice("deadreckon-".length, -".tar.xz".length);
    const payload = `deadreckon-${target}`;
    const cliBytes = extractRegularTarMember(archive, `${payload}/deadreckon`);
    const gateBytes = extractRegularTarMember(archive, `${payload}/dr-gate`);
    hydrated[arch] = { cliBytes, gateBytes };
    sha256[arch] = sha256Bytes(cliBytes);
    gateSha256[arch] = sha256Bytes(gateBytes);
    sourceArchives[arch] = {
      name: archiveName,
      sha256: sha256File(archive),
    };
  }

  for (const [arch, { cliBytes, gateBytes }] of Object.entries(hydrated)) {
    writeExecutable(path.join(appRoot, RESOURCE_PATHS[arch].cli), cliBytes);
    writeExecutable(path.join(appRoot, RESOURCE_PATHS[arch].gate), gateBytes);
  }

  const manifest = {
    schemaVersion: 1,
    cliVersion: releaseVersion,
    releaseVersion,
    gitCommit: sourceCommit,
    sourceDirty: false,
    complete: true,
    signed: true,
    sha256,
    gateSha256,
    sourceArchives,
  };
  atomicWriteJson(manifestPath, manifest);
  verifyResourceTree(appRoot, manifest, { releaseVersion, sourceCommit });
  return {
    ok: true,
    manifest: manifestPath,
    release_version: releaseVersion,
    source_commit: sourceCommit,
    architectures: Object.keys(ARCHIVES),
  };
}

function verifyResources(localArgs) {
  const appRoot = path.resolve(required(localArgs["app-root"], "--app-root"));
  const expected = {
    releaseVersion: localArgs.version ? releaseVersionArg(localArgs.version) : null,
    sourceCommit: localArgs.commit ? commitArg(localArgs.commit) : null,
  };
  const manifest = readJson(path.join(appRoot, "Resources/bin/manifest.json"));
  verifyResourceTree(appRoot, manifest, expected);
  return {
    ok: true,
    release_version: manifest.releaseVersion,
    source_commit: manifest.gitCommit,
    architectures: Object.keys(ARCHIVES),
  };
}

function verifyBundle(localArgs) {
  const app = path.resolve(required(localArgs.app, "--app"));
  const expected = {
    releaseVersion: localArgs.version ? releaseVersionArg(localArgs.version) : null,
    sourceCommit: localArgs.commit ? commitArg(localArgs.commit) : null,
  };
  const resources = path.join(app, "Contents/Resources");
  const manifest = readJson(path.join(resources, "bin/manifest.json"));
  verifyResourceTree(path.join(app, "Contents"), manifest, expected);
  return {
    ok: true,
    release_version: manifest.releaseVersion,
    source_commit: manifest.gitCommit,
    architectures: Object.keys(ARCHIVES),
  };
}

function writeTrustStatus(localArgs) {
  if (process.platform !== "darwin") {
    throw new Error("macOS app trust evidence can only be generated on macOS");
  }
  const app = path.resolve(required(localArgs.app, "--app"));
  const archive = path.resolve(required(localArgs.archive, "--archive"));
  const out = path.resolve(required(localArgs.out, "--out"));
  const releaseVersion = releaseVersionArg(localArgs.version);
  const appVersion = appVersionArg(localArgs["app-version"]);
  const sourceCommit = commitArg(localArgs.commit);

  run("codesign", ["--verify", "--strict", "--deep", "--verbose=2", app]);
  run("xcrun", ["stapler", "validate", app]);
  run("spctl", ["--assess", "--type", "execute", "--verbose=2", app]);

  const appExecutable = path.join(app, "Contents/MacOS/deadreckon");
  requireArchitectures(appExecutable, ["arm64", "x86_64"]);
  const infoVersion = runText("/usr/libexec/PlistBuddy", [
    "-c",
    "Print :CFBundleShortVersionString",
    path.join(app, "Contents/Info.plist"),
  ]).trim();
  if (infoVersion !== appVersion) {
    throw new Error(`app bundle version ${infoVersion} does not match expected ${appVersion}`);
  }
  const bundleIdentifier = runText("/usr/libexec/PlistBuddy", [
    "-c",
    "Print :CFBundleIdentifier",
    path.join(app, "Contents/Info.plist"),
  ]).trim();
  if (bundleIdentifier !== "com.itavero.deadreckon") {
    throw new Error(`unexpected app bundle identifier ${bundleIdentifier}`);
  }

  const contentsRoot = path.join(app, "Contents");
  const manifest = readJson(path.join(contentsRoot, "Resources/bin/manifest.json"));
  verifyResourceTree(contentsRoot, manifest, { releaseVersion, sourceCommit });
  for (const [arch, paths] of Object.entries(RESOURCE_PATHS)) {
    const cli = path.join(contentsRoot, paths.cli);
    const gate = path.join(contentsRoot, paths.gate);
    requireArchitectures(cli, [arch]);
    requireArchitectures(gate, [arch]);
    run("codesign", ["--verify", "--strict", cli]);
    run("codesign", ["--verify", "--strict", gate]);
  }

  const zipManifest = extractZipJson(archive, "deadreckon.app/Contents/Resources/bin/manifest.json");
  if (JSON.stringify(zipManifest) !== JSON.stringify(manifest)) {
    throw new Error("packaged app manifest differs from the verified app bundle manifest");
  }
  const embedded = embeddedHashes(contentsRoot);
  for (const [name, relative] of embeddedEntries()) {
    const packaged = extractUniqueZipMember(archive, `deadreckon.app/Contents/${relative}`);
    if (sha256Bytes(packaged) !== embedded[name]) {
      throw new Error(`packaged ${relative} differs from the verified app bundle`);
    }
  }

  const trust = {
    schema_version: 1,
    target: APP_TARGET,
    artifact: APP_ARCHIVE,
    signed: true,
    signature_kind: "Developer ID Application",
    notarized: true,
    stapled: true,
    bundle_identifier: bundleIdentifier,
    release_version: releaseVersion,
    app_version: appVersion,
    source_commit: sourceCommit,
    architectures: ["arm64", "x86_64"],
    archive_sha256: sha256File(archive),
    archive_bytes: fs.statSync(archive).size,
    embedded_binaries: embedded,
  };
  atomicWriteJson(out, trust);
  return { ok: true, trust_status: out, ...trust };
}

function verifyResourceTree(root, manifest, expected) {
  if (manifest.schemaVersion !== 1) {
    throw new Error(`unsupported app manifest schema ${manifest.schemaVersion ?? "missing"}`);
  }
  if (manifest.complete !== true || manifest.signed !== true || manifest.sourceDirty !== false) {
    throw new Error("app resources are not a complete, signed, clean-source release bundle");
  }
  if (!/^\d+\.\d+\.\d+(?:-rc\.\d+)?$/.test(manifest.releaseVersion ?? "")) {
    throw new Error("app manifest has no valid releaseVersion");
  }
  if (manifest.cliVersion !== manifest.releaseVersion) {
    throw new Error("app manifest cliVersion and releaseVersion differ");
  }
  commitArg(manifest.gitCommit);
  if (expected.releaseVersion && manifest.releaseVersion !== expected.releaseVersion) {
    throw new Error(
      `app manifest version ${manifest.releaseVersion} does not match ${expected.releaseVersion}`,
    );
  }
  if (expected.sourceCommit && manifest.gitCommit !== expected.sourceCommit) {
    throw new Error(`app manifest commit ${manifest.gitCommit} does not match ${expected.sourceCommit}`);
  }
  for (const [arch, paths] of Object.entries(RESOURCE_PATHS)) {
    const cli = path.join(root, paths.cli);
    const gate = path.join(root, paths.gate);
    assertRegularExecutable(cli);
    assertRegularExecutable(gate);
    assertDigest(cli, manifest.sha256?.[arch], `${arch} CLI`);
    assertDigest(gate, manifest.gateSha256?.[arch], `${arch} dr-gate`);
    if (!manifest.sourceArchives?.[arch]?.name || !manifest.sourceArchives?.[arch]?.sha256) {
      throw new Error(`app manifest has no source archive proof for ${arch}`);
    }
  }
  for (const field of ["sha256", "gateSha256", "sourceArchives"]) {
    const keys = Object.keys(manifest[field] ?? {}).sort();
    if (JSON.stringify(keys) !== JSON.stringify(Object.keys(ARCHIVES).sort())) {
      throw new Error(`app manifest ${field} must contain exactly arm64 and x86_64`);
    }
  }
}

function embeddedHashes(contentsRoot) {
  const hashes = {};
  for (const [name, relative] of embeddedEntries()) {
    hashes[name] = sha256File(path.join(contentsRoot, relative));
  }
  return hashes;
}

function embeddedEntries() {
  return [
    ["deadreckon_arm64", RESOURCE_PATHS.arm64.cli],
    ["dr_gate_arm64", RESOURCE_PATHS.arm64.gate],
    ["deadreckon_x86_64", RESOURCE_PATHS.x86_64.cli],
    ["dr_gate_x86_64", RESOURCE_PATHS.x86_64.gate],
  ];
}

function requireArchitectures(file, expected) {
  const actual = runText("lipo", [file, "-archs"]).trim().split(/\s+/).filter(Boolean).sort();
  const wanted = [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(wanted)) {
    throw new Error(`${file} has architectures ${actual.join(",")}, expected ${wanted.join(",")}`);
  }
}

function extractRegularTarMember(archive, member) {
  const members = runText("tar", ["-tf", archive]).split(/\r?\n/).filter(Boolean);
  assertSafeArchiveMembers(archive, members);
  if (members.filter((entry) => entry === member).length !== 1) {
    throw new Error(`${path.basename(archive)} must contain exactly one ${member}`);
  }
  const verbose = runText("tar", ["-tvf", archive, member]).trim();
  if (!verbose.startsWith("-")) {
    throw new Error(`${member} in ${path.basename(archive)} is not a regular file`);
  }
  return runBuffer("tar", ["-xOf", archive, member]);
}

function extractZipJson(archive, member) {
  return JSON.parse(extractUniqueZipMember(archive, member).toString("utf8"));
}

function extractUniqueZipMember(archive, member) {
  const members = runText("unzip", ["-Z1", archive]).split(/\r?\n/).filter(Boolean);
  assertSafeArchiveMembers(archive, members);
  if (members.filter((entry) => entry === member).length !== 1) {
    throw new Error(`${path.basename(archive)} must contain exactly one ${member}`);
  }
  return runBuffer("unzip", ["-p", archive, member]);
}

function assertSafeArchiveMembers(archive, members) {
  for (const member of members) {
    const normalized = member.replaceAll("\\", "/");
    if (
      normalized.startsWith("/") ||
      normalized.split("/").includes("..") ||
      normalized.includes("\0")
    ) {
      throw new Error(`${path.basename(archive)} contains unsafe member ${member}`);
    }
  }
}

function findUniqueAsset(root, name) {
  const found = [];
  walk(root, (file) => {
    if (path.basename(file) === name) found.push(file);
  });
  if (found.length === 0) throw new Error(`release artifact ${name} is missing under ${root}`);
  const digest = sha256File(found[0]);
  for (const duplicate of found.slice(1)) {
    if (sha256File(duplicate) !== digest) {
      throw new Error(`conflicting copies of release artifact ${name}`);
    }
  }
  return found[0];
}

function walk(root, visitor) {
  if (!fs.existsSync(root)) return;
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    const full = path.join(root, entry.name);
    if (entry.isDirectory()) walk(full, visitor);
    else if (entry.isFile()) visitor(full);
  }
}

function writeExecutable(destination, bytes) {
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  const temp = `${destination}.tmp-${process.pid}-${crypto.randomUUID()}`;
  fs.writeFileSync(temp, bytes, { mode: 0o755 });
  fs.chmodSync(temp, 0o755);
  fs.renameSync(temp, destination);
}

function atomicWriteJson(destination, value) {
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  const temp = path.join(
    path.dirname(destination),
    `.${path.basename(destination)}.tmp-${process.pid}-${crypto.randomUUID()}`,
  );
  fs.writeFileSync(temp, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o644 });
  fs.renameSync(temp, destination);
}

function assertRegularExecutable(file) {
  const stat = fs.lstatSync(file);
  if (!stat.isFile() || stat.isSymbolicLink()) throw new Error(`${file} is not a regular file`);
  if ((stat.mode & 0o111) === 0) throw new Error(`${file} is not executable`);
}

function assertDigest(file, expected, label) {
  if (!/^[a-f0-9]{64}$/.test(expected ?? "")) throw new Error(`${label} has no valid sha256 pin`);
  const actual = sha256File(file);
  if (actual !== expected) throw new Error(`${label} sha256 ${actual} does not match ${expected}`);
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function sha256File(file) {
  return sha256Bytes(fs.readFileSync(file));
}

function sha256Bytes(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

function run(commandName, values) {
  const output = spawnSync(commandName, values, { encoding: "utf8", maxBuffer: MAX_CAPTURE_BYTES });
  if (output.status !== 0) {
    throw new Error(
      `${commandName} ${values.join(" ")} failed: ${(output.stderr || output.stdout || "unknown error").trim()}`,
    );
  }
  return output;
}

function runText(commandName, values) {
  return run(commandName, values).stdout;
}

function runBuffer(commandName, values) {
  const output = spawnSync(commandName, values, { encoding: null, maxBuffer: MAX_CAPTURE_BYTES });
  if (output.status !== 0) {
    throw new Error(
      `${commandName} ${values.join(" ")} failed: ${Buffer.from(output.stderr ?? []).toString("utf8").trim()}`,
    );
  }
  return Buffer.from(output.stdout);
}

function releaseVersionArg(value) {
  const parsed = required(value, "--version");
  if (!/^\d+\.\d+\.\d+(?:-rc\.\d+)?$/.test(parsed)) {
    throw new Error(`invalid release version ${parsed}`);
  }
  return parsed;
}

function appVersionArg(value) {
  const parsed = required(value, "--app-version");
  if (!/^\d+\.\d+\.\d+$/.test(parsed)) throw new Error(`invalid app version ${parsed}`);
  return parsed;
}

function commitArg(value) {
  const parsed = required(value, "--commit");
  if (!/^[a-f0-9]{40,64}$/.test(parsed)) throw new Error(`invalid source commit ${parsed}`);
  return parsed;
}

function required(value, name) {
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function parseArgs(values) {
  const parsed = {};
  for (let index = 0; index < values.length; index += 1) {
    const value = values[index];
    if (!value.startsWith("--")) continue;
    const name = value.slice(2);
    const next = values[index + 1];
    if (next && !next.startsWith("--")) {
      parsed[name] = next;
      index += 1;
    } else {
      parsed[name] = true;
    }
  }
  return parsed;
}

export { APP_ARCHIVE, APP_TARGET, APP_TRUST_STATUS };
