//! `POST/DELETE /share/{site_id}`: public URLs through a cloudflared quick tunnel
//! (`*.trycloudflare.com`, no account). cloudflared is downloaded on first use from
//! the GitHub releases of `cloudflare/cloudflared` (verified against the asset digest
//! when GitHub provides one) into `$BOZKURT_HOME/services/cloudflared/`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bz_core::download::{self, Checksum};
use bz_core::events::Event;
use bz_core::jobs::JobHandle;
use bz_core::supervisor::{ProcessSpec, RestartPolicy};
use bz_core::{CoreError, Engine, Result};

const RELEASES: &str = "https://api.github.com/repos/cloudflare/cloudflared/releases/latest";

/// Active shares: site id → public URL.
#[derive(Debug, Default)]
pub struct Shares {
    urls: Mutex<HashMap<String, String>>,
}

/// Supervisor process name of a share.
pub fn process_name(site_id: &str) -> String {
    format!("share-{site_id}")
}

/// Release asset for this platform.
pub fn asset_name(os: &str, goarch: &str) -> String {
    match os {
        "macos" => format!("cloudflared-darwin-{goarch}.tgz"),
        "windows" => format!("cloudflared-windows-{goarch}.exe"),
        _ => format!("cloudflared-linux-{goarch}"),
    }
}

/// Find the quick-tunnel URL in a cloudflared output line.
pub fn parse_tunnel_url(line: &str) -> Option<String> {
    let mut rest = line;
    while let Some(i) = rest.find("https://") {
        let tail = &rest[i..];
        let end = tail[8..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'))
            .map(|e| e + 8)
            .unwrap_or(tail.len());
        let url = &tail[..end];
        let host = &url[8..];
        if host.ends_with(".trycloudflare.com") && !host.starts_with("api.") {
            return Some(url.to_string());
        }
        rest = &tail[end..];
    }
    None
}

impl Shares {
    pub fn url(&self, site_id: &str) -> Option<String> {
        self.urls.lock().expect("shares").get(site_id).cloned()
    }

    fn binary(e: &Engine) -> PathBuf {
        e.paths()
            .service_dir("cloudflared")
            .join(format!("cloudflared{}", e.platform().exe()))
    }

    /// Download cloudflared if missing.
    pub async fn ensure_cloudflared(e: &Engine, job: &JobHandle) -> Result<PathBuf> {
        let bin = Self::binary(e);
        if bin.is_file() {
            return Ok(bin);
        }
        job.progress(0.05, Some("Downloading cloudflared"));
        let client = download::http_client();
        let rel: serde_json::Value = client
            .get(RELEASES)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let p = e.platform();
        let want = asset_name(p.os_str(), p.goarch());
        let asset = rel["assets"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["name"] == want.as_str()))
            .ok_or_else(|| CoreError::Upstream(format!("cloudflared release has no {want}")))?;
        let url = asset["browser_download_url"]
            .as_str()
            .ok_or_else(|| CoreError::Upstream("asset without url".into()))?;
        let checksum =
            Checksum::from_github_digest(asset["digest"].as_str()).unwrap_or(Checksum::None);
        let tmp = e
            .paths()
            .tmp()
            .join(format!("cloudflared-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&tmp)?;
        let file = tmp.join(&want);
        let j = job.clone();
        let res = e
            .download_verified(&client, url, &file, &checksum, move |d, t| {
                if let Some(t) = t.filter(|t| *t > 0) {
                    j.progress(0.05 + 0.6 * d as f64 / t as f64, None);
                }
            })
            .await;
        if let Err(err) = res {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(err);
        }
        let dir = bin.parent().expect("parent").to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let r = (|| -> Result<()> {
            if want.ends_with(".tgz") {
                let x = tmp.join("x");
                download::extract_archive(&file, &x)?;
                let found = std::fs::read_dir(&x)?
                    .flatten()
                    .map(|e| e.path())
                    .find(|p| p.file_name().is_some_and(|n| n == "cloudflared"))
                    .ok_or_else(|| CoreError::Upstream("cloudflared not in archive".into()))?;
                std::fs::rename(found, &bin)?;
            } else {
                std::fs::rename(&file, &bin)?;
            }
            bz_core::fsutil::make_executable(&bin)?;
            Ok(())
        })();
        let _ = std::fs::remove_dir_all(&tmp);
        r?;
        Ok(bin)
    }

    /// Start a quick tunnel for `site_id` and wait for its URL (job body).
    pub async fn start(&self, e: &Engine, site_id: &str, job: &JobHandle) -> Result<String> {
        let site = e.site(site_id)?;
        if let Some(u) = self.url(site_id) {
            if e.supervisor().is_active(&process_name(site_id)) {
                return Ok(u);
            }
        }
        let bin = Self::ensure_cloudflared(e, job).await?;
        let (http, https) = e.web_ports();
        if !e.supervisor().is_active("caddy") {
            return Err(CoreError::Conflict(
                "caddy is not running (POST /services/caddy/start)".into(),
            ));
        }
        let mut args: Vec<String> = vec!["tunnel".into(), "--no-autoupdate".into()];
        if site.https {
            args.extend([
                "--url".into(),
                format!("https://127.0.0.1:{https}"),
                "--no-tls-verify".into(),
                "--origin-server-name".into(),
                site.domain.clone(),
            ]);
        } else {
            args.extend(["--url".into(), format!("http://127.0.0.1:{http}")]);
        }
        args.extend(["--http-host-header".into(), site.domain.clone()]);
        let name = process_name(site_id);
        let mut spec = ProcessSpec::new(&name, &bin)
            .args(args)
            .restart(RestartPolicy::never());
        spec.log_file = Some(e.paths().log_file(&name));
        job.progress(0.7, Some("Opening tunnel"));
        let _ = e.supervisor().stop(&name).await;
        e.supervisor().start(spec).await?;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(url) = e
                .supervisor()
                .logs(&name, 500)
                .iter()
                .find_map(|l| parse_tunnel_url(l))
            {
                self.urls
                    .lock()
                    .expect("shares")
                    .insert(site_id.to_string(), url.clone());
                e.events().emit(Event::ShareUrl {
                    site_id: site_id.to_string(),
                    url: url.clone(),
                });
                return Ok(url);
            }
            if !e.supervisor().is_active(&name) || Instant::now() > deadline {
                let tail = e.supervisor().logs(&name, 5).join(" | ");
                let _ = e.supervisor().stop(&name).await;
                return Err(CoreError::Upstream(format!(
                    "cloudflared did not report a tunnel URL: {tail}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Close the tunnel of `site_id` (no error when none is open).
    pub async fn stop(&self, e: &Engine, site_id: &str) -> Result<()> {
        self.urls.lock().expect("shares").remove(site_id);
        e.supervisor().stop(&process_name(site_id)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_parsing() {
        let l = "2026-10-09T10:00:00Z INF |  https://brave-lion-sun-tree.trycloudflare.com                     |";
        assert_eq!(
            parse_tunnel_url(l).as_deref(),
            Some("https://brave-lion-sun-tree.trycloudflare.com")
        );
        assert_eq!(
            parse_tunnel_url("Requesting new quick Tunnel on https://api.trycloudflare.com..."),
            None
        );
        assert_eq!(asset_name("macos", "arm64"), "cloudflared-darwin-arm64.tgz");
        assert_eq!(asset_name("linux", "amd64"), "cloudflared-linux-amd64");
    }
}
