//! Route registration. One module per old src/inc/routes/*.mjs file.

use crate::http::Router;

pub mod banned;
pub mod static_files;
pub mod theme;

pub fn build() -> Router {
    let mut r = Router::new();
    banned::register(&mut r);
    static_files::register(&mut r);
    theme::register(&mut r);
    r.finalize();
    r
}
