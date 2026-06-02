#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const args = parseArgs(process.argv.slice(2));
const dir = required(args.dir, "--dir");
const target = required(args.target, "--target");
const identity = args.identity ?? "Developer ID Application";

const archives = findArchives(dir, target);
if (archives.length === 0) {
  throw new Error(`no macOS release archives found for ${target} under ${dir}`);
}

const signedArtifacts = [];
for (const archive of archives) {
  signArchive(archive);
  signedArtifacts.push(path.relative(dir, archive).replaceAll(path.sep, "/"));
}

const trustDir = path.join(dir, "trust");
fs.mkdirSync(trustDir, { recursive: true });
fs.writeFileSync(
  path.join(trustDir, `macos-${target}.json`),
  `${JSON.stringify(
    {
      target,
      signed: true,
      signature_kind: "Developer ID Application",
      notarized: true,
      artifacts: signedArtifacts,
      verified_at: new Date().toISOString(),
    },
    null,
    2,
  )}\n`,
);

function signArchive(archive) {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), `deadreckon-sign-${target}-`));
  const extractDir = path.join(temp, "extract");
  fs.mkdirSync(extractDir);
  extractArchive(archive, extractDir);
  const binary = findBinary(extractDir);
  run("codesign", ["--sign", identity, "--options", "runtime", "--timestamp", binary]);
  run("codesign", ["--verify", "--verbose", binary]);

  const notarizationZip = path.join(temp, `${target}-notarization.zip`);
  run("ditto", ["-c", "-k", "--keepParent", binary, notarizationZip]);
  run("xcrun", [
    "notarytool",
    "submit",
    notarizationZip,
    "--apple-id",
    required(process.env.APPLE_ID, "APPLE_ID"),
    "--team-id",
    required(process.env.APPLE_TEAM_ID, "APPLE_TEAM_ID"),
    "--password",
    required(process.env.APPLE_APP_PWD, "APPLE_APP_PWD"),
    "--wait",
    "--timeout",
    "30m",
  ]);

  repackArchive(archive, extractDir);
}

function findArchives(root, targetTriple) {
  const matches = [];
  walk(root, (file) => {
    const normalized = file.replaceAll(path.sep, "/");
    if (
      normalized.includes(targetTriple) &&
      (file.endsWith(".tar.xz") || file.endsWith(".tar.gz") || file.endsWith(".tgz") || file.endsWith(".zip"))
    ) {
      matches.push(file);
    }
  });
  return matches.sort();
}

function extractArchive(archive, destination) {
  if (archive.endsWith(".zip")) {
    run("ditto", ["-x", "-k", archive, destination]);
  } else if (archive.endsWith(".tar.xz")) {
    run("tar", ["-xJf", archive, "-C", destination]);
  } else {
    run("tar", ["-xzf", archive, "-C", destination]);
  }
}

function repackArchive(archive, sourceDir) {
  fs.rmSync(archive, { force: true });
  if (archive.endsWith(".zip")) {
    run("ditto", ["-c", "-k", sourceDir, archive]);
  } else if (archive.endsWith(".tar.xz")) {
    run("tar", ["-cJf", archive, "-C", sourceDir, "."]);
  } else {
    run("tar", ["-czf", archive, "-C", sourceDir, "."]);
  }
}

function findBinary(root) {
  const candidates = [];
  walk(root, (file) => {
    if (path.basename(file) === "deadreckon") {
      candidates.push(file);
    }
  });
  if (candidates.length === 0) {
    throw new Error(`no deadreckon binary found in extracted ${target} archive`);
  }
  return candidates[0];
}

function walk(dir, visitor) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      walk(fullPath, visitor);
    } else {
      visitor(fullPath);
    }
  }
}

function run(cmd, cmdArgs) {
  const output = spawnSync(cmd, cmdArgs, { encoding: "utf8", stdio: "inherit" });
  if (output.status !== 0) {
    throw new Error(`${cmd} ${cmdArgs.join(" ")} failed`);
  }
}

function required(value, name) {
  if (!value) {
    throw new Error(`${name} is required`);
  }
  return value;
}

function parseArgs(values) {
  const parsed = {};
  for (let index = 0; index < values.length; index += 1) {
    const value = values[index];
    if (!value.startsWith("--")) {
      continue;
    }
    const key = value.slice(2);
    parsed[key] = values[index + 1];
    index += 1;
  }
  return parsed;
}
