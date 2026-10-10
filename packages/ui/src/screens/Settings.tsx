import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, Download, KeyRound, LogOut, Network, Smartphone, Trash2 } from "lucide-react";
import type { Config, ImportScan, PairingCode, RemoteInfo } from "../api/types";
import { pairingUri } from "../api/client";
import { Button, ErrorState, Field, Loading, Segmented, Switch, confirmAction } from "../components/ui";
import { QrCode } from "../components/QrCode";
import { useAppMode } from "../context";
import { LANGUAGES, useI18n, type Lang, type MessageKey } from "../i18n";
import { isValidPort, sortAddresses } from "../lib/urls";
import { portErrorField } from "../lib/daemonText";
import { errorMessage, useStore, type Theme } from "../store";
import { UI_VERSION } from "../version";

type PortKey = "http_port" | "https_port" | "dns_port";
const PORT_KEYS: PortKey[] = ["http_port", "https_port", "dns_port"];
const RESERVED_TLDS = ["dev", "app", "com", "net", "org", "io"];

/** Client-side validation of the editable config. Returns an error message key per field. */
export function validateConfigDraft(d: { tld: string; ports: Record<PortKey, string> }, cfg: Config): Partial<Record<"tld" | PortKey, MessageKey>> {
  const err: Partial<Record<"tld" | PortKey, MessageKey>> = {};
  const tld = d.tld.trim();
  if (!tld) err.tld = "settings.err.tldEmpty";
  else if (!/^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/.test(tld)) err.tld = "settings.err.tldInvalid";
  else if (RESERVED_TLDS.includes(tld)) err.tld = tld === "dev" || tld === "app" ? "settings.err.tldHsts" : "settings.err.tldReal";
  const nums: Partial<Record<PortKey, number>> = {};
  for (const k of PORT_KEYS) {
    const raw = d.ports[k].trim();
    const n = Number(raw);
    if (!raw || !/^\d+$/.test(raw) || !isValidPort(n)) err[k] = "settings.err.portRange";
    else nums[k] = n;
  }
  const taken = new Map<number, PortKey>();
  for (const k of PORT_KEYS) {
    const n = nums[k];
    if (n === undefined) continue;
    if (n === cfg.api_port || n === cfg.remote?.port) err[k] = "settings.err.portApi";
    else if (taken.has(n)) err[k] = "settings.err.portDuplicate";
    else taken.set(n, k);
  }
  return err;
}

function PairingPanel({ remote }: { remote: RemoteInfo }) {
  const { t } = useI18n();
  const api = useStore((s) => s.api);
  const run = useStore((s) => s.run);
  const [code, setCode] = useState<PairingCode | null>(null);
  const [left, setLeft] = useState(0);
  const addresses = useMemo(() => sortAddresses(remote.addresses), [remote.addresses]);
  const [addr, setAddr] = useState<string>("");
  useEffect(() => {
    if (!addr || !addresses.includes(addr)) setAddr(addresses[0] ?? "");
  }, [addresses, addr]);
  useEffect(() => {
    if (!code) return;
    const tick = () => setLeft(Math.max(0, Math.round((new Date(code.expires_at).getTime() - Date.now()) / 1000)));
    tick();
    const i = setInterval(tick, 1000);
    return () => clearInterval(i);
  }, [code]);
  const host = addr ? `${addr}:${remote.port}` : "";
  return (
    <div className="col" style={{ gap: 12 }}>
      <h3>{t("settings.pairing")}</h3>
      {!remote.running && <p className="callout warn" role="alert"><AlertTriangle size={16} aria-hidden />{t("settings.remoteNotRunning", { port: remote.port })}</p>}
      {addresses.length === 0 && remote.running && <p className="callout warn" role="alert"><AlertTriangle size={16} aria-hidden />{t("settings.noAddresses")}</p>}
      {addresses.length > 1 && (
        <Field label={t("settings.qrAddress")} hint={t("settings.qrAddressHint")} htmlFor="pair-addr">
          <select id="pair-addr" className="input mono" style={{ maxWidth: 340 }} value={addr} onChange={(e) => setAddr(e.target.value)}>
            {addresses.map((a, i) => <option key={a} value={a}>{a}:{remote.port}{i === 0 ? ` (${t("settings.recommended")})` : ""}</option>)}
          </select>
        </Field>
      )}
      {code && left > 0 && host ? (
        <div className="row wrap" style={{ gap: 24, alignItems: "flex-start" }}>
          <QrCode value={pairingUri(host, code.code)} size={176} label={t("settings.qrLabel", { host })} />
          <div className="col" style={{ gap: 8 }}>
            <span className="code-big" aria-label={code.code.split("").join(" ")}>{code.code.length === 8 ? `${code.code.slice(0, 4)} ${code.code.slice(4)}` : code.code}</span>
            <span className="muted" aria-live="off">{t("settings.expiresIn", { t: `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}` })}</span>
            <span className="muted" style={{ maxWidth: 320 }}>{t("settings.scanHint")}</span>
            <span className="faint" style={{ maxWidth: 320, fontSize: 12 }}>{t("settings.codeLimits")}</span>
            <dl className="kv">
              <dt>{t("settings.address")}</dt>
              <dd className="mono">{host}</dd>
            </dl>
          </div>
        </div>
      ) : (
        <div className="row">
          {code && <span className="muted">{t("settings.expired")}</span>}
          <Button variant="primary" icon={<KeyRound />} disabled={!host || !remote.running} onClick={async () => setCode((await run(() => api!.pairingCode())) ?? null)}>{t("settings.generateCode")}</Button>
        </div>
      )}
    </div>
  );
}

export function SettingsScreen() {
  const { t, lang, setLang, fmtRelative } = useI18n();
  const api = useStore((s) => s.api);
  const run = useStore((s) => s.run);
  const theme = useStore((s) => s.theme);
  const setTheme = useStore((s) => s.setTheme);
  const overview = useStore((s) => s.overview);
  const trackJob = useStore((s) => s.trackJob);
  const conn = useStore((s) => s.conn);
  const mode = useAppMode();
  const remoteMode = mode.mode === "remote";
  const [cfg, setCfg] = useState<Config | null>(null);
  const [cfgError, setCfgError] = useState<string | null>(null);
  const [draft, setDraft] = useState<{ tld: string; ports: Record<PortKey, string> } | null>(null);
  const [remote, setRemote] = useState<RemoteInfo | null>(null);
  const [remoteError, setRemoteError] = useState<string | null>(null);
  const [scan, setScan] = useState<ImportScan | null>(null);
  const [saving, setSaving] = useState(false);
  /** Port errors returned by bozkurtd for the last save, shown under their field. */
  const [serverErr, setServerErr] = useState<Partial<Record<PortKey, string>>>({});
  const [remoteToggleError, setRemoteToggleError] = useState<string | null>(null);
  const [lanBusy, setLanBusy] = useState(false);

  const toDraft = (c: Config) => ({ tld: c.tld, ports: Object.fromEntries(PORT_KEYS.map((k) => [k, String(c[k] ?? "")])) as Record<PortKey, string> });
  const load = () => {
    if (!api) return;
    api.config().then((c) => {
      setCfg(c);
      setDraft(toDraft(c));
      setCfgError(null);
    }).catch((e) => setCfgError(errorMessage(e)));
    if (!remoteMode) api.remote().then((r) => {
      setRemote(r);
      setRemoteError(null);
    }).catch((e) => setRemoteError(errorMessage(e)));
  };
  useEffect(load, [api, remoteMode]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (conn === "open" && (cfgError || remoteError)) load();
  }, [conn]); // eslint-disable-line react-hooks/exhaustive-deps

  const errors = cfg && draft ? validateConfigDraft(draft, cfg) : {};
  const invalid = Object.keys(errors).length > 0;
  const patch: Partial<Config> = {};
  if (cfg && draft) {
    if (draft.tld.trim() !== cfg.tld) patch.tld = draft.tld.trim();
    for (const k of PORT_KEYS) if (Number(draft.ports[k]) !== cfg[k]) patch[k] = Number(draft.ports[k]);
  }
  const dirty = Object.keys(patch).length > 0;
  const save = async () => {
    if (!cfg || invalid || !dirty) return;
    // Never send an invalid port (0, NaN, out of range): validated above, re-checked here.
    for (const k of PORT_KEYS) if (k in patch && !isValidPort(patch[k])) return;
    setSaving(true);
    setServerErr({});
    let next: Config | undefined;
    try {
      next = await api!.patchConfig(patch);
      useStore.getState().toast(t("settings.saved"), "success");
    } catch (e) {
      // bozkurtd validates every port (1-65535, all distinct, incl. api_port / remote.port): show it inline
      const pe = portErrorField(e instanceof Error ? e.message : "", lang);
      // the daemon names the later key of a clash ("api_port (80) is already used by http_port"): mark an editable one
      const field = pe && ([pe.key, pe.other].find((k) => k && (PORT_KEYS as string[]).includes(k)) as PortKey | undefined);
      if (pe && field) setServerErr({ [field]: pe.message });
      else useStore.getState().toast(errorMessage(e), "error");
    }
    if (next) {
      setCfg(next);
      setDraft(toDraft(next));
      if (patch.tld && patch.tld !== overview?.tld) {
        void useStore.getState().configureDnsVerified(patch.tld).then(async () => {
          // A TLD outside test/localhost regenerates the name-constrained local CA: it must be trusted again.
          const tls = await api!.tls().catch(() => null);
          if (tls && !tls.ca_trusted)
            useStore.getState().toast(t("settings.retrustCa", { tld: patch.tld! }), "warn", { label: t("settings.trustAgain"), run: () => void useStore.getState().trustCaVerified() });
        });
      }
      if (patch.http_port || patch.https_port || patch.dns_port) useStore.getState().toast(t("settings.restartHint"), "info");
      void useStore.getState().refreshOverview();
    }
    setSaving(false);
  };
  const portField = (k: PortKey, label: string) => (
    <Field label={label} htmlFor={`cfg-${k}`}>
      <input id={`cfg-${k}`} className="input mono" inputMode="numeric" type="number" min={1} max={65535} step={1}
        value={draft?.ports[k] ?? ""} readOnly={remoteMode} aria-invalid={!!(errors[k] || serverErr[k]) || undefined} aria-describedby={errors[k] || serverErr[k] ? `cfg-${k}-err` : undefined}
        onChange={(e) => { if (!draft) return; setDraft({ ...draft, ports: { ...draft.ports, [k]: e.target.value } }); setServerErr({}); }} />
      {errors[k] ? <span id={`cfg-${k}-err`} className="error-text">{t(errors[k]!)}</span> : serverErr[k] && <span id={`cfg-${k}-err`} className="error-text" role="alert">{serverErr[k]}</span>}
    </Field>
  );

  return (
    <div className="page" style={{ maxWidth: 820 }}>
      <div className="page-head"><div className="titles"><h1>{t("settings.title")}</h1></div></div>

      <section className="card" aria-labelledby="set-appearance">
        <h2 id="set-appearance">{t("settings.appearance")}</h2>
        <div className="form-grid">
          <Field label={t("settings.language")} htmlFor="set-lang">
            <select id="set-lang" className="input" value={lang} onChange={(e) => setLang(e.target.value as Lang)}>
              {LANGUAGES.map((l) => <option key={l.id} value={l.id}>{l.label}</option>)}
            </select>
          </Field>
          <div className="field">
            <span className="label">{t("settings.theme")}</span>
            <Segmented<Theme> label={t("settings.theme")} value={theme} onChange={setTheme} items={[
              { id: "light", label: t("settings.theme.light") },
              { id: "dark", label: t("settings.theme.dark") },
              { id: "system", label: t("settings.theme.system") },
            ]} />
          </div>
        </div>
      </section>

      {mode.mode === "remote" && (
        <section className="card">
          <h2>{t("settings.connection")}</h2>
          <dl className="kv"><dt>API</dt><dd className="mono">{mode.remoteHost ?? "—"}</dd></dl>
          <div>
            <Button variant="danger" icon={<LogOut />} onClick={async () => {
              if (await confirmAction(t("settings.disconnect"), t("settings.disconnect"))) mode.disconnect?.();
            }}>{t("settings.disconnect")}</Button>
          </div>
        </section>
      )}

      <section className="card" aria-labelledby="set-general">
        <h2 id="set-general">{t("common.general")}</h2>
        {!cfg || !draft ? (
          cfgError ? <ErrorState offline={conn !== "open" && api?.mode !== "mock"} message={cfgError} onRetry={load} /> : <Loading />
        ) : (
          <>
            {remoteMode && <p className="callout info">{t("settings.remoteReadOnly")}</p>}
            <Field label={t("settings.tld")} hint={errors.tld ? undefined : `${t("settings.tldHint", { tld: draft.tld })} ${t("settings.tldCaHint")}`} htmlFor="cfg-tld">
              <input id="cfg-tld" className="input mono" style={{ maxWidth: 200 }} value={draft.tld} readOnly={remoteMode}
                aria-invalid={!!errors.tld || undefined} aria-describedby={errors.tld ? "cfg-tld-err" : undefined}
                onChange={(e) => setDraft({ ...draft, tld: e.target.value.replace(/[^a-z0-9-]/gi, "").toLowerCase() })} />
              {errors.tld && <span id="cfg-tld-err" className="error-text">{t(errors.tld, { tld: draft.tld })}</span>}
            </Field>
            <span className="label">{t("settings.ports")}</span>
            <div className="form-grid">
              {portField("http_port", t("settings.httpPort"))}
              {portField("https_port", t("settings.httpsPort"))}
              {portField("dns_port", t("settings.dnsPort"))}
              <Field label={t("settings.apiPort")} hint={t("settings.apiPortHint")} htmlFor="cfg-api_port">
                <input id="cfg-api_port" className="input mono" value={cfg.api_port} readOnly aria-readonly="true" />
              </Field>
            </div>
            {!remoteMode && (
              <div className="row">
                <span className="spacer" />
                <Button disabled={!dirty} onClick={() => setDraft(toDraft(cfg))}>{t("common.cancel")}</Button>
                <Button variant="primary" disabled={!dirty || invalid} loading={saving} onClick={save}>{t("common.save")}</Button>
              </div>
            )}
          </>
        )}
      </section>

      {cfg && (
        <section className="card" aria-labelledby="set-network">
          <div className="card-head"><Network size={18} aria-hidden /><h2 id="set-network">{t("settings.network")}</h2></div>
          <Switch
            checked={!!cfg.lan_access}
            disabled={remoteMode || lanBusy}
            label={t("settings.lanAccess")}
            hint={t("settings.lanAccessHint", { tld: cfg.tld })}
            onChange={async (v) => {
              if (v && !(await confirmAction(t("settings.lanAccessConfirm"), t("settings.lanAccessEnable"), false))) return;
              setLanBusy(true);
              const next = await run(() => api!.patchConfig({ lan_access: v }), v ? t("settings.lanAccessOn") : t("settings.lanAccessOff"));
              if (next) setCfg(next);
              setLanBusy(false);
            }}
          />
          {cfg.lan_access && <p className="callout warn" role="alert"><AlertTriangle size={16} aria-hidden />{t("settings.lanAccessWarn")}</p>}
          <dl className="kv">
            <dt>{t("settings.allowUnverified")}</dt>
            <dd>
              <span className={`badge ${cfg.allow_unverified ? "warn" : "ok"}`}>{cfg.allow_unverified ? t("settings.allowUnverifiedOn") : t("settings.allowUnverifiedOff")}</span>
            </dd>
          </dl>
          <p className="faint" style={{ fontSize: 12.5 }}>{t("settings.allowUnverifiedHint")}</p>
        </section>
      )}

      {mode.mode === "local" && !remote && (
        <section className="card" aria-labelledby="set-remote">
          <div className="card-head"><Smartphone size={18} aria-hidden /><h2 id="set-remote">{t("settings.remote")}</h2></div>
          {remoteError ? <ErrorState offline={conn !== "open" && api?.mode !== "mock"} message={remoteError} onRetry={load} /> : <Loading />}
        </section>
      )}
      {mode.mode === "local" && remote && (
        <section className="card" aria-labelledby="set-remote">
          <div className="card-head">
            <Smartphone size={18} aria-hidden /><h2 id="set-remote">{t("settings.remote")}</h2>
            {remote.enabled && <span className={`badge ${remote.running ? "ok" : "warn"}`}>{remote.running ? t("settings.listening", { port: remote.port }) : t("settings.notListening")}</span>}
          </div>
          <p className="muted">{t("settings.remoteDesc")}</p>
          <Switch checked={remote.enabled} onChange={async (v) => {
            const prev = remote;
            setRemoteToggleError(null);
            setRemote({ ...remote, enabled: v }); // optimistic
            try {
              setRemote(await api!.setRemote(v));
            } catch (e) {
              // 409: the listener could not bind; bozkurtd did not save `remote.enabled`
              setRemoteToggleError(errorMessage(e));
              setRemote(await api!.remote().catch(() => prev));
            }
          }} label={t("settings.enableRemote")} />
          {remoteToggleError && <p className="callout danger" role="alert"><AlertTriangle size={16} aria-hidden />{remoteToggleError}</p>}
          {remote.enabled && <PairingPanel remote={remote} />}
          <h3>{t("settings.devices")}</h3>
          {remote.paired_devices.length === 0 ? (
            <span className="faint">{t("settings.noDevices")}</span>
          ) : (
            <div className="col">
              {remote.paired_devices.map((d) => (
                <div key={d.id} className="svc-tile">
                  <Smartphone size={16} aria-hidden />
                  <div className="grow col" style={{ gap: 0 }}>
                    <span className="name">{d.name}</span>
                    <span className="faint" style={{ fontSize: 12 }}>{t("settings.lastSeen", { t: fmtRelative(d.last_seen) })}</span>
                  </div>
                  <Button size="sm" variant="ghost" iconOnly icon={<Trash2 />} aria-label={`${t("common.remove")} ${d.name}`} title={`${t("common.remove")} ${d.name}`} onClick={async () => {
                    if (!(await confirmAction(t("settings.removeDeviceConfirm", { name: d.name }), t("common.remove")))) return;
                    await run(() => api!.removeDevice(d.id));
                    const r = await api!.remote().catch(() => null);
                    if (r) setRemote(r);
                  }} />
                </div>
              ))}
            </div>
          )}
        </section>
      )}

      {mode.mode === "local" && (
        <section className="card" aria-labelledby="set-import">
          <h2 id="set-import">{t("settings.import")}</h2>
          <p className="muted">{t("settings.importDesc")}</p>
          {scan && (scan.vhosts.length + scan.dns.length === 0 ? (
            <span className="faint">{t("settings.nothingToImport")}</span>
          ) : (
            <div className="col">
              {scan.vhosts.map((v) => <div key={`${v.source}:${v.domain}`} className="row mono" style={{ fontSize: 12.5 }}><span className="badge">{v.source}</span>{v.domain}<span className="muted ellipsis">{v.path}</span></div>)}
              {scan.dns.map((d) => <div key={`${d.source}:${d.file}`} className="row mono" style={{ fontSize: 12.5 }}><span className="badge">{d.source}</span>.{d.tld}<span className="muted">{d.file}</span></div>)}
            </div>
          ))}
          <div className="row">
            <Button disabled={conn !== "open" && api?.mode !== "mock"} onClick={async () => setScan((await run(() => api!.importScan())) ?? null)}>{t("settings.scan")}</Button>
            {scan && scan.vhosts.length + scan.dns.length > 0 && (
              <Button variant="primary" icon={<Download />} onClick={async () => { await trackJob(api!.importApply(scan), t("settings.import")); setScan(null); }}>
                {t("settings.importAll", { n: scan.vhosts.length + scan.dns.length })}
              </Button>
            )}
          </div>
        </section>
      )}

      <section className="card" aria-labelledby="set-about">
        <h2 id="set-about">{t("settings.about")}</h2>
        <dl className="kv">
          <dt>{t("settings.uiVersion")}</dt><dd className="mono">{UI_VERSION}</dd>
          <dt>{t("settings.daemonVersion")}</dt><dd className="mono">{overview?.version ?? (conn === "open" ? "—" : t("conn.offline"))}</dd>
          <dt>{t("settings.os")}</dt><dd className="mono">{overview ? `${overview.os} ${overview.arch}` : "—"}</dd>
          <dt>{t("settings.home")}</dt><dd className="mono">{overview?.home ?? "—"}</dd>
          <dt>License</dt><dd>MIT OR Apache-2.0</dd>
        </dl>
      </section>
    </div>
  );
}
