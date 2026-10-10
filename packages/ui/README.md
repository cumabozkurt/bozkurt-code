# @bozkurt/ui

Shared React 18 + TypeScript UI for BOZKURT CODE (desktop, mobile, browser).

- `src/api/types.ts`, `src/api/client.ts` — typed client hand-written from `docs/API.md`
  (fetch + WebSocket `/events` with exponential-backoff reconnect, configurable base URL, bearer token).
  Browsers can't set WebSocket headers, so in remote mode the token is sent as `/events?token=…`.
- `src/api/mock.ts` — in-memory daemon with realistic data, simulated jobs (`job.progress/done/failed`)
  and live log events. Enabled by `?mock` in the URL, `VITE_BOZKURT_MOCK=1`, or `localStorage["bozkurt.mock"]="1"`.
- `src/i18n` — English + Türkçe (complete, type-checked: `tr` must cover every `en` key). Default from
  `navigator.language`, switchable in Settings and the command palette.
- Screens: Overview (+ Quick Setup wizard), Sites, Runtimes, Services, Logs, Doctor, Settings (remote
  pairing QR), Jobs drawer, command palette (Ctrl/⌘ K), keyboard shortcuts (`?`), toasts.
- Design tokens are CSS variables in `src/styles/index.css` (light/dark/system). Logo: `src/assets/logo.svg`.

```sh
npm run dev -w @bozkurt/ui          # playground on http://localhost:5173/?mock (demo data)
npm run dev -w @bozkurt/ui          # …or without ?mock: real bozkurtd via the /api/v1 proxy
BOZKURT_API_TARGET=http://127.0.0.1:17878 npm run dev -w @bozkurt/ui   # another daemon port
npm run build -w @bozkurt/ui        # type-check + library bundle in dist/
```

The playground proxies `/api/v1` (REST + `/events` WebSocket) to the daemon, so bozkurtd's CORS
allow-list (`tauri://localhost`, `localhost:1420/1430`) does not apply to it.

Hosts embed it with `<BozkurtApp mode="local" />` or `<BozkurtApp mode="remote" remote={{ baseUrl, token }} />`.
A host can drive navigation with `window.dispatchEvent(new CustomEvent("bozkurt:navigate", { detail: "/sites" }))`.
