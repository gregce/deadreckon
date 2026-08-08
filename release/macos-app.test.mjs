import assert from "node:assert/strict";
import crypto from "node:crypto";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const tool = path.join(import.meta.dirname, "macos-app.mjs");
const commit = "a".repeat(40);

test("hydrates both signed-archive resource shapes and pins the exact bytes", () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "deadreckon-mac-app-test-"));
  const distrib = path.join(temp, "target/distrib");
  const appRoot = path.join(temp, "deadreckon-mac");
  fs.mkdirSync(distrib, { recursive: true });
  fs.mkdirSync(path.join(appRoot, "Resources/bin"), { recursive: true });
  for (const target of ["aarch64-apple-darwin", "x86_64-apple-darwin"]) {
    writeArchive(temp, distrib, target);
  }

  const hydrated = run([
    "hydrate",
    "--dir",
    distrib,
    "--app-root",
    appRoot,
    "--version",
    "0.8.5-rc.1",
    "--commit",
    commit,
  ]);
  assert.equal(hydrated.status, 0, hydrated.stderr);

  const manifest = JSON.parse(
    fs.readFileSync(path.join(appRoot, "Resources/bin/manifest.json"), "utf8"),
  );
  assert.equal(manifest.releaseVersion, "0.8.5-rc.1");
  assert.equal(manifest.gitCommit, commit);
  assert.equal(manifest.complete, true);
  assert.equal(manifest.signed, true);
  assert.equal(manifest.sourceDirty, false);
  assert.deepEqual(Object.keys(manifest.sha256).sort(), ["arm64", "x86_64"]);
  assert.equal(
    manifest.sha256.arm64,
    sha256(path.join(appRoot, "Resources/bin/deadreckon_darwin_arm64")),
  );
  assert.equal(
    manifest.gateSha256.x86_64,
    sha256(path.join(appRoot, "Resources/libexec/deadreckon/dr-gate")),
  );

  const verified = run([
    "verify-resources",
    "--app-root",
    appRoot,
    "--version",
    "0.8.5-rc.1",
    "--commit",
    commit,
  ]);
  assert.equal(verified.status, 0, verified.stderr);

  fs.appendFileSync(path.join(appRoot, "Resources/bin/deadreckon_darwin_arm64"), "tamper");
  const tampered = run(["verify-resources", "--app-root", appRoot]);
  assert.notEqual(tampered.status, 0);
  assert.match(tampered.stderr, /sha256/);
});

test("refuses a partial official resource bundle", () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "deadreckon-mac-app-partial-"));
  const distrib = path.join(temp, "target/distrib");
  const appRoot = path.join(temp, "deadreckon-mac");
  fs.mkdirSync(distrib, { recursive: true });
  fs.mkdirSync(path.join(appRoot, "Resources/bin"), { recursive: true });
  writeArchive(temp, distrib, "aarch64-apple-darwin");

  const output = run([
    "hydrate",
    "--dir",
    distrib,
    "--app-root",
    appRoot,
    "--version",
    "0.8.5",
    "--commit",
    commit,
  ]);
  assert.notEqual(output.status, 0);
  assert.match(output.stderr, /x86_64-apple-darwin.*missing/);
  assert.equal(fs.existsSync(path.join(appRoot, "Resources/bin/manifest.json")), false);
  assert.equal(fs.existsSync(path.join(appRoot, "Resources/bin/deadreckon_darwin_arm64")), false);
  assert.equal(fs.existsSync(path.join(appRoot, "Resources/bin/dr-gate")), false);
});

function writeArchive(temp, distrib, target) {
  const payload = path.join(temp, "payload", `deadreckon-${target}`);
  fs.mkdirSync(payload, { recursive: true });
  for (const name of ["deadreckon", "dr-gate"]) {
    const file = path.join(payload, name);
    fs.writeFileSync(file, `#!/bin/sh\necho ${name}-${target}\n`, { mode: 0o755 });
  }
  const archive = path.join(distrib, `deadreckon-${target}.tar.xz`);
  const output = spawnSync(
    "tar",
    ["-cJf", archive, "-C", path.join(temp, "payload"), `deadreckon-${target}`],
    { encoding: "utf8" },
  );
  assert.equal(output.status, 0, output.stderr);
}

function run(args) {
  return spawnSync(process.execPath, [tool, ...args], { encoding: "utf8" });
}

function sha256(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}
