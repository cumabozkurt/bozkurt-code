import { create } from "zustand";
import { ApiError } from "./api/client";
import type { BozkurtApi, BzEvent, ConnectionState, Job, JobRef, Overview, Runtime, RuntimeKind, Service, ServiceAction, ServiceName, Site } from "./api/types";
import { detectLanguage, translate, type MessageKey } from "./i18n";
import { confirmAction } from "./components/ui";
import { isUnverifiedDownload, localizeError, localizeJobMessage, localizeJobTitle } from "./lib/daemonText";
import { RUNTIME_LABEL } from "./lib/labels";

export type Theme = "light" | "dark" | "system";
export type ToastKind = "info" | "success" | "warn" | "error";
export interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
  action?: { label: string; run: () => void };
}
export interface LogLine {
  id: number;
  source: string;
  line: string;
  at: number;
}

const MAX_LOGS = 3000;
let toastSeq = 0;
let logSeq = 0;
const jobWaiters = new Map<string, ((j: Job) => void)[]>();
const quietJobs = new Set<string>();
/** Install jobs whose `unverified download` failure is answered with a dialog instead of an error toast. */
const unverifiedHandled = new Set<string>();
const jobTitle = (j: Job) => localizeJobTitle(j.title, detectLanguage());

const t = (k: MessageKey, v?: Record<string, string | number | undefined>) => translate(detectLanguage(), k, v);

export function errorMessage(e: unknown): string {
  const lang = detectLanguage();
  if (e instanceof ApiError) {
    if (e.code === "network") return t("error.network");
    if (e.code === "timeout") return t("error.timeout");
    if (e.code === "pairing_locked") return t("error.pairingLocked");
    if (e.code === "rate_limited") return t("error.rateLimited");
    return localizeError(e.message, lang);
  }
  if (e instanceof Error) return localizeError(e.message, lang);
  return t("error.generic");
}

/** Outcome of a trust/DNS job after re-checking /tls or /dns (bozkurtd may report success without effect). */
export type VerifiedResult = "ok" | "failed" | "ineffective";

function loadTheme(): Theme {
  try {
    const v = localStorage.getItem("bozkurt.theme");
    return v === "light" || v === "dark" || v === "system" ? v : "system";
  } catch {
    return "system";
  }
}

type UiFlags = "jobsOpen" | "paletteOpen" | "shortcutsOpen" | "siteDialogOpen" | "wizardOpen";

export interface State {
  api: BozkurtApi | null;
  conn: ConnectionState;
  loaded: boolean;
  overview?: Overview;
  /** Message of the last failed full refresh; null once data loaded fine. */
  loadError: string | null;
  /** Date.now() when each service's uptime_secs was received (for a ticking uptime). */
  serviceSeen: Record<string, number>;
  services: Service[];
  sites: Site[];
  runtimes: Runtime[];
  jobs: Record<string, Job>;
  logs: LogLine[];
  shares: Record<string, string>;
  toasts: Toast[];
  theme: Theme;
  jobsOpen: boolean;
  paletteOpen: boolean;
  shortcutsOpen: boolean;
  siteDialogOpen: boolean;
  wizardOpen: boolean;

  attach(api: BozkurtApi): () => void;
  refreshAll(): Promise<void>;
  refreshOverview(): Promise<void>;
  refreshSites(): Promise<void>;
  handleEvent(e: BzEvent): void;
  applyService(s: Service): void;
  applyServices(list: Service[]): void;
  applySite(s: Site): void;
  applyRuntime(r: Runtime): void;
  /** Service action that applies the returned Service (bozkurtd emits no event for DNS). */
  serviceAction(name: ServiceName, action: ServiceAction, success?: string): Promise<Service | undefined>;
  allServicesAction(action: ServiceAction): Promise<Service[] | undefined>;
  patchService(name: ServiceName, patch: { autostart?: boolean; port?: number }): Promise<Service | undefined>;
  /** Load share state for every site (GET /share/{id}). */
  loadShares(): Promise<void>;
  setShare(siteId: string, url: string | null): void;
  /** Trust the CA, then verify with GET /tls. */
  trustCaVerified(): Promise<VerifiedResult>;
  /** Configure DNS, then verify with GET /dns. */
  configureDnsVerified(tld?: string): Promise<VerifiedResult>;
  /** Await a 202 job: registers it, then resolves when job.done/job.failed arrives (null on request error).
   *  `quiet` suppresses the generic "finished" toast (the caller reports a verified result instead). */
  trackJob(ref: Promise<JobRef>, title: string, opts?: { quiet?: boolean }): Promise<Job | null>;
  waitJob(id: string): Promise<Job>;
  /** Install a runtime and wait for the job. When the daemon refuses an asset without a published checksum
   *  (`unverified download`, e.g. static-php.dev PHP builds) it explains why and offers "Install anyway",
   *  which resends with `allow_unverified: true`. `onJob` receives each job id (for inline progress). */
  installRuntime(kind: RuntimeKind, version: string, opts?: { onJob?: (jobId: string) => void }): Promise<Job | null>;
  /** Run an API call; toast on error (and on success when `success` is given). */
  run<T>(fn: () => Promise<T>, success?: string): Promise<T | undefined>;
  toast(text: string, kind?: ToastKind, action?: Toast["action"]): void;
  dismissToast(id: number): void;
  clearLogs(): void;
  setTheme(t: Theme): void;
  set(partial: Partial<Pick<State, UiFlags>>): void;
}

function upsert<T>(list: T[], item: T, key: keyof T): T[] {
  const i = list.findIndex((x) => x[key] === item[key]);
  if (i < 0) return [...list, item];
  const next = list.slice();
  next[i] = item;
  return next;
}

export const useStore = create<State>((set, get) => ({
  api: null,
  conn: "connecting",
  loaded: false,
  loadError: null,
  serviceSeen: {},
  services: [],
  sites: [],
  runtimes: [],
  jobs: {},
  logs: [],
  shares: {},
  toasts: [],
  theme: loadTheme(),
  jobsOpen: false,
  paletteOpen: false,
  shortcutsOpen: false,
  siteDialogOpen: false,
  wizardOpen: false,

  attach(api) {
    set({ api, loaded: false, loadError: null, overview: undefined, services: [], sites: [], runtimes: [], jobs: {}, logs: [], shares: {} });
    const unsubEv = api.subscribe((e) => get().handleEvent(e));
    let prev: ConnectionState | null = null;
    const unsubConn = api.onConnection((s) => {
      set({ conn: s });
      // refetch after every (re)connect so the UI never shows stale state
      if (s === "open" && prev !== null && prev !== "open") void get().refreshAll();
      prev = s;
    });
    void get().refreshAll();
    api.jobs().then((jobs) => set({ jobs: Object.fromEntries(jobs.map((j) => [j.id, j])) })).catch(() => {});
    return () => {
      unsubEv();
      unsubConn();
      api.close();
      if (get().api === api) set({ api: null });
    };
  },

  async refreshAll() {
    const api = get().api;
    if (!api) return;
    try {
      const [overview, sites] = await Promise.all([api.overview(), api.sites()]);
      if (get().api !== api) return;
      const now = Date.now();
      set({
        overview, services: overview.services, runtimes: overview.runtimes, sites, loaded: true, loadError: null,
        serviceSeen: Object.fromEntries(overview.services.map((x) => [x.name, now])),
      });
      void get().loadShares();
    } catch (e) {
      if (get().api === api) set({ loaded: true, loadError: errorMessage(e) });
    }
  },
  async refreshOverview() {
    const api = get().api;
    if (!api) return;
    try {
      const overview = await api.overview();
      const now = Date.now();
      set({ overview, services: overview.services, runtimes: overview.runtimes, loadError: null, serviceSeen: Object.fromEntries(overview.services.map((x) => [x.name, now])) });
    } catch {
      /* the connection banner covers it */
    }
  },
  async refreshSites() {
    const api = get().api;
    if (api) set({ sites: await api.sites().catch(() => get().sites) });
  },

  handleEvent(e) {
    switch (e.type) {
      case "job.progress":
      case "job.done":
      case "job.failed": {
        const prev = get().jobs[e.job.id];
        if (prev && prev.status !== "running" && e.type === "job.progress") break; // late progress after completion
        set((s) => ({ jobs: { ...s.jobs, [e.job.id]: e.job } }));
        if (e.type !== "job.progress" && prev?.status !== e.job.status) {
          const w = jobWaiters.get(e.job.id);
          jobWaiters.delete(e.job.id);
          const quiet = quietJobs.delete(e.job.id);
          w?.forEach((r) => r(e.job));
          if (e.type === "job.done") {
            if (!quiet) get().toast(t("jobs.done", { title: jobTitle(e.job) }), "success");
          } else if (unverifiedHandled.delete(e.job.id) && isUnverifiedDownload(e.job.message)) {
            // installRuntime() explains this one in a dialog
          } else {
            const msg = localizeJobMessage(e.job.message, detectLanguage());
            get().toast(t("jobs.failed", { title: jobTitle(e.job) }) + (msg ? ` — ${msg}` : ""), "error");
          }
          void get().refreshOverview();
          void get().refreshSites();
        }
        break;
      }
      case "service.status":
        get().applyService(e.service);
        break;
      case "site.changed":
        get().applySite(e.site);
        break;
      case "site.removed":
        set((s) => {
          const shares = { ...s.shares };
          delete shares[e.id];
          const sites = s.sites.filter((x) => x.id !== e.id);
          return { sites, shares, overview: s.overview ? { ...s.overview, sites_count: sites.length } : s.overview };
        });
        break;
      case "runtime.changed":
        get().applyRuntime(e.runtime);
        break;
      case "log": {
        const line: LogLine = { id: ++logSeq, source: e.source, line: e.line, at: Date.now() };
        set((s) => {
          const logs = s.logs.length >= MAX_LOGS ? s.logs.slice(s.logs.length - MAX_LOGS + 1) : s.logs.slice();
          logs.push(line);
          return { logs };
        });
        break;
      }
      case "share.url":
        get().setShare(e.site_id, e.url);
        get().toast(`${t("sites.shared")}: ${e.url}`, "success", {
          label: t("common.copy"),
          run: () => void navigator.clipboard?.writeText(e.url),
        });
        break;
    }
  },

  applyService(svc) {
    set((s) => {
      const services = upsert(s.services, svc, "name");
      return { services, serviceSeen: { ...s.serviceSeen, [svc.name]: Date.now() }, overview: s.overview ? { ...s.overview, services } : s.overview };
    });
  },
  applyServices(list) {
    list.forEach((x) => get().applyService(x));
  },
  applySite(site) {
    set((s) => {
      const sites = upsert(s.sites, site, "id");
      return { sites, overview: s.overview ? { ...s.overview, sites_count: sites.length } : s.overview };
    });
  },
  applyRuntime(rt) {
    set((s) => {
      const runtimes = upsert(s.runtimes, rt, "kind");
      return { runtimes, overview: s.overview ? { ...s.overview, runtimes } : s.overview };
    });
  },
  async serviceAction(name, action, success) {
    const api = get().api;
    if (!api) return undefined;
    const r = await get().run(() => api.serviceAction(name, action), success);
    if (r && typeof r === "object" && "name" in r) get().applyService(r);
    return r;
  },
  async allServicesAction(action) {
    const api = get().api;
    if (!api) return undefined;
    const r = await get().run(() => api.allServicesAction(action));
    if (Array.isArray(r)) get().applyServices(r);
    return r;
  },
  async patchService(name, patch) {
    const api = get().api;
    if (!api) return undefined;
    const r = await get().run(() => api.patchService(name, patch));
    if (r && typeof r === "object" && "name" in r) get().applyService(r);
    return r;
  },
  async loadShares() {
    const api = get().api;
    if (!api) return;
    const sites = get().sites;
    const results = await Promise.all(sites.map((s) => api.shareStatus(s.id).then((r) => [s.id, r.url] as const).catch(() => null)));
    if (get().api !== api) return;
    set((s) => {
      const shares = { ...s.shares };
      for (const r of results) {
        if (!r) continue; // unknown (older daemon / error): keep what events told us
        if (r[1]) shares[r[0]] = r[1];
        else delete shares[r[0]];
      }
      return { shares };
    });
  },
  setShare(siteId, url) {
    set((s) => {
      const shares = { ...s.shares };
      if (url) shares[siteId] = url;
      else delete shares[siteId];
      return { shares };
    });
  },
  async trustCaVerified() {
    const api = get().api;
    if (!api) return "failed";
    const job = await get().trackJob(api.trustCa(), localizeJobTitle("Trust local CA", detectLanguage()), { quiet: true });
    if (job?.status !== "done") return "failed";
    const tls = await api.tls().catch(() => null);
    void get().refreshOverview();
    if (tls?.ca_trusted) {
      get().toast(t("verify.caTrusted"), "success");
      return "ok";
    }
    get().toast(t("verify.caNotTrusted"), "warn", { label: t("nav.doctor"), run: () => (location.hash = "#/doctor") });
    return "ineffective";
  },
  async configureDnsVerified(tld) {
    const api = get().api;
    if (!api) return "failed";
    const job = await get().trackJob(api.configureDns(tld), localizeJobTitle("Configure DNS", detectLanguage()), { quiet: true });
    if (job?.status !== "done") return "failed";
    const dns = await api.dns().catch(() => null);
    void get().refreshOverview();
    if (dns?.configured) {
      get().toast(t("verify.dnsConfigured", { tld: dns.tld }), "success");
      return "ok";
    }
    get().toast(t("verify.dnsNotConfigured", { tld: dns?.tld ?? tld ?? get().overview?.tld ?? "test" }), "warn", { label: t("nav.doctor"), run: () => (location.hash = "#/doctor") });
    return "ineffective";
  },

  async trackJob(ref, title, opts) {
    let jobId: string;
    try {
      jobId = (await ref).job_id;
    } catch (e) {
      get().toast(errorMessage(e), "error");
      return null;
    }
    if (opts?.quiet) quietJobs.add(jobId);
    if (!get().jobs[jobId]) {
      const placeholder: Job = { id: jobId, title, status: "running", progress: 0, started_at: new Date().toISOString() };
      set((s) => ({ jobs: { ...s.jobs, [jobId]: placeholder } }));
    }
    return get().waitJob(jobId);
  },

  async installRuntime(kind, version, opts) {
    const api = get().api;
    if (!api) return null;
    const attempt = async (allow: boolean): Promise<Job | null> => {
      const ref = api.installRuntime(kind, version, allow);
      ref.then((r) => {
        if (!allow) unverifiedHandled.add(r.job_id);
        opts?.onJob?.(r.job_id);
      }).catch(() => {});
      const job = await get().trackJob(ref, `Install ${kind} ${version}`);
      if (job) unverifiedHandled.delete(job.id);
      if (allow || job?.status !== "failed" || !isUnverifiedDownload(job.message)) return job;
      const label = `${RUNTIME_LABEL[kind]} ${version}`;
      const ok = await confirmAction(
        t(kind === "php" ? "unverified.messagePhp" : "unverified.message", { name: label }),
        t("unverified.installAnyway"),
        false,
        { title: t("unverified.title", { name: label }), details: [t("unverified.pin"), t("unverified.howto")] },
      );
      return ok ? attempt(true) : job;
    };
    return attempt(false);
  },

  waitJob(id) {
    const existing = get().jobs[id];
    if (existing && existing.status !== "running") return Promise.resolve(existing);
    return new Promise<Job>((resolve) => {
      const list = jobWaiters.get(id) ?? [];
      list.push(resolve);
      jobWaiters.set(id, list);
      // Fallback poller for when the event stream is down (or events were missed).
      const poll = setInterval(async () => {
        if (!jobWaiters.has(id)) return clearInterval(poll);
        if (get().conn === "open" && get().api?.mode === "mock") return;
        try {
          const j = await get().api!.job(id);
          if (j.status !== "running" && jobWaiters.has(id)) get().handleEvent({ type: j.status === "done" ? "job.done" : "job.failed", job: j });
          else if (j.status === "running") set((s) => ({ jobs: { ...s.jobs, [id]: (s.jobs[id]?.progress ?? 0) > j.progress ? s.jobs[id] : j } }));
        } catch {
          /* keep waiting */
        }
      }, 3000);
    });
  },

  async run(fn, success) {
    try {
      const r = await fn();
      if (success) get().toast(success, "success");
      return r;
    } catch (e) {
      get().toast(errorMessage(e), "error");
      return undefined;
    }
  },

  toast(text, kind = "info", action) {
    const id = ++toastSeq;
    set((s) => ({ toasts: [...s.toasts.slice(-4), { id, kind, text, action }] }));
    setTimeout(() => get().dismissToast(id), kind === "error" || kind === "warn" ? 9000 : action ? 8000 : 5000);
  },
  dismissToast(id) {
    set((s) => ({ toasts: s.toasts.filter((x) => x.id !== id) }));
  },
  clearLogs() {
    set({ logs: [] });
  },
  setTheme(theme) {
    localStorage.setItem("bozkurt.theme", theme);
    set({ theme });
  },
  set(partial) {
    set(partial);
  },
}));

/** What a screen should render when it has no data of its own yet. */
export function useDataState() {
  const loaded = useStore((s) => s.loaded);
  const loadError = useStore((s) => s.loadError);
  const offline = useStore((s) => s.conn !== "open" && s.api?.mode !== "mock");
  const retry = useStore((s) => s.refreshAll);
  return { loaded, loadError, offline, retry };
}

export const runningJobs = (jobs: Record<string, Job>) => Object.values(jobs).filter((j) => j.status === "running");
