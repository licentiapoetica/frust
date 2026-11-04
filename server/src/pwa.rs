//! PWA web app manifest (/manifest.json), built from the `manifest:` section of config.yaml.
//!
//! Every key under `manifest:` is copied into the manifest as-is (standard Web App Manifest keys,
//! https://developer.mozilla.org/docs/Web/Manifest), overriding the defaults below.

use crate::config::Config;
use serde_json::{Value, json};

const DEFAULT_THEME_COLOR: &str = "#0096ff";

fn icon_mime(src: &str) -> &'static str {
    let s = src.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    match s.rsplit('.').next().unwrap_or("") {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "image/png",
    }
}

/// `favicon`: the site's favicon (admin panel upload or `websrv.custom_favicon`), empty for the
/// built-in one.
pub fn manifest(cfg: &Config, favicon: &str) -> Value {
    let domain = cfg.domain();
    let favicon = if favicon.is_empty() { "/s/img/favicon.gif" } else { favicon };
    let mut m = json!({
        "id": "/",
        "name": domain,
        "short_name": domain,
        "description": cfg.str_or("websrv.description", ""),
        "start_url": "/",
        "scope": "/",
        "display": "standalone",
        "background_color": "#000000",
        "theme_color": DEFAULT_THEME_COLOR,
        "icons": [{ "src": favicon, "sizes": "any", "type": icon_mime(favicon), "purpose": "any" }],
        "categories": ["entertainment", "social", "memes"],
    });
    if let (Some(out), Value::Object(over)) = (m.as_object_mut(), cfg.get("manifest")) {
        for (k, v) in over {
            if v.is_null() {
                out.remove(k);
            } else {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    m
}

/// `<meta name="theme-color">`: follows `manifest.theme_color`.
pub fn theme_color(cfg: &Config) -> &str {
    cfg.str_or("manifest.theme_color", DEFAULT_THEME_COLOR)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(raw: Value) -> Config {
        let base = std::path::PathBuf::from(".");
        Config { raw, paths: crate::config::resolve_paths(&base), base }
    }

    #[test]
    fn defaults_from_config() {
        let m = manifest(&cfg(json!({ "main": { "url": { "domain": "f0ck.me" } }, "websrv": { "description": "d" } })), "/s/img/x.webp");
        assert_eq!(m["name"], "f0ck.me");
        assert_eq!(m["description"], "d");
        assert_eq!(m["icons"][0]["type"], "image/webp");
    }

    #[test]
    fn overrides_and_removal() {
        let c = cfg(json!({ "manifest": { "name": "F", "theme_color": "#f00", "categories": null, "icons": [] } }));
        let m = manifest(&c, "");
        assert_eq!(m["name"], "F");
        assert_eq!(m["icons"], json!([]));
        assert!(m.get("categories").is_none());
        assert_eq!(theme_color(&c), "#f00");
    }
}
