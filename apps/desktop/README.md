# BOZKURT CODE — desktop (Tauri 2)

Wraps `@bozkurt/ui` (local mode → `http://127.0.0.1:7878/api/v1`).

Rust side (`src-tauri/src/lib.rs`):
- Tray / menu-bar menu: status line, *Switch PHP ▸*, *Switch Node ▸*, start all, stop all, open,
  command palette, launch at login, quit. Refreshed every 5 s from `/overview`; actions call the daemon API.
- Starts `bozkurtd` if `/health` fails: bundled sidecar in release builds (see `src-tauri/binaries/README.md`),
  otherwise `bozkurtd` next to the app executable or on `PATH`. A daemon we started is stopped on quit.
- `bozkurt://` deep links (`bozkurt://sites/<id>`, `bozkurt://runtimes/php`, `bozkurt://palette`, …).
- Plugins: dialog (folder picker), opener, shell (sidecar), deep-link, single-instance, autostart.
- Commands `open_in_editor` (VS Code CLI → `vscode://` fallback) and `open_terminal`.
- Closing the window hides it to the tray.

```sh
npm run tauri -w @bozkurt/desktop -- dev        # needs bozkurtd running or on PATH
VITE_BOZKURT_MOCK=1 npm run tauri -w @bozkurt/desktop -- dev   # UI against the mock backend
npm run tauri -w @bozkurt/desktop -- build --config src-tauri/tauri.sidecar.conf.json
```

Bundle targets: app/dmg (macOS), msi/nsis (Windows), deb/rpm/appimage (Linux). Identifier `tr.cumabozkurt.bozkurtcode` (the mobile app is `tr.cumabozkurt.bozkurtcode.mobile`).
Icons in `src-tauri/icons` were generated with `tauri icon` from `packages/ui/src/assets/app-icon.svg`.
Linux build deps: `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev libxdo-dev`.
