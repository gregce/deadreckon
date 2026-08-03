import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const TOOL = path.join(ROOT, "release", "evaluator-sidecars.mjs");
const TARGET = "x86_64-pc-windows-msvc";

test(
  "Windows assembles and inventories a cargo-dist ZIP without relying on tar",
  { skip: process.platform !== "win32" },
  () => {
    const temp = fs.mkdtempSync(path.join(os.tmpdir(), "deadreckon-windows-zip-smoke-"));
    try {
      const distrib = path.join(temp, "distrib");
      const sidecars = path.join(temp, "sidecars");
      const payload = path.join(temp, "payload", `deadreckon-${TARGET}`);
      fs.mkdirSync(distrib, { recursive: true });
      fs.mkdirSync(sidecars, { recursive: true });
      fs.mkdirSync(payload, { recursive: true });

      for (const helper of ["deadreckon.exe", "dr-gate.exe", "dr-capture.exe"]) {
        fs.writeFileSync(path.join(payload, helper), `${helper} native helper`);
      }
      writeFakeStaticElf(
        path.join(sidecars, "dr-gate-evaluator-aarch64-unknown-linux-musl"),
        0xb7,
        2 * 1024 * 1024,
      );
      writeFakeStaticElf(
        path.join(sidecars, "dr-gate-evaluator-x86_64-unknown-linux-musl"),
        0x3e,
        3 * 1024 * 1024,
      );

      const archive = path.join(distrib, `deadreckon-${TARGET}.zip`);
      run("powershell", [
        "-NoProfile",
        "-Command",
        `$ErrorActionPreference='Stop'; Compress-Archive -Path '${escapePowerShell(path.join(payload, "*"))}' -DestinationPath '${escapePowerShell(archive)}' -CompressionLevel Optimal -Force`,
      ]);
      fs.writeFileSync(
        `${archive}.sha256`,
        `${"0".repeat(64)} *${path.basename(archive)}\n`,
      );

      const assembled = run(process.execPath, [
        TOOL,
        "assemble",
        "--dir",
        distrib,
        "--target",
        TARGET,
        "--sidecars-dir",
        sidecars,
      ]);
      const inventory = JSON.parse(assembled.stdout);
      assert.equal(inventory.target, TARGET);
      assert.deepEqual(
        inventory.members.map((member) => member.name).sort(),
        [
          "deadreckon.exe",
          "dr-capture.exe",
          "dr-gate-evaluator-aarch64-unknown-linux-musl",
          "dr-gate-evaluator-x86_64-unknown-linux-musl",
          "dr-gate.exe",
        ].sort(),
      );

      run(process.execPath, [
        TOOL,
        "refresh-checksum",
        "--dir",
        distrib,
        "--target",
        TARGET,
      ]);
      run(process.execPath, [
        TOOL,
        "verify-archive",
        "--dir",
        distrib,
        "--target",
        TARGET,
      ]);
    } finally {
      fs.rmSync(temp, { recursive: true, force: true });
    }
  },
);

function writeFakeStaticElf(file, machine, size) {
  const bytes = Buffer.alloc(Math.max(size, 64 + 56));
  Buffer.from([0x7f, 0x45, 0x4c, 0x46]).copy(bytes, 0);
  bytes[4] = 2;
  bytes[5] = 1;
  bytes[6] = 1;
  bytes.writeUInt16LE(3, 16);
  bytes.writeUInt16LE(machine, 18);
  bytes.writeUInt32LE(1, 20);
  bytes.writeBigUInt64LE(64n, 32);
  bytes.writeUInt16LE(64, 52);
  bytes.writeUInt16LE(56, 54);
  bytes.writeUInt16LE(1, 56);
  bytes.writeUInt32LE(1, 64);
  fs.writeFileSync(file, bytes);
}

function run(program, args) {
  const result = spawnSync(program, args, {
    cwd: ROOT,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
  assert.equal(
    result.status,
    0,
    `${program} ${args.join(" ")} failed\nerror: ${result.error?.message ?? ""}\nstdout:\n${result.stdout ?? ""}\nstderr:\n${result.stderr ?? ""}`,
  );
  return result;
}

function escapePowerShell(value) {
  return value.replaceAll("'", "''");
}
