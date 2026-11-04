//! Comments, subscriptions, notifications helpers, halls and user halls
//! (the social half of src/inc/routeinc/f0cklib.mjs, from `getComments` on, plus the comment
//! helpers `processMentions`, `processEmbeds`).
//!
//! Public API for other ports (item page, ajax, mod/admin halls):
//! * [`comments::get_comments`], [`comments::get_comment`], [`comments::process_mentions`],
//!   [`comments::get_subscription_status`], [`comments::mark_notifications_read`]
//! * [`halls::get_halls`], [`halls::get_halls_overview`], [`halls::add_item_to_hall`],
//!   [`halls::update_hall_metadata`], [`halls::remove_item_from_hall`] and the user-hall functions.

pub mod comments;
pub mod feed_bridge;
pub mod halls;
pub mod helpers;
