import { useEffect, useMemo, useState } from "react";
import { HashRouter, Link, NavLink, Navigate, Route, Routes, useNavigate } from "react-router-dom";
import { useShallow } from "zustand/react/shallow";
import {
  AlertTriangle, Briefcase, ChevronRight, RefreshCw, Command as CommandIcon, FlaskConical, Globe, Layers, LayoutDashboard, Menu, Moon, ScrollText, Search, Server, Settings, Stethoscope, Sun, WifiOff,
} from "lucide-react";
import { createApi, type RemoteConnection } from "./api";
import type { BozkurtApi } from "./api/types";
import { AppModeContext, useAppMode, type AppModeCtx } from "./context";
import { CommandPalette, JobsDrawer, ShortcutsDialog, type NavItem } from "./components/Overlays";
import { Button, ConfirmHost, Logo, Toasts } from "./components/ui";
import { I18nProvider, useI18n } from "./i18n";
import { modKey } from "./lib/platform";
import { DoctorScreen } from "./screens/Doctor";
import { LogsScreen } from "./screens/Logs";
import { OverviewScreen } from "./screens/Overview";
import { QuickSetupDialog } from "./screens/QuickSetup";
import { RuntimesScreen } from "./screens/Runtimes";
import { ServicesScreen } from "./screens/Services";
import { SettingsScreen } from "./screens/Settings";
import { CreateSiteDialog, SitesScreen } from "./screens/Sites";
import { runningJobs, useStore } from "./store";

const NAV: NavItem[] = [
  { path: "/", key: "nav.overview", icon: <LayoutDashboard /> },
  { path: "/sites", key: "nav.sites", icon: <Globe />, keywords: "site domain vhost https share" },
  { path: "/runtimes", key: "nav.runtimes", icon: <Layers />, keywords: "php node nodejs python bun go version sürüm" },
  { path: "/services", key: "nav.services", icon: <Server />, keywords: "caddy dns mariadb mysql postgres redis mailpit php-fpm servis" },
  { path: "/logs", key: "nav.logs", icon: <ScrollText />, keywords: "log tail kayıt" },
  { path: "/doctor", key: "nav.doctor", icon: <Stethoscope />, keywords: "health check diagnose sağlık" },
  { path: "/settings", key: "nav.settings", icon: <Settings />, keywords: "config port tld remote pairing theme language ayar" },
];

function useResolvedTheme(): "light" | "dark" {
  const theme = useStore((s) => s.theme);
  const [sysDark, setSysDark] = useState(() => typeof matchMedia !== "undefined" && matchMedia("(prefers-color-scheme: dark)").matches);
  useEffect(() => {
    const mq = matchMedia("(prefers-color-scheme: dark)");
    const h = () => setSysDark(mq.matches);
    mq.addEventListener("change", h);
    return () => mq.removeEventListener("change", h);
  }, []);
  const resolved = theme === "system" ? (sysDark ? "dark" : "light") : theme;
  useEffect(() => {
    document.documentElement.dataset.theme = resolved;
  }, [resolved]);
  return resolved;
}

function isTyping(e: KeyboardEvent) {
  const el = e.target as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

function useShortcuts() {
  const navigate = useNavigate();
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      const st = useStore.getState();
      const k = e.key.toLowerCase();
      if (mod && k === "k") {
        e.preventDefault();
        st.set({ paletteOpen: !st.paletteOpen });
      } else if (mod && k === "n" && !e.shiftKey) {
        e.preventDefault();
        navigate("/sites");
        st.set({ siteDialogOpen: true });
      } else if (mod && k === "j") {
        e.preventDefault();
        st.set({ jobsOpen: !st.jobsOpen });
      } else if (mod && /^[1-7]$/.test(e.key)) {
        e.preventDefault();
        navigate(NAV[Number(e.key) - 1].path);
      } else if (e.key === "?" && !mod && !isTyping(e)) {
        e.preventDefault();
        st.set({ shortcutsOpen: true });
      }
    };
    window.addEventListener("keydown", h);
    // Hosts (desktop tray menu, bozkurt:// deep links) navigate via a DOM event.
    const onNav = (e: Event) => {
      const path = (e as CustomEvent<string>).detail;
      if (path === "palette") useStore.getState().set({ paletteOpen: true });
      else if (typeof path === "string" && path.startsWith("/")) navigate(path);
    };
    window.addEventListener("bozkurt:navigate", onNav);
    return () => {
      window.removeEventListener("keydown", h);
      window.removeEventListener("bozkurt:navigate", onNav);
    };
  }, [navigate]);
}

function ConnectionBanner() {
  const { t } = useI18n();
  const api = useStore((s) => s.api);
  const conn = useStore((s) => s.conn);
  const loaded = useStore((s) => s.loaded);
  const loadError = useStore((s) => s.loadError);
  const hasData = useStore((s) => !!s.overview);
  const mode = useAppMode();
  if (api?.mode === "mock") return <div className="banner info" role="status"><FlaskConical size={14} aria-hidden />{t("conn.mock")}</div>;
  if (!loaded) return null;
  if (conn === "open") {
    if (!loadError) return null;
    return (
      <div className="banner warn" role="alert">
        <AlertTriangle size={16} aria-hidden />
        <strong>{t("state.loadFailed")}</strong>
        <span className="grow ellipsis">{loadError}</span>
        <Button size="sm" icon={<RefreshCw />} onClick={() => void useStore.getState().refreshAll()}>{t("common.retry")}</Button>
      </div>
    );
  }
  return (
    <div className="banner warn" role="alert">
      <WifiOff size={16} aria-hidden />
      <strong>{conn === "connecting" ? t("conn.connecting") : t("conn.offline")}</strong>
      <span className="hide-mobile grow">
        {mode.mode === "remote" ? t("conn.remote", { host: mode.remoteHost ?? "" }) : t("conn.offlineHint")}
        {hasData ? ` ${t("conn.stale")}` : ""}
      </span>
    </div>
  );
}

function JobsButton() {
  const { t } = useI18n();
  const n = useStore((s) => runningJobs(s.jobs).length);
  return (
    <Button
      variant="ghost"
      icon={<Briefcase className={n ? "spin" : undefined} style={n ? { animationDuration: "2.4s" } : undefined} />}
      onClick={() => useStore.getState().set({ jobsOpen: true })}
      aria-label={n ? t("jobs.running", { n }) : t("nav.jobs")}
      title={`${t("nav.jobs")} (${modKey()} J)`}
    >
      <span className="hide-mobile">{n ? t("jobs.running", { n }) : t("nav.jobs")}</span>
    </Button>
  );
}

function MoreScreen() {
  const { t } = useI18n();
  return (
    <div className="page">
      <h1>{t("nav.more")}</h1>
      <nav className="card" style={{ padding: 6, gap: 0 }} aria-label={t("nav.more")}>
        {NAV.slice(4).map((n) => (
          <Link key={n.path} to={n.path} className="nav-link" style={{ padding: 14 }}>{n.icon}{t(n.key)}<ChevronRight size={16} style={{ marginLeft: "auto" }} /></Link>
        ))}
        <button type="button" className="nav-link" style={{ padding: 14, border: 0, background: "none", font: "inherit", cursor: "pointer", width: "100%" }} onClick={() => useStore.getState().set({ jobsOpen: true })}>
          <Briefcase />{t("nav.jobs")}<ChevronRight size={16} style={{ marginLeft: "auto" }} />
        </button>
      </nav>
    </div>
  );
}

function Shell() {
  const { t } = useI18n();
  const ui = useStore(useShallow((s) => ({ jobsOpen: s.jobsOpen, paletteOpen: s.paletteOpen, shortcutsOpen: s.shortcutsOpen, siteDialogOpen: s.siteDialogOpen, wizardOpen: s.wizardOpen })));
  const setTheme = useStore((s) => s.setTheme);
  const mode = useAppMode();
  const resolved = useResolvedTheme();
  useShortcuts();
  const openPalette = () => useStore.getState().set({ paletteOpen: true });
  const stale = useStore((s) => s.loaded && !!s.overview && s.conn === "closed" && s.api?.mode !== "mock");
  return (
    <div className="app">
      <nav className="sidebar" aria-label={t("app.name")}>
        <div className="brand">
          <Logo />
          <div className="brand-name">{t("app.name")}<small>{mode.mode === "remote" ? mode.remoteHost : t("app.tagline")}</small></div>
        </div>
        {NAV.map((n, i) => (
          <NavLink key={n.path} to={n.path} end={n.path === "/"} className={({ isActive }) => `nav-link ${isActive ? "active" : ""}`}>
            {n.icon}
            {t(n.key)}
            <span className="kbd" aria-hidden>{i + 1}</span>
          </NavLink>
        ))}
        <div className="sidebar-footer">
          <button type="button" className="search-trigger" style={{ minWidth: 0 }} onClick={openPalette}>
            <CommandIcon size={14} aria-hidden />
            <span className="grow ellipsis" style={{ textAlign: "left" }}>{t("palette.short")}</span>
            <span className="kbd">{modKey()} K</span>
          </button>
        </div>
      </nav>
      <div className="main">
        <header className="topbar">
          <span className="show-mobile" style={{ display: "flex" }}><Logo size={26} /></span>
          <button type="button" className="search-trigger" onClick={openPalette} title={t("palette.hint")}>
            <Search size={15} aria-hidden />
            <span className="grow ellipsis" style={{ textAlign: "left" }}>{t("palette.placeholder")}</span>
            <span className="kbd hide-mobile">{modKey()} K</span>
          </button>
          <span className="spacer hide-mobile" />
          <Button variant="ghost" iconOnly icon={resolved === "dark" ? <Sun /> : <Moon />} aria-label={t("palette.toggleTheme")} title={t("palette.toggleTheme")} onClick={() => setTheme(resolved === "dark" ? "light" : "dark")} />
          <JobsButton />
        </header>
        <ConnectionBanner />
        <main className="content" id="main" data-stale={stale || undefined}>
          <Routes>
            <Route path="/" element={<OverviewScreen />} />
            <Route path="/sites" element={<SitesScreen />} />
            <Route path="/runtimes" element={<RuntimesScreen />} />
            <Route path="/services" element={<ServicesScreen />} />
            <Route path="/logs" element={<LogsScreen />} />
            <Route path="/doctor" element={<DoctorScreen />} />
            <Route path="/settings" element={<SettingsScreen />} />
            <Route path="/more" element={<MoreScreen />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
        </main>
      </div>
      <nav className="tabbar" aria-label={t("app.name")}>
        {NAV.slice(0, 4).map((n) => (
          <NavLink key={n.path} to={n.path} end={n.path === "/"} className={({ isActive }) => (isActive ? "active" : "")}>{n.icon}{t(n.key)}</NavLink>
        ))}
        <NavLink to="/more" className={({ isActive }) => (isActive ? "active" : "")}><Menu />{t("nav.more")}</NavLink>
      </nav>
      {ui.paletteOpen && <CommandPalette nav={NAV} />}
      {ui.jobsOpen && <JobsDrawer />}
      {ui.shortcutsOpen && <ShortcutsDialog />}
      {ui.siteDialogOpen && <CreateSiteDialog />}
      {ui.wizardOpen && <QuickSetupDialog />}
      <ConfirmHost />
      <Toasts />
    </div>
  );
}

export interface BozkurtAppProps {
  /** "local" talks to 127.0.0.1:7878 (desktop). "remote" uses a paired connection (mobile). */
  mode?: "local" | "remote";
  remote?: RemoteConnection | null;
  /** Override the local API base URL (e.g. the playground's same-origin proxy). */
  baseUrl?: string;
  /** Provide a ready API (tests); otherwise one is created from mode/remote/?mock. */
  api?: BozkurtApi;
  onDisconnect?: () => void;
}

/** The full BOZKURT CODE UI: providers + router + shell. */
export function BozkurtApp({ mode = "local", remote, baseUrl, api: given, onDisconnect }: BozkurtAppProps) {
  const attach = useStore((s) => s.attach);
  const [ready, setReady] = useState(false);
  // The API is created inside the effect so every attach owns a fresh instance: React
  // StrictMode (dev) runs cleanup (which closes the API) and then the effect again.
  useEffect(() => {
    const api = given ?? createApi({ mode, remote, baseUrl });
    const detach = attach(api);
    setReady(true);
    return detach;
  }, [given, mode, remote, baseUrl, attach]);
  const ctx = useMemo<AppModeCtx>(() => {
    let remoteHost: string | undefined;
    try {
      remoteHost = remote ? new URL(remote.baseUrl).host : undefined;
    } catch {
      remoteHost = remote?.baseUrl;
    }
    return { mode, remoteHost, disconnect: onDisconnect };
  }, [mode, remote, onDisconnect]);
  return (
    <I18nProvider>
      <AppModeContext.Provider value={ctx}>
        <HashRouter future={{ v7_startTransition: true, v7_relativeSplatPath: true }}>{ready && <Shell />}</HashRouter>
      </AppModeContext.Provider>
    </I18nProvider>
  );
}
