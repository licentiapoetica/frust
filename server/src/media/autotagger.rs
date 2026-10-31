//! NSFW classification via external APIs (`src/inc/autotagger.mjs`).

use crate::state::AppState;
use serde_json::{Value, json};

/// Ask `apis.nsfw1` (files < 4 MiB) or `apis.nsfw2` whether `/b/<filename>` is NSFW.
pub async fn is_nsfw(st: &AppState, filename: &str, filesize: u64) -> anyhow::Result<bool> {
    let cfg = st.cfg();
    let small = filesize < 4_194_304;
    let api = if small { cfg.get("apis.nsfw1") } else { cfg.get("apis.nsfw2") };
    let url = api["url"].as_str().ok_or_else(|| anyhow::anyhow!("autotagger api url missing"))?;
    let file_url = format!("{}/b/{}", cfg.url_full(), crate::util::encode_uri_component(filename));
    let body = if small { json!({ "DataRepresentation": "URL", "Value": file_url }) } else { json!({ "url": file_url }) };
    let mut req = st.http.post(url).body(body.to_string());
    if let Some(h) = api["headers"].as_object() {
        for (k, v) in h {
            if let Some(v) = v.as_str() {
                req = req.header(k.as_str(), v);
            }
        }
    }
    let res: Value = req.send().await?.json().await?;
    Ok(if small {
        crate::config::truthy(&res["IsImageAdultClassified"]) || res["RacyClassificationScore"].as_f64().map(|s| s > 0.6).unwrap_or(false)
    } else {
        crate::config::truthy(&res["unsafe"])
    })
}
