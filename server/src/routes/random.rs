//! `/random` — redirect to a random item of the feed the visitor came from
//! (port of src/inc/routes/random.mjs).

use crate::f0cklib::{self, FeedOpts};
use crate::http::{Req, Res, Router, html_code, redirect};
use crate::state::AppState;
use serde_json::json;

pub fn register(r: &mut Router) {
    r.get(r"^\/random$", random);
}

#[derive(Default, Debug)]
struct RefOpts {
    hall: Option<String>,
    tag: Option<String>,
    user: Option<String>,
    mode: Option<String>,
    mime: Option<String>,
    strict: bool,
}

fn parse_referer(referer: &str) -> RefOpts {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static HALL: Lazy<Regex> = Lazy::new(|| Regex::new(r"/h/([^/]+)").unwrap());
    static TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"/tag/([^/]+)").unwrap());
    // The old pattern only knew `f0cks|favs`; feed links use `uploads` — accept all three.
    static USER: Lazy<Regex> = Lazy::new(|| Regex::new(r"/user/([^/]+)/(f0cks|uploads|favs)").unwrap());
    // Whole path segment only (the old pattern also matched e.g. `/tag/videogames`).
    static MIME: Lazy<Regex> = Lazy::new(|| Regex::new(r"/((?:video|audio|image)(?:,(?:video|audio|image))*)(?:/|$)").unwrap());

    let mut o = RefOpts::default();
    let Ok(url) = url::Url::parse(referer) else {
        tracing::error!("[RANDOM] Failed to parse referer URL: {referer}");
        return o;
    };
    let path = url.path();
    o.hall = HALL.captures(path).map(|c| c[1].to_string());
    o.tag = TAG.captures(path).map(|c| c[1].to_string());
    if let Some(c) = USER.captures(path) {
        o.user = Some(c[1].to_string());
        o.mode = Some(c[2].to_string());
    }
    o.mime = MIME.captures(path).map(|c| c[1].to_string());
    o.strict = url.query().map(|q| q.contains("strict=1")).unwrap_or(false);
    o
}

async fn random(st: AppState, req: Req) -> Res {
    let o = req.header("referer").filter(|r| !r.is_empty()).map(parse_referer).unwrap_or_default();

    let ratings = req.cookie("ratings").filter(|r| !r.is_empty()).map(|r| {
        crate::util::decode_lossy(r)
            .split(['|', ','])
            .filter(|x| matches!(*x, "sfw" | "nsfw" | "nsfl" | "untagged"))
            .map(str::to_string)
            .collect::<Vec<_>>()
    });
    let cookie_mime = req.cookie("mime").map(|m| crate::util::decode_lossy(m).trim().to_string()).filter(|m| !m.is_empty());
    let mime = match req.query("mime") {
        Some(q) => Some(q.to_string()),
        None => cookie_mime.or(o.mime.clone()),
    };

    let opts = FeedOpts {
        user: o.user.clone(),
        tag: o.tag.clone(),
        hall: o.hall.clone(),
        mime,
        fav: o.mode.as_deref() == Some("favs"),
        mode: Some(req.mode),
        ratings,
        strict: o.strict,
        session: req.session(),
        ..Default::default()
    };
    let data = f0cklib::get_random(&st, &opts).await?;
    // getRandom returns null for private favourites (the old route then crashed with a 500)
    if !data["success"].as_bool().unwrap_or(false) {
        let message = data["message"].as_str().unwrap_or("no uploads found :(");
        let body = crate::tpl::render(
            &st,
            Some(&req),
            "error",
            json!({
                "message": message,
                "tmp": null,
                "session": req.session_value(),
                "error_filter_hint": null,
                "error_filter_hint_link": null,
                "error_see_anyways": null,
            }),
        );
        return Ok(html_code(body, 404));
    }
    let link = &data["link"];
    let id = crate::tpl::js_to_string(&crate::tpl::to_jvalue(&data["itemid"]));
    // link.main is already percent-encoded (the old code encoded it a second time)
    let target = format!(
        "{}{}{}{}",
        link["main"].as_str().unwrap_or("/"),
        link["path"].as_str().unwrap_or(""),
        id,
        link["suffix"].as_str().unwrap_or("")
    );
    Ok(redirect(&format!("{target}?random=1")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referer() {
        let o = parse_referer("http://x/tag/videogames/p/2?strict=1");
        assert_eq!(o.tag.as_deref(), Some("videogames"));
        assert_eq!(o.mime, None);
        assert!(o.strict);
        let o = parse_referer("http://x/user/bob/favs/video,image");
        assert_eq!(o.user.as_deref(), Some("bob"));
        assert_eq!(o.mode.as_deref(), Some("favs"));
        assert_eq!(o.mime.as_deref(), Some("video,image"));
    }
}
