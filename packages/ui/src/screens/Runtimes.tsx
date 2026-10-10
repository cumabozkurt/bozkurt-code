import { useCallback, useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { CheckCircle2, Download, Layers, RefreshCw, Trash2 } from "lucide-react";
import type { RuntimeKind, Version } from "../api/types";
import { RUNTIME_KINDS } from "../api/types";
import { Button, EmptyState, ErrorState, Loading, Progress, Segmented, Tabs, confirmAction, formatBytes, isIndeterminatePhase } from "../components/ui";
import { useI18n } from "../i18n";
import { localizeJobMessage } from "../lib/daemonText";
import { RUNTIME_LABEL } from "../lib/labels";
import { errorMessage, useStore } from "../store";

type Filter = "all" | "lts" | "current" | "installed";

export function RuntimesScreen() {
  const { t, lang, fmtDate } = useI18n();
  const api = useStore((s) => s.api);
  const run = useStore((s) => s.run);
  const installRuntime = useStore((s) => s.installRuntime);
  const jobs = useStore((s) => s.jobs);
  const runtimes = useStore((s) => s.runtimes);
  const conn = useStore((s) => s.conn);
  const mock = useStore((s) => s.api?.mode === "mock");
  const [error, setError] = useState<string | null>(null);
  const [params, setParams] = useSearchParams();
  const kind = (RUNTIME_KINDS.includes(params.get("kind") as RuntimeKind) ? params.get("kind") : "php") as RuntimeKind;
  const [filter, setFilter] = useState<Filter>("all");
  const [versions, setVersions] = useState<Version[] | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [installing, setInstalling] = useState<Record<string, string>>({}); // "kind@version" -> job id

  const load = useCallback(async (refresh = false) => {
    if (!api) return;
    if (refresh) setRefreshing(true);
    try {
      setVersions(await api.runtimeVersions(kind, refresh));
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
      setVersions((v) => v ?? []);
    } finally {
      setRefreshing(false);
    }
  }, [api, kind]);

  useEffect(() => {
    setVersions(null);
  }, [kind]);
  // (Re)load on kind change and whenever the runtime changes elsewhere (palette, tray, CLI).
  const rt = runtimes.find((r) => r.kind === kind);
  const rtKey = `${rt?.active}|${rt?.installed.join(",")}`;
  useEffect(() => {
    void load();
  }, [rtKey, load]);
  useEffect(() => {
    if (conn === "open" && error) void load();
  }, [conn]); // eslint-disable-line react-hooks/exhaustive-deps

  const shown = useMemo(
    () =>
      (versions ?? []).filter((v) =>
        filter === "all" ? true : filter === "installed" ? v.installed : filter === "lts" ? v.channel === "lts" || v.channel === "stable" : v.channel === "current",
      ),
    [versions, filter],
  );

  const install = async (v: Version) => {
    const key = `${kind}@${v.version}`;
    await installRuntime(kind, v.version, { onJob: (id) => setInstalling((s) => ({ ...s, [key]: id })) });
    setInstalling((s) => {
      const n = { ...s };
      delete n[key];
      return n;
    });
    void load();
  };
  const use = async (v: Version) => {
    setBusy(v.version);
    const rt = await run(() => api!.useRuntime(kind, v.version), t("runtimes.activeNow", { kind: RUNTIME_LABEL[kind], version: v.version }));
    if (rt) useStore.getState().applyRuntime(rt);
    setBusy(null);
  };
  const uninstall = async (v: Version) => {
    if (!(await confirmAction(t("runtimes.uninstallConfirm", { kind: RUNTIME_LABEL[kind], version: v.version }), t("common.uninstall")))) return;
    setBusy(v.version);
    await run(() => api!.uninstallRuntime(kind, v.version));
    setBusy(null);
    void load();
  };

  return (
    <div className="page">
      <div className="page-head">
        <div className="titles">
          <h1>{t("runtimes.title")}</h1>
          <span className="muted">{rt?.active ? t("runtimes.activeLabel", { v: `${RUNTIME_LABEL[kind]} ${rt.active}` }) : t("runtimes.noneActive")}</span>
        </div>
        <Button icon={<RefreshCw />} loading={refreshing} onClick={() => load(true)}>{t("runtimes.refreshIndex")}</Button>
      </div>
      <Tabs
        label={t("runtimes.title")}
        value={kind}
        onChange={(k) => setParams({ kind: k }, { replace: true })}
        items={RUNTIME_KINDS.map((k) => ({ id: k, label: RUNTIME_LABEL[k] }))}
      />
      <div className="row wrap">
        <Segmented<Filter>
          label={t("common.all")}
          value={filter}
          onChange={setFilter}
          items={[
            { id: "all", label: t("runtimes.filterAll") },
            { id: "lts", label: kind === "node" ? t("runtimes.filterLts") : t("runtimes.channel.stable") },
            { id: "current", label: t("runtimes.filterCurrent") },
            { id: "installed", label: t("runtimes.filterInstalled") },
          ]}
        />
      </div>
      <section className="card table-wrap" aria-live="polite" aria-busy={versions === null}>
        {versions === null ? (
          <div style={{ padding: 16 }}><Loading /></div>
        ) : error && versions.length === 0 ? (
          <ErrorState offline={conn !== "open" && !mock} message={error} onRetry={() => load()} />
        ) : shown.length === 0 ? (
          <EmptyState icon={<Layers />} title={t("runtimes.empty")} />
        ) : (
          shown.map((v) => {
            const jobId = installing[`${kind}@${v.version}`];
            const job = jobId ? jobs[jobId] : undefined;
            return (
              <div key={v.version} className="version-row">
                <span className="v">{v.version}</span>
                <span className={`badge ${v.channel === "lts" ? "accent" : v.channel === "current" ? "ok" : v.channel === "eol" ? "danger" : ""}`}>{t(`runtimes.channel.${v.channel}`)}</span>
                {v.active && <span className="badge ok"><CheckCircle2 size={12} />{t("common.active")}</span>}
                {v.installed && !v.active && <span className="badge">{t("common.installed")}</span>}
                <span className="faint hide-mobile" style={{ fontSize: 12 }}>
                  {v.released ? t("runtimes.released", { date: fmtDate(v.released) }) : ""}
                  {v.size_bytes ? ` · ${formatBytes(v.size_bytes)}` : ""}
                </span>
                <span className="spacer" />
                {jobId ? (
                  <div className="col" style={{ width: 220, gap: 4 }}>
                    <span className="faint" style={{ fontSize: 12 }}>
                      {localizeJobMessage(job?.message, lang) ?? t("runtimes.installing")}
                      {isIndeterminatePhase(job?.message) ? "…" : ` · ${Math.round((job?.progress ?? 0) * 100)}%`}
                    </span>
                    <Progress value={job?.progress ?? 0} label={`${RUNTIME_LABEL[kind]} ${v.version}`} indeterminate={isIndeterminatePhase(job?.message)} />
                  </div>
                ) : v.installed ? (
                  <>
                    {!v.active && <Button size="sm" variant="primary" loading={busy === v.version} onClick={() => use(v)}>{t("common.use")}</Button>}
                    <Button size="sm" variant="ghost" iconOnly icon={<Trash2 />} disabled={v.active} aria-label={`${t("common.uninstall")} ${v.version}`} title={t("common.uninstall")} onClick={() => uninstall(v)} />
                  </>
                ) : (
                  <Button size="sm" icon={<Download />} onClick={() => install(v)} aria-label={`${t("common.install")} ${RUNTIME_LABEL[kind]} ${v.version}`}>{t("common.install")}</Button>
                )}
              </div>
            );
          })
        )}
      </section>
    </div>
  );
}
