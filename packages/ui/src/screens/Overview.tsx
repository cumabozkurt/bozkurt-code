import { useEffect, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { AlertTriangle, ArrowRight, CheckCircle2, Globe, Lock, Play, Rocket, Square, Wand2, X } from "lucide-react";
import type { DoctorCheck, Service } from "../api/types";
import { RUNTIME_KINDS } from "../api/types";
import { Button, ErrorState, Loading, formatDuration } from "../components/ui";
import { useI18n } from "../i18n";
import { RUNTIME_LABEL, SERVICE_LABEL } from "../lib/labels";
import { isValidPort } from "../lib/urls";
import { errorMessage, useDataState, useStore } from "../store";
import { CheckRow, useFixer } from "./Doctor";
import { useUptime } from "./Services";

const SETUP_DISMISS_KEY = "bozkurt.setupBannerDismissed";

function SvcTile({ s }: { s: Service }) {
  const { t } = useI18n();
  const action = useStore((st) => st.serviceAction);
  const uptime = useUptime(s);
  const [busy, setBusy] = useState(false);
  const up = s.status === "running" || s.status === "starting";
  return (
    <div className="svc-tile">
      <span className={`status-dot ${s.status}`} aria-hidden />
      <div className="grow col" style={{ gap: 0, minWidth: 0 }}>
        <span className="name ellipsis">{SERVICE_LABEL(s.name)}</span>
        <span className="faint ellipsis" style={{ fontSize: 12 }}>
          {t(`status.${s.status}`)}
          {s.port ? ` · :${s.port}` : ""}
          {uptime !== undefined ? ` · ${formatDuration(uptime)}` : ""}
        </span>
      </div>
      {s.status !== "not_installed" && s.status !== "external" && (
        <Button
          size="sm"
          iconOnly
          variant="ghost"
          icon={up ? <Square /> : <Play />}
          loading={busy}
          aria-label={`${up ? t("common.stop") : t("common.start")} ${SERVICE_LABEL(s.name)}`}
          title={up ? t("common.stop") : t("common.start")}
          disabled={s.status === "starting" || s.status === "stopping"}
          onClick={async () => {
            setBusy(true);
            await action(s.name, up ? "stop" : "start");
            setBusy(false);
          }}
        />
      )}
    </div>
  );
}

export function OverviewScreen() {
  const { t } = useI18n();
  const nav = useNavigate();
  const overview = useStore((s) => s.overview);
  const services = useStore((s) => s.services);
  const runtimes = useStore((s) => s.runtimes);
  const sites = useStore((s) => s.sites);
  const api = useStore((s) => s.api);
  const allServices = useStore((s) => s.allServicesAction);
  const ds = useDataState();
  const openWizard = () => useStore.getState().set({ wizardOpen: true });
  const [checks, setChecks] = useState<DoctorCheck[] | null>(null);
  const [checksError, setChecksError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [dismissed, setDismissed] = useState(() => {
    try {
      return localStorage.getItem(SETUP_DISMISS_KEY) === "1";
    } catch {
      return false;
    }
  });
  const loadChecks = () =>
    api?.doctor().then((c) => {
      setChecks(c);
      setChecksError(null);
    }).catch((e) => setChecksError(errorMessage(e)));
  const fix = useFixer(loadChecks);

  useEffect(() => {
    void loadChecks();
  }, [api, overview?.ca_trusted, overview?.dns_configured, ds.offline]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!overview) {
    return (
      <div className="page">
        <div className="page-head"><div className="titles"><h1>{t("overview.title")}</h1></div></div>
        <div className="card">
          {!ds.loaded ? <Loading /> : <ErrorState offline={ds.offline} message={ds.offline ? undefined : ds.loadError} onRetry={ds.retry} />}
        </div>
      </div>
    );
  }
  const problems = (checks ?? []).filter((c) => c.status !== "ok");
  // Setup is "done" with a trusted CA, routed DNS and any active runtime (Node-only users included).
  const needsSetup = !overview.ca_trusted || !overview.dns_configured || !runtimes.some((r) => !!r.active);
  const siteCount = sites.length || overview.sites_count;
  const badPorts = !isValidPort(overview.http_port) || !isValidPort(overview.https_port);
  const dismiss = () => {
    try {
      localStorage.setItem(SETUP_DISMISS_KEY, "1");
    } catch {
      /* ignore */
    }
    setDismissed(true);
  };

  const allAction = async (a: "start" | "stop") => {
    setBusy(a);
    await allServices(a);
    setBusy(null);
  };

  return (
    <div className="page">
      <div className="page-head">
        <div className="titles">
          <h1>{t("overview.title")}</h1>
          <span className="muted mono ellipsis">{t("overview.subtitle", { os: overview.os, arch: overview.arch, home: overview.home })}</span>
        </div>
        <Button icon={<Wand2 />} onClick={openWizard}>{t("overview.quickSetup")}</Button>
      </div>

      {needsSetup && !dismissed && (
        <div className="card" style={{ background: "linear-gradient(120deg, var(--accent-soft), var(--bg-elev) 70%)" }}>
          <div className="row wrap" style={{ gap: 16 }}>
            <Rocket size={28} color="var(--accent-fg)" aria-hidden />
            <div className="grow col" style={{ gap: 2, minWidth: 220 }}>
              <h2>{t("overview.quickSetup")}</h2>
              <p className="muted">{t("overview.quickSetupDesc")}</p>
            </div>
            <Button variant="primary" icon={<ArrowRight />} onClick={openWizard}>{t("overview.runSetup")}</Button>
            <Button variant="ghost" iconOnly icon={<X />} aria-label={t("overview.dismissSetup")} title={t("overview.dismissSetup")} onClick={dismiss} />
          </div>
        </div>
      )}

      <section aria-labelledby="ov-rt">
        <div className="row" style={{ marginBottom: 10 }}>
          <h2 id="ov-rt" className="grow">{t("overview.runtimes")}</h2>
          <Link to="/runtimes">{t("overview.viewAll")}</Link>
        </div>
        <div className="grid cols-5">
          {RUNTIME_KINDS.map((k) => {
            const r = runtimes.find((x) => x.kind === k);
            return (
              <button key={k} type="button" className="card runtime-card" onClick={() => nav(`/runtimes?kind=${k}`)}>
                <span className="kind">{RUNTIME_LABEL[k]}</span>
                <span className={`ver ${r?.active ? "" : "none"}`}>{r?.active ?? t("overview.noActive")}</span>
                <span className="faint" style={{ fontSize: 12 }}>
                  {r?.installed.length ? `${r.installed.length} ${t("common.installed").toLowerCase()}` : "—"}
                </span>
              </button>
            );
          })}
        </div>
      </section>

      <div className="grid cols-2">
        <section className="card" aria-labelledby="ov-svc">
          <div className="card-head">
            <h2 id="ov-svc">{t("overview.services")}</h2>
            <Button size="sm" icon={<Play />} loading={busy === "start"} onClick={() => allAction("start")}>{t("common.startAll")}</Button>
            <Button size="sm" icon={<Square />} loading={busy === "stop"} onClick={() => allAction("stop")}>{t("common.stopAll")}</Button>
          </div>
          <div className="grid" style={{ gridTemplateColumns: "repeat(auto-fill, minmax(min(100%, 220px), 1fr))", gap: 8 }}>
            {services.map((s) => <SvcTile key={s.name} s={s} />)}
          </div>
        </section>

        <div className="col" style={{ gap: 16 }}>
          <section className="card" aria-labelledby="ov-sites">
            <div className="card-head">
              <h2 id="ov-sites">{t("overview.sites")}</h2>
              <Link to="/sites">{t("overview.viewAll")}</Link>
            </div>
            <div className="row" style={{ gap: 14 }}>
              <span style={{ fontSize: 36, fontWeight: 750, lineHeight: 1 }}>{siteCount}</span>
              <span className="muted">{t("overview.sitesCount", { n: siteCount, tld: overview.tld })}</span>
            </div>
            <div className="row wrap" style={{ gap: 8 }}>
              <span className={`badge ${overview.ca_trusted ? "ok" : "warn"}`}><Lock size={12} />{t("overview.ca")}: {overview.ca_trusted ? t("overview.trusted") : t("overview.notTrusted")}</span>
              <span className={`badge ${overview.dns_configured ? "ok" : "warn"}`}><Globe size={12} />{t("overview.dns")}: {overview.dns_configured ? t("overview.configured") : t("overview.notConfigured")}</span>
              <span className={`badge mono ${badPorts ? "danger" : ""}`} title={badPorts ? t("overview.badPorts") : undefined}>
                {badPorts && <AlertTriangle size={12} aria-hidden />}
                {t("overview.ports", { http: overview.http_port, https: overview.https_port })}
              </span>
            </div>
          </section>

          <section className="card" aria-labelledby="ov-health">
            <div className="card-head">
              <h2 id="ov-health">{t("overview.health")}</h2>
              <Link to="/doctor">{t("nav.doctor")}</Link>
            </div>
            {checksError && checks === null ? (
              <span className="warn-text">{checksError}</span>
            ) : checks === null ? (
              <Loading />
            ) : problems.length === 0 ? (
              <div className="row" style={{ color: "var(--ok)" }}><CheckCircle2 size={18} /> {t("overview.allGood")}</div>
            ) : (
              <div className="col" style={{ gap: 0 }}>
                {problems.map((c) => <CheckRow key={c.id} c={c} onFix={fix} />)}
              </div>
            )}
          </section>
        </div>
      </div>
    </div>
  );
}
