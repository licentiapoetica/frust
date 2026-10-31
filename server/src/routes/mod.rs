//! Route registration. One module per old src/inc/routes/*.mjs file.

use crate::http::Router;

pub mod banned;
pub mod comments;
pub mod halls;
pub mod static_files;
pub mod subscriptions;
pub mod theme;
pub mod user_halls;

pub fn build() -> Router {
    let mut r = Router::new();
    banned::register(&mut r);
    static_files::register(&mut r);
    theme::register(&mut r);
    comments::register(&mut r);
    halls::register(&mut r);
    user_halls::register(&mut r);
    subscriptions::register(&mut r);
    r.finalize();
    r
}
