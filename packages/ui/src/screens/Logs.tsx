import { useMemo, useRef, useState } from "react";
import { Pause, Play, ScrollText, Search, Trash2 } from "lucide-react";
import { Button, EmptyState, LogView } from "../components/ui";
import { useT } from "../i18n";
import { useStore, type LogLine } from "../store";

export function LogsScreen() {
  const t = useT();
  const logs = useStore((s) => s.logs);
  const clear = useStore((s) => s.clearLogs);
  const [source, setSource] = useState("");
  const [q, setQ] = useState("");
  const [paused, setPaused] = useState(false);
  const frozen = useRef<LogLine[]>([]);
  if (!paused) frozen.current = logs;
  const src = paused ? frozen.current : logs;
  const sources = useMemo(() => [...new Set(logs.map((l) => l.source))].sort(), [logs]);
  const shown = useMemo(() => {
    const qq = q.toLowerCase();
    return src.filter((l) => (!source || l.source === source) && (!qq || l.line.toLowerCase().includes(qq) || l.source.toLowerCase().includes(qq)));
  }, [src, source, q]);

  return (
    <div className="page">
      <div className="page-head">
        <div className="titles">
          <h1>{t("logs.title")}</h1>
          <span className="muted">{t("logs.lines", { n: shown.length })}</span>
        </div>
      </div>
      <div className="row wrap">
        <select className="input" style={{ width: 200 }} aria-label={t("logs.allSources")} value={source} onChange={(e) => setSource(e.target.value)}>
          <option value="">{t("logs.allSources")}</option>
          {sources.map((s) => <option key={s} value={s}>{s}</option>)}
        </select>
        <div className="row grow" style={{ minWidth: 200 }}>
          <Search size={16} className="faint" aria-hidden />
          <input className="input" type="search" aria-label={t("logs.search")} placeholder={t("logs.search")} value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
        <Button icon={paused ? <Play /> : <Pause />} onClick={() => setPaused(!paused)} aria-pressed={paused}>{paused ? t("logs.resume") : t("logs.pause")}</Button>
        <Button icon={<Trash2 />} onClick={clear}>{t("logs.clear")}</Button>
      </div>
      {logs.length === 0 ? (
        <div className="card"><EmptyState icon={<ScrollText />} title={t("logs.title")} body={t("logs.empty")} /></div>
      ) : (
        <LogView label={t("logs.title")} lines={shown} showSource query={q} tall follow={!paused} />
      )}
    </div>
  );
}
