import { StrictMode, useCallback, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  BozkurtApp, ConnectScreen, I18nProvider, isMockRequested, loadRemoteConnection, parsePairingUri, platform, saveRemoteConnection, type RemoteConnection,
} from "@bozkurt/ui";

type Pairing = { host: string; code: string } | null;

/** Picks up bozkurt://pair?host=…&code=… when the app was opened from a scanned QR code / link. */
async function initialPairing(): Promise<Pairing> {
  if (!platform.isTauri()) return parsePairingUri(new URLSearchParams(location.search).get("pair") ?? "");
  try {
    const { getCurrent } = await import("@tauri-apps/plugin-deep-link");
    for (const u of (await getCurrent()) ?? []) {
      const p = parsePairingUri(u);
      if (p) return p;
    }
  } catch {
    /* plugin unavailable */
  }
  return null;
}

function MobileRoot() {
  const [conn, setConn] = useState<RemoteConnection | null>(() => loadRemoteConnection());
  const [initial, setInitial] = useState<Pairing | undefined>(undefined);
  const demo = isMockRequested();
  useEffect(() => {
    void initialPairing().then(setInitial);
  }, []);
  const disconnect = useCallback(() => {
    saveRemoteConnection(null);
    localStorage.removeItem("bozkurt.mock");
    setConn(null);
    if (demo) location.reload();
  }, [demo]);

  if (demo) return <BozkurtApp mode="local" onDisconnect={disconnect} />;
  if (!conn) {
    if (initial === undefined) return null;
    return (
      <I18nProvider>
        <ConnectScreen
          initial={initial}
          onConnected={(c) => {
            saveRemoteConnection(c);
            setConn(c);
          }}
          onDemo={() => {
            localStorage.setItem("bozkurt.mock", "1");
            location.reload();
          }}
        />
      </I18nProvider>
    );
  }
  return <BozkurtApp mode="remote" remote={conn} onDisconnect={disconnect} />;
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <MobileRoot />
  </StrictMode>,
);
