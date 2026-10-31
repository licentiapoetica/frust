//! Route registration. One module per old src/inc/routes/*.mjs file.

use crate::http::Router;

pub mod static_files;

pub fn build() -> Router {
    let mut r = Router::new();
    static_files::register(&mut r);
    r.finalize();
    r
}
