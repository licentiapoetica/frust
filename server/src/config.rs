//! config.yaml loading, environment overrides, storage path resolution and live reload.
//!
//! The file is kept as a JSON value tree because most of the code reads optional feature flags
//! with JavaScript-style defaults (`cfg.websrv.x !== false`, `!!cfg.websrv.y`). The accessors below
//! reproduce those rules so ported code reads the same as the original.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub a: PathBuf,
    pub b: PathBuf,
    pub c: PathBuf,
    pub t: PathBuf,
    pub ca: PathBuf,
    pub s: PathBuf,
    pub emojis: PathBuf,
    pub koepfe: PathBuf,
    pub fonts: PathBuf,
    pub memes: PathBuf,
    pub e: PathBuf,
    pub cu: PathBuf,
    pub pending: PathBuf,
    pub deleted: PathBuf,
    pub logs: PathBuf,
    pub tmp: PathBuf,
    pub import: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// The parsed config.yaml (with environment overrides applied).
    pub raw: Value,
    pub paths: Paths,
    /// Project root (where config.yaml, views/, public/ and migrations/ live).
    pub base: PathBuf,
}

/// JavaScript truthiness for a JSON value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `parseInt`-like integer reading of a JSON value.
pub fn as_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.trunc() as i64)),
        Value::String(s) => {
            let s = s.trim();
            let end = s
                .char_indices()
                .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
                .map(|(i, _)| i)
                .unwrap_or(s.len());
            s[..end].parse().ok()
        }
        Value::Bool(_) | Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

pub fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

static NULL: Value = Value::Null;

impl Config {
    pub fn load(base: &Path) -> Result<Config> {
        let mut raw = read_config_file(base)?;
        apply_overrides(&mut raw);
        if raw["main"]["development"].as_bool() == Some(true) && std::env::var_os("STORAGE_DIR").is_none() {
            // SAFETY: called once at startup before any other threads read the environment.
            unsafe { std::env::set_var("STORAGE_DIR", "f0ckm-data") };
        }
        if std::env::var_os("TZ").is_none() {
            if let Some(tz) = raw["main"]["timezone"].as_str() {
                unsafe { std::env::set_var("TZ", tz) };
            }
        }
        let paths = resolve_paths(base);
        Ok(Config { raw, paths, base: base.to_path_buf() })
    }

    /// Re-read config.yaml, keeping the resolved storage paths.
    pub fn reload(&self) -> Result<Config> {
        let mut raw = read_config_file(&self.base)?;
        apply_overrides(&mut raw);
        Ok(Config { raw, paths: self.paths.clone(), base: self.base.clone() })
    }

    /// Value at a dot path (`"websrv.halls_enabled"`); `Null` when missing.
    pub fn get(&self, path: &str) -> &Value {
        let mut cur = &self.raw;
        for part in path.split('.') {
            cur = match cur {
                Value::Object(m) => m.get(part).unwrap_or(&NULL),
                Value::Array(a) => part.parse::<usize>().ok().and_then(|i| a.get(i)).unwrap_or(&NULL),
                _ => &NULL,
            };
        }
        cur
    }

    /// `!!cfg.x`
    pub fn truthy(&self, path: &str) -> bool {
        truthy(self.get(path))
    }

    /// `cfg.x === false`
    pub fn is_false(&self, path: &str) -> bool {
        self.get(path) == &Value::Bool(false)
    }

    /// `cfg.x !== false` — on unless explicitly disabled.
    pub fn enabled(&self, path: &str) -> bool {
        !self.is_false(path)
    }

    pub fn str(&self, path: &str) -> Option<&str> {
        self.get(path).as_str()
    }

    /// `cfg.x || default` for strings.
    pub fn str_or<'a>(&'a self, path: &str, default: &'a str) -> &'a str {
        match self.get(path) {
            Value::String(s) if !s.is_empty() => s,
            _ => default,
        }
    }

    pub fn int(&self, path: &str) -> Option<i64> {
        as_int(self.get(path))
    }

    pub fn int_or(&self, path: &str, default: i64) -> i64 {
        self.int(path).unwrap_or(default)
    }

    pub fn f64(&self, path: &str) -> Option<f64> {
        as_f64(self.get(path))
    }

    pub fn bool(&self, path: &str) -> Option<bool> {
        self.get(path).as_bool()
    }

    pub fn str_list(&self, path: &str) -> Option<Vec<String>> {
        self.get(path)
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
    }

    // ── Frequently used values ───────────────────────────────────────────────

    pub fn development(&self) -> bool {
        self.get("main.development").as_bool() == Some(true)
    }

    pub fn url_full(&self) -> &str {
        self.str_or("main.url.full", "")
    }

    pub fn domain(&self) -> &str {
        self.str_or("main.url.domain", "")
    }

    pub fn is_secure(&self) -> bool {
        self.url_full().starts_with("https")
    }

    pub fn enable_nsfl(&self) -> bool {
        self.truthy("enable_nsfl")
    }

    /// `parseInt(cfg.nsfl_tag_id, 10) || 3`
    pub fn nsfl_tag_id(&self) -> i64 {
        match self.int("nsfl_tag_id") {
            Some(n) if n != 0 => n,
            _ => 3,
        }
    }

    pub fn port(&self) -> u16 {
        self.int("websrv.port").and_then(|p| u16::try_from(p).ok()).unwrap_or(1499)
    }

    /// Mime → extension map (`cfg.mimes`).
    pub fn mimes(&self) -> serde_json::Map<String, Value> {
        self.get("mimes").as_object().cloned().unwrap_or_default()
    }

    /// `cfg.nsfp` as tag ids.
    pub fn nsfp(&self) -> Vec<i64> {
        self.get("nsfp").as_array().map(|a| a.iter().filter_map(as_int).collect()).unwrap_or_default()
    }

    /// Bot client config by type (`irc`, `tg`, `matrix`, `slack`).
    pub fn client(&self, kind: &str) -> Option<&Value> {
        self.get("clients").as_array()?.iter().find(|c| c["type"].as_str() == Some(kind))
    }

    pub fn client_enabled(&self, kind: &str) -> bool {
        self.client(kind).map(|c| truthy(&c["enabled"])).unwrap_or(false)
    }

    pub fn database_url_parts(&self) -> (String, u16, String, String, String) {
        let host = self.str_or("sql.host", "localhost").to_string();
        let port = self.int("sql.port").and_then(|p| u16::try_from(p).ok()).unwrap_or(5432);
        let user = self.str_or("sql.user", "f0ckm").to_string();
        let password = self.str_or("sql.password", "").to_string();
        let db = self.str_or("sql.database", "f0ckm").to_string();
        (host, port, user, password, db)
    }
}

fn read_config_file(base: &Path) -> Result<Value> {
    let file = base.join("config.yaml");
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(err) => {
            let hint = if base.join("config.json").exists() {
                "An old config.json was found: convert it once with `npm run config:migrate`."
            } else {
                "Create it from the example: `cp config_example.yaml config.yaml`."
            };
            bail!("[CONFIG] Cannot read {} ({err}). {hint}", file.display());
        }
    };
    let yaml: serde_yaml::Value = serde_yaml::from_str(&text).with_context(|| format!("[CONFIG] {} is not valid YAML", file.display()))?;
    let json = serde_json::to_value(yaml).context("[CONFIG] config.yaml can't be represented as JSON")?;
    if !json.is_object() {
        bail!("[CONFIG] {} is empty or not a YAML mapping", file.display());
    }
    Ok(json)
}

fn env_first(names: &[&str]) -> Option<String> {
    names.iter().find_map(|n| std::env::var(n).ok().filter(|v| !v.is_empty()))
}

/// Values that come from the environment, not the file. Applied at startup and after every reload.
fn apply_overrides(cfg: &mut Value) {
    if !cfg["sql"].is_object() {
        cfg["sql"] = Value::Object(Default::default());
    }
    if !cfg["main"].is_object() {
        cfg["main"] = Value::Object(Default::default());
    }
    if let Some(v) = env_first(&["DB_HOST", "POSTGRES_HOST", "PGHOST"]) {
        cfg["sql"]["host"] = Value::String(v);
    }
    if let Some(v) = env_first(&["DB_PORT", "POSTGRES_PORT", "PGPORT"]) {
        if let Ok(p) = v.parse::<i64>() {
            cfg["sql"]["port"] = Value::from(p);
        }
    } else if let Some(p) = as_int(&cfg["sql"]["port"]) {
        cfg["sql"]["port"] = Value::from(p);
    }
    if let Some(v) = env_first(&["DB_USER", "POSTGRES_USER", "PGUSER"]) {
        cfg["sql"]["user"] = Value::String(v);
    }
    if let Some(v) = env_first(&["DB_PASS", "POSTGRES_PASSWORD", "PGPASSWORD"]) {
        cfg["sql"]["password"] = Value::String(v);
    }
    if let Some(v) = env_first(&["DB_NAME", "POSTGRES_DB", "PGDATABASE"]) {
        cfg["sql"]["database"] = Value::String(v);
    }

    let host = cfg["sql"]["host"].as_str().unwrap_or("").to_string();
    let is_docker = std::env::var("DB_HOST").ok().as_deref() == Some("f0ckm-db")
        || host == "f0ckm-db"
        || std::env::var("CONTAINER").ok().as_deref() == Some("true");
    if std::env::var("NODE_ENV").ok().as_deref() == Some("production") || is_docker {
        cfg["main"]["development"] = Value::Bool(false);
    }
    if cfg["main"]["development"].as_bool() == Some(true) {
        if std::env::var_os("DB_HOST").is_none() && (host == "f0ckm-db" || host.is_empty()) {
            cfg["sql"]["host"] = Value::String("localhost".into());
        }
        if std::env::var_os("DB_PORT").is_none() && as_int(&cfg["sql"]["port"]) == Some(5432) {
            cfg["sql"]["port"] = Value::from(5454);
        }
    }
}

fn canonical_or(target: PathBuf) -> PathBuf {
    if let Ok(p) = std::fs::canonicalize(&target) {
        return p;
    }
    if let (Some(parent), Some(name)) = (target.parent(), target.file_name()) {
        if let Ok(p) = std::fs::canonicalize(parent) {
            return p.join(name);
        }
    }
    target
}

fn resolve_paths(base: &Path) -> Paths {
    let storage = std::env::var("STORAGE_DIR").ok().filter(|s| !s.is_empty()).map(|s| {
        let p = PathBuf::from(s);
        if p.is_absolute() { p } else { std::env::current_dir().unwrap_or_default().join(p) }
    });
    let resolve = |default_rel: &str| -> PathBuf {
        let local = base.join(default_rel);
        let mut target = local.clone();
        if let Some(storage) = &storage {
            if let Some(sub) = default_rel.strip_prefix("public/") {
                if matches!(sub, "s/emojis" | "s/koepfe" | "s/fonts") {
                    let name = sub.rsplit('/').next().unwrap_or(sub);
                    let storage_path = storage.join(name);
                    target = if storage_path.exists() { storage_path } else { local };
                } else {
                    target = storage.join(sub);
                }
            } else {
                target = storage.join(default_rel);
            }
        }
        canonical_or(target)
    };
    let import = canonical_or(match &storage {
        Some(s) => s.join("import"),
        None => base.join("f0ckm-data/import"),
    });
    Paths {
        a: resolve("public/a"),
        b: resolve("public/b"),
        c: resolve("public/c"),
        t: resolve("public/t"),
        ca: resolve("public/ca"),
        s: canonical_or(base.join("public/s")),
        emojis: resolve("public/s/emojis"),
        koepfe: resolve("public/s/koepfe"),
        fonts: resolve("public/s/fonts"),
        memes: resolve("public/memes"),
        e: resolve("public/e"),
        cu: resolve("public/cu"),
        pending: resolve("pending"),
        deleted: resolve("deleted"),
        logs: resolve("logs"),
        tmp: resolve("tmp"),
        import,
    }
}
