#!/usr/bin/env node

const { spawnSync } = require("node:child_process");
const path = require("node:path");

function platformPackage() {
  if (process.env.DEADRECKON_PLATFORM_PACKAGE) {
    return process.env.DEADRECKON_PLATFORM_PACKAGE;
  }
  const arch = process.arch === "x64" || process.arch === "arm64" ? process.arch : null;
  if (!arch) {
    return null;
  }
  if (process.platform === "darwin") {
    return `deadreckon-darwin-${arch}`;
  }
  if (process.platform === "linux") {
    return `deadreckon-linux-${arch}`;
  }
  if (process.platform === "win32" && arch === "x64") {
    return "deadreckon-win32-x64";
  }
  return null;
}

function binaryPath(packageName) {
  const packageJson = require.resolve(`${packageName}/package.json`, {
    paths: [__dirname],
  });
  const packageMeta = require(packageJson);
  const executable = packageMeta.bin && packageMeta.bin.deadreckon;
  if (!executable) {
    throw new Error(`${packageName} does not declare a deadreckon binary`);
  }
  return path.join(path.dirname(packageJson), executable);
}

const packageName = platformPackage();
if (!packageName) {
  console.error(`deadreckon does not ship an npm binary for ${process.platform}/${process.arch}`);
  console.error("try: curl -LsSf https://github.com/gdc/deadreckon/releases/latest/download/deadreckon-installer.sh | sh");
  process.exit(1);
}

let executable;
try {
  executable = binaryPath(packageName);
} catch (error) {
  console.error(`deadreckon platform package ${packageName} is not installed`);
  console.error("try: bun install -g deadreckon");
  process.exit(1);
}

const result = spawnSync(executable, process.argv.slice(2), {
  stdio: "inherit",
});

if (result.error) {
  console.error(`failed to run ${executable}: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
}
process.exit(result.status === null ? 1 : result.status);
