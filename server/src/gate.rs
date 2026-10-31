//! "Private society" gate pages (nginx-style 502 or a Cloudflare-style error page with a hidden login).
//!
//! TODO(port): the Cloudflare-style page (cloudflare-error-page package + login injection from
//! index.mjs `buildGatePage` / `getGateLoginInjection`) is not ported yet; the plain nginx 502 is
//! served in its place.

use crate::http::Req;
use crate::state::AppState;

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

/// Dynamic Cloudflare-style gate page.
pub fn build_gate_page(_st: &AppState, _req: &Req) -> String {
    NGINX_502.to_string()
}

/// `nginx502 ?? buildGatePage(req)`
pub fn gate_or_nginx(st: &AppState, req: &Req) -> String {
    if is_cloudflare_mode(st) { build_gate_page(st, req) } else { NGINX_502.to_string() }
}
