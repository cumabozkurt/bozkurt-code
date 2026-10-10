# BOZKURT CODE — mobile companion (iOS / Android)

Tauri 2 mobile shell around `packages/ui` in **remote mode**. The first screen pairs with a desktop:

1. Desktop: Settings → Remote access → enable → *Generate pairing code* (QR + 8 digits, 5 min).
2. Phone: *Scan QR code* (camera, `tauri-plugin-barcode-scanner`) or type `host:7879` + code.
3. The app calls `POST /remote/pair` and stores `{baseUrl, token}`; every request then sends
   `Authorization: Bearer <token>` (the WebSocket uses `?token=` because browsers cannot set headers).

QR payload: `bozkurt://pair?host=<ip:port>&code=<8 digits>`. Opening that link also pre-fills and
submits the form (deep link). *Try the demo* (or `?mock`) runs the UI on the in-memory mock backend.
On narrow screens the UI switches to a bottom tab bar (Overview, Sites, Runtimes, Services, More).

## Build

```sh
npm ci
npm run tauri -w @bozkurt/mobile -- android init       # generates src-tauri/gen/android (not committed)
node apps/mobile/scripts/patch-android.mjs             # allow cleartext HTTP to the LAN daemon in release
npm run tauri -w @bozkurt/mobile -- android build --apk

npm run tauri -w @bozkurt/mobile -- ios init            # macOS + Xcode; generates src-tauri/gen/apple
npm run tauri -w @bozkurt/mobile -- ios build
```

`Info.ios.plist` is merged into the generated Info.plist (camera + local-network usage strings,
`NSAllowsLocalNetworking`). Icons in `src-tauri/icons` are generated from the BOZKURT CODE SVG
(`packages/ui/src/assets/app-icon.svg`).
