//! Live integration tests (network + real binaries). Ignored by default:
//!
//! ```sh
//! cargo test -p bz-core --test live -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::Duration;

use bz_core::models::{CreateSiteRequest, JobStatus, RuntimeKind, ServiceAction, ServiceStatus};
use bz_core::{Engine, NoopNetProvider};

async fn engine(dir: &std::path::Path) -> Engine {
    let home = dir.join("home");
    Engine::new(&home, Arc::new(NoopNetProvider::new(&home)))
        .await
        .expect("engine")
}

/// Installs a small Node release (newest 18.x — EOL, smallest current tarball) into a
/// temp BOZKURT_HOME, makes it active, and runs `node -v` both directly and via the shim.
#[tokio::test]
#[ignore = "downloads Node.js from nodejs.org"]
async fn install_node_and_run() {
    let t = tempfile::tempdir().unwrap();
    let e = engine(t.path()).await;
    let versions = e.runtime_versions(RuntimeKind::Node, true).await.unwrap();
    assert!(versions.iter().any(|v| v.version.starts_with("22.")));
    let job = e.install_runtime(RuntimeKind::Node, "18");
    let done = e
        .jobs_manager()
        .wait(&job.job_id, Duration::from_secs(600))
        .await
        .unwrap();
    assert_eq!(done.status, JobStatus::Done, "{:?}", done.message);
    let rt = e
        .runtimes()
        .into_iter()
        .find(|r| r.kind == RuntimeKind::Node)
        .unwrap();
    let active = rt.active.clone().expect("active after first install");
    assert!(active.starts_with("18."));
    println!("installed node {active}: {:?}", done.message);

    let bin = e.runtime_manager().binary(RuntimeKind::Node, &active);
    let out = std::process::Command::new(&bin).arg("-v").output().unwrap();
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    println!("{} -v => {v}", bin.display());
    assert_eq!(v, format!("v{active}"));

    #[cfg(unix)]
    {
        let shim = e.paths().bin().join("node");
        let out = std::process::Command::new(&shim)
            .arg("-v")
            .current_dir(t.path())
            .env("BOZKURT_HOME", e.paths().home())
            .env_remove("BOZKURT_NODE_VERSION")
            .output()
            .unwrap();
        let sv = String::from_utf8_lossy(&out.stdout).trim().to_string();
        println!("shim node -v => {sv}");
        assert_eq!(sv, format!("v{active}"));
        let npm = std::process::Command::new(e.paths().bin().join("npm"))
            .arg("-v")
            .current_dir(t.path())
            .env("BOZKURT_HOME", e.paths().home())
            .output()
            .unwrap();
        println!(
            "shim npm -v => {}",
            String::from_utf8_lossy(&npm.stdout).trim()
        );
        assert!(npm.status.success());
    }
    let listed = e.runtime_versions(RuntimeKind::Node, false).await.unwrap();
    assert!(listed
        .iter()
        .any(|v| v.version == active && v.installed && v.active));
    e.uninstall_runtime(RuntimeKind::Node, &active)
        .await
        .unwrap();
    assert!(e
        .runtimes()
        .iter()
        .all(|r| r.kind != RuntimeKind::Node || r.installed.is_empty()));
}

/// Fetches every live runtime catalog for this platform and checks the first asset URL
/// actually exists (HTTP HEAD/GET range).
#[tokio::test]
#[ignore = "queries upstream release indexes"]
async fn live_catalogs_resolve() {
    let t = tempfile::tempdir().unwrap();
    let e = engine(t.path()).await;
    let client = bz_core::download::http_client();
    for kind in RuntimeKind::ALL {
        let rel = e
            .runtime_manager()
            .catalog()
            .releases(kind, true)
            .await
            .unwrap_or_else(|err| panic!("{kind}: {err}"));
        let top = &rel[0];
        println!(
            "{kind}: {} releases, newest {} ({:?})",
            rel.len(),
            top.version,
            top.channel
        );
        for a in &top.assets {
            let r = client
                .get(&a.url)
                .header("Range", "bytes=0-0")
                .send()
                .await
                .unwrap();
            println!("  {} -> {}", a.url, r.status());
            assert!(r.status().is_success(), "{} -> {}", a.url, r.status());
            if let bz_core::download::Checksum::SumsFile { .. } = &a.checksum {
                assert!(a.checksum.resolve(&client).await.unwrap().is_some());
            }
        }
    }
}

/// Installs Caddy, serves a static site on unprivileged ports and fetches it.
#[tokio::test]
#[ignore = "downloads Caddy from GitHub and binds local ports"]
async fn caddy_serves_static_site() {
    let t = tempfile::tempdir().unwrap();
    let e = engine(t.path()).await;
    e.patch_config(&serde_json::json!({"http_port": 18080, "https_port": 18443}))
        .await
        .unwrap();
    let proj = t.path().join("hello");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("index.html"), "hello from bozkurt").unwrap();
    let site = e
        .create_site(&CreateSiteRequest {
            path: proj.to_string_lossy().into_owned(),
            https: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    let job = e.install_service("caddy", None).unwrap();
    let done = e
        .jobs_manager()
        .wait(&job.job_id, Duration::from_secs(300))
        .await
        .unwrap();
    assert_eq!(done.status, JobStatus::Done, "{:?}", done.message);
    println!("{:?}", done.message);
    let s = e
        .service_action("caddy", ServiceAction::Start)
        .await
        .unwrap();
    assert_eq!(s.status, ServiceStatus::Running, "{s:?}");
    let client = bz_core::download::local_client();
    let mut body = String::new();
    for _ in 0..50 {
        if let Ok(r) = client
            .get("http://127.0.0.1:18080/")
            .header("Host", &site.domain)
            .send()
            .await
        {
            body = r.text().await.unwrap_or_default();
            if body.contains("hello") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    println!("GET http://{}:18080/ => {body}", site.domain);
    assert_eq!(body, "hello from bozkurt");
    // hot reload: unsecure → secure switches to an https block with tls internal
    e.site_action(&site.id, bz_core::models::SiteAction::Secure)
        .await
        .unwrap();
    let r = client
        .get("http://127.0.0.1:18080/")
        .header("Host", &site.domain)
        .send()
        .await
        .unwrap();
    println!(
        "after secure: {} {:?}",
        r.status(),
        r.headers().get("location")
    );
    assert_eq!(r.status().as_u16(), 308);
    e.shutdown().await;
}

/// Installs PHP (static-php.dev cli+fpm) and Caddy, serves a PHP site through the
/// php-fpm pool and checks the response.
#[cfg(unix)]
#[tokio::test]
#[ignore = "downloads PHP and Caddy, binds local ports"]
async fn php_site_via_fpm() {
    let t = tempfile::tempdir().unwrap();
    let e = engine(t.path()).await;
    e.patch_config(&serde_json::json!({"http_port": 18081, "https_port": 18444}))
        .await
        .unwrap();
    for job in [
        e.install_runtime(RuntimeKind::Php, "8.4"),
        e.install_service("caddy", None).unwrap(),
    ] {
        let done = e
            .jobs_manager()
            .wait(&job.job_id, Duration::from_secs(600))
            .await
            .unwrap();
        assert_eq!(done.status, JobStatus::Done, "{:?}", done.message);
        println!("{:?}", done.message);
    }
    let proj = t.path().join("phpinfo");
    std::fs::create_dir_all(proj.join("public")).unwrap();
    std::fs::write(
        proj.join("public/index.php"),
        "<?php echo 'php ', PHP_VERSION, ' ', php_sapi_name(), ' ', $_SERVER['REQUEST_URI'], ' ', getenv('APP_ENV');",
    )
    .unwrap();
    std::fs::write(proj.join("public/.env"), "SECRET=1").unwrap();
    let mut env = std::collections::BTreeMap::new();
    env.insert("APP_ENV".to_string(), "local".to_string());
    let site = e
        .create_site(&CreateSiteRequest {
            path: proj.to_string_lossy().into_owned(),
            https: Some(false),
            env: Some(env),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(site.kind, bz_core::models::SiteKind::Php);
    assert_eq!(site.root.as_deref(), Some("public"));
    let s = e
        .service_action("caddy", ServiceAction::Start)
        .await
        .unwrap();
    assert_eq!(s.status, ServiceStatus::Running, "{s:?}");
    let pool = e.service("php-fpm-8.4").await.unwrap();
    println!("pool: {pool:?}");
    assert_eq!(pool.status, ServiceStatus::Running);
    let client = bz_core::download::local_client();
    let mut body = String::new();
    for _ in 0..50 {
        if let Ok(r) = client
            .get("http://127.0.0.1:18081/some/route?x=1")
            .header("Host", &site.domain)
            .send()
            .await
        {
            body = r.text().await.unwrap_or_default();
            if body.starts_with("php ") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    println!("GET /some/route => {body}");
    assert!(body.starts_with("php 8.4."), "{body}");
    assert!(body.contains("fpm-fcgi /some/route?x=1 local"), "{body}");
    let r = client
        .get("http://127.0.0.1:18081/.env")
        .header("Host", &site.domain)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 404);
    e.shutdown().await;
}

/// Installs and starts Postgres, Redis (Valkey, Linux only) and Mailpit on free ports.
#[tokio::test]
#[ignore = "downloads database/service binaries"]
async fn data_services_start() {
    let t = tempfile::tempdir().unwrap();
    let e = engine(t.path()).await;
    let mut names = vec!["postgres", "mailpit"];
    if cfg!(target_os = "linux") {
        names.push("redis");
    }
    for name in &names {
        let port = bz_core::ports::free_port().unwrap();
        e.patch_service(
            name,
            &bz_core::models::ServicePatch {
                port: Some(port),
                autostart: None,
            },
        )
        .await
        .unwrap();
        let job = e.install_service(name, None).unwrap();
        let done = e
            .jobs_manager()
            .wait(&job.job_id, Duration::from_secs(600))
            .await
            .unwrap();
        assert_eq!(done.status, JobStatus::Done, "{name}: {:?}", done.message);
        let s = e.service_action(name, ServiceAction::Start).await.unwrap();
        println!("{name}: {:?} -> {s:?}", done.message);
        assert_eq!(
            s.status,
            ServiceStatus::Running,
            "{name}: {s:?} logs: {:?}",
            e.service_logs(name, 30).unwrap().lines
        );
        assert!(
            bz_core::ports::is_listening(port).await,
            "{name} not listening on {port}"
        );
    }
    let all = e.all_services_action(ServiceAction::Stop).await.unwrap();
    for s in all.iter().filter(|s| names.contains(&s.name.as_str())) {
        assert_eq!(s.status, ServiceStatus::Stopped, "{s:?}");
    }
}
