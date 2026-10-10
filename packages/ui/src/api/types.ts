// Types mirrored by hand from docs/API.md (bozkurtd HTTP API v1). Keep in sync with the contract.

export type RuntimeKind = "php" | "node" | "python" | "bun" | "go";
export const RUNTIME_KINDS: RuntimeKind[] = ["php", "node", "python", "bun", "go"];

export type ServiceName =
  | "caddy"
  | "dns"
  | "mariadb"
  | "postgres"
  | "redis"
  | "mailpit"
  | `php-fpm-${string}`;

export type ServiceStatus =
  | "stopped"
  | "starting"
  | "running"
  | "stopping"
  | "crashed"
  | "external"
  | "not_installed";

export type VersionChannel = "lts" | "current" | "stable" | "eol";

export interface Version {
  version: string;
  channel: VersionChannel;
  installed: boolean;
  active: boolean;
  released?: string;
  size_bytes?: number;
}

export interface Runtime {
  kind: RuntimeKind;
  active?: string;
  installed: string[];
}

export interface Service {
  name: ServiceName;
  status: ServiceStatus;
  pid?: number;
  port?: number;
  version?: string;
  autostart: boolean;
  uptime_secs?: number;
  message?: string;
}

export type SiteKind = "php" | "proxy" | "static" | "app";

export interface Site {
  id: string;
  domain: string;
  aliases: string[];
  path: string;
  kind: SiteKind;
  root?: string;
  php?: string;
  node?: string;
  proxy_port?: number;
  command?: string;
  app_port?: number;
  app_status?: ServiceStatus;
  https: boolean;
  wildcard: boolean;
  framework?: string;
  env: Record<string, string>;
  created_at: string;
}

export type JobStatus = "running" | "done" | "failed";

export interface Job {
  id: string;
  title: string;
  status: JobStatus;
  progress: number; // 0..1
  message?: string;
  started_at: string;
  finished_at?: string;
}

export interface Overview {
  version: string;
  os: string;
  arch: string;
  home: string;
  tld: string;
  runtimes: Runtime[];
  services: Service[];
  sites_count: number;
  ca_trusted: boolean;
  dns_configured: boolean;
  http_port: number;
  https_port: number;
}

export interface DoctorCheck {
  id: string;
  title: string;
  status: "ok" | "warn" | "fail";
  detail: string;
  fix?: string;
}

export interface PortOwner {
  port: number;
  pid?: number;
  process?: string;
  ours: boolean;
  /** Something (ours or not) is listening on the port (contract v1 additions). */
  in_use: boolean;
}

export interface Detection {
  path: string;
  framework?: string;
  kind: SiteKind;
  root?: string;
  php?: string;
  node?: string;
  command?: string;
  suggested_domain: string;
}

export interface CreateSiteRequest {
  path: string;
  domain?: string;
  kind?: SiteKind;
  root?: string;
  php?: string;
  node?: string;
  proxy_port?: number;
  command?: string;
  https?: boolean;
  wildcard?: boolean;
  aliases?: string[];
  env?: Record<string, string>;
}

export type SiteAction = "start" | "stop" | "restart" | "secure" | "unsecure";
export type ServiceAction = "start" | "stop" | "restart";

export interface DnsInfo {
  tld: string;
  port: number;
  configured: boolean;
  running: boolean;
}

export interface TlsInfo {
  ca_path: string;
  ca_trusted: boolean;
  /** `Option<String>` in Rust (`TlsStatus`): omitted when the daemon has no expiry to report. */
  ca_expires?: string;
}

/** config.toml as JSON (see ARCHITECTURE.md). Extra keys are preserved. */
export interface Config {
  tld: string;
  dns_port: number;
  http_port: number;
  https_port: number;
  api_port: number;
  autostart: string[];
  remote: { enabled: boolean; port: number };
  /** Serve sites on every interface instead of loopback only (default false). */
  lan_access?: boolean;
  /** Accept downloads without a published checksum and pin their sha256 (default false; config.toml only in the UI). */
  allow_unverified?: boolean;
  [key: string]: unknown;
}

export interface ImportVhost {
  source: string;
  domain: string;
  path: string;
  php?: string;
}
export interface ImportDns {
  source: string;
  tld: string;
  file: string;
}
export interface ImportScan {
  vhosts: ImportVhost[];
  dns: ImportDns[];
}

export interface PairedDevice {
  id: string;
  name: string;
  last_seen: string;
}

export interface RemoteInfo {
  enabled: boolean;
  /** The LAN listener is actually up (enabled but not running = bind failed). */
  running: boolean;
  port: number;
  addresses: string[];
  paired_devices: PairedDevice[];
}

export interface PairingCode {
  code: string;
  expires_at: string;
}

export interface JobRef {
  job_id: string;
}

/** `GET /share/{site_id}`: url is null when the site is not shared. */
export interface ShareState {
  site_id: string;
  url: string | null;
}

export interface PairResponse {
  token: string;
  device_id?: string;
}

export type BzEvent =
  | { type: "job.progress"; job: Job }
  | { type: "job.done"; job: Job }
  | { type: "job.failed"; job: Job }
  | { type: "service.status"; service: Service }
  | { type: "site.changed"; site: Site }
  | { type: "site.removed"; id: string }
  | { type: "runtime.changed"; runtime: Runtime }
  | { type: "log"; source: string; line: string }
  | { type: "share.url"; site_id: string; url: string };

export interface ApiErrorBody {
  error: { code: string; message: string };
}

export type ConnectionState = "connecting" | "open" | "closed";

/** The full surface of the daemon API. Implemented by HttpApi (real) and MockApi (demo). */
export interface BozkurtApi {
  readonly mode: "http" | "mock";
  health(): Promise<{ ok: boolean; version: string }>;
  overview(): Promise<Overview>;

  runtimes(): Promise<Runtime[]>;
  runtimeVersions(kind: RuntimeKind, refresh?: boolean): Promise<Version[]>;
  /** `allowUnverified` sends `allow_unverified: true`: accept (and pin) an asset without a published checksum. */
  installRuntime(kind: RuntimeKind, version: string, allowUnverified?: boolean): Promise<JobRef>;
  uninstallRuntime(kind: RuntimeKind, version: string): Promise<void>;
  useRuntime(kind: RuntimeKind, version: string): Promise<Runtime>;

  services(): Promise<Service[]>;
  installService(name: ServiceName, version?: string): Promise<JobRef>;
  serviceAction(name: ServiceName, action: ServiceAction): Promise<Service>;
  allServicesAction(action: ServiceAction): Promise<Service[]>;
  patchService(name: ServiceName, patch: { autostart?: boolean; port?: number }): Promise<Service>;
  serviceLogs(name: ServiceName, lines?: number): Promise<{ lines: string[] }>;

  sites(): Promise<Site[]>;
  createSite(req: CreateSiteRequest): Promise<Site>;
  site(id: string): Promise<Site>;
  patchSite(id: string, patch: Partial<Site>): Promise<Site>;
  deleteSite(id: string): Promise<void>;
  siteAction(id: string, action: SiteAction): Promise<Site>;
  siteLogs(id: string, lines?: number): Promise<{ lines: string[] }>;
  detect(path: string): Promise<Detection>;

  dns(): Promise<DnsInfo>;
  configureDns(tld?: string): Promise<JobRef>;
  tls(): Promise<TlsInfo>;
  /** `system: true` also trusts the CA in the OS-wide store (contract v1 additions). */
  trustCa(system?: boolean): Promise<JobRef>;
  untrustCa(): Promise<JobRef>;
  unconfigureDns(): Promise<JobRef>;
  port(port: number): Promise<PortOwner>;
  doctor(): Promise<DoctorCheck[]>;
  /** Run one machine fix id (`dns.configure`, `tls.trust`, `services.<n>.<a>`, ...). */
  doctorFix(id: string): Promise<JobRef>;

  databases(engine: string): Promise<{ databases: string[] }>;
  createDatabase(engine: string, name: string): Promise<unknown>;
  dropDatabase(engine: string, name: string): Promise<void>;

  jobs(): Promise<Job[]>;
  job(id: string): Promise<Job>;

  config(): Promise<Config>;
  patchConfig(patch: Partial<Config>): Promise<Config>;

  importScan(): Promise<ImportScan>;
  importApply(body: ImportScan): Promise<JobRef>;

  share(siteId: string): Promise<JobRef>;
  unshare(siteId: string): Promise<void>;
  shareStatus(siteId: string): Promise<ShareState>;

  remote(): Promise<RemoteInfo>;
  setRemote(enabled: boolean): Promise<RemoteInfo>;
  pair(code: string, deviceName: string): Promise<PairResponse>;
  pairingCode(): Promise<PairingCode>;
  removeDevice(id: string): Promise<void>;

  /** Subscribe to the event bus. Returns an unsubscribe function. */
  subscribe(listener: (e: BzEvent) => void): () => void;
  onConnection(listener: (s: ConnectionState) => void): () => void;
  close(): void;
}
