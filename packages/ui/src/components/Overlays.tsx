import { Fragment, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import {
  ArrowRight, Briefcase, CheckCircle2, Download, ExternalLink, Globe, Keyboard, Languages, Layers, Lock, Moon, Play, Plus, RotateCw, Server, Square, Wand2, XCircle,
} from "lucide-react";
import type { ServiceAction } from "../api/types";
import { RUNTIME_KINDS } from "../api/types";
import { useI18n, type MessageKey } from "../i18n";
import { localizeJobMessage, localizeJobTitle } from "../lib/daemonText";
import { RUNTIME_LABEL, SERVICE_LABEL } from "../lib/labels";
import { modKey, openUrl } from "../lib/platform";
import { siteUrl } from "../lib/urls";
import { useStore } from "../store";
import { Button, Dialog, Drawer, EmptyState, Progress, isIndeterminatePhase } from "./ui";

/** `keywords` let the palette find a page by what is on it ("node" → Runtimes). */
export interface NavItem { path: string; key: MessageKey; icon: ReactNode; keywords?: string }

interface Command { id: string; group: MessageKey; title: string; hint?: string; icon: ReactNode; run: () => void; keywords?: string }

/** Substring match first (earlier = better), then subsequence match. 0 = no match. */
function score(q: string, text: string): number {
  if (!q) return 1;
  const s = text.toLowerCase();
  const idx = s.indexOf(q);
  if (idx >= 0) return 100 - Math.min(idx, 90);
  let i = 0;
  for (const ch of s) if (ch === q[i]) i++;
  return i === q.length ? 5 : 0;
}

/** Ctrl/Cmd+K: every navigation target, action, site, runtime switch and service action. */
export function CommandPalette({ nav }: { nav: NavItem[] }) {
  const { t, lang, setLang } = useI18n();
  const navigate = useNavigate();
  const api = useStore((s) => s.api);
  const run = useStore((s) => s.run);
  const sites = useStore((s) => s.sites);
  const ports = useStore((s) => s.overview);
  const runtimes = useStore((s) => s.runtimes);
  const services = useStore((s) => s.services);
  const st = useStore.getState;
  const close = () => st().set({ paletteOpen: false });
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  const commands = useMemo<Command[]>(() => {
    const cmds: Command[] = [];
    nav.forEach((n, i) =>
      cmds.push({ id: `nav:${n.path}`, group: "palette.group.navigate", title: t("palette.go", { page: t(n.key) }), hint: `${modKey()} ${i + 1}`, icon: n.icon, run: () => navigate(n.path), keywords: n.keywords }),
    );
    const act = (id: string, title: string, icon: ReactNode, fn: () => void, hint?: string) => cmds.push({ id, group: "palette.group.actions", title, icon, run: fn, hint });
    act("add-site", t("sites.add"), <Plus />, () => { navigate("/sites"); st().set({ siteDialogOpen: true }); }, `${modKey()} N`);
    act("wizard", t("overview.quickSetup"), <Wand2 />, () => st().set({ wizardOpen: true }));
    act("start-all", t("common.startAll"), <Play />, () => void st().allServicesAction("start"));
    act("stop-all", t("common.stopAll"), <Square />, () => void st().allServicesAction("stop"));
    act("restart-all", t("common.restartAll"), <RotateCw />, () => void st().allServicesAction("restart"));
    act("trust", t("wizard.trustNow"), <Lock />, () => void st().trustCaVerified());
    act("dns", t("wizard.configureDns"), <Globe />, () => void st().configureDnsVerified());
    act("jobs", t("nav.jobs"), <Briefcase />, () => st().set({ jobsOpen: true }), `${modKey()} J`);
    act("theme", t("palette.toggleTheme"), <Moon />, () => st().setTheme(document.documentElement.dataset.theme === "dark" ? "light" : "dark"));
    act("lang", t("palette.switchLanguage"), <Languages />, () => setLang(lang === "en" ? "tr" : "en"));
    act("help", t("palette.showShortcuts"), <Keyboard />, () => st().set({ shortcutsOpen: true }), "?");
    for (const s of sites) {
      cmds.push({ id: `site:${s.id}`, group: "palette.group.sites", title: t("palette.openSite", { domain: s.domain }), icon: <ExternalLink />, run: () => void openUrl(siteUrl(s, ports)), keywords: s.path });
      cmds.push({ id: `site-d:${s.id}`, group: "palette.group.sites", title: `${s.domain} · ${t("sites.tabSettings")}`, icon: <Globe />, run: () => navigate(`/sites?site=${s.id}`) });
    }
    for (const k of RUNTIME_KINDS)
      cmds.push({
        id: `rt-install:${k}`, group: "palette.group.runtimes", title: t("palette.installRuntime", { kind: RUNTIME_LABEL[k] }), icon: <Download />,
        run: () => navigate(`/runtimes?kind=${k}`), keywords: `${k} ${RUNTIME_LABEL[k]} install kur`,
      });
    for (const r of runtimes)
      for (const v of r.installed)
        if (v !== r.active)
          cmds.push({
            id: `rt:${r.kind}:${v}`, group: "palette.group.runtimes", title: t("palette.useRuntime", { kind: RUNTIME_LABEL[r.kind], version: v }), icon: <Layers />,
            keywords: r.kind,
            run: async () => {
              const rt = await run(() => api!.useRuntime(r.kind, v), t("runtimes.activeNow", { kind: RUNTIME_LABEL[r.kind], version: v }));
              if (rt) st().applyRuntime(rt);
            },
          });
    for (const s of services) {
      if (s.status === "not_installed" || s.status === "external") continue;
      const actions: ServiceAction[] = s.status === "running" ? ["stop", "restart"] : ["start"];
      for (const a of actions)
        cmds.push({
          id: `svc:${s.name}:${a}`, group: "palette.group.services", title: t("palette.serviceAction", { action: t(`common.${a}`), name: SERVICE_LABEL(s.name) }), icon: <Server />,
          keywords: s.name,
          run: () => void st().serviceAction(s.name, a),
        });
    }
    return cmds;
  }, [nav, t, navigate, st, api, run, sites, ports, runtimes, services, lang, setLang]);

  const shown = useMemo(() => {
    const qq = q.trim().toLowerCase();
    if (!qq) return commands;
    return commands
      .map((c) => ({ c, s: Math.max(score(qq, c.title), c.keywords ? score(qq, c.keywords) / 2 : 0) }))
      .filter((x) => x.s > 0)
      .sort((a, b) => b.s - a.s)
      .map((x) => x.c)
      .slice(0, 50);
  }, [commands, q]);
  useEffect(() => setSel(0), [q]);
  useEffect(() => {
    listRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [sel]);
  const exec = (c?: Command) => {
    if (!c) return;
    close();
    c.run();
  };

  let lastGroup = "";
  return (
    <div className="overlay top" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="dialog palette" role="dialog" aria-modal="true" aria-label={t("palette.hint")}>
        <input
          autoFocus
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-label={t("palette.placeholder")}
          aria-activedescendant={shown[sel] ? `pc-${sel}` : undefined}
          placeholder={t("palette.placeholder")}
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") { e.preventDefault(); setSel((s) => Math.min(shown.length - 1, s + 1)); }
            else if (e.key === "ArrowUp") { e.preventDefault(); setSel((s) => Math.max(0, s - 1)); }
            else if (e.key === "Enter") { e.preventDefault(); exec(shown[sel]); }
            else if (e.key === "Escape") { e.preventDefault(); close(); }
          }}
        />
        <div className="palette-list" id="palette-list" role="listbox" ref={listRef}>
          {shown.length === 0 && <div className="palette-item faint">{t("palette.empty")}</div>}
          {shown.map((c, i) => {
            const header = !q && c.group !== lastGroup ? <div className="palette-group" role="presentation">{t(c.group)}</div> : null;
            lastGroup = c.group;
            return (
              <Fragment key={c.id}>
                {header}
                <div id={`pc-${i}`} role="option" aria-selected={i === sel} className="palette-item" onMouseMove={() => sel !== i && setSel(i)} onClick={() => exec(c)}>
                  {c.icon}
                  <span className="ellipsis">{c.title}</span>
                  {c.hint ? <span className="hint kbd">{c.hint}</span> : <ArrowRight className="hint" size={14} />}
                </div>
              </Fragment>
            );
          })}
        </div>
      </div>
    </div>
  );
}

export function JobsDrawer() {
  const { t, lang, fmtRelative } = useI18n();
  const jobs = useStore((s) => s.jobs);
  const set = useStore((s) => s.set);
  const list = Object.values(jobs).sort((a, b) => Number(b.status === "running") - Number(a.status === "running") || b.started_at.localeCompare(a.started_at));
  return (
    <Drawer title={t("jobs.title")} onClose={() => set({ jobsOpen: false })}>
      {list.length === 0 ? (
        <EmptyState icon={<Briefcase />} title={t("jobs.title")} body={t("jobs.empty")} />
      ) : (
        list.map((j) => {
          const title = localizeJobTitle(j.title, lang);
          const msg = localizeJobMessage(j.message, lang);
          const indet = j.status === "running" && isIndeterminatePhase(j.message);
          return (
            <div key={j.id} className="card" style={{ padding: 12, gap: 8 }}>
              <div className="row">
                {j.status === "done" ? <CheckCircle2 size={16} color="var(--ok)" aria-label={t("jobs.statusDone")} /> : j.status === "failed" ? <XCircle size={16} color="var(--danger)" aria-label={t("jobs.statusFailed")} /> : <RotateCw size={16} className="spin" color="var(--accent-fg)" aria-label={t("jobs.statusRunning")} />}
                <strong className="grow ellipsis" title={title}>{title}</strong>
                <span className="faint" style={{ fontSize: 12 }}>{fmtRelative(j.started_at)}</span>
              </div>
              {j.status === "running" && <Progress value={j.progress} label={title} indeterminate={indet} />}
              {msg && (
                <span className={j.status === "failed" ? "error-text" : "muted"} style={{ fontSize: 12.5 }}>
                  {msg}
                  {j.status === "running" && !indet ? ` · ${Math.round(j.progress * 100)}%` : ""}
                </span>
              )}
            </div>
          );
        })
      )}
    </Drawer>
  );
}

export function ShortcutsDialog() {
  const { t } = useI18n();
  const set = useStore((s) => s.set);
  const rows: [string, string][] = [
    [`${modKey()} K`, t("shortcuts.palette")],
    [`${modKey()} N`, t("shortcuts.newSite")],
    [`${modKey()} J`, t("shortcuts.jobs")],
    [`${modKey()} 1–7`, t("shortcuts.goto")],
    ["?", t("shortcuts.help")],
    ["Esc", t("shortcuts.close")],
  ];
  return (
    <Dialog size="sm" title={t("shortcuts.title")} onClose={() => set({ shortcutsOpen: false })} footer={<Button onClick={() => set({ shortcutsOpen: false })}>{t("common.close")}</Button>}>
      <dl className="kv" style={{ gap: "10px 24px" }}>
        {rows.map(([k, d]) => (
          <Fragment key={k}>
            <dt><span className="kbd">{k}</span></dt>
            <dd>{d}</dd>
          </Fragment>
        ))}
      </dl>
    </Dialog>
  );
}
