#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const HOST_TARGETS = [
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "aarch64-unknown-linux-gnu",
  "x86_64-unknown-linux-gnu",
  "x86_64-pc-windows-msvc",
];

const EVALUATORS = [
  {
    name: "dr-gate-evaluator-aarch64-unknown-linux-musl",
    target: "aarch64-unknown-linux-musl",
    machine: 0xb7,
  },
  {
    name: "dr-gate-evaluator-x86_64-unknown-linux-musl",
    target: "x86_64-unknown-linux-musl",
    machine: 0x3e,
  },
];

// Release binaries are substantially larger than Node's 1 MiB spawnSync
// default. Keep extraction bounded, but high enough for optimized host and
// evaluator binaries so archive inventory cannot fail with an opaque ENOBUFS.
const MAX_ARCHIVE_MEMBER_BYTES = 128 * 1024 * 1024;
const GATE_PROTOCOL_MARKER = "deadreckon-gate-evaluator-protocol-v1";
const BUNDLE_BUILD_ID_PATTERN = /deadreckon-bundle-build-id-sha256:[a-f0-9]{64}/g;

const command = process.argv[2];
const args = parseArgs(process.argv.slice(3));

try {
  switch (command) {
    case "verify-sidecars":
      writeJson(
        verifySidecarDirectory(
          required(args["sidecars-dir"], "--sidecars-dir"),
          args.target ?? null,
        ),
      );
      break;
    case "assemble":
      writeJson(assembleArchive(args));
      break;
    case "verify-archive":
      writeJson(verifyArchiveCommand(args));
      break;
    case "refresh-checksum":
      writeJson(refreshArchiveChecksum(args));
      break;
    case "manifest":
      writeArchiveManifest(args);
      break;
    case "verify-manifest":
      verifyArchiveManifest(args);
      break;
    case "patch-installers":
      patchInstallers(required(args.dir, "--dir"));
      break;
    case "verify-installers":
      verifyInstallers(required(args.dir, "--dir"));
      break;
    default:
      throw new Error(
        "usage: evaluator-sidecars.mjs <verify-sidecars|assemble|verify-archive|refresh-checksum|manifest|verify-manifest|patch-installers|verify-installers>",
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

function assembleArchive(localArgs) {
  const dir = required(localArgs.dir, "--dir");
  const target = requireHostTarget(localArgs.target);
  const sidecarsDir = required(localArgs["sidecars-dir"], "--sidecars-dir");
  const sidecars = verifySidecarDirectory(sidecarsDir);
  const archive = oneTargetArchive(dir, target);
  const membersBefore = listArchive(archive);
  assertSafeArchiveMembers(archive, membersBefore);

  const temp = fs.mkdtempSync(path.join(os.tmpdir(), `deadreckon-assemble-${target}-`));
  const extractDir = path.join(temp, "extract");
  fs.mkdirSync(extractDir);
  extractArchive(archive, extractDir);
  const payload = payloadDirectory(extractDir, target);
  assertHostHelpers(payload, target);

  for (const evaluator of EVALUATORS) {
    const source = sidecars.find((entry) => entry.name === evaluator.name)?.path;
    if (!source) {
      throw new Error(`verified evaluator ${evaluator.name} disappeared before assembly`);
    }
    const destination = path.join(payload, evaluator.name);
    fs.copyFileSync(source, destination);
    fs.chmodSync(destination, 0o755);
    // Keep evaluator archive headers stable across repeated assembly.
    fs.utimesSync(destination, 0, 0);
  }

  repackArchive(archive, extractDir);
  return archiveInventory(archive, target);
}

function verifyArchiveCommand(localArgs) {
  const dir = required(localArgs.dir, "--dir");
  const target = requireHostTarget(localArgs.target);
  const archive = oneTargetArchive(dir, target);
  assertArchiveChecksum(archive);
  return archiveInventory(archive, target);
}

function refreshArchiveChecksum(localArgs) {
  const dir = required(localArgs.dir, "--dir");
  const target = requireHostTarget(localArgs.target);
  const archive = oneTargetArchive(dir, target);
  const checksum = `${archive}.sha256`;
  if (!fs.existsSync(checksum)) {
    throw new Error(`cargo-dist checksum sibling is missing for ${path.basename(archive)}`);
  }
  const digest = sha256File(archive);
  fs.writeFileSync(checksum, `${digest} *${path.basename(archive)}\n`);
  return {
    archive: path.basename(archive),
    checksum: path.basename(checksum),
    sha256: digest,
  };
}

function assertArchiveChecksum(archive) {
  const checksum = `${archive}.sha256`;
  if (!fs.existsSync(checksum)) {
    throw new Error(`cargo-dist checksum sibling is missing for ${path.basename(archive)}`);
  }
  const raw = fs.readFileSync(checksum, "utf8").trim();
  const match = /^([a-f0-9]{64})\s+[*]?(.+)$/.exec(raw);
  if (!match || match[2] !== path.basename(archive)) {
    throw new Error(`${path.basename(checksum)} is not a valid cargo-dist archive checksum`);
  }
  const actual = sha256File(archive);
  if (match[1] !== actual) {
    throw new Error(`${path.basename(checksum)} is stale for final archive ${path.basename(archive)}`);
  }
}

function writeArchiveManifest(localArgs) {
  const out = required(localArgs.out, "--out");
  const manifest = buildArchiveManifest(
    required(localArgs.dir, "--dir"),
    localArgs.target ? [requireHostTarget(localArgs.target)] : HOST_TARGETS,
  );
  fs.writeFileSync(out, `${JSON.stringify(manifest, null, 2)}\n`);
  writeJson({ ok: true, archives: manifest.archives.length, evaluators: manifest.evaluators.length });
}

function verifyArchiveManifest(localArgs) {
  const manifestPath = required(localArgs.manifest, "--manifest");
  const expected = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  if (expected.schema_version !== 1 || !Array.isArray(expected.archives)) {
    throw new Error(`unsupported archive-member manifest schema in ${manifestPath}`);
  }
  const targets = expected.archives.map((archive) => requireHostTarget(archive.target));
  const actual = buildArchiveManifest(required(localArgs.dir, "--dir"), targets);
  if (JSON.stringify(expected) !== JSON.stringify(actual)) {
    throw new Error(
      "release-archive-members.json does not match the final release archives; regenerate it after assembly and signing",
    );
  }
  writeJson({ ok: true, archives: actual.archives.length, evaluators: actual.evaluators.length });
}

function buildArchiveManifest(dir, targets) {
  const uniqueTargets = [...new Set(targets)].sort();
  const archives = uniqueTargets.map((target) => {
    const archive = oneTargetArchive(dir, target);
    assertArchiveChecksum(archive);
    return archiveInventory(archive, target);
  });
  const evaluators = EVALUATORS.map((evaluator) => {
    const occurrences = archives.map((archive) => {
      const member = archive.members.find((entry) => entry.name === evaluator.name);
      if (!member) {
        throw new Error(`${archive.name} is missing ${evaluator.name}`);
      }
      return member;
    });
    const digests = new Set(occurrences.map((entry) => entry.sha256));
    if (digests.size !== 1) {
      throw new Error(`${evaluator.name} differs across release archives`);
    }
    const sizes = new Set(occurrences.map((entry) => entry.bytes));
    if (sizes.size !== 1) {
      throw new Error(`${evaluator.name} has inconsistent sizes across release archives`);
    }
    return {
      name: evaluator.name,
      target: evaluator.target,
      sha256: occurrences[0].sha256,
      bytes: occurrences[0].bytes,
    };
  });
  return {
    schema_version: 1,
    evaluators,
    archives: archives.sort((left, right) => left.name.localeCompare(right.name)),
  };
}

function archiveInventory(archive, target) {
  const listed = listArchive(archive);
  assertSafeArchiveMembers(archive, listed);
  const payloadName = `deadreckon-${target}`;
  const wrappedPrefix = `${payloadName}/`;
  const wrapped = hostHelpers(target).every((name) => listed.includes(`${wrappedPrefix}${name}`));
  const flat = hostHelpers(target).every((name) => listed.includes(name));
  if (wrapped === flat || (flat && !target.endsWith("windows-msvc"))) {
    throw new Error(`${path.basename(archive)} has an unsupported or ambiguous payload layout`);
  }
  const payloadPrefix = wrapped ? wrappedPrefix : "";
  const requiredNames = [...hostHelpers(target), ...EVALUATORS.map((entry) => entry.name)];
  for (const name of requiredNames) {
    const expectedPath = `${payloadPrefix}${name}`;
    const occurrences = listed.filter((entry) => entry === expectedPath);
    if (occurrences.length !== 1) {
      throw new Error(`${path.basename(archive)} must contain exactly one ${expectedPath}`);
    }
  }

  const members = listed
    .filter((entry) => !entry.endsWith("/"))
    .map((entry) => {
      const bytes = extractArchiveMember(archive, entry);
      const name = path.posix.basename(entry);
      const evaluator = EVALUATORS.find((candidate) => candidate.name === name);
      if (evaluator) {
        validateStaticLinuxElf(bytes, evaluator);
      }
      const gateBundleMember =
        name === "deadreckon" ||
        name === "deadreckon.exe" ||
        name === "dr-gate" ||
        name === "dr-gate.exe" ||
        name === "dr-capture" ||
        name === "dr-capture.exe" ||
        evaluator;
      const gateProtocolMember =
        name === "dr-gate" || name === "dr-gate.exe" || evaluator;
      const bundleBuildId = gateBundleMember
        ? gateProtocolMember
          ? requireGateBundleIdentity(bytes, name)
          : requireBundleBuildIdentity(bytes, name)
        : null;
      return {
        path: entry,
        name,
        role: evaluator
          ? "sandbox-evaluator"
          : hostHelpers(target).includes(name)
            ? "host-helper"
            : "supporting-file",
        sha256: sha256Bytes(bytes),
        bytes: bytes.length,
        target: evaluator?.target ?? (hostHelpers(target).includes(name) ? target : null),
        ...(bundleBuildId ? { bundle_build_id: bundleBuildId } : {}),
      };
    })
    .sort((left, right) => left.path.localeCompare(right.path));

  const bundleBuildIds = new Set(
    members.filter((member) => member.bundle_build_id).map((member) => member.bundle_build_id),
  );
  if (bundleBuildIds.size !== 1) {
    throw new Error(
      `${path.basename(archive)} mixes incompatible DeadReckon gate build bundles: ${[
        ...bundleBuildIds,
      ].join(", ")}`,
    );
  }

  return {
    name: path.basename(archive),
    target,
    sha256: sha256File(archive),
    bytes: fs.statSync(archive).size,
    members,
  };
}

function verifySidecarDirectory(dir, target = null) {
  const evaluators = target
    ? EVALUATORS.filter((evaluator) => evaluator.target === target)
    : EVALUATORS;
  if (evaluators.length === 0) {
    throw new Error(`unsupported evaluator target ${target}`);
  }
  const verified = evaluators.map((evaluator) => {
    const matches = [];
    walk(dir, (file) => {
      if (path.basename(file) === evaluator.name) {
        matches.push(file);
      }
    });
    if (matches.length !== 1) {
      throw new Error(`${dir} must contain exactly one ${evaluator.name}; found ${matches.length}`);
    }
    const bytes = fs.readFileSync(matches[0]);
    validateStaticLinuxElf(bytes, evaluator);
    const bundleBuildId = requireGateBundleIdentity(bytes, evaluator.name);
    return {
      name: evaluator.name,
      target: evaluator.target,
      path: matches[0],
      sha256: sha256Bytes(bytes),
      bytes: bytes.length,
      bundle_build_id: bundleBuildId,
    };
  });
  const bundleBuildIds = new Set(verified.map((entry) => entry.bundle_build_id));
  if (bundleBuildIds.size !== 1) {
    throw new Error(
      `gate evaluator sidecars mix incompatible DeadReckon build bundles: ${[
        ...bundleBuildIds,
      ].join(", ")}`,
    );
  }
  return verified;
}

function requireGateBundleIdentity(bytes, name) {
  const text = bytes.toString("latin1");
  if (!text.includes(GATE_PROTOCOL_MARKER)) {
    throw new Error(`${name} is missing ${GATE_PROTOCOL_MARKER}`);
  }
  return requireBundleBuildIdentity(bytes, name);
}

function requireBundleBuildIdentity(bytes, name) {
  const text = bytes.toString("latin1");
  const identities = [...new Set(text.match(BUNDLE_BUILD_ID_PATTERN) ?? [])];
  if (identities.length !== 1) {
    throw new Error(`${name} must embed exactly one DeadReckon bundle build identity`);
  }
  return identities[0];
}

function validateStaticLinuxElf(bytes, evaluator) {
  if (bytes.length < 64 || !bytes.subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46]))) {
    throw new Error(`${evaluator.name} is not an ELF binary`);
  }
  if (bytes[4] !== 2 || bytes[5] !== 1) {
    throw new Error(`${evaluator.name} must be a little-endian ELF64 binary`);
  }
  const machine = bytes.readUInt16LE(18);
  if (machine !== evaluator.machine) {
    throw new Error(
      `${evaluator.name} has ELF machine 0x${machine.toString(16)}, expected 0x${evaluator.machine.toString(16)}`,
    );
  }
  const programHeaderOffset = Number(bytes.readBigUInt64LE(32));
  const programHeaderSize = bytes.readUInt16LE(54);
  const programHeaderCount = bytes.readUInt16LE(56);
  if (programHeaderOffset < 64 || programHeaderSize < 56 || programHeaderCount === 0) {
    throw new Error(`${evaluator.name} has no valid ELF64 program-header table`);
  }
  const tableEnd = programHeaderOffset + programHeaderSize * programHeaderCount;
  if (!Number.isSafeInteger(tableEnd) || tableEnd > bytes.length) {
    throw new Error(`${evaluator.name} has an out-of-bounds ELF program-header table`);
  }
  for (let index = 0; index < programHeaderCount; index += 1) {
    const offset = programHeaderOffset + index * programHeaderSize;
    if (bytes.readUInt32LE(offset) === 3) {
      throw new Error(`${evaluator.name} contains PT_INTERP and is not statically linked`);
    }
  }
}

function patchInstallers(dir) {
  const shellInstallers = findNamedFiles(dir, "deadreckon-installer.sh");
  const powershellInstallers = findNamedFiles(dir, "deadreckon-installer.ps1");
  if (shellInstallers.length === 0 || powershellInstallers.length === 0) {
    throw new Error(`${dir} must contain deadreckon-installer.sh and deadreckon-installer.ps1`);
  }
  for (const installer of shellInstallers) {
    const original = fs.readFileSync(installer, "utf8");
    const patched = original
      .replace(/^(\s*_bins=")([^"]*deadreckon[^"]*)(")\s*$/gm, (_line, prefix, bins, suffix) => {
        return `${prefix}${appendInstallerNames(bins, bins.includes(".exe"))}${suffix}`;
      })
      .replace(
        /^(\s*_bins_js_array=')([^']*deadreckon[^']*)(')\s*$/gm,
        (_line, prefix, bins, suffix) => {
          const names = [...bins.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
          return `${prefix}${appendInstallerNames(names.join(" "), names.some((name) => name.endsWith(".exe")))
            .split(/\s+/)
            .map((name) => `"${name}"`)
            .join(",")}${suffix}`;
        },
      );
    fs.writeFileSync(installer, patched);
  }
  for (const installer of powershellInstallers) {
    const original = fs.readFileSync(installer, "utf8");
    const patched = original.replace(
      /("bins"\s*=\s*@\()([^)]*deadreckon[^)]*)(\))/g,
      (_line, prefix, bins, suffix) => {
        const names = [...bins.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
        const patchedNames = appendInstallerNames(names.join(" "), true)
          .split(/\s+/)
          .map((name) => `"${name}"`)
          .join(", ");
        return `${prefix}${patchedNames}${suffix}`;
      },
    );
    fs.writeFileSync(installer, patched);
  }
  verifyInstallers(dir);
}

function appendInstallerNames(value, windows) {
  const names = value.trim().split(/\s+/).filter(Boolean);
  const expected = [...hostHelpers(windows ? "x86_64-pc-windows-msvc" : "x86_64-apple-darwin"), ...EVALUATORS.map((entry) => entry.name)];
  for (const name of expected) {
    if (!names.includes(name)) {
      names.push(name);
    }
  }
  return names.join(" ");
}

function verifyInstallers(dir) {
  const shellInstallers = findNamedFiles(dir, "deadreckon-installer.sh");
  const powershellInstallers = findNamedFiles(dir, "deadreckon-installer.ps1");
  if (shellInstallers.length === 0 || powershellInstallers.length === 0) {
    throw new Error(`${dir} must contain deadreckon-installer.sh and deadreckon-installer.ps1`);
  }
  for (const installer of shellInstallers) {
    const text = fs.readFileSync(installer, "utf8");
    for (const target of HOST_TARGETS) {
      const archiveName = target.endsWith("windows-msvc")
        ? `deadreckon-${target}.zip`
        : `deadreckon-${target}.tar.xz`;
      const start = text.indexOf(`"${archiveName}")`);
      if (start === -1) {
        throw new Error(`${path.basename(installer)} has no case for ${archiveName}`);
      }
      const end = text.indexOf(";;", start);
      const block = text.slice(start, end === -1 ? text.length : end);
      const bins = /^\s*_bins="([^"]+)"/m.exec(block);
      if (!bins) {
        throw new Error(`${path.basename(installer)} has no _bins install list for ${target}`);
      }
      const installed = new Set(bins[1].split(/\s+/).filter(Boolean));
      for (const name of [...hostHelpers(target), ...EVALUATORS.map((entry) => entry.name)]) {
        if (!installed.has(name)) {
          throw new Error(`${path.basename(installer)} does not install ${name} for ${target}`);
        }
      }
    }
  }
  for (const installer of powershellInstallers) {
    const text = fs.readFileSync(installer, "utf8");
    const archiveName = "deadreckon-x86_64-pc-windows-msvc.zip";
    const blocks = [...text.matchAll(new RegExp(`"artifact_name"\\s*=\\s*"${escapeRegExp(archiveName)}"[\\s\\S]*?"bins"\\s*=\\s*@\\(([^)]*)\\)`, "g"))];
    if (blocks.length === 0) {
      throw new Error(`${path.basename(installer)} has no install block for ${archiveName}`);
    }
    for (const block of blocks) {
      for (const name of [
        ...hostHelpers("x86_64-pc-windows-msvc"),
        ...EVALUATORS.map((entry) => entry.name),
      ]) {
        if (!block[1].includes(`"${name}"`)) {
          throw new Error(`${path.basename(installer)} does not install ${name}`);
        }
      }
    }
  }
  writeJson({
    ok: true,
    shell_installers: shellInstallers.length,
    powershell_installers: powershellInstallers.length,
  });
}

function hostHelpers(target) {
  return target.endsWith("windows-msvc")
    ? ["deadreckon.exe", "dr-gate.exe", "dr-capture.exe"]
    : ["deadreckon", "dr-gate", "dr-capture"];
}

function assertHostHelpers(payload, target) {
  for (const name of hostHelpers(target)) {
    const file = path.join(payload, name);
    if (!fs.existsSync(file) || !fs.statSync(file).isFile()) {
      throw new Error(`${path.basename(payload)} is missing native helper ${name}`);
    }
  }
}

function payloadDirectory(extractDir, target) {
  const expected = path.join(extractDir, `deadreckon-${target}`);
  const topLevel = fs
    .readdirSync(extractDir, { withFileTypes: true })
    .filter((entry) => entry.name !== ".DS_Store");
  if (fs.existsSync(expected) && fs.statSync(expected).isDirectory()) {
    if (topLevel.length !== 1 || topLevel[0].name !== `deadreckon-${target}`) {
      throw new Error(`archive must contain only top-level directory deadreckon-${target}`);
    }
    return expected;
  }
  if (target.endsWith("windows-msvc") && topLevel.length > 0 && topLevel.every((entry) => entry.isFile())) {
    return extractDir;
  }
  throw new Error(
    `archive must contain wrapped deadreckon-${target} payload or a flat Windows payload`,
  );
}

function oneTargetArchive(dir, target) {
  const matches = [];
  walk(dir, (file) => {
    const basename = path.basename(file);
    if (
      basename === `deadreckon-${target}.tar.xz` ||
      basename === `deadreckon-${target}.tar.gz` ||
      basename === `deadreckon-${target}.tgz` ||
      basename === `deadreckon-${target}.zip`
    ) {
      matches.push(file);
    }
  });
  if (matches.length === 0) {
    throw new Error(`no release archive found for ${target} under ${dir}`);
  }
  const byDigest = new Map(matches.map((file) => [sha256File(file), file]));
  if (byDigest.size !== 1) {
    throw new Error(`conflicting duplicate release archives found for ${target} under ${dir}`);
  }
  return [...byDigest.values()][0];
}

function listArchive(archive) {
  const result = archive.endsWith(".zip")
    ? process.platform === "win32"
      ? listZipArchiveOnWindows(archive)
      : spawnSync("unzip", ["-Z1", archive], { encoding: "utf8" })
    : spawnSync("tar", ["-tf", archive], { encoding: "utf8" });
  if (result.error || result.status !== 0) {
    const detail = result.error?.message ?? result.stderr?.toString("utf8") ?? "unknown error";
    throw new Error(`could not list ${archive}: ${detail}`);
  }
  return result.stdout.split(/\r?\n/).filter(Boolean);
}

function extractArchive(archive, destination) {
  if (archive.endsWith(".zip")) {
    if (process.platform === "win32") {
      extractZipArchiveOnWindows(archive, destination);
    } else {
      run("unzip", ["-q", archive, "-d", destination]);
    }
  } else {
    run("tar", ["-xf", archive, "-C", destination]);
  }
}

function extractArchiveMember(archive, member) {
  if (archive.endsWith(".zip") && process.platform === "win32") {
    return extractZipArchiveMemberOnWindows(archive, member);
  }
  const options = {
    encoding: "buffer",
    maxBuffer: MAX_ARCHIVE_MEMBER_BYTES,
  };
  const result = archive.endsWith(".zip")
    ? spawnSync("unzip", ["-p", archive, member], options)
    : spawnSync("tar", ["-xOf", archive, member], options);
  if (result.error || result.status !== 0) {
    const detail = result.error?.message ?? result.stderr?.toString("utf8") ?? "unknown error";
    throw new Error(`could not extract ${member} from ${archive}: ${detail}`);
  }
  return result.stdout;
}

function listZipArchiveOnWindows(archive) {
  const script = [
    "$ErrorActionPreference='Stop'",
    "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)",
    "Add-Type -AssemblyName System.IO.Compression",
    "Add-Type -AssemblyName System.IO.Compression.FileSystem",
    `$zip=[System.IO.Compression.ZipFile]::OpenRead('${escapePowerShell(archive)}')`,
    "try { foreach ($entry in $zip.Entries) { [Console]::Out.WriteLine($entry.FullName) } } finally { $zip.Dispose() }",
  ].join("; ");
  return spawnSync("powershell", ["-NoProfile", "-Command", script], {
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
}

function extractZipArchiveOnWindows(archive, destination) {
  run("powershell", [
    "-NoProfile",
    "-Command",
    `$ErrorActionPreference='Stop'; Expand-Archive -LiteralPath '${escapePowerShell(archive)}' -DestinationPath '${escapePowerShell(destination)}' -Force`,
  ]);
}

function extractZipArchiveMemberOnWindows(archive, member) {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "deadreckon-zip-member-"));
  const output = path.join(temp, "member.bin");
  const script = [
    "$ErrorActionPreference='Stop'",
    "Add-Type -AssemblyName System.IO.Compression",
    "Add-Type -AssemblyName System.IO.Compression.FileSystem",
    `$zip=[System.IO.Compression.ZipFile]::OpenRead('${escapePowerShell(archive)}')`,
    `try { $matches=@($zip.Entries | Where-Object { $_.FullName -ceq '${escapePowerShell(member)}' }); if ($matches.Count -ne 1) { throw 'expected exactly one archive member ${escapePowerShell(member)}' }; [System.IO.Compression.ZipFileExtensions]::ExtractToFile($matches[0], '${escapePowerShell(output)}', $true) } finally { $zip.Dispose() }`,
  ].join("; ");
  try {
    run("powershell", ["-NoProfile", "-Command", script]);
    const size = fs.statSync(output).size;
    if (size > MAX_ARCHIVE_MEMBER_BYTES) {
      throw new Error(
        `archive member ${member} is ${size} bytes, exceeding the ${MAX_ARCHIVE_MEMBER_BYTES}-byte inventory limit`,
      );
    }
    return fs.readFileSync(output);
  } finally {
    fs.rmSync(temp, { recursive: true, force: true });
  }
}

function repackArchive(archive, sourceDir) {
  const entries = fs.readdirSync(sourceDir).filter((name) => name !== ".DS_Store").sort();
  if (entries.length === 0) {
    throw new Error(`nothing to repack in ${sourceDir}`);
  }
  const suffix = archiveSuffix(archive);
  const staged = `${archive.slice(0, -suffix.length)}.assembled-${process.pid}${suffix}`;
  if (fs.existsSync(staged)) {
    fs.unlinkSync(staged);
  }
  if (suffix === ".zip") {
    if (process.platform === "win32") {
      createZipArchiveOnWindows(staged, sourceDir);
    } else {
      run("zip", ["-X", "-q", "-r", staged, ...entries], { cwd: sourceDir });
    }
  } else {
    const flag = suffix === ".tar.xz" ? "-cJf" : "-czf";
    run("tar", [flag, staged, "-C", sourceDir, ...entries], {
      env: { ...process.env, COPYFILE_DISABLE: "1" },
    });
  }
  fs.copyFileSync(staged, archive);
  fs.unlinkSync(staged);
}

function createZipArchiveOnWindows(archive, sourceDir) {
  const files = [];
  walk(sourceDir, (file) => files.push(file));
  if (files.length === 0) {
    throw new Error(`nothing to add to Windows ZIP from ${sourceDir}`);
  }
  const script = [
    "$ErrorActionPreference='Stop'",
    "Add-Type -AssemblyName System.IO.Compression",
    "Add-Type -AssemblyName System.IO.Compression.FileSystem",
    `$root=[System.IO.Path]::GetFullPath('${escapePowerShell(sourceDir)}')`,
    "if (!$root.EndsWith([System.IO.Path]::DirectorySeparatorChar.ToString())) { $root += [System.IO.Path]::DirectorySeparatorChar }",
    "$items=@(Get-ChildItem -LiteralPath $root -Recurse -File | Sort-Object FullName)",
    "if ($items.Count -eq 0) { throw 'nothing to add to Windows ZIP' }",
    `$zip=[System.IO.Compression.ZipFile]::Open('${escapePowerShell(archive)}',[System.IO.Compression.ZipArchiveMode]::Create)`,
    "$timestamp=[DateTimeOffset]::Parse('1980-01-02T00:00:00Z')",
    "try { foreach ($item in $items) { $name=$item.FullName.Substring($root.Length).Replace([char]92,[char]47); $entry=$zip.CreateEntry($name,[System.IO.Compression.CompressionLevel]::Optimal); $entry.LastWriteTime=$timestamp; $source=$item.OpenRead(); $target=$entry.Open(); try { $source.CopyTo($target) } finally { $target.Dispose(); $source.Dispose() } } } finally { $zip.Dispose() }",
  ].join("; ");
  try {
    run("powershell", ["-NoProfile", "-Command", script]);
  } catch (error) {
    fs.rmSync(archive, { force: true });
    throw error;
  }
}

function archiveSuffix(archive) {
  for (const suffix of [".tar.xz", ".tar.gz", ".tgz", ".zip"]) {
    if (archive.endsWith(suffix)) {
      return suffix;
    }
  }
  throw new Error(`unsupported release archive: ${archive}`);
}

function assertSafeArchiveMembers(archive, members) {
  for (const member of members) {
    const normalized = member.replaceAll("\\", "/");
    if (
      normalized === "." ||
      normalized === "./" ||
      normalized.startsWith("./") ||
      normalized.startsWith("/") ||
      normalized.split("/").includes("..")
    ) {
      throw new Error(`${path.basename(archive)} contains unsafe archive member ${member}`);
    }
  }
}

function findNamedFiles(dir, name) {
  const matches = [];
  walk(dir, (file) => {
    if (path.basename(file) === name) {
      matches.push(file);
    }
  });
  return matches.sort();
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

function requireHostTarget(target) {
  if (!HOST_TARGETS.includes(target)) {
    throw new Error(`unsupported host target ${target ?? ""}; expected one of ${HOST_TARGETS.join(", ")}`);
  }
  return target;
}

function run(program, programArgs, options = {}) {
  const result = spawnSync(program, programArgs, { encoding: "utf8", ...options });
  if (result.error || result.status !== 0) {
    const detail = result.error?.message ? `\nerror:\n${result.error.message}` : "";
    throw new Error(
      `${program} ${programArgs.join(" ")} failed${detail}\nstdout:\n${result.stdout ?? ""}\nstderr:\n${result.stderr ?? ""}`,
    );
  }
}

function sha256File(file) {
  return sha256Bytes(fs.readFileSync(file));
}

function sha256Bytes(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

function escapePowerShell(value) {
  return value.replaceAll("'", "''");
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function required(value, name) {
  if (!value) {
    throw new Error(`${name} is required`);
  }
  return value;
}

function writeJson(value) {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}
