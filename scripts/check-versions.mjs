#!/usr/bin/env node
// SPDX-License-Identifier: MIT OR Apache-2.0
// Fails (exit 1) unless every version string in the repo agrees, and — when a tag is
// given (`node scripts/check-versions.mjs v1.0.0`) — equals the tag without its "v".
// Used by release.yml (tag check) and ci.yml (drift check).
import { readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (p) => readFileSync(join(root, p), "utf8");
const json = (p) => JSON.parse(read(p));

const cargo = read("Cargo.toml");
const ws = cargo.match(/^\[workspace\.package\][^[]*?^version\s*=\s*"([^"]+)"/m);
const found = {
  "Cargo.toml [workspace.package].version": ws ? ws[1] : "(missing)",
  "package.json": json("package.json").version,
  "packages/ui/package.json": json("packages/ui/package.json").version,
  "apps/desktop/package.json": json("apps/desktop/package.json").version,
  "apps/desktop/package.json @bozkurt/ui dep": json("apps/desktop/package.json").dependencies?.["@bozkurt/ui"],
  "apps/mobile/package.json": json("apps/mobile/package.json").version,
  "apps/mobile/package.json @bozkurt/ui dep": json("apps/mobile/package.json").dependencies?.["@bozkurt/ui"],
  "apps/desktop/src-tauri/tauri.conf.json": json("apps/desktop/src-tauri/tauri.conf.json").version,
  "apps/mobile/src-tauri/tauri.conf.json": json("apps/mobile/src-tauri/tauri.conf.json").version,
};
// Cargo.lock entries of the workspace crates must have been regenerated too.
const lock = read("Cargo.lock");
for (const name of ["bz-core", "bz-net", "bz-daemon", "bz-cli", "bz-mcp", "bozkurt-desktop", "bozkurt-mobile"]) {
  const m = lock.match(new RegExp(`name = "${name}"\\nversion = "([^"]+)"`));
  found[`Cargo.lock ${name}`] = m ? m[1] : "(missing)";
}

const tag = process.argv[2];
const expected = tag ? tag.replace(/^v/, "") : found["Cargo.toml [workspace.package].version"];
let bad = 0;
for (const [where, v] of Object.entries(found)) {
  const ok = v === expected;
  if (!ok) bad++;
  console.log(`${ok ? "ok  " : "FAIL"} ${where}: ${v}`);
}
if (bad) {
  const msg = tag ? `tag ${tag} does not match ${bad} version string(s)` : `${bad} version string(s) differ from Cargo.toml (${expected})`;
  console.log(`::error::${msg}`);
  process.exit(1);
}
console.log(`all version strings = ${expected}`);
