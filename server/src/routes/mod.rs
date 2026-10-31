//! Route registration. One module per old src/inc/routes/*.mjs file.

use crate::http::Router;

pub mod apiv2;
pub mod banned;
pub mod index;
pub mod random;
pub mod register;
pub mod comments;
pub mod halls;
pub mod static_files;
pub mod subscriptions;
pub mod theme;
pub mod uploads;
pub mod user_halls;
pub mod mod_panel;
pub mod notifications;
pub mod nsfp;
pub mod pending;
pub mod reports;
pub mod warnings;

pub fn build() -> Router {
    let mut r = Router::new();
    banned::register(&mut r);
    index::register(&mut r);
    random::register(&mut r);
    register::register(&mut r);
    apiv2::anon::register(&mut r);
    apiv2::passkeys::register(&mut r);
    static_files::register(&mut r);
    theme::register(&mut r);
    uploads::register(&mut r);
    comments::register(&mut r);
    halls::register(&mut r);
    user_halls::register(&mut r);
    subscriptions::register(&mut r);
    mod_panel::register(&mut r);
    reports::register(&mut r);
    warnings::register(&mut r);
    pending::register(&mut r);
    nsfp::register(&mut r);
    notifications::register(&mut r);
    r.finalize();
    r
}
