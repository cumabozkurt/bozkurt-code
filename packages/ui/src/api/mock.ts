// In-memory mock of bozkurtd. Enabled with `?mock` in the URL or VITE_BOZKURT_MOCK=1.
// Realistic data, simulated jobs with progress events, live log lines.
import { ApiError } from "./client";
import type {
  BozkurtApi,
  BzEvent,
  Config,
  ConnectionState,
  CreateSiteRequest,
  Detection,
  DoctorCheck,
  ImportScan,
  Job,
  JobRef,
  Overview,
  PairedDevice,
  Runtime,
  RuntimeKind,
  Service,
  ServiceAction,
  ServiceName,
  ShareState,
  Site,
  SiteAction,
  Version,
} from "./types";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const uid = () =>
  globalThis.crypto?.randomUUID?.() ?? Math.random().toString(16).slice(2) + Date.now().toString(16);
const now = () => new Date().toISOString();
const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v));
const ts = () => new Date().toISOString().slice(11, 19);

const CATALOG: Record<RuntimeKind, Omit<Version, "installed" | "active">[]> = {
  php: [
    { version: "8.4.13", channel: "current", released: "2026-09-25", size_bytes: 31_400_000 },
    { version: "8.3.26", channel: "stable", released: "2026-09-25", size_bytes: 30_100_000 },
    { version: "8.2.29", channel: "stable", released: "2026-07-03", size_bytes: 29_600_000 },
    { version: "8.1.33", channel: "eol", released: "2026-07-03", size_bytes: 28_800_000 },
    { version: "8.0.30", channel: "eol", released: "2023-08-03", size_bytes: 27_900_000 },
  ],
  node: [
    { version: "24.9.0", channel: "current", released: "2026-09-24", size_bytes: 49_000_000 },
    { version: "22.20.0", channel: "lts", released: "2026-09-24", size_bytes: 47_200_000 },
    { version: "20.19.5", channel: "lts", released: "2026-09-03", size_bytes: 44_800_000 },
    { version: "18.20.8", channel: "eol", released: "2025-03-27", size_bytes: 41_000_000 },
  ],
  python: [
    { version: "3.14.0", channel: "current", released: "2026-10-07", size_bytes: 36_000_000 },
    { version: "3.13.8", channel: "stable", released: "2026-10-07", size_bytes: 34_000_000 },
    { version: "3.12.11", channel: "stable", released: "2026-06-03", size_bytes: 33_000_000 },
    { version: "3.11.13", channel: "stable", released: "2026-06-03", size_bytes: 32_000_000 },
  ],
  bun: [
    { version: "1.2.23", channel: "current", released: "2026-09-28", size_bytes: 38_000_000 },
    { version: "1.2.22", channel: "stable", released: "2026-09-14", size_bytes: 38_000_000 },
    { version: "1.1.45", channel: "eol", released: "2025-01-08", size_bytes: 36_000_000 },
  ],
  go: [
    { version: "1.25.2", channel: "current", released: "2026-10-07", size_bytes: 71_000_000 },
    { version: "1.24.8", channel: "stable", released: "2026-10-07", size_bytes: 70_000_000 },
    { version: "1.23.12", channel: "eol", released: "2026-08-06", size_bytes: 68_000_000 },
  ],
};

// Keys are service names (emitted as `service:<name>`, like bozkurtd) or `site:<domain>`.
const LOG_SAMPLES: Record<string, string[]> = {
  caddy: [
    'info http.log.access handled request {"host":"shop.test","uri":"/","status":200,"duration":0.012}',
    'info http.log.access handled request {"host":"api.test","uri":"/v1/users","status":200,"duration":0.034}',
    'info http.log.access handled request {"host":"blog.test","uri":"/wp-admin/","status":302,"duration":0.051}',
    'info tls.cache.maintenance certificate is fresh {"identifiers":["shop.test"]}',
  ],
  "php-fpm-8.3": [
    "NOTICE: [pool www] child 48211 started",
    "NOTICE: [pool www] 'pm.max_children' 8, currently 2 active",
    'WARNING: [pool www] child 48190 said into stderr: "Deprecated: Creation of dynamic property"',
  ],
  mariadb: [
    "[Note] Aborted connection 41 to db: 'shop' user: 'root' host: 'localhost' (Got timeout reading communication packets)",
    "[Note] InnoDB: Buffer pool(s) load completed",
  ],
  redis: ["* 1 changes in 3600 seconds. Saving...", "* Background saving terminated with success", "* DB saved on disk"],
  mailpit: ['[smtpd] message received from=<no-reply@shop.test> to=<test@example.com> subject="Order #1042 confirmed"'],
  dns: ["query A shop.test → 127.0.0.1", "query AAAA api.test → ::1", "query A cdn.blog.test → 127.0.0.1"],
  "site:dashboard.test": ["  VITE v6.0.3  ready in 412 ms", "  ➜  Local:   http://localhost:5174/", "hmr update /src/App.tsx"],
};

export class MockApi implements BozkurtApi {
  readonly mode = "mock" as const;
  private listeners = new Set<(e: BzEvent) => void>();
  private connListeners = new Set<(s: ConnectionState) => void>();
  private timers: ReturnType<typeof setInterval>[] = [];
  private installed: Record<RuntimeKind, string[]> = {
    php: ["8.3.26", "8.2.29"],
    node: ["22.20.0", "20.19.5"],
    python: ["3.13.8"],
    bun: [],
    go: [],
  };
  private active: Partial<Record<RuntimeKind, string>> = { php: "8.3.26", node: "22.20.0", python: "3.13.8" };
  private jobsMap = new Map<string, Job>();
  private shares = new Map<string, string>();
  private devices: PairedDevice[] = [
    { id: "dev-1", name: "Cuma's iPhone", last_seen: new Date(Date.now() - 3600_000).toISOString() },
  ];
  private cfg: Config = {
    tld: "test",
    dns_port: 5390,
    http_port: 80,
    https_port: 443,
    api_port: 7878,
    autostart: ["caddy", "dns", "php-fpm-8.3", "mariadb"],
    remote: { enabled: false, port: 7879 },
    lan_access: false,
    allow_unverified: false,
  };
  /** download URL file name -> pinned sha256 (bozkurtd: state.json `download_pins`). */
  private downloadPins = new Map<string, string>();
  /** Pairing brute-force state (bozkurtd: 30 attempts/min overall, code revoked after 10 wrong guesses). */
  private pairAttempts: number[] = [];
  private pairFailures = 0;
  private pairLocked = false;
  private caTrusted = false;
  private dnsConfigured = false;
  private code?: { code: string; expires_at: string };
  private svc: Service[] = [
    { name: "caddy", status: "running", pid: 48102, port: 443, version: "2.10.2", autostart: true, uptime_secs: 15_320 },
    { name: "dns", status: "running", port: 5390, version: "builtin", autostart: true, uptime_secs: 15_320 },
    { name: "php-fpm-8.3", status: "running", pid: 48190, port: 9083, version: "8.3.26", autostart: true, uptime_secs: 15_300 },
    { name: "php-fpm-8.2", status: "stopped", port: 9082, version: "8.2.29", autostart: false },
    { name: "mariadb", status: "running", pid: 48233, port: 3306, version: "11.4.8", autostart: true, uptime_secs: 15_280 },
    { name: "postgres", status: "stopped", port: 5432, version: "17.6", autostart: false },
    { name: "redis", status: "crashed", port: 6379, version: "8.2.1", autostart: false, message: "exited with code 1: Address already in use" },
    // Like bozkurtd, Mailpit's `port` is SMTP (1025); its web UI is always on 8025.
    { name: "mailpit", status: "not_installed", port: 1025, autostart: false },
  ];
  private sitesList: Site[] = [
    {
      id: "s-shop", domain: "shop.test", aliases: ["admin.shop.test"], path: "/Users/cuma/Code/shop", kind: "php",
      root: "public", php: "8.3", https: true, wildcard: false, framework: "laravel",
      env: { APP_ENV: "local", APP_DEBUG: "true", DB_DATABASE: "shop" }, created_at: "2026-09-12T10:21:00Z",
    },
    {
      id: "s-blog", domain: "blog.test", aliases: [], path: "/Users/cuma/Sites/blog", kind: "php",
      root: ".", php: "8.2", https: true, wildcard: true, framework: "wordpress", env: {}, created_at: "2026-08-02T08:00:00Z",
    },
    {
      id: "s-dash", domain: "dashboard.test", aliases: [], path: "/Users/cuma/Code/dashboard", kind: "app",
      node: "22", command: "npm run dev", app_port: 5174, app_status: "running", https: true, wildcard: false,
      framework: "vite", env: { VITE_API_URL: "https://api.test" }, created_at: "2026-09-30T14:12:00Z",
    },
    {
      id: "s-api", domain: "api.test", aliases: [], path: "/Users/cuma/Code/api", kind: "proxy", proxy_port: 8000,
      https: false, wildcard: false, framework: "fastapi", env: {}, created_at: "2026-10-01T09:45:00Z",
    },
    {
      id: "s-docs", domain: "docs.test", aliases: [], path: "/Users/cuma/Code/docs/dist", kind: "static",
      https: true, wildcard: false, env: {}, created_at: "2026-10-05T17:00:00Z",
    },
  ];

  constructor(private readonly latency = 180) {}

  /** Background log/uptime timers run only while someone is subscribed (StrictMode-safe). */
  private startTimers() {
    if (this.timers.length) return;
    this.timers.push(
      setInterval(() => {
        const sources = Object.keys(LOG_SAMPLES).filter((s) => {
          const svc = this.svc.find((x) => x.name === s);
          return !svc || svc.status === "running";
        });
        const src = sources[Math.floor(Math.random() * sources.length)];
        const lines = LOG_SAMPLES[src];
        const source = src.startsWith("site:") ? src : `service:${src}`;
        this.emit({ type: "log", source, line: `${ts()} ${lines[Math.floor(Math.random() * lines.length)]}` });
      }, 1400),
    );
    this.timers.push(
      setInterval(() => {
        for (const s of this.svc) if (s.status === "running" && s.uptime_secs !== undefined) s.uptime_secs += 5;
      }, 5000),
    );
  }

  private async lag<T>(v: T): Promise<T> {
    await sleep(this.latency * (0.5 + Math.random()));
    return clone(v);
  }
  private emit(e: BzEvent) {
    this.listeners.forEach((l) => l(e));
  }
  private fail(status: number, code: string, message: string): never {
    throw new ApiError(status, code, message);
  }
  private findSvc(name: ServiceName) {
    return this.svc.find((s) => s.name === name) ?? this.fail(404, "not_found", `Unknown service ${name}`);
  }
  private findSite(id: string) {
    return this.sitesList.find((s) => s.id === id) ?? this.fail(404, "not_found", `Unknown site ${id}`);
  }

  /** Simulate a long-running job. `steps` are messages; `onDone` mutates state. */
  private startJob(title: string, steps: string[], onDone: () => void, durationMs = 4000): JobRef {
    const job: Job = { id: uid(), title, status: "running", progress: 0, message: steps[0], started_at: now() };
    this.jobsMap.set(job.id, job);
    const tick = durationMs / 20;
    let i = 0;
    const timer = setInterval(() => {
      i++;
      job.progress = Math.min(1, i / 20);
      job.message = steps[Math.min(steps.length - 1, Math.floor((i / 20) * steps.length))];
      if (i >= 20) {
        clearInterval(timer);
        try {
          onDone();
          job.status = "done";
          job.message = "Done";
          job.finished_at = now();
          this.emit({ type: "job.done", job: clone(job) });
        } catch (e) {
          job.status = "failed";
          job.message = (e as Error).message;
          job.finished_at = now();
          this.emit({ type: "job.failed", job: clone(job) });
        }
      } else {
        this.emit({ type: "job.progress", job: clone(job) });
      }
    }, tick);
    setTimeout(() => this.emit({ type: "job.progress", job: clone(job) }), 0);
    return { job_id: job.id };
  }

  private runtime(kind: RuntimeKind): Runtime {
    return { kind, active: this.active[kind], installed: [...this.installed[kind]] };
  }
  private allRuntimes() {
    return (Object.keys(this.installed) as RuntimeKind[]).map((k) => this.runtime(k));
  }

  async health() { return this.lag({ ok: true, version: "1.0.0-mock" }); }
  async overview(): Promise<Overview> {
    return this.lag({
      version: "1.0.0-mock", os: "macos", arch: "aarch64", home: "/Users/cuma/.bozkurt", tld: this.cfg.tld,
      runtimes: this.allRuntimes(), services: this.svc, sites_count: this.sitesList.length, ca_trusted: this.caTrusted,
      dns_configured: this.dnsConfigured, http_port: this.cfg.http_port, https_port: this.cfg.https_port,
    });
  }

  async runtimes() { return this.lag(this.allRuntimes()); }
  async runtimeVersions(kind: RuntimeKind, refresh = false) {
    if (refresh) await sleep(600);
    return this.lag(
      CATALOG[kind].map((v) => ({ ...v, installed: this.installed[kind].includes(v.version), active: this.active[kind] === v.version })),
    );
  }
  async installRuntime(kind: RuntimeKind, version: string, allowUnverified = false) {
    if (!CATALOG[kind].some((v) => v.version === version)) this.fail(404, "unknown_version", `${kind} ${version} not in catalog`);
    if (this.installed[kind].includes(version)) this.fail(409, "already_installed", `${kind} ${version} is already installed`);
    // Like bozkurtd on macOS/Linux: static-php.dev publishes no checksums, so PHP fails closed
    // unless allowed (per install or `allow_unverified` in config) or pinned by an earlier install.
    const file = `php-${version}-cli-macos-aarch64.tar.gz`;
    const unverified = kind === "php" && !this.downloadPins.has(file);
    const allow = allowUnverified || !!this.cfg.allow_unverified;
    if (unverified && !allow) {
      return this.lag(
        this.startJob(`Install ${kind} ${version}`, ["Resolving download"], () => {
          throw new Error(
            `unverified download: ${file} has no published checksum and no sha256 pinned from an earlier install, so it cannot be verified. ` +
              "Retry with `allow_unverified` (`bz runtime install --allow-unverified`, or `allow_unverified = true` in config.toml) to trust it once; " +
              "its sha256 is then pinned and later downloads must match",
          );
        }, 600),
      );
    }
    const steps = unverified
      ? ["Resolving download", "Downloading", `Unverified download: ${file} has no published checksum; sha256 pinned for later installs`, "Extracting", "Writing shims"]
      : ["Resolving download", "Downloading", "Verifying SHA-256", "Extracting", "Writing shims"];
    return this.lag(
      this.startJob(`Install ${kind} ${version}`, steps, () => {
        if (unverified) this.downloadPins.set(file, uid().replace(/-/g, "").padEnd(64, "0").slice(0, 64));
        this.installed[kind].push(version);
        if (!this.active[kind]) this.active[kind] = version;
        this.emit({ type: "runtime.changed", runtime: this.runtime(kind) });
        if (kind === "php") {
          const mm = version.split(".").slice(0, 2).join(".");
          if (!this.svc.some((s) => s.name === `php-fpm-${mm}`))
            this.svc.push({ name: `php-fpm-${mm}`, status: "stopped", port: 9000 + Number(mm.replace(".", "")), version, autostart: false });
        }
      }),
    );
  }
  async uninstallRuntime(kind: RuntimeKind, version: string) {
    if (this.active[kind] === version) this.fail(409, "in_use", `${kind} ${version} is the active version`);
    this.installed[kind] = this.installed[kind].filter((v) => v !== version);
    this.emit({ type: "runtime.changed", runtime: this.runtime(kind) });
    await this.lag(null);
  }
  async useRuntime(kind: RuntimeKind, version: string) {
    if (!this.installed[kind].includes(version)) this.fail(409, "not_installed", `${kind} ${version} is not installed`);
    this.active[kind] = version;
    const rt = this.runtime(kind);
    this.emit({ type: "runtime.changed", runtime: rt });
    return this.lag(rt);
  }

  async services() { return this.lag(this.svc); }
  async installService(name: ServiceName, version?: string) {
    const s = this.findSvc(name);
    return this.lag(
      this.startJob(`Install ${name}`, ["Downloading", "Verifying SHA-256", "Extracting", "Initializing data dir"], () => {
        s.status = "stopped";
        s.version = version ?? (name === "mailpit" ? "1.27.9" : s.version);
        s.port = s.port ?? (name === "mailpit" ? 1025 : undefined);
        this.emit({ type: "service.status", service: clone(s) });
      }),
    );
  }
  private transition(s: Service, action: ServiceAction) {
    if (s.status === "not_installed") this.fail(409, "not_installed", `${s.name} is not installed`);
    const up = action !== "stop";
    s.status = up ? "starting" : "stopping";
    s.message = undefined;
    this.emit({ type: "service.status", service: clone(s) });
    setTimeout(() => {
      s.status = up ? "running" : "stopped";
      s.pid = up ? 40000 + Math.floor(Math.random() * 9000) : undefined;
      s.uptime_secs = up ? 0 : undefined;
      this.emit({ type: "service.status", service: clone(s) });
      this.emit({ type: "log", source: `service:${s.name}`, line: `${ts()} ${up ? "started" : "stopped"} (${action})` });
    }, 900 + Math.random() * 600);
  }
  async serviceAction(name: ServiceName, action: ServiceAction) {
    const s = this.findSvc(name);
    this.transition(s, action);
    return this.lag(s);
  }
  async allServicesAction(action: ServiceAction) {
    for (const s of this.svc)
      if (s.status !== "not_installed" && (action !== "start" || s.status !== "running")) this.transition(s, action);
    return this.lag(this.svc);
  }
  async patchService(name: ServiceName, patch: { autostart?: boolean; port?: number }) {
    const s = this.findSvc(name);
    Object.assign(s, patch);
    this.emit({ type: "service.status", service: clone(s) });
    return this.lag(s);
  }
  async serviceLogs(name: ServiceName, lines = 200) {
    const sample = LOG_SAMPLES[name] ?? [`${name} ready`];
    const out = Array.from({ length: Math.min(lines, 40) }, (_, i) => {
      const d = new Date(Date.now() - (40 - i) * 37_000).toISOString().slice(11, 19);
      return `${d} ${sample[i % sample.length]}`;
    });
    return this.lag({ lines: out });
  }

  async sites() { return this.lag(this.sitesList); }
  async createSite(r: CreateSiteRequest) {
    const det = await this.detect(r.path);
    const domain = (r.domain || det.suggested_domain).replace(/\.+$/, "");
    const full = domain.includes(".") ? domain : `${domain}.${this.cfg.tld}`;
    if (this.sitesList.some((s) => s.domain === full)) this.fail(409, "domain_taken", `${full} is already in use`);
    const site: Site = {
      id: "s-" + uid().slice(0, 8), domain: full, aliases: r.aliases ?? [], path: r.path, kind: r.kind ?? det.kind,
      root: r.root ?? det.root, php: r.php ?? det.php, node: r.node ?? det.node, proxy_port: r.proxy_port,
      command: r.command ?? det.command, https: r.https ?? true, wildcard: r.wildcard ?? false, framework: det.framework,
      env: r.env ?? {}, created_at: now(),
    };
    if (site.kind === "app") {
      site.app_status = "stopped";
      site.app_port = 5100 + this.sitesList.length;
    }
    this.sitesList.push(site);
    this.emit({ type: "site.changed", site: clone(site) });
    return this.lag(site);
  }
  async site(id: string) { return this.lag(this.findSite(id)); }
  async patchSite(id: string, patch: Partial<Site>) {
    const s = this.findSite(id);
    Object.assign(s, patch, { id: s.id, created_at: s.created_at });
    this.emit({ type: "site.changed", site: clone(s) });
    return this.lag(s);
  }
  async deleteSite(id: string) {
    this.findSite(id);
    this.sitesList = this.sitesList.filter((s) => s.id !== id);
    this.emit({ type: "site.removed", id });
    await this.lag(null);
  }
  async siteAction(id: string, action: SiteAction) {
    const s = this.findSite(id);
    if (action === "secure" || action === "unsecure") s.https = action === "secure";
    else {
      if (s.kind !== "app") this.fail(400, "not_app", "Only app sites can be started or stopped");
      s.app_status = action === "stop" ? "stopped" : "running";
    }
    this.emit({ type: "site.changed", site: clone(s) });
    return this.lag(s);
  }
  async siteLogs(id: string) {
    const s = this.findSite(id);
    const sample = LOG_SAMPLES[`site:${s.domain}`] ?? LOG_SAMPLES.caddy.map((l) => l.replace(/"host":"[^"]+"/, `"host":"${s.domain}"`));
    return this.lag({
      lines: Array.from({ length: 24 }, (_, i) => `${new Date(Date.now() - (24 - i) * 61_000).toISOString().slice(11, 19)} ${sample[i % sample.length]}`),
    });
  }
  async detect(path: string): Promise<Detection> {
    if (!path.trim()) this.fail(400, "bad_path", "Path is required");
    const name = path.replace(/[\\/]+$/, "").split(/[\\/]/).pop()!.toLowerCase().replace(/[^a-z0-9-]/g, "-") || "site";
    const p = path.toLowerCase();
    let d: Omit<Detection, "path" | "suggested_domain"> = { kind: "php", root: "public", php: "8.3", framework: "laravel" };
    if (p.includes("wp") || p.includes("wordpress")) d = { kind: "php", root: ".", php: "8.2", framework: "wordpress" };
    else if (p.includes("next")) d = { kind: "app", node: "22", command: "npm run dev", framework: "nextjs" };
    else if (p.includes("vite") || p.includes("react") || p.includes("vue")) d = { kind: "app", node: "22", command: "npm run dev", framework: "vite" };
    else if (p.includes("django") || p.includes("py")) d = { kind: "app", command: "python manage.py runserver $PORT", framework: "django" };
    else if (p.includes("static") || p.includes("dist")) d = { kind: "static" };
    return this.lag({ path, suggested_domain: `${name}.${this.cfg.tld}`, ...d });
  }

  async dns() { return this.lag({ tld: this.cfg.tld, port: this.cfg.dns_port, configured: this.dnsConfigured, running: true }); }
  async configureDns(tld?: string) {
    return this.lag(
      this.startJob(
        "Configure DNS resolver",
        ["Requesting elevation", "Backing up resolver config", `Writing /etc/resolver/${tld ?? this.cfg.tld}`, "Verifying resolution"],
        () => {
          if (tld) this.cfg.tld = tld;
          this.dnsConfigured = true;
        },
        3000,
      ),
    );
  }
  async tls() {
    return this.lag({ ca_path: "/Users/cuma/.bozkurt/ca/rootCA.pem", ca_trusted: this.caTrusted, ca_expires: "2036-09-12T10:00:00Z" });
  }
  async trustCa(system?: boolean) {
    return this.lag(this.startJob(system ? "Trust local CA (system store)" : "Trust local CA", ["Locating trust store", "Adding root certificate", "Verifying trust"], () => { this.caTrusted = true; }, 2500));
  }
  async untrustCa() {
    return this.lag(this.startJob("Remove local CA trust", ["Removing root certificate"], () => { this.caTrusted = false; }, 1500));
  }
  async unconfigureDns() {
    return this.lag(this.startJob("Remove DNS routing", [`Removing .${this.cfg.tld} routing`], () => { this.dnsConfigured = false; }, 1500));
  }
  async port(port: number) {
    const s = this.svc.find((x) => x.port === port && x.status === "running");
    if (s) return this.lag({ port, pid: s.pid, process: s.name, ours: true, in_use: true });
    if (port === 6379) return this.lag({ port, pid: 812, process: "redis-server (Homebrew)", ours: false, in_use: true });
    return this.lag({ port, ours: false, in_use: false });
  }
  async doctorFix(id: string) {
    if (id === "dns.configure") return this.configureDns();
    if (id === "tls.trust") return this.trustCa();
    const m = /^services\.(.+)\.(start|stop|restart)$/.exec(id);
    if (!m) this.fail(404, "not_found", `unknown fix \`${id}\``);
    const s = this.findSvc(m![1] as ServiceName);
    return this.lag(this.startJob(`${m![2]} ${s.name}`, ["Working"], () => this.transition(s, m![2] as ServiceAction), 800));
  }
  private dbs: Record<string, string[]> = { mariadb: ["shop", "blog"], postgres: [] };
  async databases(engine: string) { return this.lag({ databases: [...(this.dbs[engine] ?? [])] }); }
  async createDatabase(engine: string, name: string) {
    (this.dbs[engine] ??= []).push(name);
    return this.lag({ name });
  }
  async dropDatabase(engine: string, name: string) {
    this.dbs[engine] = (this.dbs[engine] ?? []).filter((d) => d !== name);
    await this.lag(null);
  }
  async doctor(): Promise<DoctorCheck[]> {
    // Same ids, titles and fix hints as bozkurtd (bz-core engine.rs doctor()).
    const caddy = this.findSvc("caddy");
    const hasPhp = this.installed.php.length > 0;
    const phpSites = this.sitesList.filter((s) => s.kind === "php");
    const installedRt = (Object.keys(this.installed) as RuntimeKind[]).filter((k) => this.installed[k].length).map((k) => `${k} ${this.active[k] ?? this.installed[k][0]}`);
    const checks: DoctorCheck[] = [
      { id: "home", title: "BOZKURT home is writable", status: "ok", detail: "/Users/cuma/.bozkurt" },
      { id: "path", title: "Shims are on PATH", status: "warn", detail: "/Users/cuma/.bozkurt/bin is not on PATH", fix: 'Add to your shell profile: export PATH="/Users/cuma/.bozkurt/bin:$PATH"' },
      caddy.status === "not_installed"
        ? { id: "caddy", title: "Web server (Caddy)", status: "warn", detail: "Caddy is not installed", fix: "bz service install caddy" }
        : { id: "caddy", title: "Web server (Caddy)", status: "ok", detail: `${caddy.version ?? ""} ${caddy.status}`.trim() },
      { id: "port_http", title: "HTTP port", status: "ok", detail: `port ${this.cfg.http_port} available` },
      { id: "port_https", title: "HTTPS port", status: "ok", detail: `port ${this.cfg.https_port} available` },
      installedRt.length
        ? { id: "runtimes", title: "Runtimes", status: "ok", detail: installedRt.join(", ") }
        : { id: "runtimes", title: "Runtimes", status: "warn", detail: "no runtime installed", fix: "bz runtime install php 8.4  (or node lts)" },
      { id: "sites", title: "Site directories exist", status: "ok", detail: `${this.sitesList.length} site(s)` },
      // Proposed in docs/audit/DAEMON_TODO_FROM_UI.md (not in bozkurtd yet).
      !hasPhp && phpSites.length
        ? { id: "site_runtimes", title: "Sites have their runtime", status: "fail", detail: `${phpSites.map((s) => s.domain).join(", ")} need PHP, none installed`, fix: "runtimes.php.install" }
        : { id: "site_runtimes", title: "Sites have their runtime", status: "ok", detail: "all sites have their runtime" },
      this.caTrusted
        ? { id: "tls", title: "Local CA trusted", status: "ok", detail: "/Users/cuma/.bozkurt/ca/rootCA.pem" }
        : { id: "tls", title: "Local CA trusted", status: "warn", detail: "/Users/cuma/.bozkurt/ca/rootCA.pem is not trusted", fix: "tls.trust" },
      this.dnsConfigured
        ? { id: "dns", title: "Local DNS", status: "ok", detail: `.${this.cfg.tld} → 127.0.0.1 (port ${this.cfg.dns_port})` }
        : { id: "dns", title: "Local DNS", status: "warn", detail: `.${this.cfg.tld} is not routed to the local resolver`, fix: "dns.configure" },
    ];
    const redis = this.findSvc("redis");
    if (redis.status === "crashed") checks.push({ id: "service.redis", title: "Redis", status: "fail", detail: redis.message ?? "crashed", fix: "services.redis.restart" });
    return this.lag(checks);
  }

  async jobs() { return this.lag([...this.jobsMap.values()].reverse()); }
  async job(id: string) { return this.lag(this.jobsMap.get(id) ?? this.fail(404, "not_found", "Unknown job")); }

  async config() { return this.lag(this.cfg); }
  async patchConfig(patch: Partial<Config>) {
    const next: Config = { ...this.cfg, ...patch, remote: { ...this.cfg.remote, ...(patch.remote ?? {}) } };
    // Same rules and messages as bozkurtd `Config::validate_ports` (400 invalid_input; nothing saved).
    const ports: [string, number][] = [
      ["http_port", next.http_port], ["https_port", next.https_port], ["dns_port", next.dns_port], ["api_port", next.api_port], ["remote.port", next.remote.port],
    ];
    for (const [k, p] of ports)
      if (!Number.isInteger(p) || p < 1 || p > 65535) this.fail(400, "invalid_input", `invalid input: ${k} must be between 1 and 65535`);
    ports.forEach(([k, p], i) => {
      const other = ports.slice(0, i).find(([, q]) => q === p);
      if (other) this.fail(400, "invalid_input", `invalid input: ${k} (${p}) is already used by ${other[0]}`);
    });
    if (next.remote.enabled && !this.cfg.remote.enabled) this.bindRemote(next.remote.port);
    this.cfg = next;
    return this.lag(this.cfg);
  }
  /** Ports "taken" by other programs in the demo: enabling remote access on one of them fails like bozkurtd (409). */
  private bindRemote(port: number) {
    if ([3306, 5432, 6379, 8080].includes(port)) this.fail(409, "conflict", `conflict: cannot listen on 0.0.0.0:${port}: Address already in use (os error 98)`);
  }

  async importScan(): Promise<ImportScan> {
    return this.lag({
      vhosts: [
        { source: "valet", domain: "legacy.test", path: "/Users/cuma/Sites/legacy", php: "8.1" },
        { source: "mamp", domain: "client-a.local", path: "/Applications/MAMP/htdocs/client-a" },
      ],
      dns: [{ source: "valet", tld: "test", file: "/etc/resolver/test" }],
    });
  }
  async importApply(body: ImportScan) {
    return this.lag(
      this.startJob("Import sites", ["Backing up", "Creating sites", "Generating certificates"], () => {
        for (const v of body.vhosts) {
          const site: Site = { id: "s-" + uid().slice(0, 8), domain: v.domain, aliases: [], path: v.path, kind: "php", php: v.php, https: true, wildcard: false, env: {}, created_at: now() };
          this.sitesList.push(site);
          this.emit({ type: "site.changed", site: clone(site) });
        }
      }),
    );
  }

  async share(siteId: string) {
    const s = this.findSite(siteId);
    return this.lag(
      this.startJob(`Share ${s.domain}`, ["Downloading cloudflared", "Opening tunnel"], () => {
        const url = `https://${s.domain.split(".")[0]}-${Math.random().toString(36).slice(2, 7)}.trycloudflare.com`;
        this.shares.set(siteId, url);
        setTimeout(() => this.emit({ type: "share.url", site_id: siteId, url }), 50);
      }, 2500),
    );
  }
  async unshare(siteId: string) {
    this.shares.delete(siteId);
    await this.lag(null);
  }
  async shareStatus(siteId: string): Promise<ShareState> {
    this.findSite(siteId);
    return this.lag({ site_id: siteId, url: this.shares.get(siteId) ?? null });
  }

  async remote() {
    return this.lag({
      enabled: this.cfg.remote.enabled,
      running: this.cfg.remote.enabled,
      port: this.cfg.remote.port,
      // Unsorted on purpose (like bozkurtd): a VPN/bridge address may come first.
      addresses: this.cfg.remote.enabled ? ["10.77.1.1", "172.17.0.1", "192.168.1.24"] : [],
      paired_devices: this.devices,
    });
  }
  async setRemote(enabled: boolean) {
    // bozkurtd binds first and saves `remote.enabled` only once the listener is up
    if (enabled) this.bindRemote(this.cfg.remote.port);
    this.cfg.remote.enabled = enabled;
    return this.remote();
  }
  async pair(code: string, device_name: string) {
    await sleep(400);
    const t = Date.now();
    this.pairAttempts = this.pairAttempts.filter((x) => t - x < 60_000);
    if (this.pairAttempts.length >= 30) this.fail(429, "rate_limited", "Too many pairing attempts; wait a minute");
    this.pairAttempts.push(t);
    if (this.pairLocked) this.fail(429, "pairing_locked", "Too many wrong pairing codes; the code was revoked. Generate a new one on the computer");
    // "00000000" always pairs in the demo (there is no desktop to read a code from)
    if (code !== "00000000" && (!/^\d{8}$/.test(code) || !this.code || this.code.code !== code || Date.parse(this.code.expires_at) < t)) {
      if (this.code && ++this.pairFailures >= 10) {
        this.code = undefined;
        this.pairLocked = true;
        this.fail(429, "pairing_locked", "Too many wrong pairing codes; the code was revoked. Generate a new one on the computer");
      }
      this.fail(401, "bad_code", "Pairing code is invalid or expired");
    }
    this.code = undefined; // single use
    this.pairFailures = 0;
    const id = "dev-" + uid().slice(0, 6);
    this.devices.push({ id, name: device_name, last_seen: now() });
    return { token: "mock-" + uid(), device_id: id };
  }
  async pairingCode() {
    this.pairLocked = false;
    this.pairFailures = 0;
    this.code = { code: String(Math.floor(Math.random() * 100_000_000)).padStart(8, "0"), expires_at: new Date(Date.now() + 5 * 60_000).toISOString() };
    return this.lag(this.code);
  }
  async removeDevice(id: string) {
    this.devices = this.devices.filter((d) => d.id !== id);
    await this.lag(null);
  }

  subscribe(l: (e: BzEvent) => void) {
    this.startTimers();
    this.listeners.add(l);
    return () => {
      this.listeners.delete(l);
    };
  }
  onConnection(l: (s: ConnectionState) => void) {
    this.connListeners.add(l);
    l("open");
    return () => {
      this.connListeners.delete(l);
    };
  }
  close() {
    this.timers.forEach(clearInterval);
    this.timers = [];
    this.listeners.clear();
  }
}
