//! Fresh DeckFiles screens. This crate intentionally depends only on the neutral UI and playback
//! contract layers; it has no Plex, Jellyfin, appkit, data, session, or legacy screen dependency.

#[path = "../../screens/src/files_browser.rs"]
pub mod files_browser;
#[path = "../../screens/src/add_source_modal.rs"]
pub mod add_source_modal;
#[path = "../../screens/src/deck_settings.rs"]
pub mod deck_settings;
#[path = "../../screens/src/deck_player.rs"]
pub mod deck_player;
