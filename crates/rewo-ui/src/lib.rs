//! rewo-ui — the client's screen and widget models, without a GPU.
//!
//! Transcriptions of `net.minecraft.client.gui`: the screen framework and its
//! layouts, every screen Rewo has (pause, options, death, disconnect, stats,
//! advancements, book, sign, chat, container geometry, recipe book), the chat
//! log, edit boxes and command suggestions. Moved out of `rewo-world` so world
//! simulation and UI models rebuild independently. The dependency runs one
//! way: this crate reads world types (`chat_style`, `inventory`, `menu_layout`,
//! `stats`); `rewo-world` never names this crate.

pub mod active_text;
pub mod advancements_screen;
pub mod anvil;
pub mod book_view_screen;
pub mod chat;
pub mod chat_screen;
pub mod command_suggestions;
pub mod confirm_link_screen;
pub mod death_screen;
pub mod disconnect_screen;
pub mod edit_box;
pub mod ghost_slots;
pub mod layout;
pub mod menu_screen;
pub mod merchant_screen;
pub mod nine_slice;
pub mod options_screen;
pub mod pause_screen;
pub mod recipe_book_screen;
pub mod recipe_overlay;
pub mod recipe_search;
pub mod screen;
pub mod server_links_screen;
pub mod sign_edit_screen;
pub mod stats_screen;
