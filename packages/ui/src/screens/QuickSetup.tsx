import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { AlertTriangle, CheckCircle2, PartyPopper } from "lucide-react";
import type { DnsInfo, RuntimeKind, ServiceName, TlsInfo, Version } from "../api/types";
import { Button, Dialog, ErrorState, Loading } from "../components/ui";
import { useT } from "../i18n";
import { RUNTIME_LABEL, SERVICE_LABEL } from "../lib/labels";
import { errorMessage, useStore } from "../store";

type Step = "php" | "node" | "trust" | "dns" | "services" | "done";
const STEPS: Step[] = ["php", "node", "trust", "dns", "services", "done"];

function RuntimeStep({ kind, onDone }: { kind: RuntimeKind; onDone: () => void }) {
  const t = useT();
  const api = useStore((s) => s.api);
  const installRuntime = useStore((s) => s.installRuntime);
  const run = useStore((s) => s.run);
  const [versions, setVersions] = useState<Version[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pick, setPick] = useState("");
  const [busy, setBusy] = useState(false);
  const load = () => {
    setError(null);
    setVersions(null);
    api
      ?.runtimeVersions(kind)
      .then((vs) => {
        setVersions(vs);
        const rec = vs.find((v) => v.active) ?? vs.find((v) => v.channel === (kind === "node" ? "lts" : "stable")) ?? vs[0];
        setPick(rec?.version ?? "");
      })
      .catch((e) => {
        setVersions([]);
        setError(errorMessage(e));
      });
  };
  useEffect(load, [api, kind]); // eslint-disable-line react-hooks/exhaustive-deps
  const sel = versions?.find((v) => v.version === pick);
  const go = async () => {
    if (!api || !sel) return;
    setBusy(true);
    if (!sel.installed) {
      const job = await installRuntime(kind, sel.version);
      if (job?.status !== "done") return setBusy(false);
    }
    const r = sel.active ? true : await run(() => api.useRuntime(kind, sel.version));
    if (r && r !== true) useStore.getState().applyRuntime(r);
    setBusy(false);
    if (r) onDone();
  };
  return (
    <>
      <p className="muted">{t(kind === "php" ? "wizard.phpDesc" : "wizard.nodeDesc")}</p>
      <div className="choice-list" role="radiogroup" aria-label={RUNTIME_LABEL[kind]}>
        {versions === null && <Loading />}
        {error && <ErrorState message={error} onRetry={load} />}
        {versions
          ?.filter((v) => v.channel !== "eol" || v.installed)
          .map((v) => (
            <label key={v.version} className="choice">
              <input type="radio" name={`rt-${kind}`} checked={pick === v.version} onChange={() => setPick(v.version)} />
              <span className="mono grow" style={{ fontWeight: 650 }}>{v.version}</span>
              <span className={`badge ${v.channel === "lts" ? "accent" : ""}`}>{t(`runtimes.channel.${v.channel}`)}</span>
              {v.installed && <span className="badge ok">{v.active ? t("common.active") : t("common.installed")}</span>}
            </label>
          ))}
      </div>
      <div className="row">
        <span className="spacer" />
        <Button onClick={onDone}>{t("common.skip")}</Button>
        <Button variant="primary" loading={busy} disabled={!sel} onClick={go}>
          {sel?.installed ? t("wizard.useInstalled", { v: sel.version }) : t("wizard.installAndUse", { v: sel?.version ?? "" })}
        </Button>
      </div>
    </>
  );
}

interface Pending {
  key: string;
  text: string;
  action: () => void;
  actionLabel: string;
}

/** The last step: honest about what is still missing (re-checked from the daemon, not assumed). */
function DoneStep({ onClose, goto }: { onClose: () => void; goto: (s: Step) => void }) {
  const t = useT();
  const nav = useNavigate();
  const api = useStore((s) => s.api);
  const runtimes = useStore((s) => s.runtimes);
  const services = useStore((s) => s.services);
  const tld = useStore((s) => s.overview?.tld ?? "test");
  const [tls, setTls] = useState<TlsInfo | null | undefined>(undefined);
  const [dns, setDns] = useState<DnsInfo | null | undefined>(undefined);
  useEffect(() => {
    void useStore.getState().refreshOverview();
    api?.tls().then(setTls).catch(() => setTls(null));
    api?.dns().then(setDns).catch(() => setDns(null));
  }, [api]);
  if (tls === undefined || dns === undefined) return <Loading />;
  const pending: Pending[] = [];
  const active = (k: RuntimeKind) => runtimes.find((r) => r.kind === k)?.active;
  if (!active("php") && !active("node"))
    pending.push({ key: "rt", text: t("wizard.pendingRuntime"), actionLabel: t("wizard.php"), action: () => goto("php") });
  if (!tls?.ca_trusted) pending.push({ key: "tls", text: t("wizard.pendingTrust"), actionLabel: t("wizard.trustNow"), action: () => goto("trust") });
  if (!dns?.configured) pending.push({ key: "dns", text: t("wizard.pendingDns", { tld }), actionLabel: t("wizard.configureDns"), action: () => goto("dns") });
  const caddy = services.find((s) => s.name === "caddy");
  if (!caddy || caddy.status !== "running")
    pending.push({
      key: "caddy",
      text: caddy?.status === "not_installed" ? t("wizard.pendingCaddyInstall") : t("wizard.pendingCaddyStart"),
      actionLabel: t("wizard.services"),
      action: () => goto("services"),
    });
  const ok = pending.length === 0;
  return (
    <div className="empty" style={{ padding: "16px 0" }}>
      <div className={`empty-icon ${ok ? "" : "warn"}`} aria-hidden>{ok ? <PartyPopper /> : <AlertTriangle />}</div>
      <h2>{ok ? t("wizard.done") : t("wizard.almostDone")}</h2>
      {ok ? (
        <p>{t("wizard.doneDesc", { tld })}</p>
      ) : (
        <div className="col" style={{ alignSelf: "stretch", textAlign: "left" }}>
          <p>{t("wizard.pendingIntro")}</p>
          <ul className="pending-list">
            {pending.map((p) => (
              <li key={p.key}>
                <AlertTriangle size={14} aria-hidden />
                <span className="grow">{p.text}</span>
                <Button size="sm" onClick={p.action}>{p.actionLabel}</Button>
              </li>
            ))}
          </ul>
        </div>
      )}
      <div className="row">
        <Button onClick={onClose}>{t("common.close")}</Button>
        <Button variant="primary" onClick={() => { onClose(); nav("/sites"); useStore.getState().set({ siteDialogOpen: true }); }}>{t("sites.add")}</Button>
      </div>
    </div>
  );
}

export function QuickSetupDialog() {
  const t = useT();
  const api = useStore((s) => s.api);
  const overview = useStore((s) => s.overview);
  const services = useStore((s) => s.services);
  const trackJob = useStore((s) => s.trackJob);
  const serviceAction = useStore((s) => s.serviceAction);
  const close = () => useStore.getState().set({ wizardOpen: false });
  const [step, setStep] = useState<Step>("php");
  const [busy, setBusy] = useState(false);
  const [stepError, setStepError] = useState<string | null>(null);
  const tld = overview?.tld ?? "test";
  const next = () => {
    setStepError(null);
    setStep(STEPS[Math.min(STEPS.length - 1, STEPS.indexOf(step) + 1)]);
  };
  const goto = (s: Step) => {
    setStepError(null);
    setStep(s);
  };
  const activePhp = overview?.runtimes.find((r) => r.kind === "php")?.active?.split(".").slice(0, 2).join(".");
  // Caddy is offered even when missing ("Install & start"): without it no site is served.
  const listed = services.filter((s) => s.status !== "external" && (s.status !== "not_installed" || s.name === "caddy"));
  const [chosen, setChosen] = useState<Set<ServiceName>>(
    () => new Set(listed.filter((s) => s.autostart || ["caddy", "dns", "mariadb", `php-fpm-${activePhp}`].includes(s.name)).map((s) => s.name)),
  );

  const verifiedStep = async (fn: () => Promise<"ok" | "failed" | "ineffective">, failKey: "wizard.trustIneffective" | "wizard.dnsIneffective") => {
    setBusy(true);
    setStepError(null);
    const r = await fn();
    setBusy(false);
    if (r === "ok") next();
    else if (r === "ineffective") setStepError(t(failKey, { tld }));
  };
  const startServices = async () => {
    if (!api) return;
    setBusy(true);
    const failed: string[] = [];
    for (const name of chosen) {
      const s = useStore.getState().services.find((x) => x.name === name);
      if (s?.status === "not_installed") {
        const job = await trackJob(api.installService(name), `Install ${name}`);
        if (job?.status !== "done") {
          failed.push(SERVICE_LABEL(name));
          continue;
        }
      }
      const cur = useStore.getState().services.find((x) => x.name === name);
      if (cur && cur.status !== "running") {
        const r = await serviceAction(name, "start");
        if (!r) failed.push(SERVICE_LABEL(name));
      }
    }
    setBusy(false);
    if (failed.length) setStepError(t("wizard.servicesFailed", { names: failed.join(", ") }));
    else next();
  };

  const idx = STEPS.indexOf(step);
  const titles: Record<Step, string> = {
    php: t("wizard.php"), node: t("wizard.node"), trust: t("wizard.trust"), dns: t("wizard.dns"), services: t("wizard.services"), done: t("wizard.summary"),
  };

  return (
    <Dialog title={`${t("wizard.title")} · ${titles[step]}`} onClose={close}>
      <div className="wizard-steps" role="img" aria-label={`${idx + 1} / ${STEPS.length}`}>
        {STEPS.map((s, i) => <span key={s} className={i <= idx ? "done" : ""} />)}
      </div>
      {step === "php" && <RuntimeStep key="php" kind="php" onDone={next} />}
      {step === "node" && <RuntimeStep key="node" kind="node" onDone={next} />}
      {step === "trust" && (
        <>
          <p className="muted">{t("wizard.trustDesc", { tld })}</p>
          {stepError && <p className="callout warn" role="alert"><AlertTriangle size={16} aria-hidden />{stepError}</p>}
          <div className="row">
            {overview?.ca_trusted && <span className="badge ok"><CheckCircle2 size={12} />{t("wizard.alreadyDone")}</span>}
            <span className="spacer" />
            <Button onClick={next}>{overview?.ca_trusted ? t("common.next") : t("common.skip")}</Button>
            {!overview?.ca_trusted && <Button variant="primary" loading={busy} onClick={() => verifiedStep(() => useStore.getState().trustCaVerified(), "wizard.trustIneffective")}>{stepError ? t("common.retry") : t("wizard.trustNow")}</Button>}
          </div>
        </>
      )}
      {step === "dns" && (
        <>
          <p className="muted">{t("wizard.dnsDesc", { tld })}</p>
          {stepError && <p className="callout warn" role="alert"><AlertTriangle size={16} aria-hidden />{stepError}</p>}
          <div className="row">
            {overview?.dns_configured && <span className="badge ok"><CheckCircle2 size={12} />{t("wizard.alreadyDone")}</span>}
            <span className="spacer" />
            <Button onClick={next}>{overview?.dns_configured ? t("common.next") : t("common.skip")}</Button>
            {!overview?.dns_configured && <Button variant="primary" loading={busy} onClick={() => verifiedStep(() => useStore.getState().configureDnsVerified(), "wizard.dnsIneffective")}>{stepError ? t("common.retry") : t("wizard.configureDns")}</Button>}
          </div>
        </>
      )}
      {step === "services" && (
        <>
          <p className="muted">{t("wizard.servicesDesc")}</p>
          {stepError && <p className="callout warn" role="alert"><AlertTriangle size={16} aria-hidden />{stepError}</p>}
          <div className="choice-list">
            {listed.length === 0 && <span className="muted">{t("services.empty")}</span>}
            {listed.map((s) => (
              <label key={s.name} className="choice">
                <input
                  type="checkbox"
                  checked={chosen.has(s.name)}
                  onChange={(e) => {
                    const n = new Set(chosen);
                    if (e.target.checked) n.add(s.name);
                    else n.delete(s.name);
                    setChosen(n);
                  }}
                />
                <span className={`status-dot ${s.status}`} aria-hidden />
                <span className="grow">{SERVICE_LABEL(s.name)}</span>
                <span className="faint" style={{ fontSize: 12 }}>
                  {s.status === "not_installed" ? t("wizard.willInstall") : t(`status.${s.status}`)}
                </span>
              </label>
            ))}
          </div>
          <div className="row">
            <span className="spacer" />
            <Button onClick={next}>{t("common.skip")}</Button>
            <Button variant="primary" loading={busy} disabled={!chosen.size} onClick={startServices}>
              {[...chosen].some((n) => services.find((s) => s.name === n)?.status === "not_installed") ? t("wizard.installAndStart") : t("wizard.startServices")}
            </Button>
          </div>
        </>
      )}
      {step === "done" && <DoneStep onClose={close} goto={goto} />}
    </Dialog>
  );
}
