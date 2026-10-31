//! "Private society" gate pages: an nginx-style 502, or a Cloudflare-style error page (port of the
//! `cloudflare-error-page` package as configured in index.mjs) with a hidden login/register modal.
//! Templates: `views/gate/cloudflare.html`, `views/gate/login_injection.html`.

use crate::http::Req;
use crate::state::AppState;
use serde_json::json;

pub const NGINX_502: &str = "<html>\n<head><title>502 Bad Gateway</title></head>\n<body bgcolor=\"white\">\n<center><h1>502 Bad Gateway</h1></center>\n<hr><center>nginx</center>\n</body>\n</html>";

/// Gate in Cloudflare mode (`websrv.private_society_gate: cloudflare`).
pub fn is_cloudflare_mode(st: &AppState) -> bool {
    let cfg = st.cfg();
    cfg.truthy("websrv.private_society") && cfg.str("websrv.private_society_gate") == Some("cloudflare")
}

/// Custom gate template name (`private_society_gate: custom`).
pub fn custom_template(st: &AppState) -> Option<String> {
    let cfg = st.cfg();
    if cfg.truthy("websrv.private_society") && cfg.str("websrv.private_society_gate") == Some("custom") {
        cfg.str("websrv.private_society_gate_template").filter(|t| !t.is_empty()).map(|t| t.trim_end_matches(".html").to_string())
    } else {
        None
    }
}

/// Host shown on the page: the request host unless it is a local name, else the configured domain.
fn display_host(st: &AppState, req: &Req) -> String {
    let raw = req.header("x-forwarded-host").or_else(|| req.header("host")).unwrap_or("");
    let first = raw.split(',').next().unwrap_or("").trim();
    let host = if let Some(rest) = first.strip_prefix('[') {
        match rest.find(']') {
            Some(i) => rest[..i].to_string(),
            None => first.to_string(),
        }
    } else {
        first.split(':').next().unwrap_or("").trim().to_string()
    };
    const LOCAL: [&str; 6] = ["localhost", "127.0.0.1", "::1", "f0ckm", "0.0.0.0", "localhost.localdomain"];
    if host.is_empty() || LOCAL.contains(&host.to_lowercase().as_str()) {
        st.cfg().domain().to_string()
    } else {
        host
    }
}

/// 16 random uppercase hex digits, like a Cloudflare Ray ID.
fn ray_id() -> String {
    crate::util::random_hex(8).to_uppercase()
}

/// Dynamic Cloudflare-style gate page (fresh Ray ID and timestamp per request). The host and the
/// visitor IP come from request headers and are HTML-escaped (the old code inserted them raw).
pub fn build_gate_page(st: &AppState, req: &Req) -> String {
    let cfg = st.cfg();
    let host = display_host(st, req);
    let recaptcha = cfg.truthy("recaptcha.enabled") && cfg.truthy("recaptcha.site_key") && !req.is_onion() && !req.is_localhost();
    let ctx = json!({
        "host": host,
        "time": chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        "ray_id": ray_id(),
        "client_ip": req.real_ip(),
        "statuses": [
            { "id": "browser", "icon": "browser", "status": "ok", "location": "You", "name": "Browser" },
            { "id": "cloudflare", "icon": "cloud", "status": "ok",
              "location": cfg.str("websrv.private_society_gate_location").filter(|l| !l.is_empty()).unwrap_or("Frankfurt"), "name": "Cloudflare" },
            { "id": "host", "icon": "server", "status": "error", "location": host, "name": "Host" },
        ],
        "recaptcha": recaptcha,
        "recaptcha_site_key": cfg.str_or("recaptcha.site_key", ""),
    });
    st.tpl.render("gate/cloudflare", &ctx)
}

/// `nginx502 ?? buildGatePage(req)`
pub fn gate_or_nginx(st: &AppState, req: &Req) -> String {
    if is_cloudflare_mode(st) { build_gate_page(st, req) } else { NGINX_502.to_string() }
}
