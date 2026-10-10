//! `GET/POST/DELETE /services/{engine}/databases` — list/create/drop databases through
//! the engine's own client binary (bundled `mariadb`/`mysql` or `psql`; for an
//! external MySQL/MariaDB the client on PATH).
//!
//! Connection: TCP 127.0.0.1:<service port>; MariaDB as `root` (bundled installs are
//! initialised with an empty root password), PostgreSQL as `postgres` (trust auth).
//! Credentials can be overridden in `config.toml`:
//! ```toml
//! [databases.mariadb]
//! user = "root"
//! password = "secret"
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use bz_core::models::ServiceStatus;
use bz_core::services::{ServiceKind, ServiceLayout};
use bz_core::{CoreError, Engine, Result};

/// Which client protocol to speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbEngine {
    Mysql,
    Postgres,
}

impl DbEngine {
    /// `mariadb` / `mysql` → Mysql, `postgres` / `postgresql` → Postgres.
    pub fn parse(name: &str) -> Result<(DbEngine, &'static str)> {
        match name {
            "mariadb" | "mysql" => Ok((DbEngine::Mysql, "mariadb")),
            "postgres" | "postgresql" | "pgsql" => Ok((DbEngine::Postgres, "postgres")),
            o => Err(CoreError::InvalidInput(format!(
                "`{o}` has no databases (use mariadb, mysql or postgres)"
            ))),
        }
    }
}

/// Database names we accept: `[A-Za-z0-9_-]{1,64}`.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(CoreError::InvalidInput(format!(
            "invalid database name `{name}` (letters, digits, _ and - only, max 64)"
        )));
    }
    Ok(())
}

const MYSQL_SYSTEM: [&str; 4] = ["information_schema", "performance_schema", "mysql", "sys"];

fn which(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for n in names {
            for cand in [dir.join(n), dir.join(format!("{n}.exe"))] {
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

fn client_binary(dist: &Path, names: &[&str]) -> Option<PathBuf> {
    let exe = if cfg!(windows) { ".exe" } else { "" };
    for n in names {
        let p = dist.join("bin").join(format!("{n}{exe}"));
        if p.is_file() {
            return Some(p);
        }
    }
    which(names)
}

struct Conn {
    engine: DbEngine,
    client: PathBuf,
    port: u16,
    user: String,
    password: Option<String>,
}

async fn connect(e: &Engine, name: &str) -> Result<Conn> {
    let (engine, svc_name) = DbEngine::parse(name)?;
    let svc = e.service(svc_name).await?;
    match svc.status {
        ServiceStatus::Running | ServiceStatus::External => {}
        ServiceStatus::NotInstalled => {
            return Err(CoreError::NotFound(format!(
                "{svc_name} is not installed (POST /services/{svc_name}/install)"
            )))
        }
        _ => {
            return Err(CoreError::Conflict(format!(
                "{svc_name} is not running (POST /services/{svc_name}/start)"
            )))
        }
    }
    let port = svc
        .port
        .ok_or_else(|| CoreError::Conflict(format!("{svc_name} has no port")))?;
    let kind = ServiceKind::parse(svc_name)?;
    let layout = ServiceLayout::new(e.paths(), &kind);
    let names: &[&str] = match engine {
        DbEngine::Mysql => &["mariadb", "mysql"],
        DbEngine::Postgres => &["psql"],
    };
    let client = client_binary(&layout.dist, names).ok_or_else(|| {
        CoreError::NotFound(format!(
            "no {} client found (install {svc_name} or put it on PATH)",
            names.join("/")
        ))
    })?;
    let creds = e
        .config()
        .extra
        .get("databases")
        .and_then(|d| d.get(svc_name))
        .cloned();
    let get = |k: &str| {
        creds
            .as_ref()
            .and_then(|c| c.get(k))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    Ok(Conn {
        engine,
        client,
        port,
        user: get("user").unwrap_or_else(|| match engine {
            DbEngine::Mysql => "root".into(),
            DbEngine::Postgres => "postgres".into(),
        }),
        password: get("password"),
    })
}

async fn run(c: &Conn, sql: &str) -> Result<Vec<String>> {
    let mut cmd = tokio::process::Command::new(&c.client);
    match c.engine {
        DbEngine::Mysql => {
            cmd.args([
                "--no-defaults",
                "--protocol=TCP",
                "-h",
                "127.0.0.1",
                "-P",
                &c.port.to_string(),
                "-u",
                &c.user,
                "--batch",
                "--skip-column-names",
                "-e",
                sql,
            ]);
            if let Some(p) = &c.password {
                cmd.env("MYSQL_PWD", p);
            }
        }
        DbEngine::Postgres => {
            cmd.args([
                "-h",
                "127.0.0.1",
                "-p",
                &c.port.to_string(),
                "-U",
                &c.user,
                "-d",
                "postgres",
                "-X",
                "-A",
                "-t",
                "-v",
                "ON_ERROR_STOP=1",
                "-c",
                sql,
            ]);
            if let Some(p) = &c.password {
                cmd.env("PGPASSWORD", p);
            }
        }
    }
    cmd.stdin(std::process::Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(30), cmd.output())
        .await
        .map_err(|_| CoreError::Upstream(format!("{} timed out", c.client.display())))??;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(CoreError::Upstream(format!(
            "{} failed: {err}",
            c.client
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// User databases of `engine`.
pub async fn list(e: &Engine, engine: &str) -> Result<Vec<String>> {
    let c = connect(e, engine).await?;
    let sql = match c.engine {
        DbEngine::Mysql => "SHOW DATABASES",
        DbEngine::Postgres => {
            "SELECT datname FROM pg_database WHERE NOT datistemplate AND datname <> 'postgres' ORDER BY 1"
        }
    };
    let mut v = run(&c, sql).await?;
    if c.engine == DbEngine::Mysql {
        v.retain(|d| !MYSQL_SYSTEM.contains(&d.as_str()));
    }
    Ok(v)
}

/// Create database `name` (UTF-8).
pub async fn create(e: &Engine, engine: &str, name: &str) -> Result<()> {
    validate_name(name)?;
    let c = connect(e, engine).await?;
    let sql = match c.engine {
        DbEngine::Mysql => {
            format!("CREATE DATABASE `{name}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci")
        }
        DbEngine::Postgres => format!("CREATE DATABASE \"{name}\" ENCODING 'UTF8'"),
    };
    run(&c, &sql).await.map(|_| ())
}

/// Drop database `name`.
pub async fn drop(e: &Engine, engine: &str, name: &str) -> Result<()> {
    validate_name(name)?;
    let c = connect(e, engine).await?;
    if c.engine == DbEngine::Mysql && MYSQL_SYSTEM.contains(&name) {
        return Err(CoreError::InvalidInput(format!(
            "refusing to drop system database `{name}`"
        )));
    }
    let sql = match c.engine {
        DbEngine::Mysql => format!("DROP DATABASE `{name}`"),
        DbEngine::Postgres => format!("DROP DATABASE \"{name}\""),
    };
    run(&c, &sql).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(validate_name("shop_dev-1").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("a`b").is_err());
        assert!(validate_name("x; DROP").is_err());
        assert_eq!(DbEngine::parse("mysql").unwrap().1, "mariadb");
        assert!(DbEngine::parse("redis").is_err());
    }
}
