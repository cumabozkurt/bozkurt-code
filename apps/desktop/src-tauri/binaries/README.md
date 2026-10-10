# Sidecar binaries

Release builds bundle `bozkurtd`, `bz` and `bozkurt-mcp` as Tauri sidecars (`bundle.externalBin` in
`../tauri.sidecar.conf.json`). CI copies the compiled binaries here as `<name>-<target-triple>[.exe]`
(e.g. `bozkurtd-aarch64-apple-darwin`) and builds with two config overlays:

```sh
TRIPLE=aarch64-apple-darwin
cargo build --release --locked --target $TRIPLE -p bz-daemon -p bz-cli -p bz-mcp
for b in bozkurtd bz bozkurt-mcp; do
  cp target/$TRIPLE/release/$b apps/desktop/src-tauri/binaries/$b-$TRIPLE
done
npm run tauri -w @bozkurt/desktop -- build --target $TRIPLE --config src-tauri/tauri.sidecar.conf.json
```

`../tauri.release.conf.json` (used by `release.yml` only) adds `THIRD_PARTY_LICENSES.md` as a resource
and the signed updater artifacts; it needs `TAURI_SIGNING_PRIVATE_KEY`, so local builds leave it out.

The base `tauri.conf.json` does not list the sidecars so that `cargo check` / `tauri dev` work without
them; in that case the app looks for `bozkurtd` next to its own executable and then on `PATH`.
Staged binaries in this folder are build output — do not commit them.

Where the sidecars end up after installation: macOS `BOZKURT CODE.app/Contents/MacOS/`, Linux deb/rpm
`/usr/bin/`, Windows the install directory (next to `bozkurt-code.exe`).
