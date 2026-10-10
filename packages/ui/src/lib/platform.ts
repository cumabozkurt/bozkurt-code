// Thin wrappers over Tauri plugins with browser fallbacks. Plugins are imported lazily so the
// UI still runs in a plain browser (mock/demo mode, vite preview).

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export function isMac(): boolean {
  return typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
}

export const modKey = () => (isMac() ? "⌘" : "Ctrl");

/** Native folder picker in Tauri; returns null in a browser (caller shows a text input). */
export async function pickFolder(title?: string): Promise<string | null> {
  if (!isTauri()) return null;
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const r = await open({ directory: true, multiple: false, title });
    return typeof r === "string" ? r : null;
  } catch {
    return null;
  }
}

export async function openUrl(url: string): Promise<void> {
  if (isTauri()) {
    try {
      const { openUrl: o } = await import("@tauri-apps/plugin-opener");
      await o(url);
      return;
    } catch {
      /* fall through */
    }
  }
  window.open(url, "_blank", "noopener,noreferrer");
}

/** Opens the project in the editor (desktop command: `code` CLI, else vscode:// handler). */
export async function openInEditor(path: string): Promise<void> {
  if (isTauri()) {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("open_in_editor", { path });
      return;
    } catch {
      /* command missing (mobile) → url */
    }
  }
  await openUrl(`vscode://file/${path.replace(/\\/g, "/").replace(/^\/+/, "")}`);
}

/** Opens a terminal at `path` (desktop command). Returns false when unsupported (caller copies `cd` command). */
export async function openTerminal(path: string): Promise<boolean> {
  if (!isTauri()) return false;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("open_terminal", { path });
    return true;
  } catch {
    return false;
  }
}

/** Scan a QR code with the camera (Tauri mobile barcode-scanner plugin). */
export async function scanQr(): Promise<string | null> {
  if (!isTauri()) return null;
  const { scan, Format, requestPermissions, checkPermissions } = await import("@tauri-apps/plugin-barcode-scanner");
  let perm = await checkPermissions();
  if (perm !== "granted") perm = await requestPermissions();
  if (perm !== "granted") return null;
  const r = await scan({ windowed: false, formats: [Format.QRCode] });
  return r?.content ?? null;
}

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export function deviceName(): string {
  const ua = typeof navigator !== "undefined" ? navigator.userAgent : "";
  if (/iPhone/.test(ua)) return "iPhone";
  if (/iPad/.test(ua)) return "iPad";
  if (/Android/.test(ua)) return "Android";
  return "Mobile device";
}
