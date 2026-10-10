//! Tool catalog (`docs/MCP.md#tools`): names, tiers, JSON schemas, annotations.

use bz_core::models::RuntimeKind;
use serde_json::{json, Map, Value};

/// Safety tier (`docs/MCP.md#safety-tiers`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Read,
    Change,
    Destructive,
    System,
}

impl Tier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Read => "read",
            Tier::Change => "change",
            Tier::Destructive => "destructive",
            Tier::System => "system",
        }
    }

    /// Needs `confirm: true` (otherwise a preview is returned).
    pub fn needs_confirm(&self) -> bool {
        matches!(self, Tier::Destructive | Tier::System)
    }
}

#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub title: String,
    pub description: String,
    pub tier: Tier,
    pub input_schema: Value,
    /// Reaches outside the machine (public tunnel).
    pub open_world: bool,
    pub idempotent: bool,
}

impl ToolDef {
    /// JSON for `tools/list`.
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "title": self.title,
            "description": format!("[tier: {}] {}", self.tier.as_str(), self.description),
            "inputSchema": self.input_schema,
            "annotations": {
                "title": self.title,
                "readOnlyHint": self.tier == Tier::Read,
                "destructiveHint": self.tier.needs_confirm(),
                "idempotentHint": self.idempotent,
                "openWorldHint": self.open_world,
            }
        })
    }
}

fn s(desc: &str) -> Value {
    json!({ "type": "string", "description": desc })
}
fn b(desc: &str) -> Value {
    json!({ "type": "boolean", "description": desc })
}
fn port(desc: &str) -> Value {
    json!({ "type": "integer", "minimum": 1, "maximum": 65535, "description": desc })
}
fn confirm() -> Value {
    b("Must be true to actually perform the action. Without it the tool only returns a preview of what would change; show it to the user and ask first.")
}
fn wait() -> Value {
    json!({
        "type": ["boolean", "integer"],
        "description": "Wait for the job to finish: true (up to 120 s) or a number of seconds (max 600). Default: return the job_id at once; poll it with job_get."
    })
}
const SITE: &str = "Site id, domain (shop.test) or bare name (shop)";

fn obj(props: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let mut p = Map::new();
    for (k, v) in props {
        p.insert(k.to_string(), v);
    }
    json!({
        "type": "object",
        "properties": Value::Object(p),
        "required": required,
        "additionalProperties": false
    })
}

fn site_fields() -> Vec<(&'static str, Value)> {
    vec![
        (
            "domain",
            s("Domain, e.g. shop.test (a bare name gets the configured TLD)"),
        ),
        (
            "kind",
            json!({ "type": "string", "enum": ["php", "proxy", "static", "app"], "description": "How the site is served" }),
        ),
        (
            "root",
            s("Document root relative to the project (e.g. public)"),
        ),
        ("php", s("PHP version (partial allowed, e.g. 8.3)")),
        ("node", s("Node version for app sites")),
        ("proxy_port", port("Port to reverse-proxy to (kind proxy)")),
        (
            "command",
            s("Dev command BOZKURT runs with PORT set (kind app)"),
        ),
        (
            "https",
            b("Serve over HTTPS with a locally trusted certificate"),
        ),
        ("wildcard", b("Also answer *.domain")),
        (
            "aliases",
            json!({ "type": "array", "items": { "type": "string" }, "description": "Extra domains" }),
        ),
        (
            "env",
            json!({ "type": "object", "additionalProperties": { "type": "string" }, "description": "Environment variables" }),
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
fn def(
    name: &str,
    title: &str,
    tier: Tier,
    desc: &str,
    schema: Value,
    idempotent: bool,
) -> ToolDef {
    ToolDef {
        name: name.into(),
        title: title.into(),
        description: desc.into(),
        tier,
        input_schema: schema,
        open_world: false,
        idempotent,
    }
}

/// Every tool, in documentation order (55).
pub fn all() -> Vec<ToolDef> {
    use Tier::*;
    let mut v = vec![
        def("detect_project", "Detect project", Read,
            "Detect framework, site kind, document root and required PHP/Node version of a project directory.",
            obj(vec![("path", s("Absolute project directory"))], &["path"]), true),
        def("sites_list", "List sites", Read, "List all local sites.", obj(vec![], &[]), true),
        def("sites_get", "Get site", Read, "Get one site.", obj(vec![("site", s(SITE))], &["site"]), true),
        def("sites_logs", "Site logs", Read, "Recent log lines of a site (app output / access errors).",
            obj(vec![("site", s(SITE)), ("lines", json!({"type":"integer","minimum":1,"maximum":5000,"default":200}))], &["site"]), true),
    ];
    let mut create = vec![("path", s("Absolute project directory"))];
    create.extend(site_fields());
    create.push(("confirm", confirm()));
    v.push(def("sites_create", "Create site", Change,
        "Link a project directory as a local site. Fields left out are filled by project detection. Setting `command` or `env` returns a preview unless confirm: true. Undo with sites_delete.",
        obj(create, &["path"]), false));
    let mut update = vec![("site", s(SITE)), ("path", s("New project directory"))];
    update.extend(site_fields());
    v.push(def(
        "sites_update",
        "Update site",
        Change,
        "Change fields of an existing site (partial update). Changing `command` or `env` returns a preview unless confirm: true.",
        obj({ update.push(("confirm", confirm())); update }, &["site"]),
        true,
    ));
    for (a, t, d) in [
        ("start", "Start app site", "Start the dev command of an app site. Returns a preview showing the exact shell command unless confirm: true (once per command per session)."),
        ("stop", "Stop app site", "Stop the dev command of an app site."),
        ("restart", "Restart app site", "Restart the dev command of an app site. Returns a preview showing the exact shell command unless confirm: true (once per command per session)."),
        ("secure", "Enable HTTPS", "Serve the site over HTTPS with a locally trusted certificate. Undo with sites_unsecure."),
        ("unsecure", "Disable HTTPS", "Serve the site over plain HTTP. Undo with sites_secure."),
    ] {
        let mut props = vec![("site", s(SITE))];
        if matches!(a, "start" | "restart") {
            props.push(("confirm", confirm()));
        }
        v.push(def(&format!("sites_{a}"), t, Change, d, obj(props, &["site"]), true));
    }
    v.push(def(
        "sites_delete",
        "Delete site",
        Destructive,
        "Remove a site, its certificate and Caddy entry. Project files are NOT deleted.",
        obj(vec![("site", s(SITE)), ("confirm", confirm())], &["site"]),
        true,
    ));

    for k in RuntimeKind::ALL {
        let n = k.as_str();
        let pretty = match k {
            RuntimeKind::Php => "PHP",
            RuntimeKind::Node => "Node.js",
            RuntimeKind::Python => "Python",
            RuntimeKind::Bun => "Bun",
            RuntimeKind::Go => "Go",
        };
        v.push(def(&format!("{n}_list"), &format!("List {pretty} versions"), Read,
            &format!("List {pretty} versions. Installed only unless available=true (live upstream index)."),
            obj(vec![("available", b("Include versions that can be installed (default false)")),
                     ("refresh", b("Bypass the 6 h index cache"))], &[]), true));
        v.push(def(&format!("{n}_active"), &format!("Active {pretty}"), Read,
            &format!("The active {pretty} version and installed versions; with path, also what that project requires and which installed version satisfies it."),
            obj(vec![("path", s("Project directory to resolve for"))], &[]), true));
        v.push(def(&format!("{n}_install"), &format!("Install {pretty}"), Change,
            &format!("Install a {pretty} version (partial, lts or latest allowed). Long-running: returns a job_id unless wait is set. Undo with {n}_uninstall."),
            obj(vec![("version", s("Version: 8.3, 22, 3.12, lts, latest…")), ("wait", wait())], &["version"]), true));
        v.push(def(
            &format!("{n}_use"),
            &format!("Use {pretty}"),
            Change,
            &format!("Make an installed {pretty} version the default (shims on PATH)."),
            obj(
                vec![("version", s("Installed version (partial allowed)"))],
                &["version"],
            ),
            true,
        ));
        v.push(def(
            &format!("{n}_uninstall"),
            &format!("Uninstall {pretty}"),
            Destructive,
            &format!("Delete an installed {pretty} version."),
            obj(
                vec![
                    ("version", s("Installed version (partial allowed)")),
                    ("confirm", confirm()),
                ],
                &["version"],
            ),
            true,
        ));
    }

    let svc = || {
        s("Service: caddy, dns, mariadb (mysql), postgres, redis (valkey), mailpit, php-fpm-<major.minor>")
    };
    let svc_all = || s("Service name, or \"all\"");
    v.extend([
        def("services_list", "List services", Read, "All services with status, port, version and uptime.", obj(vec![], &[]), true),
        def("services_logs", "Service logs", Read, "Recent log lines of a service.",
            obj(vec![("name", svc()), ("lines", json!({"type":"integer","minimum":1,"maximum":5000,"default":200}))], &["name"]), true),
        def("services_install", "Install service", Change,
            "Install a service (optionally a version). Long-running: returns a job_id unless wait is set.",
            obj(vec![("name", svc()), ("version", s("Version (optional)")), ("wait", wait())], &["name"]), true),
        def("services_start", "Start service", Change, "Start a service or all services.", obj(vec![("name", svc_all())], &["name"]), true),
        def("services_stop", "Stop service", Change,
            "Stop a service or all services. Stopping caddy or dns stops every local site from serving/resolving.",
            obj(vec![("name", svc_all())], &["name"]), true),
        def("services_restart", "Restart service", Change, "Restart a service or all services.", obj(vec![("name", svc_all())], &["name"]), false),
        def("services_configure", "Configure service", Change, "Change a service's autostart flag and/or port.",
            obj(vec![("name", svc()), ("autostart", b("Start with the daemon")), ("port", port("Listen port"))], &["name"]), true),
    ]);

    let engine = || json!({ "type": "string", "description": "mariadb (default), postgres, or an external mysql", "default": "mariadb" });
    let dbname = || json!({ "type": "string", "pattern": "^[A-Za-z0-9_\\-]{1,64}$", "description": "Database name" });
    v.extend([
        def("db_list", "List databases", Read, "List databases on mariadb/postgres.", obj(vec![("engine", engine())], &[]), true),
        def("db_create", "Create database", Change, "Create a database. Undo with db_drop.",
            obj(vec![("name", dbname()), ("engine", engine())], &["name"]), true),
        def("db_drop", "Drop database", Destructive, "Drop a database and all its data.",
            obj(vec![("name", dbname()), ("engine", engine()), ("confirm", confirm())], &["name"]), true),
        def("doctor", "Doctor", Read, "Run diagnostics. Each failing check has a `fix` to follow as-is.", obj(vec![], &[]), true),
        def("dns_status", "DNS status", Read, "TLD, resolver port, whether OS routing is configured and the resolver runs.", obj(vec![], &[]), true),
        def("config_get", "Get config", Read, "Read config.toml (whole, or one dotted key such as remote.port).",
            obj(vec![("key", s("Dotted key (optional)"))], &[]), true),
        def("job_get", "Get job", Read, "Status/progress of a background job.", obj(vec![("job_id", s("Job id"))], &["job_id"]), true),
        def("config_set", "Set config", Destructive,
            "Merge a partial object into config.toml. Changing tld or ports affects every site.",
            obj(vec![("patch", json!({"type":"object","description":"Partial config, e.g. {\"tld\":\"dev\"}"})), ("confirm", confirm())], &["patch"]), true),
        def("tls_trust", "Trust local CA", System,
            "Add the BOZKURT local CA to the OS trust store (the OS may show its own password prompt). Long-running job.",
            obj(vec![("confirm", confirm()), ("system", b("Also the system-wide store (default true)")), ("wait", wait())], &[]), true),
    ]);
    let mut share = def(
        "share_start",
        "Share site publicly",
        System,
        "Expose a site on a public tunnel URL. The result includes the URL. Stop with share_stop.",
        obj(
            vec![("site", s(SITE)), ("confirm", confirm()), ("wait", wait())],
            &["site"],
        ),
        false,
    );
    share.open_world = true;
    v.push(share);
    v.push(def(
        "share_stop",
        "Stop sharing",
        Change,
        "Stop the public tunnel of a site.",
        obj(vec![("site", s(SITE))], &["site"]),
        true,
    ));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_matches_spec() {
        let t = all();
        assert_eq!(t.len(), 55);
        let mut names: Vec<&str> = t.iter().map(|x| x.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 55, "duplicate tool names");
        for n in [
            "sites_delete",
            "php_uninstall",
            "go_install",
            "db_drop",
            "config_set",
            "tls_trust",
            "share_start",
            "job_get",
        ] {
            assert!(names.contains(&n), "{n}");
        }
        for d in &t {
            let j = d.to_json();
            assert_eq!(j["inputSchema"]["type"], "object");
            if d.tier.needs_confirm() {
                assert!(
                    j["inputSchema"]["properties"]["confirm"].is_object(),
                    "{}",
                    d.name
                );
                assert_eq!(j["annotations"]["destructiveHint"], true);
            }
            assert_eq!(j["annotations"]["readOnlyHint"], d.tier == Tier::Read);
        }
    }
}
