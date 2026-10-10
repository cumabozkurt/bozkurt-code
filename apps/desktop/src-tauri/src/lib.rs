//! BOZKURT CODE desktop shell.
//!
//! - Wraps the shared React UI (packages/ui) in a Tauri 2 window.
//! - Tray / menu-bar menu: status, switch PHP ▸, switch Node ▸, start all, stop all, open, quit.
//!   Every action calls the daemon HTTP API on 127.0.0.1:<api_port> (docs/API.md), with the
//!   per-install token from `$BOZKURT_HOME/api-token`.
//! - Starts `bozkurtd` (sidecar in release bundles, next-to-exe / PATH in dev) when /health fails.
//!   The daemon detaches and outlives the window: sites keep being served after the app quits.
//!   "Quit and stop services" asks it to stop through `POST /daemon/shutdown`.
//! - `bozkurt://` deep links → UI routes. Launch-at-login (autostart plugin). Close-to-tray.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WindowEvent, Wry};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_shell::ShellExt;

const DEFAULT_API_PORT: u16 = 7878;
const TRAY_ID: &str = "bozkurt-tray";
const NAVIGATE_EVENT: &str = "bozkurt://navigate";

/// Subset of the daemon's `Overview` (docs/API.md) used by the tray.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
struct Overview {
    #[serde(default)]
    version: String,
    #[serde(default)]
    runtimes: Vec<Runtime>,
    #[serde(default)]
    services: Vec<Service>,
    #[serde(default)]
    sites_count: u32,
}
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
struct Runtime {
    kind: String,
    active: Option<String>,
    #[serde(default)]
    installed: Vec<String>,
}
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
struct Service {
    name: String,
    status: String,
}

#[derive(Default)]
struct DaemonState {
    /// Last overview rendered into the tray (outer None = never rendered).
    last: Mutex<Option<Option<Overview>>>,
}

/// `$BOZKURT_HOME`, else `~/.bozkurt` (macOS/Linux) or `%LOCALAPPDATA%\Bozkurt` (Windows);
/// the same rules as `bz_core::paths::default_home`.
fn bozkurt_home() -> PathBuf {
    if let Some(h) = std::env::var_os("BOZKURT_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(h);
    }
    platform_home()
}

#[cfg(windows)]
fn platform_home() -> PathBuf {
    if let Some(l) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
        return PathBuf::from(l).join("Bozkurt");
    }
    if let Some(p) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(p)
            .join("AppData")
            .join("Local")
            .join("Bozkurt");
    }
    PathBuf::from(r"C:\Bozkurt")
}

#[cfg(not(windows))]
fn platform_home() -> PathBuf {
    match std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        Some(h) => PathBuf::from(h).join(".bozkurt"),
        None => PathBuf::from("/tmp/.bozkurt"),
    }
}

/// `api_port` from `config.toml` (same parsing as `bz`), else 7878.
fn api_port_from_toml(text: &str) -> u16 {
    text.parse::<toml::Table>()
        .ok()
        .and_then(|t| t.get("api_port").and_then(|v| v.as_integer()))
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0)
        .unwrap_or(DEFAULT_API_PORT)
}

fn api_port() -> u16 {
    std::fs::read_to_string(bozkurt_home().join("config.toml"))
        .map(|t| api_port_from_toml(&t))
        .unwrap_or(DEFAULT_API_PORT)
}

/// `http://127.0.0.1:<api_port>/api/v1`, re-read on every call so a port change is picked up.
fn api_base() -> String {
    format!("http://127.0.0.1:{}/api/v1", api_port())
}

/// The per-install local API token (created by bozkurtd at startup).
fn api_token() -> Option<String> {
    let t = std::fs::read_to_string(bozkurt_home().join("api-token")).ok()?;
    let t = t.trim();
    (t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())).then(|| t.to_string())
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .build()
        .expect("http client")
}

fn request(method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
    let rb = http().request(method, format!("{}{path}", api_base()));
    match api_token() {
        Some(t) => rb.bearer_auth(t),
        None => rb,
    }
}

async fn daemon_healthy() -> bool {
    matches!(request(reqwest::Method::GET, "/health").send().await, Ok(r) if r.status().is_success())
}

async fn fetch_overview() -> Option<Overview> {
    let r = request(reqwest::Method::GET, "/overview")
        .send()
        .await
        .ok()?;
    if !r.status().is_success() {
        return None;
    }
    r.json::<Overview>().await.ok()
}

async fn post(path: &str, body: serde_json::Value) -> Result<(), String> {
    let r = request(reqwest::Method::POST, path)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if r.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "{} {}",
            r.status(),
            r.text().await.unwrap_or_default()
        ))
    }
}

/// Start the daemon if it is not answering. Release bundles ship it as a sidecar; otherwise we look
/// next to our own executable and then on PATH.
async fn ensure_daemon(app: &AppHandle) {
    if daemon_healthy().await {
        return;
    }
    // bozkurtd detaches itself (it re-execs with --foreground in a new session), so the
    // process we spawn exits right away; the daemon is then tracked through the API only.
    let mut spawned = false;
    if let Ok(cmd) = app.shell().sidecar("bozkurtd") {
        match cmd.spawn() {
            Ok(_) => spawned = true,
            Err(e) => eprintln!("[bozkurt] sidecar spawn failed: {e}"),
        }
    }
    if !spawned {
        let exe = if cfg!(windows) {
            "bozkurtd.exe"
        } else {
            "bozkurtd"
        };
        let candidate = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join(exe)))
            .filter(|p| p.exists())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| exe.to_string());
        if let Err(e) = app.shell().command(candidate).spawn() {
            eprintln!("[bozkurt] could not start bozkurtd: {e}");
            return;
        }
    }
    for _ in 0..40 {
        if daemon_healthy().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    eprintln!(
        "[bozkurt] bozkurtd did not answer on {} within 10 s",
        api_base()
    );
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn navigate(app: &AppHandle, route: &str) {
    show_main(app);
    let _ = app.emit(NAVIGATE_EVENT, route.to_string());
}

/// bozkurt://sites → "/sites", bozkurt://sites/<id> → "/sites?site=<id>",
/// bozkurt://runtimes/php → "/runtimes?kind=php", bozkurt://palette → "palette".
fn deep_link_route(url: &str) -> Option<String> {
    let rest = url.strip_prefix("bozkurt://")?;
    let rest = rest
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    let mut parts = rest.split('/').filter(|s| !s.is_empty());
    let head = parts.next().unwrap_or("");
    let arg = parts.next();
    Some(match (head, arg) {
        ("" | "open" | "overview", _) => "/".into(),
        ("palette", _) => "palette".into(),
        ("sites", Some(id)) => format!("/sites?site={id}"),
        ("runtimes", Some(kind)) => format!("/runtimes?kind={kind}"),
        ("sites" | "runtimes" | "services" | "logs" | "doctor" | "settings", None) => {
            format!("/{head}")
        }
        _ => return None,
    })
}

fn status_line(ov: Option<&Overview>) -> String {
    match ov {
        None => "○ bozkurtd not running".to_string(),
        Some(o) => {
            let running = o.services.iter().filter(|s| s.status == "running").count();
            let installed = o
                .services
                .iter()
                .filter(|s| s.status != "not_installed")
                .count();
            let crashed = o.services.iter().any(|s| s.status == "crashed");
            format!(
                "{} {running}/{installed} services · {} sites",
                if crashed { "▲" } else { "●" },
                o.sites_count
            )
        }
    }
}

fn runtime_submenu(
    app: &AppHandle,
    ov: Option<&Overview>,
    kind: &str,
    label: &str,
) -> tauri::Result<Submenu<Wry>> {
    let sub = Submenu::with_id(app, format!("menu:{kind}"), label, ov.is_some())?;
    match ov.and_then(|o| o.runtimes.iter().find(|r| r.kind == kind)) {
        Some(rt) if !rt.installed.is_empty() => {
            for v in &rt.installed {
                let active = rt.active.as_deref() == Some(v.as_str());
                let item = CheckMenuItem::with_id(
                    app,
                    format!("use:{kind}:{v}"),
                    v,
                    true,
                    active,
                    None::<&str>,
                )?;
                sub.append(&item)?;
            }
        }
        _ => {
            let item = MenuItem::with_id(
                app,
                format!("install:{kind}"),
                "Install a version…",
                true,
                None::<&str>,
            )?;
            sub.append(&item)?;
        }
    }
    Ok(sub)
}

fn build_menu(app: &AppHandle, ov: Option<&Overview>) -> tauri::Result<Menu<Wry>> {
    let status = MenuItem::with_id(app, "status", status_line(ov), false, None::<&str>)?;
    let php = runtime_submenu(app, ov, "php", "Switch PHP")?;
    let node = runtime_submenu(app, ov, "node", "Switch Node")?;
    let start_all = MenuItem::with_id(
        app,
        "start_all",
        "Start all services",
        ov.is_some(),
        None::<&str>,
    )?;
    let stop_all = MenuItem::with_id(
        app,
        "stop_all",
        "Stop all services",
        ov.is_some(),
        None::<&str>,
    )?;
    let open = MenuItem::with_id(app, "open", "Open BOZKURT CODE", true, None::<&str>)?;
    let palette = MenuItem::with_id(app, "palette", "Command palette…", true, None::<&str>)?;
    let autostart_on = app.autolaunch().is_enabled().unwrap_or(false);
    let login = CheckMenuItem::with_id(
        app,
        "autostart",
        "Launch at login",
        true,
        autostart_on,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit BOZKURT CODE", true, None::<&str>)?;
    let quit_all = MenuItem::with_id(
        app,
        "quit_stop",
        "Quit and stop services",
        ov.is_some(),
        None::<&str>,
    )?;
    let s1 = PredefinedMenuItem::separator(app)?;
    let s2 = PredefinedMenuItem::separator(app)?;
    let s3 = PredefinedMenuItem::separator(app)?;
    let items: [&dyn IsMenuItem<Wry>; 13] = [
        &status, &s1, &php, &node, &start_all, &stop_all, &s2, &open, &palette, &login, &s3, &quit,
        &quit_all,
    ];
    Menu::with_items(app, &items)
}

/// Rebuild the tray menu only when the overview changed (rebuilding closes an open menu on some OSes).
fn refresh_tray(app: &AppHandle, ov: Option<Overview>) {
    let state = app.state::<DaemonState>();
    let mut last = state.last.lock().unwrap();
    if last.as_ref() == Some(&ov) {
        return;
    }
    if let (Some(tray), Ok(menu)) = (app.tray_by_id(TRAY_ID), build_menu(app, ov.as_ref())) {
        let _ = tray.set_menu(Some(menu));
        let tip = match &ov {
            Some(o) => format!("BOZKURT CODE {}", o.version),
            None => "BOZKURT CODE — daemon offline".into(),
        };
        let _ = tray.set_tooltip(Some(tip));
    }
    *last = Some(ov);
}

fn force_refresh(app: &AppHandle) {
    app.state::<DaemonState>().last.lock().unwrap().take();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        let ov = fetch_overview().await;
        refresh_tray(&app, ov);
    });
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "open" => show_main(app),
        "palette" => navigate(app, "palette"),
        "quit" => app.exit(0),
        "quit_stop" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = post("/daemon/shutdown", json!({})).await {
                    eprintln!("[bozkurt] stopping bozkurtd failed: {e}");
                }
                app.exit(0);
            });
        }
        "autostart" => {
            let m = app.autolaunch();
            let _ = if m.is_enabled().unwrap_or(false) {
                m.disable()
            } else {
                m.enable()
            };
            force_refresh(app);
        }
        "start_all" | "stop_all" => {
            let action = if id == "start_all" { "start" } else { "stop" };
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = post(&format!("/services/all/{action}"), json!({})).await {
                    eprintln!("[bozkurt] {action} all failed: {e}");
                }
                force_refresh(&app);
            });
        }
        other => {
            if let Some((kind, version)) =
                other.strip_prefix("use:").and_then(|r| r.split_once(':'))
            {
                let (kind, version) = (kind.to_string(), version.to_string());
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = post(
                        &format!("/runtimes/{kind}/use"),
                        json!({ "version": version }),
                    )
                    .await
                    {
                        eprintln!("[bozkurt] use {kind} {version} failed: {e}");
                    }
                    force_refresh(&app);
                });
            } else if let Some(kind) = other.strip_prefix("install:") {
                navigate(app, &format!("/runtimes?kind={kind}"));
            }
        }
    }
}

/// Opens `path` in the user's editor: VS Code CLI (`code`) when available, else the vscode:// handler.
#[tauri::command]
fn open_in_editor(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let try_cli = |bin: &str| std::process::Command::new(bin).arg(&path).spawn().is_ok();
    if try_cli("code") || (cfg!(windows) && try_cli("code.cmd")) {
        return Ok(());
    }
    let url = format!(
        "vscode://file/{}",
        path.replace('\\', "/").trim_start_matches('/')
    );
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// The daemon API base URL (`http://127.0.0.1:<api_port>/api/v1`, from config.toml).
/// The UI passes it as `<BozkurtApp baseUrl=…>`.
#[tauri::command]
fn get_api_base() -> String {
    api_base()
}

/// The per-install local API token, for clients that want to send it
/// (`Authorization: Bearer`, or `?token=` on `/events`). The webview's own origin is
/// already allow-listed, so the UI works without it.
#[tauri::command]
fn get_api_token() -> Option<String> {
    api_token()
}

/// Opens a terminal window at `path`.
#[tauri::command]
fn open_terminal(path: String) -> Result<(), String> {
    use std::process::Command;
    #[cfg(target_os = "macos")]
    let ok = Command::new("open")
        .args(["-a", "Terminal", &path])
        .spawn()
        .is_ok();
    if !std::path::Path::new(&path).is_dir() {
        return Err(format!("{path} is not a directory"));
    }
    // No shell parsing of the path: `wt -d` gets it as one argument, and the cmd fallback
    // opens a new console with the path as its working directory.
    #[cfg(windows)]
    let ok = Command::new("wt").args(["-d", &path]).spawn().is_ok() || {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        Command::new("cmd")
            .creation_flags(CREATE_NEW_CONSOLE)
            .current_dir(&path)
            .spawn()
            .is_ok()
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let ok = [
        ("x-terminal-emulator", vec![]),
        ("gnome-terminal", vec!["--working-directory", path.as_str()]),
        ("konsole", vec!["--workdir", path.as_str()]),
        ("xfce4-terminal", vec!["--working-directory", path.as_str()]),
        ("alacritty", vec!["--working-directory", path.as_str()]),
        ("kitty", vec!["--directory", path.as_str()]),
        ("xterm", vec![]),
    ]
    .iter()
    .any(|(bin, args)| {
        Command::new(bin)
            .args(args)
            .current_dir(&path)
            .spawn()
            .is_ok()
    });
    if ok {
        Ok(())
    } else {
        Err("no terminal emulator found".into())
    }
}

const UPDATE_EVENT: &str = "bozkurt://update-available";

/// Asks the release endpoint for a newer signed build. When there is one it emits
/// [`UPDATE_EVENT`] (`{version, current_version, notes}`) and asks the user with a native
/// dialog; on "Install and restart" the update is downloaded, its minisign signature is
/// verified against the embedded public key by the updater plugin, installed, and the app
/// restarts. "Later" does nothing until the next launch. Errors are logged, never fatal.
async fn check_for_update(app: &AppHandle) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    use tauri_plugin_updater::UpdaterExt;
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            eprintln!("[bozkurt] updater unavailable: {e}");
            return;
        }
    };
    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => return,
        Err(e) => {
            eprintln!("[bozkurt] update check failed: {e}");
            return;
        }
    };
    let _ = app.emit(
        UPDATE_EVENT,
        json!({
            "version": update.version,
            "current_version": update.current_version,
            "notes": update.body,
        }),
    );
    let text = format!(
        "BOZKURT CODE {} is available (you have {}).\n\nInstall it now? The app restarts after the update; running sites keep being served by bozkurtd.",
        update.version, update.current_version
    );
    let h = app.clone();
    app.dialog()
        .message(text)
        .title("Update available")
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Install and restart".into(),
            "Later".into(),
        ))
        .show(move |install| {
            if !install {
                return;
            }
            tauri::async_runtime::spawn(async move {
                match update.download_and_install(|_, _| {}, || {}).await {
                    Ok(()) => h.restart(),
                    Err(e) => {
                        eprintln!("[bozkurt] update install failed: {e}");
                        h.dialog()
                            .message(format!("The update could not be installed: {e}"))
                            .title("Update failed")
                            .kind(MessageDialogKind::Error)
                            .show(|_| {});
                    }
                }
            });
        });
}

pub fn run() {
    tauri::Builder::default()
        // Must be first: a second launch (or a bozkurt:// link) is forwarded to this instance.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main(app)
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .manage(DaemonState::default())
        .invoke_handler(tauri::generate_handler![
            open_in_editor,
            open_terminal,
            get_api_base,
            get_api_token
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            if std::env::args().any(|a| a == "--minimized") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }

            // Tray / menu bar
            let menu = build_menu(&handle, None)?;
            let mut tray = TrayIconBuilder::with_id(TRAY_ID)
                .tooltip("BOZKURT CODE")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, ev| on_menu(app, ev.id().as_ref()));
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            // Deep links
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                #[cfg(any(windows, target_os = "linux"))]
                let _ = app.deep_link().register_all();
                let h = handle.clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        if let Some(route) = deep_link_route(url.as_str()) {
                            navigate(&h, &route);
                        }
                    }
                });
            }

            // Updater: its config (endpoint + pubkey) comes from tauri.release.conf.json, which
            // only release builds with a signing key get. Dev builds and forks have none, and the
            // plugin refuses to start without a pubkey, so it is registered only when configured.
            // Installing stays a user action: check_for_update asks before downloading.
            if app.config().plugins.0.contains_key("updater") {
                handle.plugin(tauri_plugin_updater::Builder::new().build())?;
                let h = handle.clone();
                tauri::async_runtime::spawn(async move { check_for_update(&h).await });
            }

            // Daemon + tray status loop
            tauri::async_runtime::spawn(async move {
                ensure_daemon(&handle).await;
                loop {
                    let ov = fetch_overview().await;
                    refresh_tray(&handle, ov);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps BOZKURT CODE running in the tray / menu bar.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building BOZKURT CODE")
        .run(|_app, _event| {
            // The daemon outlives the window on purpose (sites keep being served); the tray's
            // "Quit and stop services" stops it explicitly.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                show_main(_app);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{api_port_from_toml, deep_link_route, status_line, Overview, Service};

    #[test]
    fn api_port_parsing() {
        assert_eq!(api_port_from_toml("api_port = 9000"), 9000);
        assert_eq!(api_port_from_toml("tld = \"test\""), 7878);
        assert_eq!(api_port_from_toml("api_port = 0"), 7878);
        assert_eq!(api_port_from_toml("api_port = 70000"), 7878);
        assert_eq!(api_port_from_toml("not toml ["), 7878);
    }

    #[test]
    fn deep_links() {
        assert_eq!(
            deep_link_route("bozkurt://sites").as_deref(),
            Some("/sites")
        );
        assert_eq!(
            deep_link_route("bozkurt://sites/abc").as_deref(),
            Some("/sites?site=abc")
        );
        assert_eq!(
            deep_link_route("bozkurt://runtimes/php/").as_deref(),
            Some("/runtimes?kind=php")
        );
        assert_eq!(deep_link_route("bozkurt://open").as_deref(), Some("/"));
        assert_eq!(
            deep_link_route("bozkurt://palette").as_deref(),
            Some("palette")
        );
        assert_eq!(deep_link_route("bozkurt://nope"), None);
        assert_eq!(deep_link_route("https://x"), None);
    }

    #[test]
    fn status() {
        assert!(status_line(None).contains("not running"));
        let ov = Overview {
            services: vec![
                Service {
                    name: "caddy".into(),
                    status: "running".into(),
                },
                Service {
                    name: "redis".into(),
                    status: "crashed".into(),
                },
                Service {
                    name: "mailpit".into(),
                    status: "not_installed".into(),
                },
            ],
            sites_count: 3,
            ..Default::default()
        };
        assert_eq!(status_line(Some(&ov)), "▲ 1/2 services · 3 sites");
    }
}
