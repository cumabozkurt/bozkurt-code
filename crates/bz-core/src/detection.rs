//! Zero-config project detection (`POST /detect`): framework, site kind, document
//! root, runtime versions, dev command and a suggested domain.
//!
//! Dev commands may contain `$PORT`; the site manager substitutes the assigned port
//! before running them (works on every OS because substitution happens in Rust).

use std::path::Path;

use serde_json::Value;

use crate::models::{Detection, RuntimeKind, SiteKind};
use crate::runtimes::project_version;

/// Node package manager used by a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl PackageManager {
    /// Detect from lock files (npm when none).
    pub fn detect(dir: &Path) -> PackageManager {
        if dir.join("bun.lockb").exists() || dir.join("bun.lock").exists() {
            PackageManager::Bun
        } else if dir.join("pnpm-lock.yaml").exists() {
            PackageManager::Pnpm
        } else if dir.join("yarn.lock").exists() {
            PackageManager::Yarn
        } else {
            PackageManager::Npm
        }
    }

    /// Command running package script `script`, with extra args passed through.
    pub fn run(&self, script: &str, extra: &str) -> String {
        let base = match self {
            PackageManager::Npm => format!("npm run {script}"),
            PackageManager::Pnpm => format!("pnpm {script}"),
            PackageManager::Yarn => format!("yarn {script}"),
            PackageManager::Bun => format!("bun run {script}"),
        };
        if extra.is_empty() {
            base
        } else if *self == PackageManager::Npm {
            format!("{base} -- {extra}")
        } else {
            format!("{base} {extra}")
        }
    }
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

fn has_dep(pkg: &Value, name: &str) -> bool {
    ["dependencies", "devDependencies"]
        .iter()
        .any(|k| pkg.get(k).and_then(|d| d.get(name)).is_some())
}

fn has_script(pkg: &Value, name: &str) -> bool {
    pkg.get("scripts").and_then(|s| s.get(name)).is_some()
}

fn composer_requires(c: &Value, name: &str) -> bool {
    ["require", "require-dev"]
        .iter()
        .any(|k| c.get(k).and_then(|d| d.get(name)).is_some())
}

fn read_text(dir: &Path, f: &str) -> String {
    std::fs::read_to_string(dir.join(f))
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Python dependency declared in requirements*.txt / pyproject.toml / Pipfile.
fn python_dep(dir: &Path, name: &str) -> bool {
    let blob = [
        read_text(dir, "requirements.txt"),
        read_text(dir, "requirements-dev.txt"),
        read_text(dir, "pyproject.toml"),
        read_text(dir, "Pipfile"),
    ]
    .join("\n");
    blob.lines().any(|l| {
        let l = l.trim().trim_start_matches(['"', '\'']);
        l.starts_with(name)
            && l[name.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
    })
}

/// Turn a directory name into a DNS label (`My Shop_v2` → `my-shop-v2`).
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "site".into()
    } else {
        out.chars().take(63).collect()
    }
}

/// Detect what lives at `path`. Never fails: unknown → `static`.
pub fn detect(path: &Path, tld: &str) -> Detection {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "site".into());
    let mut d = Detection {
        path: path.to_string_lossy().into_owned(),
        framework: None,
        kind: SiteKind::Static,
        root: None,
        php: None,
        node: None,
        command: None,
        suggested_domain: format!("{}.{}", slugify(&name), tld),
    };
    let composer = read_json(&path.join("composer.json"));
    let pkg = read_json(&path.join("package.json"));
    let pm = PackageManager::detect(path);

    let set_php = |d: &mut Detection, fw: &str, root: Option<&str>| {
        d.framework = Some(fw.into());
        d.kind = SiteKind::Php;
        d.root = root.map(str::to_string);
    };

    // ---- PHP frameworks -----------------------------------------------------------
    let c = composer.clone().unwrap_or(Value::Null);
    let artisan = path.join("artisan").exists();
    if artisan
        && (composer_requires(&c, "laravel/framework") || path.join("public/index.php").exists())
    {
        set_php(&mut d, "laravel", Some("public"));
    } else if composer_requires(&c, "symfony/framework-bundle")
        || (path.join("bin/console").exists() && path.join("public/index.php").exists())
    {
        set_php(&mut d, "symfony", Some("public"));
    } else if path.join("wp-config.php").exists()
        || path.join("wp-login.php").exists()
        || path.join("wp-includes").is_dir()
    {
        set_php(&mut d, "wordpress", None);
    } else if path.join("web/wp").is_dir() && path.join("web/wp-config.php").exists() {
        // Bedrock layout
        set_php(&mut d, "wordpress", Some("web"));
    } else if path.join("web/core/lib/Drupal.php").exists() {
        set_php(&mut d, "drupal", Some("web"));
    } else if path.join("core/lib/Drupal.php").exists() {
        set_php(&mut d, "drupal", None);
    }

    // ---- JavaScript ---------------------------------------------------------------
    if d.kind == SiteKind::Static {
        if let Some(pkg) = &pkg {
            // `--host 127.0.0.1`: Vite's default `localhost` binds only `::1` on many
            // macOS / Node >= 17 setups (P1-16)
            let strict = "--host 127.0.0.1 --port $PORT --strictPort";
            let (fw, cmd): (Option<&str>, Option<String>) = if has_dep(pkg, "next") {
                (Some("nextjs"), Some(pm.run("dev", "--port $PORT")))
            } else if has_dep(pkg, "nuxt") || has_dep(pkg, "nuxt3") {
                (Some("nuxt"), Some(pm.run("dev", "--port $PORT")))
            } else if has_dep(pkg, "astro") {
                (
                    Some("astro"),
                    Some(pm.run("dev", "--host 127.0.0.1 --port $PORT")),
                )
            } else if has_dep(pkg, "@sveltejs/kit") {
                (Some("sveltekit"), Some(pm.run("dev", strict)))
            } else if has_dep(pkg, "@remix-run/dev") || has_dep(pkg, "@react-router/dev") {
                (Some("remix"), Some(pm.run("dev", "--port $PORT")))
            } else if has_dep(pkg, "vite") {
                (Some("vite"), Some(pm.run("dev", strict)))
            } else if has_dep(pkg, "express") || has_dep(pkg, "fastify") || has_dep(pkg, "koa") {
                let script = if has_script(pkg, "dev") {
                    "dev"
                } else {
                    "start"
                };
                (Some("express"), Some(pm.run(script, "")))
            } else if has_script(pkg, "dev") {
                (None, Some(pm.run("dev", "")))
            } else {
                (None, None)
            };
            if let Some(cmd) = cmd {
                d.kind = SiteKind::App;
                d.framework = fw.map(str::to_string);
                d.command = Some(cmd);
            }
        }
    }

    // ---- Python -------------------------------------------------------------------
    if d.kind == SiteKind::Static {
        if path.join("manage.py").exists() {
            d.kind = SiteKind::App;
            d.framework = Some("django".into());
            d.command = Some("python manage.py runserver 127.0.0.1:$PORT".into());
        } else if python_dep(path, "fastapi") {
            let module = ["main.py", "app.py", "app/main.py", "src/main.py"]
                .iter()
                .find(|f| path.join(f).exists())
                .map(|f| f.trim_end_matches(".py").replace('/', "."))
                .unwrap_or_else(|| "main".into());
            d.kind = SiteKind::App;
            d.framework = Some("fastapi".into());
            d.command = Some(format!(
                "python -m uvicorn {module}:app --host 127.0.0.1 --port $PORT --reload"
            ));
        } else if python_dep(path, "flask") {
            d.kind = SiteKind::App;
            d.framework = Some("flask".into());
            d.command = Some("python -m flask run --host 127.0.0.1 --port $PORT --debug".into());
        }
    }

    // ---- Go -----------------------------------------------------------------------
    if d.kind == SiteKind::Static && path.join("go.mod").exists() {
        d.kind = SiteKind::App;
        d.framework = Some("go".into());
        d.command = Some("go run .".into());
    }

    // ---- Generic PHP / static -----------------------------------------------------
    if d.kind == SiteKind::Static {
        if path.join("public/index.php").exists() {
            d.kind = SiteKind::Php;
            d.root = Some("public".into());
        } else if path.join("index.php").exists() || composer.is_some() {
            d.kind = SiteKind::Php;
        } else {
            for r in ["public", "dist", "build", "_site", "out"] {
                if path.join(r).join("index.html").exists() && !path.join("index.html").exists() {
                    d.root = Some(r.into());
                    break;
                }
            }
            if d.framework.is_none() {
                d.framework = Some("static".into());
            }
        }
    }

    if d.kind == SiteKind::Php {
        d.php = project_version(RuntimeKind::Php, path);
    }
    d.node = project_version(RuntimeKind::Node, path);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(files: &[(&str, &str)]) -> Detection {
        let t = tempfile::tempdir().unwrap();
        for (f, c) in files {
            let p = t.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        }
        detect(t.path(), "test")
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("My Shop_v2"), "my-shop-v2");
        assert_eq!(slugify("--x--"), "x");
        assert_eq!(slugify("çok"), "ok");
        assert_eq!(slugify("!!!"), "site");
    }

    #[test]
    fn laravel() {
        let d = det(&[
            ("artisan", ""),
            ("public/index.php", ""),
            (
                "composer.json",
                r#"{"require":{"php":"^8.2","laravel/framework":"^11.0"}}"#,
            ),
            (".nvmrc", "20"),
        ]);
        assert_eq!(d.framework.as_deref(), Some("laravel"));
        assert_eq!(d.kind, SiteKind::Php);
        assert_eq!(d.root.as_deref(), Some("public"));
        assert_eq!(d.php.as_deref(), Some("8.2"));
        assert_eq!(d.node.as_deref(), Some("20"));
    }

    #[test]
    fn symfony_wordpress_drupal() {
        let s = det(&[
            (
                "composer.json",
                r#"{"require":{"symfony/framework-bundle":"7.*"}}"#,
            ),
            ("public/index.php", ""),
        ]);
        assert_eq!(
            (s.framework.as_deref(), s.root.as_deref()),
            (Some("symfony"), Some("public"))
        );
        let w = det(&[("wp-config.php", ""), ("wp-includes/version.php", "")]);
        assert_eq!(
            (w.framework.as_deref(), w.root.as_deref()),
            (Some("wordpress"), None)
        );
        let b = det(&[("web/wp/wp-login.php", ""), ("web/wp-config.php", "")]);
        assert_eq!(
            (b.framework.as_deref(), b.root.as_deref()),
            (Some("wordpress"), Some("web"))
        );
        let dr = det(&[("web/core/lib/Drupal.php", ""), ("composer.json", "{}")]);
        assert_eq!(
            (dr.framework.as_deref(), dr.root.as_deref()),
            (Some("drupal"), Some("web"))
        );
    }

    #[test]
    fn generic_php_and_static() {
        let p = det(&[("index.php", "<?php")]);
        assert_eq!(
            (p.kind, p.framework.as_deref(), p.root.as_deref()),
            (SiteKind::Php, None, None)
        );
        let s = det(&[("dist/index.html", "")]);
        assert_eq!(
            (s.kind, s.root.as_deref()),
            (SiteKind::Static, Some("dist"))
        );
        assert_eq!(s.framework.as_deref(), Some("static"));
        let e = det(&[]);
        assert_eq!(e.kind, SiteKind::Static);
    }

    #[test]
    fn js_frameworks() {
        let n = det(&[(
            "package.json",
            r#"{"dependencies":{"next":"15"},"scripts":{"dev":"next dev"}}"#,
        )]);
        assert_eq!(n.framework.as_deref(), Some("nextjs"));
        assert_eq!(n.kind, SiteKind::App);
        assert_eq!(n.command.as_deref(), Some("npm run dev -- --port $PORT"));
        let v = det(&[
            (
                "package.json",
                r#"{"devDependencies":{"vite":"6"},"scripts":{"dev":"vite"}}"#,
            ),
            ("pnpm-lock.yaml", ""),
        ]);
        assert_eq!(v.framework.as_deref(), Some("vite"));
        assert_eq!(
            v.command.as_deref(),
            Some("pnpm dev --host 127.0.0.1 --port $PORT --strictPort")
        );
        let sk = det(&[
            (
                "package.json",
                r#"{"devDependencies":{"@sveltejs/kit":"2","vite":"6"}}"#,
            ),
            ("bun.lock", ""),
        ]);
        assert_eq!(sk.framework.as_deref(), Some("sveltekit"));
        assert!(sk.command.unwrap().starts_with("bun run dev"));
        let a = det(&[("package.json", r#"{"dependencies":{"astro":"5"}}"#)]);
        assert_eq!(a.framework.as_deref(), Some("astro"));
        assert_eq!(
            a.command.as_deref(),
            Some("npm run dev -- --host 127.0.0.1 --port $PORT")
        );
        let nu = det(&[
            ("package.json", r#"{"dependencies":{"nuxt":"3"}}"#),
            ("yarn.lock", ""),
        ]);
        assert_eq!(nu.command.as_deref(), Some("yarn dev --port $PORT"));
        let r = det(&[(
            "package.json",
            r#"{"devDependencies":{"@remix-run/dev":"2"}}"#,
        )]);
        assert_eq!(r.framework.as_deref(), Some("remix"));
        let x = det(&[(
            "package.json",
            r#"{"dependencies":{"express":"4"},"scripts":{"start":"node index.js"},"engines":{"node":">=20"}}"#,
        )]);
        assert_eq!(x.framework.as_deref(), Some("express"));
        assert_eq!(x.command.as_deref(), Some("npm run start"));
        assert_eq!(x.node.as_deref(), Some("20"));
    }

    #[test]
    fn python_and_go() {
        let dj = det(&[("manage.py", ""), ("requirements.txt", "Django==5.0")]);
        assert_eq!(dj.framework.as_deref(), Some("django"));
        assert!(dj.command.unwrap().contains("runserver 127.0.0.1:$PORT"));
        let fa = det(&[
            ("requirements.txt", "fastapi>=0.110\nuvicorn"),
            ("app/main.py", ""),
        ]);
        assert_eq!(fa.framework.as_deref(), Some("fastapi"));
        assert!(fa.command.unwrap().contains("uvicorn app.main:app"));
        let fl = det(&[("pyproject.toml", "dependencies = [\n  \"flask>=3\",\n]")]);
        assert_eq!(fl.framework.as_deref(), Some("flask"));
        let notflask = det(&[("requirements.txt", "flask-cors-helper-x")]);
        assert_ne!(notflask.framework.as_deref(), Some("flask"));
        let g = det(&[("go.mod", "module x\n\ngo 1.23\n")]);
        assert_eq!(
            (g.framework.as_deref(), g.kind),
            (Some("go"), SiteKind::App)
        );
        assert_eq!(g.command.as_deref(), Some("go run ."));
    }

    #[test]
    fn suggested_domain() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("My Shop");
        std::fs::create_dir_all(&p).unwrap();
        assert_eq!(detect(&p, "test").suggested_domain, "my-shop.test");
    }
}
