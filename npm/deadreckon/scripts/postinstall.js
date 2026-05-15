#!/usr/bin/env node

const fs = require("node:fs");
const os = require("node:os");
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

function packageVersion() {
  return require(path.join(__dirname, "..", "package.json")).version;
}

function resolveBinary(packageName) {
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
  process.exit(0);
}

let binary;
try {
  binary = resolveBinary(packageName);
} catch (_) {
  process.exit(0);
}

const home = process.env.DEADRECKON_HOME || path.join(os.homedir(), ".deadreckon");
fs.mkdirSync(home, { recursive: true });
fs.writeFileSync(
  path.join(home, "install-receipt.json"),
  `${JSON.stringify(
    {
      channel: "npm",
      channel_version: packageVersion(),
      binary_path: binary,
      installed_at: new Date().toISOString(),
      install_source: `npm:deadreckon@${packageVersion()}`,
      platform_package: packageName,
      receipt_version: 1,
    },
    null,
    2,
  )}\n`,
);
