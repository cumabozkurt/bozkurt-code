//! Streaming downloads with progress + SHA-256 verification, and archive extraction
//! (tar.gz / tar.xz / zip / bare binary) into a staging dir followed by an atomic rename.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CoreError, Result};

/// User agent sent to every upstream.
pub const USER_AGENT: &str = concat!("bozkurt-code/", env!("CARGO_PKG_VERSION"));

/// Build the shared HTTP client.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(120))
        .build()
        .expect("reqwest client")
}

/// Client for talking to local endpoints (Caddy admin API, health checks): never uses
/// `HTTP(S)_PROXY`, which would otherwise route 127.0.0.1 through a corporate proxy.
pub fn local_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("reqwest client")
}

/// Where the expected SHA-256 of an asset comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Checksum {
    /// Known hex digest (from an upstream JSON index or GitHub's asset `digest`).
    Sha256 { hex: String },
    /// A `SHASUMS256.txt`-style file (or a single-hash `.sha256` file) to fetch at install
    /// time; `filename` is the entry to look up.
    SumsFile { url: String, filename: String },
    /// Upstream publishes no checksum (the download is still served over HTTPS).
    None,
}

impl Checksum {
    pub fn sha256(hex: impl Into<String>) -> Checksum {
        Checksum::Sha256 {
            hex: hex.into().to_ascii_lowercase(),
        }
    }

    /// Parse GitHub's `"sha256:<hex>"` asset digest.
    pub fn from_github_digest(d: Option<&str>) -> Option<Checksum> {
        d.and_then(|d| d.strip_prefix("sha256:"))
            .map(Checksum::sha256)
    }

    /// Resolve to a concrete hex digest (fetching a sums file if needed).
    pub async fn resolve(&self, client: &reqwest::Client) -> Result<Option<String>> {
        match self {
            Checksum::Sha256 { hex } => Ok(Some(hex.clone())),
            Checksum::None => Ok(None),
            Checksum::SumsFile { url, filename } => {
                let text = client
                    .get(url)
                    .send()
                    .await?
                    .error_for_status()?
                    .text()
                    .await?;
                parse_sums(&text, filename)
                    .map(Some)
                    .ok_or_else(|| CoreError::Upstream(format!("{filename} not listed in {url}")))
            }
        }
    }
}

/// Find `filename`'s digest in a sums file. Accepts `hash  name`, `hash *name`,
/// `hash  ./dir/name`, and a file holding just one bare hash.
pub fn parse_sums(text: &str, filename: &str) -> Option<String> {
    let all: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    for l in &all {
        let mut parts = l.split_whitespace();
        let (Some(hash), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        let name = name.trim_start_matches('*');
        let base = name.rsplit('/').next().unwrap_or(name);
        if (name == filename || base == filename) && is_hex64(hash) {
            return Some(hash.to_ascii_lowercase());
        }
    }
    if all.len() == 1 {
        let h = all[0].split_whitespace().next().unwrap_or("");
        if is_hex64(h) {
            return Some(h.to_ascii_lowercase());
        }
    }
    None
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// SHA-256 of a file on disk.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex::encode(h.finalize()))
}

/// How a download was verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verification {
    /// Against a checksum published by the upstream.
    Upstream,
    /// Against the sha256 pinned on the first (explicitly allowed) download.
    Pinned,
    /// Not verified: first download, allowed by `allow_unverified`; its sha256 is now pinned.
    FirstUse,
}

fn file_name(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

/// The error for a download that cannot be verified.
pub fn unverified_error(url: &str) -> CoreError {
    CoreError::Unverified(format!(
        "{} has no published checksum and no sha256 pinned from an earlier install, so it \
         cannot be verified. Retry with `allow_unverified` (`bz runtime install --allow-unverified`, \
         or `allow_unverified = true` in config.toml) to trust it once; its sha256 is then \
         pinned and later downloads must match",
        file_name(url)
    ))
}

/// Stream `url` into `dest`, calling `progress(downloaded, total)` as bytes arrive, and
/// verify against `checksum`. On mismatch `dest` is deleted. Returns the actual digest.
/// Fails closed: `Checksum::None` is refused before anything is fetched (use
/// [`download_tofu`] for upstreams without checksums).
pub async fn download(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    checksum: &Checksum,
    progress: impl FnMut(u64, Option<u64>) + Send,
) -> Result<String> {
    if *checksum == Checksum::None {
        return Err(unverified_error(url));
    }
    let expected = checksum.resolve(client).await?;
    download_expect(client, url, dest, expected, progress).await
}

/// [`download`] with trust on first use for `Checksum::None` (P1-1): a sha256 pinned in
/// `state.json` (`download_pins`, keyed by URL) is enforced; without a pin the download is
/// refused unless `allow_unverified`, in which case the digest is pinned afterwards.
pub async fn download_tofu(
    client: &reqwest::Client,
    state: &crate::state::StateStore,
    url: &str,
    dest: &Path,
    checksum: &Checksum,
    allow_unverified: bool,
    progress: impl FnMut(u64, Option<u64>) + Send,
) -> Result<(String, Verification)> {
    if *checksum != Checksum::None {
        let d = download(client, url, dest, checksum, progress).await?;
        return Ok((d, Verification::Upstream));
    }
    if let Some(pin) = state.read(|s| s.download_pins.get(url).cloned()) {
        return match download_expect(client, url, dest, Some(pin.clone()), progress).await {
            Ok(d) => Ok((d, Verification::Pinned)),
            Err(CoreError::Checksum { file, actual, .. }) => Err(CoreError::Checksum {
                file: format!("{file} (differs from the sha256 pinned on first download)"),
                expected: pin,
                actual,
            }),
            Err(e) => Err(e),
        };
    }
    if !allow_unverified {
        return Err(unverified_error(url));
    }
    let d = download_expect(client, url, dest, None, progress).await?;
    tracing::warn!("unverified download {url} (allow_unverified); pinned sha256={d}");
    state.update(|s| {
        s.download_pins.insert(url.to_string(), d.clone());
        Ok(())
    })?;
    Ok((d, Verification::FirstUse))
}

async fn download_expect(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected: Option<String>,
    mut progress: impl FnMut(u64, Option<u64>) + Send,
) -> Result<String> {
    if let Some(p) = dest.parent() {
        tokio::fs::create_dir_all(p).await?;
    }
    let resp = client.get(url).send().await?;
    let resp = resp
        .error_for_status()
        .map_err(|e| CoreError::Upstream(format!("GET {url}: {e}")))?;
    let total = resp.content_length();
    let mut stream = resp.bytes_stream();
    let mut file = std::fs::File::create(dest)?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    progress(0, total);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| CoreError::Upstream(format!("GET {url}: {e}")))?;
        hasher.update(&chunk);
        file.write_all(&chunk)?;
        done += chunk.len() as u64;
        progress(done, total);
    }
    file.flush()?;
    drop(file);
    let actual = hex::encode(hasher.finalize());
    if let Some(exp) = expected {
        if exp != actual {
            let _ = std::fs::remove_file(dest);
            return Err(CoreError::Checksum {
                file: url.rsplit('/').next().unwrap_or(url).to_string(),
                expected: exp,
                actual,
            });
        }
    }
    Ok(actual)
}

/// Archive container format, derived from the file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    TarGz,
    TarXz,
    Zip,
    /// Not an archive: copied as-is (e.g. `composer.phar`).
    Raw,
}

impl ArchiveKind {
    pub fn from_name(name: &str) -> ArchiveKind {
        let n = name.to_ascii_lowercase();
        if n.ends_with(".tar.gz") || n.ends_with(".tgz") {
            ArchiveKind::TarGz
        } else if n.ends_with(".tar.xz") || n.ends_with(".txz") {
            ArchiveKind::TarXz
        } else if n.ends_with(".zip") {
            ArchiveKind::Zip
        } else {
            ArchiveKind::Raw
        }
    }
}

/// Extract one archive into `dir` (which must exist). Paths escaping `dir` are rejected.
pub fn extract_archive(archive: &Path, dir: &Path) -> Result<()> {
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let f = std::fs::File::open(archive)?;
    let r = std::io::BufReader::new(f);
    match ArchiveKind::from_name(&name) {
        ArchiveKind::TarGz => unpack_tar(tar::Archive::new(flate2::read::GzDecoder::new(r)), dir),
        ArchiveKind::TarXz => unpack_tar(tar::Archive::new(xz2::read::XzDecoder::new(r)), dir),
        ArchiveKind::Zip => unpack_zip(archive, dir),
        ArchiveKind::Raw => {
            std::fs::copy(archive, dir.join(&name))?;
            Ok(())
        }
    }
}

fn unpack_tar<R: std::io::Read>(mut a: tar::Archive<R>, dir: &Path) -> Result<()> {
    a.set_preserve_permissions(true);
    a.set_preserve_mtime(true);
    a.set_overwrite(true);
    for entry in a.entries()? {
        let mut e = entry?;
        // unpack_in refuses paths that escape `dir` (returns Ok(false)).
        if !e.unpack_in(dir)? {
            return Err(CoreError::InvalidInput(format!(
                "archive entry escapes target: {:?}",
                e.path().ok()
            )));
        }
    }
    Ok(())
}

fn unpack_zip(path: &Path, dir: &Path) -> Result<()> {
    let f = std::fs::File::open(path)?;
    let mut z = zip::ZipArchive::new(f)
        .map_err(|e| CoreError::InvalidInput(format!("bad zip {path:?}: {e}")))?;
    for i in 0..z.len() {
        let mut file = z
            .by_index(i)
            .map_err(|e| CoreError::InvalidInput(format!("bad zip entry: {e}")))?;
        let Some(rel) = file.enclosed_name() else {
            return Err(CoreError::InvalidInput(format!(
                "zip entry escapes target: {}",
                file.name()
            )));
        };
        let out = dir.join(rel);
        if file.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut w = std::fs::File::create(&out)?;
        std::io::copy(&mut file, &mut w)?;
        #[cfg(unix)]
        if let Some(mode) = file.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o7777))?;
        }
    }
    Ok(())
}

/// If `dir` contains exactly one entry and it is a directory, move its contents up one
/// level (`node-v22.1.0-linux-x64/bin/node` → `bin/node`).
pub fn flatten_single_dir(dir: &Path) -> Result<()> {
    let entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    if entries.len() != 1 || !entries[0].file_type()?.is_dir() {
        return Ok(());
    }
    let inner = entries[0].path();
    let tmp = dir.join(format!(".flatten-{}", uuid::Uuid::new_v4().simple()));
    std::fs::rename(&inner, &tmp)?;
    for e in std::fs::read_dir(&tmp)? {
        let e = e?;
        std::fs::rename(e.path(), dir.join(e.file_name()))?;
    }
    std::fs::remove_dir(&tmp)?;
    Ok(())
}

/// Extract `archives` (in order, into one tree) to a staging directory next to `dest`,
/// optionally flatten a single top-level dir, run `post` on the staging dir, then
/// atomically rename the staging dir to `dest`. `dest` must not exist yet unless
/// `replace` is set (then the old tree is moved aside and removed after the swap).
pub fn install_tree(
    archives: &[PathBuf],
    dest: &Path,
    flatten: bool,
    replace: bool,
    post: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let parent = dest
        .parent()
        .ok_or_else(|| CoreError::InvalidInput(format!("bad destination {dest:?}")))?;
    std::fs::create_dir_all(parent)?;
    if dest.exists() && !replace {
        return Err(CoreError::Conflict(format!("{dest:?} already exists")));
    }
    let staging = parent.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&staging)?;
    let res = (|| -> Result<()> {
        for a in archives {
            let sub = staging.join(format!(".part-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&sub)?;
            extract_archive(a, &sub)?;
            if flatten {
                flatten_single_dir(&sub)?;
            }
            merge_into(&sub, &staging)?;
            std::fs::remove_dir_all(&sub)?;
        }
        post(&staging)
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    let old = if dest.exists() {
        let aside = parent.join(format!(".old-{}", uuid::Uuid::new_v4().simple()));
        std::fs::rename(dest, &aside)?;
        Some(aside)
    } else {
        None
    };
    if let Err(e) = std::fs::rename(&staging, dest) {
        if let Some(o) = &old {
            let _ = std::fs::rename(o, dest);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e.into());
    }
    if let Some(o) = old {
        let _ = std::fs::remove_dir_all(o);
    }
    Ok(())
}

/// Move every entry of `from` into `to` (overwriting files, merging directories).
fn merge_into(from: &Path, to: &Path) -> Result<()> {
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() && target.is_dir() {
            merge_into(&e.path(), &target)?;
        } else {
            if target.exists() {
                if target.is_dir() {
                    std::fs::remove_dir_all(&target)?;
                } else {
                    std::fs::remove_file(&target)?;
                }
            }
            std::fs::rename(e.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_targz(path: &Path, files: &[(&str, &[u8], u32)]) {
        let f = std::fs::File::create(path).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        for (name, data, mode) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(*mode);
            h.set_cksum();
            b.append_data(&mut h, name, *data).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap();
    }

    fn make_tarxz(path: &Path, files: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let xz = xz2::write::XzEncoder::new(f, 1);
        let mut b = tar::Builder::new(xz);
        for (name, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, name, *data).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap();
    }

    fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts: zip::write::SimpleFileOptions =
            zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        for (name, data) in files {
            z.start_file(*name, opts).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn sums_parsing() {
        let h = "a".repeat(64);
        let txt = format!(
            "{h}  node-v22.1.0-linux-x64.tar.xz\n{}  other.zip\n",
            "b".repeat(64)
        );
        assert_eq!(
            parse_sums(&txt, "node-v22.1.0-linux-x64.tar.xz"),
            Some(h.clone())
        );
        assert_eq!(parse_sums(&txt, "other.zip"), Some("b".repeat(64)));
        assert_eq!(parse_sums(&txt, "missing"), None);
        let single = format!("{h}  postgresql-18.tar.gz\n");
        assert_eq!(parse_sums(&single, "postgresql-18.tar.gz"), Some(h.clone()));
        assert_eq!(parse_sums(&format!("{h}\n"), "x"), Some(h.clone()));
        assert_eq!(parse_sums(&format!("{h} *./dir/x.zip"), "x.zip"), Some(h));
        assert_eq!(
            Checksum::from_github_digest(Some("sha256:ABC")),
            Some(Checksum::sha256("abc"))
        );
    }

    #[test]
    fn kinds() {
        assert_eq!(ArchiveKind::from_name("a.tar.gz"), ArchiveKind::TarGz);
        assert_eq!(ArchiveKind::from_name("a.tar.xz"), ArchiveKind::TarXz);
        assert_eq!(ArchiveKind::from_name("a.ZIP"), ArchiveKind::Zip);
        assert_eq!(ArchiveKind::from_name("composer.phar"), ArchiveKind::Raw);
    }

    #[test]
    fn install_tree_flattens_merges_and_is_atomic() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("node-v1-linux-x64.tar.gz");
        make_targz(
            &a,
            &[
                ("node-v1/bin/node", b"#!/bin/sh\necho v1\n", 0o755),
                ("node-v1/README", b"hi", 0o644),
            ],
        );
        let b = t.path().join("extra.tar.xz");
        make_tarxz(&b, &[("lib/x.txt", b"xz!"), ("XZNOTE", b"n")]);
        let z = t.path().join("z.zip");
        make_zip(&z, &[("tool/run", b"zip!"), ("ZIPNOTE", b"n")]);
        let dest = t.path().join("rt/node/1.0.0");
        install_tree(&[a.clone(), b, z], &dest, true, false, |s| {
            std::fs::write(s.join("post"), "ok")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("README")).unwrap(), "hi");
        assert_eq!(
            std::fs::read_to_string(dest.join("lib/x.txt")).unwrap(),
            "xz!"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("tool/run")).unwrap(),
            "zip!"
        );
        assert!(dest.join("post").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(dest.join("bin/node"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(m & 0o111, 0o111);
            let m = std::fs::metadata(dest.join("tool/run"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(m & 0o111, 0o111);
        }
        // second install without replace conflicts; failing post leaves no trace
        assert!(install_tree(std::slice::from_ref(&a), &dest, true, false, |_| Ok(())).is_err());
        let dest2 = t.path().join("rt/node/2.0.0");
        let r = install_tree(&[a], &dest2, true, false, |_| {
            Err(CoreError::InvalidInput("nope".into()))
        });
        assert!(r.is_err());
        assert!(!dest2.exists());
        let leftovers: Vec<_> = std::fs::read_dir(t.path().join("rt/node"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, vec!["1.0.0"]);
    }

    #[test]
    fn tar_path_traversal_rejected() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("evil.tar.gz");
        // tar::Builder refuses `..` paths, so craft the header name manually.
        let f = std::fs::File::create(&a).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        let mut h = tar::Header::new_old();
        h.as_old_mut().name[..10].copy_from_slice(b"../evil.sh");
        h.set_size(1);
        h.set_mode(0o644);
        h.set_cksum();
        b.append(&h, &b"x"[..]).unwrap();
        b.into_inner().unwrap().finish().unwrap();
        let out = t.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        assert!(extract_archive(&a, &out).is_err());
        assert!(!t.path().join("evil.sh").exists());
    }

    /// Serve `body` (shared, swappable) on 127.0.0.1 for every request.
    async fn serve(body: std::sync::Arc<std::sync::Mutex<Vec<u8>>>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                let b = body.lock().unwrap().clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = s.read(&mut buf).await;
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        b.len()
                    );
                    let _ = s.write_all(head.as_bytes()).await;
                    let _ = s.write_all(&b).await;
                    let _ = s.shutdown().await;
                });
            }
        });
        format!("http://{addr}/php-8.4.1-cli-linux-x86_64.tar.gz")
    }

    #[tokio::test]
    async fn unverified_downloads_fail_closed_and_are_pinned_on_first_use() {
        let t = tempfile::tempdir().unwrap();
        let state = crate::state::StateStore::open(&t.path().join("state.json")).unwrap();
        let body = std::sync::Arc::new(std::sync::Mutex::new(b"first build".to_vec()));
        let url = serve(body.clone()).await;
        let client = local_client();
        let dest = t.path().join("a.tar.gz");

        // plain download(): None is refused before anything is fetched
        let e = download(&client, &url, &dest, &Checksum::None, |_, _| {})
            .await
            .unwrap_err();
        assert_eq!(e.code(), "unverified_download");
        assert!(!dest.exists());

        // no pin, not allowed → refused with a clear message, nothing pinned
        let e = download_tofu(
            &client,
            &state,
            &url,
            &dest,
            &Checksum::None,
            false,
            |_, _| {},
        )
        .await
        .unwrap_err();
        assert_eq!(e.code(), "unverified_download");
        assert!(e.to_string().contains("allow-unverified"), "{e}");
        assert!(state.get().download_pins.is_empty());

        // allowed → downloaded and pinned
        let (d, v) = download_tofu(
            &client,
            &state,
            &url,
            &dest,
            &Checksum::None,
            true,
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(v, Verification::FirstUse);
        assert_eq!(state.get().download_pins.get(&url), Some(&d));
        let reopened = crate::state::StateStore::open(&t.path().join("state.json")).unwrap();
        assert_eq!(reopened.get().download_pins.get(&url), Some(&d));

        // later downloads are verified against the pin, even without the flag
        let (d2, v) = download_tofu(
            &client,
            &state,
            &url,
            &dest,
            &Checksum::None,
            false,
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!((d2.as_str(), v), (d.as_str(), Verification::Pinned));

        // upstream swaps the file → refused (also with the flag), pin unchanged
        *body.lock().unwrap() = b"tampered build".to_vec();
        for allow in [false, true] {
            let e = download_tofu(
                &client,
                &state,
                &url,
                &dest,
                &Checksum::None,
                allow,
                |_, _| {},
            )
            .await
            .unwrap_err();
            assert_eq!(e.code(), "checksum_mismatch");
            assert!(e.to_string().contains("pinned"), "{e}");
            assert!(!dest.exists());
        }
        assert_eq!(state.get().download_pins.get(&url), Some(&d));

        // a published checksum still wins and needs no pin
        let good = hex::encode(Sha256::digest(b"tampered build"));
        let (_, v) = download_tofu(
            &client,
            &state,
            &url,
            &dest,
            &Checksum::sha256(good),
            false,
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(v, Verification::Upstream);
    }

    #[test]
    fn sha256_of_file() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("f");
        std::fs::write(&p, "abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
