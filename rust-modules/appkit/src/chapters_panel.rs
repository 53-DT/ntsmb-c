//! The playing item's chapters, as the HUD's Chapters section reads them. The strip itself is a
//! `cards::Shelf` the section's compiled state places (`player_hud::strip`); this module is the ONE read
//! of the chapter list and the one number the strip's drawing shares with it.
//!
//! Data comes from the PLAYING leaf (`metadata::playing_chapters`, loaded with `?includeChapters=1`
//! on the same fetch the track store already makes), never from `metadata::current()` — the same
//! identity rule `appkit/track_menu.rs` and `appkit/skip_pill.rs` state. Reading `current()` is what made
//! the Chapters tab vanish for every episode started from a show detail page: `current()` is then
//! the SHOW, and a show container carries no `Chapter[]`.
use plx_data::metadata;
use std::sync::atomic::{AtomicU8, Ordering};

/// Focus rim on the selected chapter. The card family's resting sheen ([`plx_ui::theme::CARD_SHEEN`] .22 /
/// 1px) washes out over the hardware video plane the same way an unkeyed control's edge did — pure
/// white and a thicker stroke are what keep "which chapter" readable from the couch while scrolling.
/// Colour is the unkeyed focus edge ([`plx_ui::theme::CONTROL_RIM_FOCUS_UNKEYED`]); width is deliberately a
/// step over that control's 1.25 so a 288-wide still reads as selected, not merely edged.
pub(crate) const CH_FOCUS_RING_W: f32 = 2.5;

/// the playing leaf's chapters — the ONE read, so within a frame the count, the open, the seek and
/// the draw cannot end up describing different items. ACROSS frames the store can still be replaced
/// (a new play retires it, `route::request_play`).
pub(crate) fn chapters<'a>(meta: metadata::MetadataView<'a>) -> &'a [metadata::Chapter] {
    meta.playing_chapters()
}

/// The `rk` the SMB player gives the item it plays (`app::playback::smb_playing_item`).
pub const SMB_RK: &str = "smb-local";

/// The label the HUD's second tab wears while an SMB video plays (see [`is_play_next`]): an arrow,
/// not a word, so it takes no more room than the tab already has.
pub const PLAY_NEXT_LABEL: &str = "\u{25B6}";

/// The SMB folder queue as the HUD sees it: bit 0 = an SMB file is playing, bit 1 = a previous
/// video exists, bit 2 = a next video exists. Written by the app each frame it builds the HUD model
/// (`app::run::hud_model`) and when a file starts; read by [`has_chapters`] and by
/// `player_hud::Controls::now`. A flag rather than a call into the SMB crate so the appkit layer
/// does not depend on it.
static SMB_QUEUE: AtomicU8 = AtomicU8::new(0);

pub fn set_smb_queue(active: bool, previous: bool, next: bool) {
    let bits = u8::from(active) | (u8::from(active && previous) << 1) | (u8::from(active && next) << 2);
    SMB_QUEUE.store(bits, Ordering::Relaxed);
}

/// `(previous, next)` of the SMB folder queue while an SMB file plays, else `None`.
pub fn smb_queue() -> Option<(bool, bool)> {
    let bits = SMB_QUEUE.load(Ordering::Relaxed);
    (bits & 1 != 0).then_some((bits & 2 != 0, bits & 4 != 0))
}

/// **Is the HUD's second tab "Play Next" rather than "Chapters"?** True while an SMB file plays.
/// Normal (Plex/Jellyfin) playback keeps the chapter list unchanged.
pub fn is_play_next(meta: metadata::MetadataView<'_>) -> bool {
    meta.playing().is_some_and(|item| item.rk == SMB_RK)
}

/// whether the second HUD tab is available — drives showing/hiding it. For a normal item: it has
/// chapters. For an SMB item: a next video exists in the folder queue, so on the last video the
/// tab is simply not offered.
pub fn has_chapters(meta: metadata::MetadataView<'_>) -> bool {
    if is_play_next(meta) {
        return smb_queue().is_some_and(|(_, next)| next);
    }
    !chapters(meta).is_empty()
}
