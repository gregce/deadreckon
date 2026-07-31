#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";

const target = process.argv[2];
if (!target) {
  throw new Error("usage: patch-formula.mjs <formula-file-or-directory>");
}

const HOST_HELPERS = ["deadreckon", "dr-gate", "dr-capture"];
const EVALUATOR_SIDECARS = [
  "dr-gate-evaluator-aarch64-unknown-linux-musl",
  "dr-gate-evaluator-x86_64-unknown-linux-musl",
];

function walk(root) {
  const stat = fs.statSync(root);
  if (stat.isFile()) {
    return root.endsWith(".rb") ? [root] : [];
  }
  return fs.readdirSync(root, { withFileTypes: true }).flatMap((entry) => {
    const entryPath = path.join(root, entry.name);
    return entry.isDirectory() ? walk(entryPath) : entryPath.endsWith(".rb") ? [entryPath] : [];
  });
}

function withRequires(formula) {
  if (formula.includes('require "json"') && formula.includes('require "time"')) {
    return formula;
  }
  return `require "fileutils"\nrequire "json"\nrequire "time"\n\n${formula}`;
}

function withReceiptMethod(formula) {
  if (formula.includes("def write_deadreckon_receipt!")) {
    return formula;
  }
  const method = `
  def write_deadreckon_receipt!
    receipt_dir = File.join(Dir.home, ".deadreckon")
    FileUtils.mkdir_p(receipt_dir)
    File.write(
      File.join(receipt_dir, "install-receipt.json"),
      JSON.pretty_generate({
        "channel" => "brew",
        "channel_version" => version.to_s,
        "binary_path" => File.join(bin, "deadreckon"),
        "installed_at" => Time.now.utc.iso8601,
        "install_source" => "brew:gregce/tap/deadreckon",
        "platform_package" => nil,
        "receipt_version" => 1,
      }) + "\\n",
    )
  end

`;
  const anchor = "  def install\n";
  if (!formula.includes(anchor)) {
    throw new Error("formula does not contain def install");
  }
  return formula.replace(anchor, `${method}${anchor}`);
}

function withReceiptCall(formula) {
  if (formula.includes("    write_deadreckon_receipt!\n")) {
    return formula;
  }
  const anchor = "    install_binary_aliases!\n";
  if (!formula.includes(anchor)) {
    throw new Error("formula does not contain install_binary_aliases!");
  }
  return formula.replace(anchor, `${anchor}\n    write_deadreckon_receipt!\n`);
}

function withCompleteBinaryInstall(formula) {
  let installLines = 0;
  const patched = formula.replace(/^(\s*)bin\.install\s+(.+)$/gm, (line, indent, rawNames) => {
    const names = [...rawNames.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
    if (!names.includes("deadreckon")) {
      return line;
    }
    installLines += 1;
    for (const helper of HOST_HELPERS) {
      if (!names.includes(helper)) {
        throw new Error(`formula bin.install is missing native helper ${helper}`);
      }
    }
    for (const evaluator of EVALUATOR_SIDECARS) {
      if (!names.includes(evaluator)) {
        names.push(evaluator);
      }
    }
    return `${indent}bin.install ${names.map((name) => `"${name}"`).join(", ")}`;
  });
  if (installLines === 0) {
    throw new Error("formula has no deadreckon bin.install lines");
  }
  return patched;
}

const formulae = walk(path.resolve(target));
if (formulae.length === 0) {
  throw new Error(`no Homebrew formulae found under ${target}`);
}

for (const formulaPath of formulae) {
  const original = fs.readFileSync(formulaPath, "utf8");
  const patched = withReceiptCall(
    withReceiptMethod(withRequires(withCompleteBinaryInstall(original))),
  );
  fs.writeFileSync(formulaPath, patched);
}
