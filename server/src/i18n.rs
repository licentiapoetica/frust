//! Translations from locales/*.json (dot-notation keys, `{name}` interpolation, English fallback).
//! Files are re-read when they change on disk, so edits apply without a restart.

use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

pub const VALID_LANGS: [&str; 4] = ["en", "de", "nl", "zange"];

pub struct I18n {
    dir: PathBuf,
    cache: RwLock<HashMap<String, (Option<SystemTime>, Arc<Value>)>>,
}

impl I18n {
    pub fn new(dir: PathBuf) -> I18n {
        I18n { dir, cache: RwLock::new(HashMap::new()) }
    }

    fn locale(&self, lang: &str) -> Arc<Value> {
        // Only known language codes reach the filesystem.
        if !lang.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') || lang.is_empty() {
            return Arc::new(Value::Null);
        }
        let path = self.dir.join(format!("{lang}.json"));
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if let Some((t, v)) = self.cache.read().get(lang) {
            if *t == mtime {
                return v.clone();
            }
        }
        let v = match std::fs::read_to_string(&path).map(|s| serde_json::from_str::<Value>(&s)) {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                tracing::warn!("[i18n] Failed to parse locale \"{lang}\": {e}");
                Value::Null
            }
            Err(e) => {
                tracing::warn!("[i18n] Failed to load locale \"{lang}\": {e}");
                Value::Null
            }
        };
        let v = Arc::new(v);
        self.cache.write().insert(lang.to_string(), (mtime, v.clone()));
        v
    }

    pub fn translator(&self, lang: &str) -> Translator {
        let primary = self.locale(lang);
        let fallback = if lang != "en" { self.locale("en") } else { Arc::new(Value::Null) };
        Translator { lang: lang.to_string(), primary, fallback }
    }
}

#[derive(Clone)]
pub struct Translator {
    pub lang: String,
    primary: Arc<Value>,
    fallback: Arc<Value>,
}

fn deep_get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    let mut cur = v;
    for part in key.split('.') {
        cur = cur.as_object()?.get(part)?;
    }
    if cur.is_null() { None } else { Some(cur) }
}

impl Translator {
    /// Raw lookup (may be a non-string value such as an object or array).
    pub fn lookup(&self, key: &str) -> Value {
        deep_get(&self.primary, key)
            .or_else(|| deep_get(&self.fallback, key))
            .cloned()
            .unwrap_or_else(|| Value::String(key.to_string()))
    }

    pub fn t(&self, key: &str) -> String {
        match self.lookup(key) {
            Value::String(s) => s,
            other => other.to_string(),
        }
    }

    pub fn t_with(&self, key: &str, data: &[(&str, &str)]) -> String {
        let mut s = self.t(key);
        for (k, v) in data {
            s = s.replace(&format!("{{{k}}}"), v);
        }
        s
    }

    /// `t(key, data)` with a JSON object of replacements; non-string results are returned as-is.
    pub fn t_value(&self, key: &str, data: Option<&serde_json::Map<String, Value>>) -> Value {
        let v = self.lookup(key);
        let Value::String(mut s) = v else { return v };
        if let Some(data) = data {
            for (k, val) in data {
                let rep = match val {
                    Value::String(x) => x.clone(),
                    Value::Null => "null".into(),
                    other => other.to_string(),
                };
                s = s.replace(&format!("{{{k}}}"), &rep);
            }
        }
        Value::String(s)
    }
}
