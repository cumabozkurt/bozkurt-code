import { DEFAULT_BASE_URL, HttpApi } from "./client";
import { MockApi } from "./mock";
import type { BozkurtApi } from "./types";

export * from "./types";
export * from "./client";
export { MockApi } from "./mock";

export interface RemoteConnection {
  baseUrl: string;
  token: string;
  name?: string;
  /** Id of this device on the daemon (from the pair response), when known. */
  deviceId?: string;
}

const REMOTE_KEY = "bozkurt.remote";

export function loadRemoteConnection(): RemoteConnection | null {
  try {
    const raw = localStorage.getItem(REMOTE_KEY);
    return raw ? (JSON.parse(raw) as RemoteConnection) : null;
  } catch {
    return null;
  }
}
export function saveRemoteConnection(c: RemoteConnection | null) {
  if (c) localStorage.setItem(REMOTE_KEY, JSON.stringify(c));
  else localStorage.removeItem(REMOTE_KEY);
}

/** True when the mock backend should be used: `?mock` in URL/hash, VITE_BOZKURT_MOCK=1, or localStorage flag. */
export function isMockRequested(): boolean {
  if (typeof window === "undefined") return false;
  if (/[?&]mock(=1|=true|&|#|$)/.test(window.location.href)) return true;
  if (import.meta.env?.VITE_BOZKURT_MOCK === "1") return true;
  try {
    return localStorage.getItem("bozkurt.mock") === "1";
  } catch {
    return false;
  }
}

export interface CreateApiOptions {
  mode: "local" | "remote";
  remote?: RemoteConnection | null;
  baseUrl?: string;
}

export function createApi(opts: CreateApiOptions): BozkurtApi {
  if (isMockRequested()) return new MockApi();
  if (opts.mode === "remote" && opts.remote) return new HttpApi({ baseUrl: opts.remote.baseUrl, token: opts.remote.token });
  const envBase = (import.meta.env?.VITE_BOZKURT_API as string | undefined) || undefined;
  return new HttpApi({ baseUrl: opts.baseUrl ?? envBase ?? DEFAULT_BASE_URL });
}
