import type {
  BozkurtApi,
  BzEvent,
  Config,
  ConnectionState,
  CreateSiteRequest,
  Detection,
  DnsInfo,
  DoctorCheck,
  ImportScan,
  Job,
  JobRef,
  Overview,
  PairingCode,
  PairResponse,
  PortOwner,
  RemoteInfo,
  Runtime,
  RuntimeKind,
  Service,
  ServiceAction,
  ServiceName,
  ShareState,
  Site,
  SiteAction,
  TlsInfo,
  Version,
} from "./types";

export const DEFAULT_BASE_URL = "http://127.0.0.1:7878/api/v1";

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    public readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

export interface HttpApiOptions {
  /** e.g. http://127.0.0.1:7878/api/v1 or http://192.168.1.20:7879/api/v1 */
  baseUrl?: string;
  /** Bearer token for remote mode. */
  token?: string;
  /** Request timeout in ms (default 15000). */
  timeoutMs?: number;
  fetchImpl?: typeof fetch;
  WebSocketImpl?: typeof WebSocket;
}

type Listener<T> = (v: T) => void;

/**
 * WebSocket event stream with exponential-backoff auto-reconnect.
 * Browsers can't set headers on WebSocket, so in remote mode the token is
 * sent as `?token=` (daemon must accept it; see report).
 */
export class EventStream {
  private ws?: WebSocket;
  private listeners = new Set<Listener<BzEvent>>();
  private stateListeners = new Set<Listener<ConnectionState>>();
  private attempt = 0;
  private timer?: ReturnType<typeof setTimeout>;
  private closed = false;
  state: ConnectionState = "closed";

  constructor(
    private readonly url: string,
    private readonly WS: typeof WebSocket = WebSocket,
  ) {}

  private setState(s: ConnectionState) {
    this.state = s;
    this.stateListeners.forEach((l) => l(s));
  }

  private connect() {
    if (this.closed) return;
    this.setState("connecting");
    let ws: WebSocket;
    try {
      ws = new this.WS(this.url);
    } catch {
      this.scheduleReconnect();
      return;
    }
    this.ws = ws;
    ws.onopen = () => {
      this.attempt = 0;
      this.setState("open");
    };
    ws.onmessage = (msg) => {
      try {
        const ev = JSON.parse(typeof msg.data === "string" ? msg.data : "") as BzEvent;
        if (ev && typeof ev.type === "string") this.listeners.forEach((l) => l(ev));
      } catch {
        /* ignore malformed frames */
      }
    };
    ws.onclose = () => {
      if (this.ws !== ws) return; // a stale socket from before close()/re-open
      this.ws = undefined;
      this.setState("closed");
      this.scheduleReconnect();
    };
    ws.onerror = () => ws.close();
  }

  private scheduleReconnect() {
    if (this.closed || this.timer) return;
    const delay = Math.min(30_000, 500 * 2 ** this.attempt) * (0.75 + Math.random() * 0.5);
    this.attempt++;
    this.timer = setTimeout(() => {
      this.timer = undefined;
      this.connect();
    }, delay);
  }

  subscribe(l: Listener<BzEvent>): () => void {
    // A closed stream is re-opened by a new subscriber. React StrictMode (dev) runs
    // effect cleanup + re-run on the same instance; without this the stream stayed
    // closed for good and the UI showed a permanent "daemon not reachable" banner.
    if (this.closed) {
      this.closed = false;
      this.attempt = 0;
    }
    this.listeners.add(l);
    if (!this.ws && !this.timer) this.connect();
    return () => this.listeners.delete(l);
  }

  onState(l: Listener<ConnectionState>): () => void {
    this.stateListeners.add(l);
    l(this.state);
    return () => this.stateListeners.delete(l);
  }

  close() {
    this.closed = true;
    if (this.timer) clearTimeout(this.timer);
    this.timer = undefined;
    const ws = this.ws;
    this.ws = undefined;
    if (ws) {
      ws.onclose = null;
      ws.onerror = null;
      ws.onmessage = null;
      // Closing a CONNECTING socket logs a browser warning; let it open, then close it.
      if (ws.readyState === 0) ws.onopen = () => ws.close();
      else ws.close();
    }
    this.listeners.clear();
    this.setState("closed");
  }
}

export function eventsUrl(baseUrl: string, token?: string): string {
  const u = new URL(baseUrl.replace(/\/+$/, "") + "/events");
  u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
  if (token) u.searchParams.set("token", token);
  return u.toString();
}

const enc = encodeURIComponent;

export class HttpApi implements BozkurtApi {
  readonly mode = "http" as const;
  readonly baseUrl: string;
  private token?: string;
  private timeoutMs: number;
  private fetchImpl: typeof fetch;
  private stream: EventStream;

  constructor(opts: HttpApiOptions = {}) {
    this.baseUrl = (opts.baseUrl ?? DEFAULT_BASE_URL).replace(/\/+$/, "");
    this.token = opts.token;
    this.timeoutMs = opts.timeoutMs ?? 15_000;
    this.fetchImpl = opts.fetchImpl ?? ((...a) => fetch(...a));
    this.stream = new EventStream(eventsUrl(this.baseUrl, this.token), opts.WebSocketImpl);
  }

  private async req<T>(method: string, path: string, body?: unknown): Promise<T> {
    const ctrl = new AbortController();
    const t = setTimeout(() => ctrl.abort(), this.timeoutMs);
    const headers: Record<string, string> = { Accept: "application/json" };
    if (body !== undefined) headers["Content-Type"] = "application/json";
    if (this.token) headers.Authorization = `Bearer ${this.token}`;
    let res: Response;
    try {
      res = await this.fetchImpl(this.baseUrl + path, {
        method,
        headers,
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: ctrl.signal,
      });
    } catch (e) {
      const aborted = (e as Error)?.name === "AbortError";
      throw new ApiError(0, aborted ? "timeout" : "network", aborted ? "Request timed out" : "Cannot reach bozkurtd");
    } finally {
      clearTimeout(t);
    }
    if (res.status === 204) return undefined as T;
    const text = await res.text();
    let data: unknown = undefined;
    if (text) {
      try {
        data = JSON.parse(text);
      } catch {
        data = text;
      }
    }
    if (!res.ok) {
      const err = (data as { error?: { code?: string; message?: string } })?.error;
      throw new ApiError(res.status, err?.code ?? `http_${res.status}`, err?.message ?? res.statusText ?? "Request failed");
    }
    return data as T;
  }

  health() { return this.req<{ ok: boolean; version: string }>("GET", "/health"); }
  overview() { return this.req<Overview>("GET", "/overview"); }

  runtimes() { return this.req<Runtime[]>("GET", "/runtimes"); }
  runtimeVersions(kind: RuntimeKind, refresh = false) {
    return this.req<Version[]>("GET", `/runtimes/${kind}/versions${refresh ? "?refresh=true" : ""}`);
  }
  installRuntime(kind: RuntimeKind, version: string, allowUnverified = false) {
    return this.req<JobRef>("POST", `/runtimes/${kind}/install`, allowUnverified ? { version, allow_unverified: true } : { version });
  }
  uninstallRuntime(kind: RuntimeKind, version: string) { return this.req<void>("DELETE", `/runtimes/${kind}/${enc(version)}`); }
  useRuntime(kind: RuntimeKind, version: string) { return this.req<Runtime>("POST", `/runtimes/${kind}/use`, { version }); }

  services() { return this.req<Service[]>("GET", "/services"); }
  installService(name: ServiceName, version?: string) {
    return this.req<JobRef>("POST", `/services/${enc(name)}/install`, version ? { version } : {});
  }
  serviceAction(name: ServiceName, action: ServiceAction) { return this.req<Service>("POST", `/services/${enc(name)}/${action}`); }
  allServicesAction(action: ServiceAction) { return this.req<Service[]>("POST", `/services/all/${action}`); }
  patchService(name: ServiceName, patch: { autostart?: boolean; port?: number }) {
    return this.req<Service>("PATCH", `/services/${enc(name)}`, patch);
  }
  serviceLogs(name: ServiceName, lines = 200) { return this.req<{ lines: string[] }>("GET", `/services/${enc(name)}/logs?lines=${lines}`); }

  sites() { return this.req<Site[]>("GET", "/sites"); }
  createSite(r: CreateSiteRequest) { return this.req<Site>("POST", "/sites", r); }
  site(id: string) { return this.req<Site>("GET", `/sites/${enc(id)}`); }
  patchSite(id: string, patch: Partial<Site>) { return this.req<Site>("PATCH", `/sites/${enc(id)}`, patch); }
  deleteSite(id: string) { return this.req<void>("DELETE", `/sites/${enc(id)}`); }
  siteAction(id: string, action: SiteAction) { return this.req<Site>("POST", `/sites/${enc(id)}/${action}`); }
  siteLogs(id: string, lines = 200) { return this.req<{ lines: string[] }>("GET", `/sites/${enc(id)}/logs?lines=${lines}`); }
  detect(path: string) { return this.req<Detection>("POST", "/detect", { path }); }

  dns() { return this.req<DnsInfo>("GET", "/dns"); }
  configureDns(tld?: string) { return this.req<JobRef>("POST", "/dns/configure", tld ? { tld } : {}); }
  tls() { return this.req<TlsInfo>("GET", "/tls"); }
  trustCa(system?: boolean) { return this.req<JobRef>("POST", "/tls/trust", system === undefined ? undefined : { system }); }
  untrustCa() { return this.req<JobRef>("POST", "/tls/untrust"); }
  unconfigureDns() { return this.req<JobRef>("POST", "/dns/unconfigure"); }
  port(port: number) { return this.req<PortOwner>("GET", `/ports/${port}`); }
  doctor() { return this.req<DoctorCheck[]>("GET", "/doctor"); }
  doctorFix(id: string) { return this.req<JobRef>("POST", `/doctor/fix/${enc(id)}`); }

  databases(engine: string) { return this.req<{ databases: string[] }>("GET", `/services/${enc(engine)}/databases`); }
  createDatabase(engine: string, name: string) { return this.req<unknown>("POST", `/services/${enc(engine)}/databases`, { name }); }
  dropDatabase(engine: string, name: string) { return this.req<void>("DELETE", `/services/${enc(engine)}/databases/${enc(name)}`); }

  jobs() { return this.req<Job[]>("GET", "/jobs"); }
  job(id: string) { return this.req<Job>("GET", `/jobs/${enc(id)}`); }

  config() { return this.req<Config>("GET", "/config"); }
  patchConfig(patch: Partial<Config>) { return this.req<Config>("PATCH", "/config", patch); }

  importScan() { return this.req<ImportScan>("GET", "/import/scan"); }
  importApply(body: ImportScan) { return this.req<JobRef>("POST", "/import", body); }

  share(siteId: string) { return this.req<JobRef>("POST", `/share/${enc(siteId)}`); }
  unshare(siteId: string) { return this.req<void>("DELETE", `/share/${enc(siteId)}`); }
  shareStatus(siteId: string) { return this.req<ShareState>("GET", `/share/${enc(siteId)}`); }

  remote() { return this.req<RemoteInfo>("GET", "/remote"); }
  setRemote(enabled: boolean) { return this.req<RemoteInfo>("POST", `/remote/${enabled ? "enable" : "disable"}`); }
  pair(code: string, device_name: string) { return this.req<PairResponse>("POST", "/remote/pair", { code, device_name }); }
  pairingCode() { return this.req<PairingCode>("POST", "/remote/pairing-code"); }
  removeDevice(id: string) { return this.req<void>("DELETE", `/remote/devices/${enc(id)}`); }

  subscribe(l: (e: BzEvent) => void) { return this.stream.subscribe(l); }
  onConnection(l: (s: ConnectionState) => void) { return this.stream.onState(l); }
  close() { this.stream.close(); }
}

/** Pair with a remote daemon (no token yet). `host` may be "192.168.1.5:7879" or a full URL. */
export async function pairRemote(host: string, code: string, deviceName: string): Promise<{ baseUrl: string; token: string; deviceId?: string }> {
  const baseUrl = normalizeRemoteBase(host);
  const api = new HttpApi({ baseUrl });
  try {
    const { token, device_id } = await api.pair(code, deviceName);
    return { baseUrl, token, deviceId: device_id };
  } finally {
    api.close();
  }
}

export function normalizeRemoteBase(host: string): string {
  let h = host.trim();
  if (!/^https?:\/\//i.test(h)) h = "http://" + h;
  const u = new URL(h);
  if (!u.port && u.protocol === "http:") u.port = "7879";
  const path = u.pathname.replace(/\/+$/, "");
  u.pathname = path.endsWith("/api/v1") ? path : path + "/api/v1";
  return u.toString().replace(/\/+$/, "");
}

/** Digits in a bozkurtd pairing code (8 since 1.0.0; 6 before). */
export const PAIRING_CODE_DIGITS = 8;

/** Pairing QR payload: bozkurt://pair?host=<ip:port>&code=<8 digits> */
export function pairingUri(host: string, code: string): string {
  return `bozkurt://pair?host=${enc(host)}&code=${enc(code)}`;
}

export function parsePairingUri(text: string): { host: string; code: string } | null {
  try {
    const t = text.trim();
    if (t.startsWith("{")) {
      const j = JSON.parse(t) as { host?: string; code?: string };
      return j.host && j.code ? { host: j.host, code: j.code } : null;
    }
    const u = new URL(t.replace(/^bozkurt:\/\//, "http://bozkurt/"));
    const host = u.searchParams.get("host");
    const code = u.searchParams.get("code");
    return host && code ? { host, code } : null;
  } catch {
    return null;
  }
}
