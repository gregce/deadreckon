#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const args = parseArgs(process.argv.slice(2));
const dir = required(args.dir, "--dir");
const target = required(args.target, "--target");
const cert = required(args.cert, "--cert");
const password = required(args.password, "--password");
const timestampUrl = args.timestamp ?? "http://timestamp.digicert.com";

const archives = findArchives(dir, target);
if (archives.length === 0) {
  throw new Error(`no Windows release archives found for ${target} under ${dir}`);
}

const signedArtifacts = [];
for (const archive of archives) {
  signArchive(archive);
  signedArtifacts.push(path.relative(dir, archive).replaceAll(path.sep, "/"));
}

const trustDir = path.join(dir, "trust");
fs.mkdirSync(trustDir, { recursive: true });
fs.writeFileSync(
  path.join(trustDir, `windows-${target}.json`),
  `${JSON.stringify(
    {
      target,
      signed: true,
      signature_kind: "Authenticode",
      notarized: false,
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
  run("signtool", [
    "sign",
    "/f",
    cert,
    "/p",
    password,
    "/fd",
    "SHA256",
    "/tr",
    timestampUrl,
    "/td",
    "SHA256",
    binary,
  ]);
  run("signtool", ["verify", "/pa", "/v", binary]);
  repackArchive(archive, extractDir);
}

function findArchives(root, targetTriple) {
  const matches = [];
  walk(root, (file) => {
    const normalized = file.replaceAll(path.sep, "/");
    if (normalized.includes(targetTriple) && file.endsWith(".zip")) {
      matches.push(file);
    }
  });
  return matches.sort();
}

function extractArchive(archive, destination) {
  run("powershell", [
    "-NoProfile",
    "-Command",
    `Expand-Archive -LiteralPath '${escapePowerShell(archive)}' -DestinationPath '${escapePowerShell(destination)}' -Force`,
  ]);
}

function repackArchive(archive, sourceDir) {
  fs.rmSync(archive, { force: true });
  run("powershell", [
    "-NoProfile",
    "-Command",
    `Compress-Archive -Path '${escapePowerShell(path.join(sourceDir, "*"))}' -DestinationPath '${escapePowerShell(archive)}' -Force`,
  ]);
}

function findBinary(root) {
  const candidates = [];
  walk(root, (file) => {
    if (path.basename(file) === "deadreckon.exe") {
      candidates.push(file);
    }
  });
  if (candidates.length === 0) {
    throw new Error(`no deadreckon.exe found in extracted ${target} archive`);
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

function escapePowerShell(value) {
  return value.replaceAll("'", "''");
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
