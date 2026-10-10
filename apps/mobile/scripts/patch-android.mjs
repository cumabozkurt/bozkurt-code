// SPDX-License-Identifier: MIT OR Apache-2.0
// Run after `tauri android init` (CI and local release builds). The daemon's LAN listener is plain
// HTTP on a private address, so release builds must allow cleartext traffic (Tauri's template only
// allows it in debug builds). Fails (exit 1) when the generated project does not end up allowing it,
// so a changed Tauri template cannot silently ship an APK that cannot reach the daemon.
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "gen", "android", "app");
const gradle = ["build.gradle.kts", "build.gradle"].map((f) => join(root, f)).find(existsSync);
if (!gradle) {
  console.error("gen/android not found — run `npm run tauri -w @bozkurt/mobile -- android init` first");
  process.exit(1);
}
const src = readFileSync(gradle, "utf8");
const FALSE = /manifestPlaceholders\["usesCleartextTraffic"\]\s*=\s*"false"/g;
const TRUE = /manifestPlaceholders\["usesCleartextTraffic"\]\s*=\s*"true"/;
const out = src.replace(FALSE, 'manifestPlaceholders["usesCleartextTraffic"] = "true"');
if (FALSE.test(out) || !TRUE.test(out)) {
  console.error(
    `${gradle}: could not set usesCleartextTraffic=true (the Tauri Android template changed?).\n` +
      "Release APKs would be unable to reach the daemon over http://<lan-ip>:7879. Update this script.",
  );
  process.exit(1);
}
// Every build type must allow it: count the remaining placeholders that are not "true".
const remaining = [...out.matchAll(/manifestPlaceholders\["usesCleartextTraffic"\]\s*=\s*"([^"]*)"/g)].filter((m) => m[1] !== "true");
if (remaining.length) {
  console.error(`${gradle}: usesCleartextTraffic still "${remaining[0][1]}" in ${remaining.length} place(s)`);
  process.exit(1);
}
if (out !== src) writeFileSync(gradle, out);
console.log(out === src ? `${gradle}: usesCleartextTraffic already true` : `patched ${gradle}: usesCleartextTraffic=true`);
