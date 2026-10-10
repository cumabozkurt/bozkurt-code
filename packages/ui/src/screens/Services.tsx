import { useEffect, useState } from "react";
import { Download, ExternalLink, FileText, Play, RotateCw, Server, Square } from "lucide-react";
import type { PortOwner, Service, ServiceName } from "../api/types";
import { Button, Drawer, EmptyState, ErrorState, Loading, LogView, StatusBadge, Switch, formatDuration, useNow } from "../components/ui";
import { useT } from "../i18n";
import { SERVICE_LABEL, serviceDescKey, serviceWebUrl } from "../lib/labels";
import { openUrl } from "../lib/platform";
import { errorMessage, useDataState, useStore } from "../store";

/** Uptime that keeps ticking between daemon updates (uptime_secs + time since it was received). */
export function useUptime(s: Service): number | undefined {
  const seen = useStore((st) => st.serviceSeen[s.name]);
  const now = useNow(1000);
  if (s.status !== "running" || s.uptime_secs === undefined) return undefined;
  return s.uptime_secs + Math.max(0, Math.floor((now - (seen ?? now)) / 1000));
}

function ServiceLogs({ name, onClose }: { name: ServiceName; onClose: () => void }) {
  const t = useT();
  const api = useStore((s) => s.api);
  const live = useStore((s) => s.logs);
  const [initial, setInitial] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [follow, setFollow] = useState(true);
  const [since] = useState(() => Date.now());
  const load = () => {
    setError(null);
    api?.serviceLogs(name, 200).then((r) => setInitial(r.lines)).catch((e) => {
      setInitial([]);
      setError(errorMessage(e));
    });
  };
  useEffect(load, [api, name]); // eslint-disable-line react-hooks/exhaustive-deps
  // bozkurtd tags live service lines as `service:<name>` (docs/API.md v1 additions).
  const src = `service:${name}`;
  const lines = [...(initial ?? []).map((line) => ({ line })), ...live.filter((l) => (l.source === src || l.source === name) && l.at >= since)];
  return (
    <Drawer wide title={`${SERVICE_LABEL(name)} · ${t("common.logs")}`} onClose={onClose} actions={<Switch checked={follow} onChange={setFollow} label={t("services.autoScroll")} />}>
      {initial === null ? <Loading /> : error && lines.length === 0 ? <ErrorState message={error} onRetry={load} /> : (
        <LogView label={`${SERVICE_LABEL(name)} ${t("common.logs")}`} lines={lines} follow={follow} tall />
      )}
    </Drawer>
  );
}

function ServiceCard({ s, onLogs }: { s: Service; onLogs: () => void }) {
  const t = useT();
  const api = useStore((st) => st.api);
  const action = useStore((st) => st.serviceAction);
  const patch = useStore((st) => st.patchService);
  const trackJob = useStore((st) => st.trackJob);
  const tld = useStore((st) => st.overview?.tld ?? "test");
  const [owner, setOwner] = useState<PortOwner | null>(null);
  const [installing, setInstalling] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const uptime = useUptime(s);
  useEffect(() => {
    // Only blame another process when the port is really taken by someone else.
    if (s.status === "crashed" && s.port && api) api.port(s.port).then((o) => setOwner(o.in_use !== false && !o.ours ? o : null)).catch(() => {});
    else setOwner(null);
  }, [s.status, s.port, api]);
  const up = s.status === "running" || s.status === "starting";
  const locked = s.status === "starting" || s.status === "stopping" || s.status === "external" || !!busy;
  const web = serviceWebUrl(s.name);
  const act = async (a: "start" | "stop" | "restart") => {
    setBusy(a);
    await action(s.name, a);
    setBusy(null);
  };

  return (
    <section className="card" aria-label={SERVICE_LABEL(s.name)}>
      <div className="card-head">
        <span className={`status-dot ${s.status}`} aria-hidden />
        <h2>{SERVICE_LABEL(s.name)}</h2>
        <StatusBadge status={s.status} />
      </div>
      <p className="muted" style={{ fontSize: 13 }}>{t(serviceDescKey(s.name), { tld })}</p>
      <div className="row wrap" style={{ gap: 6 }}>
        {s.version && <span className="badge mono">v{s.version}</span>}
        {s.port && <span className="badge mono">:{s.port}</span>}
        {s.pid && <span className="badge mono">{t("services.pid", { pid: s.pid })}</span>}
        {uptime !== undefined && <span className="badge">{t("services.uptime", { t: formatDuration(uptime) })}</span>}
      </div>
      {s.message && <p className="mono" style={{ color: s.status === "crashed" ? "var(--danger)" : "var(--text-muted)", fontSize: 12.5 }}>{s.message}</p>}
      {owner && <p style={{ color: "var(--warn)", fontSize: 13 }}>{t("services.portOwner", { port: owner.port, process: `${owner.process ?? "?"}${owner.pid ? ` (pid ${owner.pid})` : ""}` })}</p>}
      {s.status === "not_installed" ? (
        <>
          <p className="faint" style={{ fontSize: 12.5 }}>{t("services.notInstalledHint", { name: SERVICE_LABEL(s.name) })}</p>
          <div className="row">
            <Button variant="primary" icon={<Download />} loading={installing} onClick={async () => {
              setInstalling(true);
              await trackJob(api!.installService(s.name), `${t("common.install")} ${SERVICE_LABEL(s.name)}`);
              setInstalling(false);
            }}>{t("common.install")}</Button>
          </div>
        </>
      ) : (
        <>
          <div className="row wrap">
            {up ? (
              <Button size="sm" icon={<Square />} disabled={locked} loading={busy === "stop"} onClick={() => act("stop")}>{t("common.stop")}</Button>
            ) : (
              <Button size="sm" variant="primary" icon={<Play />} disabled={locked} loading={busy === "start"} onClick={() => act("start")}>{t("common.start")}</Button>
            )}
            <Button size="sm" icon={<RotateCw />} disabled={locked} loading={busy === "restart"} onClick={() => act("restart")}>{t("common.restart")}</Button>
            <Button size="sm" icon={<FileText />} onClick={onLogs}>{t("common.logs")}</Button>
            {web && <Button size="sm" icon={<ExternalLink />} disabled={s.status !== "running" && s.status !== "external"} onClick={() => openUrl(web)} title={web}>{t("services.openWebUi")}</Button>}
          </div>
          <Switch checked={s.autostart} onChange={(v) => void patch(s.name, { autostart: v })} label={t("services.autostart")} hint={t("services.autostartHint")} />
        </>
      )}
    </section>
  );
}

export function ServicesScreen() {
  const t = useT();
  const services = useStore((s) => s.services);
  const allAction = useStore((s) => s.allServicesAction);
  const ds = useDataState();
  const [logs, setLogs] = useState<ServiceName | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const all = async (a: "start" | "stop" | "restart") => {
    setBusy(a);
    await allAction(a);
    setBusy(null);
  };
  const noData = services.length === 0;
  return (
    <div className="page">
      <div className="page-head">
        <div className="titles"><h1>{t("services.title")}</h1></div>
        <Button icon={<Play />} loading={busy === "start"} disabled={noData} onClick={() => all("start")}>{t("common.startAll")}</Button>
        <Button icon={<RotateCw />} loading={busy === "restart"} disabled={noData} onClick={() => all("restart")}>{t("common.restartAll")}</Button>
        <Button icon={<Square />} loading={busy === "stop"} disabled={noData} onClick={() => all("stop")}>{t("common.stopAll")}</Button>
      </div>
      {noData ? (
        <div className="card">
          {!ds.loaded ? <Loading /> : ds.offline || ds.loadError ? <ErrorState offline={ds.offline} message={ds.offline ? undefined : ds.loadError} onRetry={ds.retry} /> : <EmptyState icon={<Server />} title={t("services.title")} body={t("services.empty")} />}
        </div>
      ) : (
        <div className="grid cols-3">
          {services.map((s) => <ServiceCard key={s.name} s={s} onLogs={() => setLogs(s.name)} />)}
        </div>
      )}
      {logs && <ServiceLogs name={logs} onClose={() => setLogs(null)} />}
    </div>
  );
}
