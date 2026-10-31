//! Regex router with flummpress-compatible matching order.
//!
//! flummpress keeps routes in a Map sorted by `String([key, handlers])` in *reverse* order and
//! takes the first entry whose key matches the path and that has a handler for the method.
//! For a RegExp key that string is `/<source>/<flags>,[object Object]`; for a string key it is
//! `<path>,[object Object]` (string keys match the exact path only). We compute the same sort
//! key so overlapping routes resolve exactly as before.
//!
//! Patterns are given as the JavaScript regex *source* (what is between the slashes of the
//! regex literal, e.g. `r"^\/random$"`) so the ordering key is identical.

use super::{Guard, Req, Res};
use crate::state::AppState;
use axum::http::Method;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFut = Pin<Box<dyn Future<Output = Res> + Send>>;
pub type HandlerFn = Arc<dyn Fn(AppState, Req) -> BoxFut + Send + Sync>;

#[derive(Clone)]
pub struct Endpoint {
    pub guard: Option<Guard>,
    pub handler: HandlerFn,
}

enum Matcher {
    Exact(String),
    Regex(fancy_regex::Regex),
}

struct Route {
    sort_key: String,
    matcher: Matcher,
    names: Vec<String>,
    methods: HashMap<Method, Endpoint>,
}

#[derive(Default)]
pub struct Router {
    routes: Vec<Route>,
    index: HashMap<String, usize>,
    /// Routes checked before the sorted table, in registration order (the old "bypass" middleware).
    pre: Vec<Route>,
}

pub struct Matched {
    pub endpoint: Endpoint,
    pub params: HashMap<String, String>,
}

/// Turn a JS regex source into one the Rust engine accepts (`\/` → `/`).
fn js_to_rust_regex(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('/') => out.push('/'),
                Some(n) => {
                    out.push('\\');
                    out.push(n);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `RegExp.prototype.source` as V8 prints it: unescaped `/` outside character classes is escaped.
fn escape_js_source(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 8);
    let (mut escaped, mut in_class) = (false, false);
    for c in src.chars() {
        if escaped {
            out.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => {
                escaped = true;
                out.push(c);
            }
            '[' => {
                in_class = true;
                out.push(c);
            }
            ']' => {
                in_class = false;
                out.push(c);
            }
            '/' if !in_class => out.push_str("\\/"),
            _ => out.push(c),
        }
    }
    out
}

fn compile(src: &str) -> (fancy_regex::Regex, Vec<String>) {
    let re = fancy_regex::Regex::new(&js_to_rust_regex(src)).unwrap_or_else(|e| panic!("invalid route regex {src:?}: {e}"));
    let names = re.capture_names().flatten().map(str::to_string).collect();
    (re, names)
}

fn into_handler<F, Fut>(f: F) -> HandlerFn
where
    F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Res> + Send + 'static,
{
    Arc::new(move |st, req| Box::pin(f(st, req)))
}

impl Router {
    pub fn new() -> Router {
        Router::default()
    }

    fn make_route(key: &str, is_regex: bool) -> Route {
        if is_regex {
            let (re, names) = compile(key);
            Route {
                sort_key: format!("/{}/,[object Object]", escape_js_source(key)),
                matcher: Matcher::Regex(re),
                names,
                methods: HashMap::new(),
            }
        } else {
            Route {
                sort_key: format!("{key},[object Object]"),
                matcher: Matcher::Exact(key.to_string()),
                names: Vec::new(),
                methods: HashMap::new(),
            }
        }
    }

    fn register(&mut self, method: Method, key: &str, is_regex: bool, ep: Endpoint) {
        let map_key = format!("{}{key}", if is_regex { "re:" } else { "str:" });
        let idx = match self.index.get(&map_key) {
            Some(i) => *i,
            None => {
                self.routes.push(Self::make_route(key, is_regex));
                self.index.insert(map_key, self.routes.len() - 1);
                self.routes.len() - 1
            }
        };
        self.routes[idx].methods.insert(method, ep);
    }

    /// Sort like flummpress (reverse of the JS default string sort, which compares UTF-16 units).
    pub fn finalize(&mut self) {
        self.routes.sort_by(|a, b| {
            let ka: Vec<u16> = a.sort_key.encode_utf16().collect();
            let kb: Vec<u16> = b.sort_key.encode_utf16().collect();
            kb.cmp(&ka)
        });
        self.index.clear();
    }

    // ── Regex routes (JS regex source) ──────────────────────────────────────

    pub fn route<F, Fut>(&mut self, method: Method, re: &str, guard: Option<Guard>, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.register(method, re, true, Endpoint { guard, handler: into_handler(f) });
        self
    }

    pub fn get<F, Fut>(&mut self, re: &str, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::GET, re, None, f)
    }

    pub fn post<F, Fut>(&mut self, re: &str, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::POST, re, None, f)
    }

    pub fn put<F, Fut>(&mut self, re: &str, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::PUT, re, None, f)
    }

    pub fn delete<F, Fut>(&mut self, re: &str, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::DELETE, re, None, f)
    }

    pub fn patch<F, Fut>(&mut self, re: &str, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::PATCH, re, None, f)
    }

    pub fn head<F, Fut>(&mut self, re: &str, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::HEAD, re, None, f)
    }

    /// Route with a guard (the old `router.get(path, lib.auth, handler)` form).
    pub fn get_g<F, Fut>(&mut self, re: &str, guard: Guard, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::GET, re, Some(guard), f)
    }

    pub fn post_g<F, Fut>(&mut self, re: &str, guard: Guard, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::POST, re, Some(guard), f)
    }

    pub fn put_g<F, Fut>(&mut self, re: &str, guard: Guard, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::PUT, re, Some(guard), f)
    }

    pub fn delete_g<F, Fut>(&mut self, re: &str, guard: Guard, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::DELETE, re, Some(guard), f)
    }

    pub fn patch_g<F, Fut>(&mut self, re: &str, guard: Guard, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::PATCH, re, Some(guard), f)
    }

    // ── String routes (exact path) ──────────────────────────────────────────

    pub fn exact<F, Fut>(&mut self, method: Method, path: &str, guard: Option<Guard>, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.register(method, path, false, Endpoint { guard, handler: into_handler(f) });
        self
    }

    // ── Groups: `router.group(/^\/api\/v2/, g => g.get(/^\/x$/, ...))` ───────

    /// Combined regex source of a group route, as flummpress builds it.
    pub fn group_path(prefix: &str, p: &str) -> String {
        let joined = format!("{prefix}{}", if p == "/" { "$" } else { p });
        // `.replace(/(\\\/){1,}/g, "/")`
        let mut out = String::with_capacity(joined.len());
        let mut rest = joined.as_str();
        while let Some(i) = rest.find("\\/") {
            out.push_str(&rest[..i]);
            out.push('/');
            rest = &rest[i + 2..];
            while let Some(r) = rest.strip_prefix("\\/") {
                rest = r;
            }
        }
        out.push_str(rest);
        out
    }

    // ── Bypass routes (checked first, registration order) ───────────────────

    pub fn pre<F, Fut>(&mut self, method: Method, re: &str, guard: Option<Guard>, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        let mut r = Self::make_route(re, true);
        r.methods.insert(method, Endpoint { guard, handler: into_handler(f) });
        self.pre.push(r);
        self
    }

    fn try_route(r: &Route, path: &str, method: &Method) -> Option<Matched> {
        let ep = r.methods.get(method)?;
        match &r.matcher {
            Matcher::Exact(p) => (p == path).then(|| Matched { endpoint: ep.clone(), params: HashMap::new() }),
            Matcher::Regex(re) => {
                let caps = re.captures(path).ok().flatten()?;
                let mut params = HashMap::new();
                for n in &r.names {
                    if let Some(m) = caps.name(n) {
                        params.insert(n.clone(), m.as_str().to_string());
                    }
                }
                Some(Matched { endpoint: ep.clone(), params })
            }
        }
    }

    pub fn find(&self, path: &str, method: &Method) -> Option<Matched> {
        for r in &self.pre {
            if let Some(m) = Self::try_route(r, path, method) {
                return Some(m);
            }
        }
        // flummpress: first route (in sorted order) whose key matches AND has the method.
        for r in &self.routes {
            if let Some(m) = Self::try_route(r, path, method) {
                return Some(m);
            }
        }
        None
    }

    pub fn len(&self) -> usize {
        self.routes.len() + self.pre.len()
    }
}

/// Group builder: `let mut g = Group::new(&mut router, r"^\/api\/v2"); g.get(r"^\/x$", h);`
pub struct Group<'a> {
    router: &'a mut Router,
    prefix: String,
}

impl<'a> Group<'a> {
    pub fn new(router: &'a mut Router, prefix: &str) -> Group<'a> {
        Group { router, prefix: prefix.to_string() }
    }

    pub fn route<F, Fut>(&mut self, method: Method, p: &str, guard: Option<Guard>, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        let path = Router::group_path(&self.prefix, p);
        self.router.route(method, &path, guard, f);
        self
    }

    pub fn get<F, Fut>(&mut self, p: &str, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::GET, p, None, f)
    }

    pub fn post<F, Fut>(&mut self, p: &str, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::POST, p, None, f)
    }

    pub fn put<F, Fut>(&mut self, p: &str, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::PUT, p, None, f)
    }

    pub fn delete<F, Fut>(&mut self, p: &str, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::DELETE, p, None, f)
    }

    pub fn patch<F, Fut>(&mut self, p: &str, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::PATCH, p, None, f)
    }

    pub fn get_g<F, Fut>(&mut self, p: &str, guard: Guard, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::GET, p, Some(guard), f)
    }

    pub fn post_g<F, Fut>(&mut self, p: &str, guard: Guard, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::POST, p, Some(guard), f)
    }

    pub fn put_g<F, Fut>(&mut self, p: &str, guard: Guard, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::PUT, p, Some(guard), f)
    }

    pub fn delete_g<F, Fut>(&mut self, p: &str, guard: Guard, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::DELETE, p, Some(guard), f)
    }

    pub fn patch_g<F, Fut>(&mut self, p: &str, guard: Guard, f: F) -> &mut Self
    where
        F: Fn(AppState, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        self.route(Method::PATCH, p, Some(guard), f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_path_collapses_escaped_slashes() {
        assert_eq!(Router::group_path(r"^\/api\/v2", r"\/tags\/(?<id>\d+)$"), r"^/api/v2/tags/(?<id>\d+)$");
        assert_eq!(Router::group_path(r"^\/settings", "/"), r"^/settings$");
    }

    #[test]
    fn source_escaping_matches_v8() {
        assert_eq!(escape_js_source("^/api/v2/x/[^/]+$"), r"^\/api\/v2\/x\/[^/]+$");
        assert_eq!(escape_js_source(r"^\/a$"), r"^\/a$");
    }

    #[test]
    fn js_regex_translation() {
        let (re, names) = compile(r"^\/user\/(?<user>[^/]+)\/(?<itemid>(?!(?:f0cks|uploads)$)[a-zA-Z0-9_-]+)$");
        assert_eq!(names, vec!["user", "itemid"]);
        assert!(re.is_match("/user/x/abc").unwrap());
        assert!(!re.is_match("/user/x/uploads").unwrap());
    }
}
