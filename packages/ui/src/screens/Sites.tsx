import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import {
  AlertTriangle, ChevronDown, ChevronRight, Code2, Download, ExternalLink, FolderOpen, Globe, Lock, LockOpen, Play, Plus, RotateCw, Search, Share2, Sparkles, Square, Terminal, Trash2, X,
} from "lucide-react";
import type { CreateSiteRequest, Detection, Site, SiteKind } from "../api/types";
import { Button, Dialog, Drawer, EmptyState, ErrorState, Field, Loading, LogView, StatusBadge, Switch, Tabs, confirmAction } from "../components/ui";
import { useT, type TFn } from "../i18n";
import { RUNTIME_LABEL } from "../lib/labels";
import { copyText, isTauri, openInEditor, openTerminal, openUrl, pickFolder } from "../lib/platform";
import { siteUrl } from "../lib/urls";
import { errorMessage, useDataState, useStore } from "../store";

const KINDS: SiteKind[] = ["php", "app", "proxy", "static"];
export { siteUrl };

/** siteUrl() bound to the daemon's current HTTP/HTTPS ports. */
export function useSiteUrl() {
  const http = useStore((s) => s.overview?.http_port);
  const https = useStore((s) => s.overview?.https_port);
  return useCallback((s: Pick<Site, "https" | "domain">) => siteUrl(s, { http_port: http, https_port: https }), [http, https]);
}

/** The framework column/badge repeats the kind for static sites ("static"/"static"); hide that. */
const shownFramework = (s: { framework?: string; kind: SiteKind }) => (s.framework && s.framework !== s.kind ? s.framework : undefined);

const needsNode = (f: { kind?: SiteKind; node?: string; command?: string }) =>
  f.kind === "app" && (!!f.node || /^(npm|npx|pnpm|yarn|node)\b/.test((f.command ?? "").trim()));

/** Inline warning when a PHP (or Node app) site would be created without that runtime installed. */
function MissingRuntime({ kind }: { kind: "php" | "node" }) {
  const t = useT();
  const navigate = useNavigate();
  const api = useStore((s) => s.api);
  const installRuntime = useStore((s) => s.installRuntime);
  const installed = useStore((s) => s.runtimes.find((r) => r.kind === kind)?.installed.length ?? 0);
  const [busy, setBusy] = useState(false);
  if (installed > 0) return null;
  const label = RUNTIME_LABEL[kind];
  const install = async () => {
    if (!api) return;
    setBusy(true);
    try {
      const vs = await api.runtimeVersions(kind);
      const rec = vs.find((v) => v.channel === (kind === "node" ? "lts" : "stable")) ?? vs.find((v) => v.channel !== "eol") ?? vs[0];
      if (rec) await installRuntime(kind, rec.version);
    } catch (e) {
      useStore.getState().toast(errorMessage(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="callout warn" role="alert">
      <AlertTriangle size={16} aria-hidden />
      <span className="grow">{t("sites.runtimeMissing", { kind: label })}</span>
      <Button size="sm" variant="primary" icon={<Download />} loading={busy} onClick={install}>{t("sites.installRuntime", { kind: label })}</Button>
      <Button size="sm" onClick={() => { useStore.getState().set({ siteDialogOpen: false }); navigate(`/runtimes?kind=${kind}`); }}>{t("sites.chooseVersion")}</Button>
    </div>
  );
}

const majorMinor = (v: string) => v.split(".").slice(0, 2).join(".");
const major = (v: string) => v.split(".")[0];

function useRuntimeOptions() {
  const runtimes = useStore((s) => s.runtimes);
  return useMemo(() => ({
    php: [...new Set(runtimes.find((r) => r.kind === "php")?.installed.map(majorMinor) ?? [])],
    node: [...new Set(runtimes.find((r) => r.kind === "node")?.installed.map(major) ?? [])],
  }), [runtimes]);
}

type FormState = Omit<CreateSiteRequest, "aliases" | "env"> & { aliases: string; env: [string, string][] };

function toForm(src: Partial<Site & Detection>, path: string): FormState {
  return {
    path,
    domain: src.domain ?? src.suggested_domain ?? "",
    kind: src.kind ?? "php",
    root: src.root ?? "",
    php: src.php ?? "",
    node: src.node ?? "",
    proxy_port: src.proxy_port,
    command: src.command ?? "",
    https: src.https ?? true,
    wildcard: src.wildcard ?? false,
    aliases: (src.aliases ?? []).join(", "),
    env: Object.entries(src.env ?? {}),
  };
}
function fromForm(f: FormState): CreateSiteRequest {
  const v = (s?: string) => (s && s.trim() ? s.trim() : undefined);
  return {
    path: f.path,
    domain: v(f.domain),
    kind: f.kind,
    root: f.kind === "php" || f.kind === "static" ? v(f.root) : undefined,
    php: f.kind === "php" ? v(f.php) : undefined,
    node: f.kind === "app" ? v(f.node) : undefined,
    command: f.kind === "app" ? v(f.command) : undefined,
    proxy_port: f.kind === "proxy" ? f.proxy_port : undefined,
    https: f.https,
    wildcard: f.wildcard,
    aliases: f.aliases.split(",").map((s) => s.trim()).filter(Boolean),
    env: Object.fromEntries(f.env.filter(([k]) => k.trim()).map(([k, val]) => [k.trim(), val])),
  };
}

function EnvEditor({ value, onChange }: { value: [string, string][]; onChange: (v: [string, string][]) => void }) {
  const t = useT();
  return (
    <div className="col">
      {value.length === 0 && <span className="faint">{t("sites.envEmpty")}</span>}
      {value.map(([k, v], i) => (
        <div key={i} className="env-row">
          <input className="input mono" aria-label={t("sites.envKey")} placeholder="KEY" value={k} onChange={(e) => onChange(value.map((p, j) => (j === i ? [e.target.value, p[1]] : p)))} />
          <input className="input mono" aria-label={t("sites.envValue")} placeholder="value" value={v} onChange={(e) => onChange(value.map((p, j) => (j === i ? [p[0], e.target.value] : p)))} />
          <Button iconOnly variant="ghost" icon={<X />} aria-label={`${t("common.remove")} ${k}`} onClick={() => onChange(value.filter((_, j) => j !== i))} />
        </div>
      ))}
      <div><Button size="sm" icon={<Plus />} onClick={() => onChange([...value, ["", ""]])}>{t("sites.addVar")}</Button></div>
    </div>
  );
}

function SiteFields({ f, set, t, idp }: { f: FormState; set: (p: Partial<FormState>) => void; t: TFn; idp: string }) {
  const opts = useRuntimeOptions();
  const id = (s: string) => `${idp}-${s}`;
  return (
    <>
      <div className="form-grid">
        <Field label={t("sites.domain")} htmlFor={id("domain")}>
          <input id={id("domain")} className="input mono" value={f.domain} onChange={(e) => set({ domain: e.target.value })} placeholder="shop.test" />
        </Field>
        <Field label={t("sites.kind")} htmlFor={id("kind")}>
          <select id={id("kind")} className="input" value={f.kind} onChange={(e) => set({ kind: e.target.value as SiteKind })}>
            {KINDS.map((k) => <option key={k} value={k}>{t(`sites.kind.${k}`)}</option>)}
          </select>
        </Field>
        {(f.kind === "php" || f.kind === "static") && (
          <Field label={t("sites.root")} hint={t("sites.rootHint")} htmlFor={id("root")}>
            <input id={id("root")} className="input mono" value={f.root} onChange={(e) => set({ root: e.target.value })} placeholder={t("sites.rootPlaceholder")} />
          </Field>
        )}
        {f.kind === "php" && (
          <Field label={t("sites.php")} htmlFor={id("php")}>
            <select id={id("php")} className="input" value={f.php} onChange={(e) => set({ php: e.target.value })}>
              <option value="">{t("sites.default")}</option>
              {[...new Set([...opts.php, ...(f.php ? [f.php] : [])])].map((v) => <option key={v} value={v}>PHP {v}</option>)}
            </select>
          </Field>
        )}
        {f.kind === "app" && (
          <>
            <Field label={t("sites.node")} htmlFor={id("node")}>
              <select id={id("node")} className="input" value={f.node} onChange={(e) => set({ node: e.target.value })}>
                <option value="">{t("sites.default")}</option>
                {[...new Set([...opts.node, ...(f.node ? [f.node] : [])])].map((v) => <option key={v} value={v}>Node {v}</option>)}
              </select>
            </Field>
            <Field label={t("sites.command")} htmlFor={id("cmd")}>
              <input id={id("cmd")} className="input mono" value={f.command} onChange={(e) => set({ command: e.target.value })} placeholder="npm run dev" />
            </Field>
          </>
        )}
        {f.kind === "proxy" && (
          <Field label={t("sites.proxyPort")} htmlFor={id("port")}>
            <input id={id("port")} className="input mono" type="number" min={1} max={65535} value={f.proxy_port ?? ""} onChange={(e) => set({ proxy_port: e.target.value ? Number(e.target.value) : undefined })} placeholder="3000" />
          </Field>
        )}
      </div>
      {f.kind === "php" && <MissingRuntime kind="php" />}
      {needsNode(f) && <MissingRuntime kind="node" />}
      <Field label={t("sites.aliases")} hint={t("sites.aliasesHint")} htmlFor={id("aliases")}>
        <input id={id("aliases")} className="input mono" value={f.aliases} onChange={(e) => set({ aliases: e.target.value })} />
      </Field>
      <div className="row wrap" style={{ gap: 24 }}>
        <Switch checked={!!f.https} onChange={(v) => set({ https: v })} label={t("sites.https")} />
        <Switch checked={!!f.wildcard} onChange={(v) => set({ wildcard: v })} label={t("sites.wildcard")} hint={t("sites.wildcardHint", { domain: f.domain || "site" })} />
      </div>
    </>
  );
}

export function CreateSiteDialog() {
  const t = useT();
  const api = useStore((s) => s.api);
  const close = () => useStore.getState().set({ siteDialogOpen: false });
  const [path, setPath] = useState("");
  const [det, setDet] = useState<Detection | null>(null);
  const [detecting, setDetecting] = useState(false);
  const [f, setF] = useState<FormState | null>(null);
  const [saving, setSaving] = useState(false);
  const [showEnv, setShowEnv] = useState(false);
  const [detectError, setDetectError] = useState<string | null>(null);
  const [createError, setCreateError] = useState<string | null>(null);
  const siteLink = useSiteUrl();

  const detect = async (p: string) => {
    if (!api || !p.trim()) return;
    setDetecting(true);
    setDetectError(null);
    try {
      const d = await api.detect(p.trim());
      setDet(d);
      setF(toForm({ ...d, https: true }, p.trim()));
    } catch (e) {
      setDet(null);
      setF(null);
      setDetectError(errorMessage(e));
    } finally {
      setDetecting(false);
    }
  };
  const browse = async () => {
    const p = await pickFolder(t("sites.pickFolder"));
    if (p) {
      setPath(p);
      void detect(p);
    }
  };
  const submit = async () => {
    if (!api || !f) return;
    setSaving(true);
    setCreateError(null);
    try {
      const s = await api.createSite(fromForm(f));
      useStore.getState().applySite(s);
      useStore.getState().toast(t("sites.created", { domain: s.domain }), "success", { label: t("common.open"), run: () => void openUrl(siteLink(s)) });
      close();
    } catch (e) {
      setCreateError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog
      title={t("sites.add")}
      onClose={close}
      footer={
        <>
          <Button onClick={close}>{t("common.cancel")}</Button>
          <Button variant="primary" disabled={!f} loading={saving} onClick={submit} title={!f ? t("sites.detectFirst") : undefined}>{t("common.create")}</Button>
        </>
      }
    >
      <form className="col" onSubmit={(e) => { e.preventDefault(); void detect(path); }}>
        <label className="label" htmlFor="new-site-path">{t("sites.path")}</label>
        <div className="row wrap">
          <input id="new-site-path" className="input mono grow" style={{ minWidth: 200 }} autoFocus value={path} placeholder={t("sites.folderPlaceholder")}
            aria-invalid={!!detectError || undefined} aria-describedby={detectError ? "new-site-path-err" : "new-site-path-hint"}
            onChange={(e) => setPath(e.target.value)} onBlur={() => path.trim() && path.trim() !== det?.path && void detect(path)} />
          {isTauri() && <Button icon={<FolderOpen />} onClick={browse}>{t("sites.pickFolder")}</Button>}
          <Button type="submit" icon={<Sparkles />} loading={detecting} disabled={!path.trim()}>{t("sites.detect")}</Button>
        </div>
        {detectError ? (
          <span id="new-site-path-err" className="error-text" role="alert">{detectError}</span>
        ) : !f && !detecting ? (
          <span id="new-site-path-hint" className="hint-text">{t("sites.detectFirst")}</span>
        ) : null}
      </form>
      {detecting && <span className="muted" role="status">{t("sites.detecting")}</span>}
      {det && f && (
        <>
          <span className="badge accent" style={{ alignSelf: "flex-start" }}>
            <Sparkles size={12} />
            {shownFramework(det) ? t("sites.detected", { framework: det.framework! }) : t("sites.detectedGeneric", { kind: t(`sites.kind.${det.kind}`) })}
          </span>
          <SiteFields idp="new-site" f={f} set={(p) => setF({ ...f, ...p })} t={t} />
          <div className="col">
            <button type="button" className="disclosure" aria-expanded={showEnv} aria-controls="new-site-env" onClick={() => setShowEnv(!showEnv)}>
              {showEnv ? <ChevronDown size={16} aria-hidden /> : <ChevronRight size={16} aria-hidden />}
              {t("sites.envCount", { n: f.env.length })}
            </button>
            {showEnv && <div id="new-site-env"><EnvEditor value={f.env} onChange={(env) => setF({ ...f, env })} /></div>}
          </div>
        </>
      )}
      {createError && <p className="error-text" role="alert">{createError}</p>}
    </Dialog>
  );
}

function SiteDrawer({ site, onClose }: { site: Site; onClose: () => void }) {
  const t = useT();
  const api = useStore((s) => s.api);
  const run = useStore((s) => s.run);
  const trackJob = useStore((s) => s.trackJob);
  const toast = useStore((s) => s.toast);
  const shareUrl = useStore((s) => s.shares[site.id]);
  const setShare = useStore((s) => s.setShare);
  const liveLogs = useStore((s) => s.logs);
  const siteLink = useSiteUrl();
  const [tab, setTab] = useState<"settings" | "env" | "logs">("settings");
  const [f, setF] = useState(() => toForm(site, site.path));
  const [saving, setSaving] = useState(false);
  const [logs, setLogs] = useState<string[]>([]);
  const [sharing, setSharing] = useState(false);
  const [since] = useState(() => Date.now());
  useEffect(() => setF(toForm(site, site.path)), [site]);
  // Share state survives reloads: ask the daemon instead of relying on share.url events only.
  useEffect(() => {
    api?.shareStatus(site.id).then((r) => setShare(site.id, r.url)).catch(() => {});
  }, [api, site.id, setShare]);
  useEffect(() => {
    if (tab === "logs") api?.siteLogs(site.id).then((r) => setLogs(r.lines)).catch(() => setLogs([]));
  }, [tab, api, site.id]);
  const live = liveLogs.filter((l) => l.at >= since && (l.source === `site:${site.domain}` || l.line.includes(site.domain)));

  const save = async () => {
    if (!api) return;
    setSaving(true);
    const body = fromForm(f);
    const r = await run(() => api.patchSite(site.id, { ...body, aliases: body.aliases ?? [], env: body.env ?? {} }), t("sites.saved", { domain: site.domain }));
    if (r) useStore.getState().applySite(r);
    setSaving(false);
  };
  const remove = async () => {
    if (!(await confirmAction(t("sites.deleteConfirm", { domain: site.domain }), t("common.remove")))) return;
    const ok = await run(async () => {
      await api!.deleteSite(site.id);
      return true;
    });
    if (ok) {
      toast(t("sites.deleted", { domain: site.domain }), "success");
      onClose();
    }
  };
  const share = async () => {
    if (!api) return;
    if (shareUrl) {
      setSharing(true);
      await run(async () => {
        await api.unshare(site.id);
        setShare(site.id, null);
      }, t("sites.shareStopped", { domain: site.domain }));
      setSharing(false);
      return;
    }
    // A quick tunnel makes the site reachable by anyone on the internet: always confirm.
    if (!(await confirmAction(t("sites.shareConfirm", { url: siteLink(site) }), t("sites.share"), false))) return;
    setSharing(true);
    const job = await trackJob(api.share(site.id), `Share ${site.domain}`);
    if (job?.status === "done") {
      const st = await api.shareStatus(site.id).catch(() => null);
      if (st) setShare(site.id, st.url ?? (job.message?.startsWith("https://") ? job.message : null));
    }
    setSharing(false);
  };
  const siteAction = async (a: "start" | "stop" | "restart") => {
    const r = await run(() => api!.siteAction(site.id, a));
    if (r) useStore.getState().applySite(r);
  };
  const terminal = async () => {
    if (!(await openTerminal(site.path))) {
      const cmd = `cd ${JSON.stringify(site.path)}`;
      await copyText(cmd);
      toast(t("sites.terminalCopied", { cmd }), "info");
    }
  };

  return (
    <Drawer wide title={site.domain} onClose={onClose}>
      <div className="row wrap">
        <Button variant="primary" icon={<ExternalLink />} onClick={() => openUrl(siteLink(site))} title={siteLink(site)}>{t("sites.openBrowser")}</Button>
        <Button icon={<Code2 />} onClick={() => openInEditor(site.path)}>{t("sites.openEditor")}</Button>
        <Button icon={<Terminal />} onClick={terminal}>{t("sites.openTerminal")}</Button>
        <Button icon={<Share2 />} variant={shareUrl ? "danger" : "default"} loading={sharing} onClick={share}>{shareUrl ? t("sites.stopShare") : t("sites.share")}</Button>
      </div>
      {shareUrl && (
        <div className="row card" style={{ padding: 10 }}>
          <Globe size={16} color="var(--accent-fg)" aria-hidden />
          <a className="mono grow ellipsis" href={shareUrl} onClick={(e) => { e.preventDefault(); void openUrl(shareUrl); }}>{shareUrl}</a>
          <Button size="sm" onClick={() => copyText(shareUrl).then(() => toast(t("common.copied"), "success"))}>{t("common.copy")}</Button>
        </div>
      )}
      {site.kind === "app" && (
        <div className="row wrap card" style={{ padding: 10 }}>
          <strong className="grow">{t("sites.app")} {site.app_port ? <span className="mono faint">:{site.app_port}</span> : null}</strong>
          {site.app_status && <StatusBadge status={site.app_status} />}
          <Button size="sm" icon={<Play />} onClick={() => siteAction("start")}>{t("common.start")}</Button>
          <Button size="sm" icon={<RotateCw />} onClick={() => siteAction("restart")}>{t("common.restart")}</Button>
          <Button size="sm" icon={<Square />} onClick={() => siteAction("stop")}>{t("common.stop")}</Button>
        </div>
      )}
      <dl className="kv">
        <dt>{t("sites.path")}</dt>
        <dd className="mono">{site.path}</dd>
        <dt>{t("sites.url")}</dt>
        <dd className="mono">{siteLink(site)}</dd>
        {shownFramework(site) && (
          <>
            <dt>{t("sites.framework")}</dt>
            <dd>{site.framework}</dd>
          </>
        )}
      </dl>
      <Tabs label={site.domain} value={tab} onChange={setTab} items={[
        { id: "settings", label: t("sites.tabSettings") },
        { id: "env", label: `${t("sites.tabEnv")} (${Object.keys(site.env).length})` },
        { id: "logs", label: t("sites.tabLogs") },
      ]} />
      {tab === "settings" && (
        <>
          <SiteFields idp={`site-${site.id}`} f={f} set={(p) => setF({ ...f, ...p })} t={t} />
          <div className="row">
            <Button variant="danger" icon={<Trash2 />} onClick={remove}>{t("common.remove")}</Button>
            <span className="spacer" />
            <Button variant="primary" loading={saving} onClick={save}>{t("common.save")}</Button>
          </div>
        </>
      )}
      {tab === "env" && (
        <>
          <EnvEditor value={f.env} onChange={(env) => setF({ ...f, env })} />
          <div className="row"><span className="spacer" /><Button variant="primary" loading={saving} onClick={save}>{t("common.save")}</Button></div>
        </>
      )}
      {tab === "logs" && <LogView label={`${site.domain} ${t("common.logs")}`} lines={[...logs.map((line) => ({ line })), ...live]} />}
    </Drawer>
  );
}

export function SitesScreen() {
  const t = useT();
  const sites = useStore((s) => s.sites);
  const overview = useStore((s) => s.overview);
  const api = useStore((s) => s.api);
  const run = useStore((s) => s.run);
  const shares = useStore((s) => s.shares);
  const ds = useDataState();
  const siteLink = useSiteUrl();
  const [params, setParams] = useSearchParams();
  const [q, setQ] = useState("");
  const openId = params.get("site");
  const open = sites.find((s) => s.id === openId);
  const filtered = useMemo(
    () =>
      sites
        .filter((s) => !q || [s.domain, s.path, s.framework ?? "", ...s.aliases].some((x) => x.toLowerCase().includes(q.toLowerCase())))
        .sort((a, b) => a.domain.localeCompare(b.domain)),
    [sites, q],
  );
  const add = () => useStore.getState().set({ siteDialogOpen: true });
  const select = (id: string | null) => {
    const n = new URLSearchParams(params);
    if (id) n.set("site", id);
    else n.delete("site");
    setParams(n, { replace: true });
  };

  return (
    <div className="page">
      <div className="page-head">
        <div className="titles"><h1>{t("sites.title")}</h1></div>
        {sites.length > 0 && (
          <div className="row" style={{ minWidth: 220 }}>
            <Search size={16} className="faint" aria-hidden />
            <input className="input" type="search" aria-label={t("sites.filter")} placeholder={t("sites.filter")} value={q} onChange={(e) => setQ(e.target.value)} />
          </div>
        )}
        <Button variant="primary" icon={<Plus />} onClick={add}>{t("sites.add")}</Button>
      </div>

      {sites.length === 0 && (!ds.loaded || ds.offline || ds.loadError) ? (
        <div className="card">
          {!ds.loaded ? <Loading /> : <ErrorState offline={ds.offline} message={ds.offline ? undefined : ds.loadError} onRetry={ds.retry} />}
        </div>
      ) : sites.length === 0 ? (
        <div className="card">
          <EmptyState
            icon={<Globe />}
            title={t("sites.emptyTitle")}
            body={t("sites.emptyBody", { tld: overview?.tld ?? "test" })}
            action={<Button variant="primary" icon={<Plus />} onClick={add}>{t("sites.add")}</Button>}
          />
        </div>
      ) : (
        <div className="card table-wrap">
          <div className="scroll">
            <table className="table">
              <thead>
                <tr>
                  <th>{t("sites.domain")}</th>
                  <th className="hide-sm">{t("sites.kind")}</th>
                  <th className="hide-sm">{t("sites.framework")}</th>
                  <th className="hide-sm">{t("common.version")}</th>
                  <th className="hide-sm">{t("sites.path")}</th>
                  <th><span className="sr-only">{t("common.open")}</span></th>
                </tr>
              </thead>
              <tbody>
                {filtered.map((s) => (
                  <tr key={s.id} onClick={() => select(s.id)} onKeyDown={(e) => e.key === "Enter" && select(s.id)} tabIndex={0} aria-label={s.domain}>
                    <td>
                      <div className="row">
                        {s.https ? <Lock size={14} color="var(--ok)" aria-hidden /> : <LockOpen size={14} className="faint" aria-hidden />}
                        <span className="sr-only">{s.https ? "HTTPS" : "HTTP"}</span>
                        <div className="col" style={{ gap: 0, minWidth: 0 }}>
                          <strong className="mono">{s.domain}</strong>
                          <span className="faint show-sm" style={{ fontSize: 12 }}>
                            {t(`sites.kind.${s.kind}`)}{s.php ? ` · PHP ${s.php}` : s.node ? ` · Node ${s.node}` : s.proxy_port ? ` · :${s.proxy_port}` : ""}
                          </span>
                        </div>
                        {s.wildcard && <span className="badge" title={t("sites.wildcard")}>*.</span>}
                        {shares[s.id] && <span className="badge accent" title={t("sites.shared")}><Share2 size={11} /></span>}
                        {s.kind === "app" && s.app_status && <span className={`status-dot ${s.app_status}`} title={t(`status.${s.app_status}`)} />}
                      </div>
                    </td>
                    <td className="hide-sm">{t(`sites.kind.${s.kind}`)}</td>
                    <td className="hide-sm">{shownFramework(s) ?? <span className="faint">—</span>}</td>
                    <td className="hide-sm mono">{s.php ? `PHP ${s.php}` : s.node ? `Node ${s.node}` : s.proxy_port ? `:${s.proxy_port}` : "—"}</td>
                    <td className="hide-sm mono faint ellipsis" style={{ maxWidth: 260 }}>{s.path}</td>
                    <td style={{ textAlign: "right" }}>
                      <div className="row" style={{ justifyContent: "flex-end" }} onClick={(e) => e.stopPropagation()}>
                        <Button size="sm" variant="ghost" iconOnly icon={s.https ? <Lock /> : <LockOpen />} aria-pressed={s.https}
                          aria-label={s.https ? t("sites.disableHttps", { domain: s.domain }) : t("sites.enableHttps", { domain: s.domain })}
                          title={s.https ? t("sites.disableHttps", { domain: s.domain }) : t("sites.enableHttps", { domain: s.domain })}
                          onClick={async () => {
                            const r = await run(() => api!.siteAction(s.id, s.https ? "unsecure" : "secure"));
                            if (r) useStore.getState().applySite(r);
                          }} />
                        <Button size="sm" variant="ghost" iconOnly icon={<ExternalLink />} aria-label={`${t("sites.openBrowser")} ${s.domain}`} title={siteLink(s)} onClick={() => openUrl(siteLink(s))} />
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
      {open && <SiteDrawer site={open} onClose={() => select(null)} />}
    </div>
  );
}
