#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const platforms = [
  { name: "deadreckon-darwin-arm64", target: "aarch64-apple-darwin", binary: "deadreckon" },
  { name: "deadreckon-darwin-x64", target: "x86_64-apple-darwin", binary: "deadreckon" },
  { name: "deadreckon-linux-arm64", target: "aarch64-unknown-linux-gnu", binary: "deadreckon" },
  { name: "deadreckon-linux-x64", target: "x86_64-unknown-linux-gnu", binary: "deadreckon" },
  { name: "deadreckon-win32-x64", target: "x86_64-pc-windows-msvc", binary: "deadreckon.exe" },
];

function argValue(name) {
  const index = process.argv.indexOf(name);
  return index === -1 ? null : process.argv[index + 1];
}

const tag = argValue("--tag");
const artifacts = argValue("--artifacts") || "target/distrib";
if (!tag) {
  throw new Error("usage: prepare-release.mjs --tag <vX.Y.Z> [--artifacts target/distrib]");
}

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..", "..");
const npmRoot = path.join(repoRoot, "npm");
const version = tag.replace(/^v/, "");

function walk(dir) {
  if (!fs.existsSync(dir)) {
    return [];
  }
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const entryPath = path.join(dir, entry.name);
    return entry.isDirectory() ? walk(entryPath) : [entryPath];
  });
}

function listArchive(archive) {
  if (archive.endsWith(".zip")) {
    const result = spawnSync("unzip", ["-Z1", archive], { encoding: "utf8" });
    return result.status === 0 ? result.stdout.trim().split(/\r?\n/).filter(Boolean) : [];
  }
  const result = spawnSync("tar", ["-tf", archive], { encoding: "utf8" });
  return result.status === 0 ? result.stdout.trim().split(/\r?\n/).filter(Boolean) : [];
}

function extractArchive(archive, entry) {
  if (archive.endsWith(".zip")) {
    const result = spawnSync("unzip", ["-p", archive, entry], { encoding: "buffer" });
    if (result.status === 0) {
      return result.stdout;
    }
  } else {
    const result = spawnSync("tar", ["-xOf", archive, entry], { encoding: "buffer" });
    if (result.status === 0) {
      return result.stdout;
    }
  }
  throw new Error(`failed to extract ${entry} from ${archive}`);
}

function binaryBytes(artifactRoot, platform) {
  const files = walk(artifactRoot);
  const extracted = files.find((file) => {
    const normalized = file.replaceAll(path.sep, "/");
    return normalized.includes(platform.target) && path.basename(file) === platform.binary;
  });
  if (extracted) {
    return fs.readFileSync(extracted);
  }

  const archives = files.filter((file) => {
    const normalized = file.replaceAll(path.sep, "/");
    return (
      normalized.includes(platform.target) &&
      (file.endsWith(".tar.gz") ||
        file.endsWith(".tgz") ||
        file.endsWith(".tar.xz") ||
        file.endsWith(".zip"))
    );
  });
  for (const archive of archives) {
    const entry = listArchive(archive).find((candidate) => path.basename(candidate) === platform.binary);
    if (entry) {
      return extractArchive(archive, entry);
    }
  }
  throw new Error(`could not find ${platform.binary} for ${platform.target} under ${artifactRoot}`);
}

function writePlatformPackage(platform) {
  const packageDir = path.join(npmRoot, platform.name);
  const binDir = path.join(packageDir, "bin");
  fs.rmSync(binDir, { force: true, recursive: true });
  fs.mkdirSync(binDir, { recursive: true });

  const binaryPath = path.join(binDir, platform.binary);
  fs.writeFileSync(binaryPath, binaryBytes(path.resolve(artifacts), platform));
  fs.chmodSync(binaryPath, 0o755);

  const template = fs.readFileSync(path.join(packageDir, "package.json.template"), "utf8");
  fs.writeFileSync(path.join(packageDir, "package.json"), template.replaceAll("__VERSION__", version));
}

function writeWrapperPackage() {
  const packageJsonPath = path.join(npmRoot, "deadreckon", "package.json");
  const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
  packageJson.version = version;
  for (const platform of platforms) {
    packageJson.optionalDependencies[platform.name] = version;
  }
  fs.writeFileSync(packageJsonPath, `${JSON.stringify(packageJson, null, 2)}\n`);
}

for (const platform of platforms) {
  writePlatformPackage(platform);
}
writeWrapperPackage();
