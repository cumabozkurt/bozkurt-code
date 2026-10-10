import { useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { AlertTriangle, CheckCircle2, Copy, Download, FileText, Layers, RefreshCw, Stethoscope, Wrench, XCircle } from "lucide-react";
import type { DoctorCheck, RuntimeKind, ServiceAction, ServiceName } from "../api/types";
import { RUNTIME_KINDS } from "../api/types";
import { Button, EmptyState, ErrorState, Loading } from "../components/ui";
import { useI18n, useT } from "../i18n";
import { localizeCheck } from "../lib/daemonText";
import { SERVICE_LABEL } from "../lib/labels";
import { copyText } from "../lib/platform";
import { errorMessage, useStore } from "../store";

/** What a DoctorCheck.fix can do from the UI. */
export type FixPlan =
  | { kind: "dns" }
  | { kind: "tls" }
  | { kind: "service"; name: ServiceName; action: ServiceAction }
  | { kind: "installService"; name: ServiceName }
  | { kind: "runtime"; runtime?: RuntimeKind }
  | { kind: "logs"; name: ServiceName }
  | { kind: "daemon"; id: string }
  | { kind: "copy"; text: string };

/**
 * Maps a fix hint to an action. bozkurtd sends machine ids for `dns.configure`, `tls.trust` and
 * `services.<name>.<start|stop|restart>`; other hints are CLI text. Known CLI hints are mapped
 * client-side to the equivalent API call; anything else is only offered as "Copy command".
 */
export function planFix(fix: string): FixPlan {
  const f = fix.trim();
  if (f === "dns.configure" || f === "bz dns configure") return { kind: "dns" };
  if (f === "tls.trust" || f === "bz tls trust") return { kind: "tls" };
  let m = /^services\.([\w.-]+)\.(start|stop|restart)$/.exec(f);
  if (m) return { kind: "service", name: m[1] as ServiceName, action: m[2] as ServiceAction };
  m = /^bz service (start|stop|restart) ([\w.-]+)$/.exec(f);
  if (m) return { kind: "service", name: m[2] as ServiceName, action: m[1] as ServiceAction };
  m = /^(?:services\.([\w.-]+)\.install|bz service install ([\w.-]+))$/.exec(f);
  if (m) return { kind: "installService", name: (m[1] ?? m[2]) as ServiceName };
  m = /^(?:runtimes\.(\w+)\.install|bz runtime install (\w+)\b.*)$/.exec(f);
  if (m) {
    const k = (m[1] ?? m[2]) as RuntimeKind;
    return { kind: "runtime", runtime: RUNTIME_KINDS.includes(k) ? k : undefined };
  }
  m = /^bz service logs ([\w.-]+)$/.exec(f);
  if (m) return { kind: "logs", name: m[1] as ServiceName };
  if (/^[a-z_]+(\.[\w-]+)+$/i.test(f)) return { kind: "daemon", id: f };
  return { kind: "copy", text: f };
}

export function useFixer(after?: () => unknown) {
  const t = useT();
  const navigate = useNavigate();
  const api = useStore((s) => s.api);
  const st = useStore.getState;
  return useCallback(
    async (c: DoctorCheck) => {
      if (!api || !c.fix) return;
      const p = planFix(c.fix);
      switch (p.kind) {
        case "dns":
          await st().configureDnsVerified();
          break;
        case "tls":
          await st().trustCaVerified();
          break;
        case "service":
          await st().serviceAction(p.name, p.action, t("doctor.fixStarted"));
          break;
        case "installService":
          await st().trackJob(api.installService(p.name), `Install ${p.name}`);
          break;
        case "daemon":
          await st().trackJob(api.doctorFix(p.id), c.title);
          break;
        case "runtime":
          navigate(`/runtimes?kind=${p.runtime ?? "php"}`);
          return;
        case "logs":
          navigate("/services");
          return;
        case "copy":
          await copyText(p.text);
          st().toast(t("doctor.copied"), "info");
          return;
      }
      await after?.();
    },
    [api, st, t, navigate, after],
  );
}

function FixButton({ c, onFix }: { c: DoctorCheck; onFix: (c: DoctorCheck) => Promise<void> | void }) {
  const t = useT();
  const [busy, setBusy] = useState(false);
  const p = planFix(c.fix!);
  const go = async () => {
    setBusy(true);
    try {
      await onFix(c);
    } finally {
      setBusy(false);
    }
  };
  const label =
    p.kind === "copy" ? t("doctor.copyCommand")
    : p.kind === "runtime" ? t("doctor.openRuntimes")
    : p.kind === "logs" ? t("doctor.showLogs")
    : p.kind === "installService" ? t("doctor.installService", { name: SERVICE_LABEL(p.name) })
    : t("common.fix");
  const icon = p.kind === "copy" ? <Copy /> : p.kind === "runtime" ? <Layers /> : p.kind === "logs" ? <FileText /> : p.kind === "installService" ? <Download /> : <Wrench />;
  const primary = p.kind !== "copy" && p.kind !== "logs";
  return (
    <Button size="sm" variant={primary ? "primary" : "default"} icon={icon} loading={busy} onClick={go} aria-label={`${label}: ${c.title}`}>
      {label}
    </Button>
  );
}

export function CheckRow({ c, onFix }: { c: DoctorCheck; onFix?: (c: DoctorCheck) => Promise<void> | void }) {
  const { t, lang } = useI18n();
  const { title, detail } = localizeCheck(c, lang);
  const plan = c.fix ? planFix(c.fix) : null;
  return (
    <div className={`check ${c.status}`}>
      {c.status === "ok" ? <CheckCircle2 aria-label={t("doctor.ok")} /> : c.status === "warn" ? <AlertTriangle aria-label={t("doctor.warn")} /> : <XCircle aria-label={t("doctor.fail")} />}
      <div className="grow col" style={{ gap: 2 }}>
        <strong>{title}</strong>
        <span className="muted" style={{ fontSize: 13 }}>{detail}</span>
        {plan?.kind === "copy" && c.status !== "ok" && <code className="fix-text">{plan.text}</code>}
      </div>
      {c.fix && c.status !== "ok" && onFix && <FixButton c={{ ...c, title }} onFix={onFix} />}
    </div>
  );
}

export function DoctorScreen() {
  const t = useT();
  const api = useStore((s) => s.api);
  const conn = useStore((s) => s.conn);
  const [checks, setChecks] = useState<DoctorCheck[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const load = useCallback(async () => {
    if (!api) return;
    setLoading(true);
    try {
      setChecks(await api.doctor());
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }, [api]);
  useEffect(() => {
    void load();
  }, [load]);
  // Re-run once the daemon comes back.
  useEffect(() => {
    if (conn === "open" && error) void load();
  }, [conn]); // eslint-disable-line react-hooks/exhaustive-deps
  const fix = useFixer(load);
  const count = (s: DoctorCheck["status"]) => checks?.filter((c) => c.status === s).length ?? 0;
  const order = { fail: 0, warn: 1, ok: 2 } as const;

  return (
    <div className="page">
      <div className="page-head">
        <div className="titles">
          <h1>{t("doctor.title")}</h1>
          <span className="muted">{t("doctor.subtitle")}</span>
        </div>
        <Button icon={<RefreshCw />} loading={loading} onClick={load}>{t("doctor.run")}</Button>
      </div>
      <section className="card" aria-busy={loading}>
        {error && !checks ? (
          <ErrorState offline={conn !== "open" && api?.mode !== "mock"} message={error} onRetry={load} />
        ) : checks === null ? (
          <Loading />
        ) : checks.length === 0 ? (
          <EmptyState icon={<Stethoscope />} title={t("doctor.noChecks")} />
        ) : (
          <>
            {error && <p className="warn-text" role="alert">{t("state.staleResults", { msg: error })}</p>}
            <div className="row wrap" aria-label={t("doctor.summary", { ok: count("ok"), warn: count("warn"), fail: count("fail") })}>
              <span className="badge ok">{count("ok")} {t("doctor.ok")}</span>
              <span className="badge warn">{count("warn")} {t("doctor.warn")}</span>
              <span className="badge danger">{count("fail")} {t("doctor.fail")}</span>
            </div>
            <div className="col" style={{ gap: 0 }}>
              {[...checks].sort((a, b) => order[a.status] - order[b.status]).map((c) => <CheckRow key={c.id} c={c} onFix={fix} />)}
            </div>
          </>
        )}
      </section>
    </div>
  );
}
