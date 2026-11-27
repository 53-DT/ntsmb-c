//! DeckFiles-only directional navigation state.
//!
//! This module owns the remote's spatial focus. It is deliberately inside the standalone
//! DeckFiles application crate and has no dependency on FinNative or its application router.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FocusZone {
    /// Focus is in the Files page's left navigation rail.
    Sidebar,
    /// Focus is in the active page's main content.
    #[default]
    Content,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemoteControl {
    zone: FocusZone,
    sidebar_index: usize,
}

impl RemoteControl {
    pub fn zone(&self) -> FocusZone { self.zone }
    pub fn sidebar_index(&self) -> usize { self.sidebar_index }
    pub fn focus_sidebar(&mut self) { self.zone = FocusZone::Sidebar; }
    pub fn focus_content(&mut self) { self.zone = FocusZone::Content; }

    pub fn move_sidebar(&mut self, delta: isize, item_count: usize) {
        if item_count == 0 { self.sidebar_index = 0; return; }
        self.zone = FocusZone::Sidebar;
        self.sidebar_index = (self.sidebar_index as isize + delta)
            .clamp(0, item_count as isize - 1) as usize;
    }

    pub fn reconcile_sidebar(&mut self, item_count: usize) {
        self.sidebar_index = self.sidebar_index.min(item_count.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_focus_is_bounded_and_content_is_explicit() {
        let mut remote = RemoteControl::default();
        remote.move_sidebar(-1, 5);
        assert_eq!(remote.sidebar_index(), 0);
        assert_eq!(remote.zone(), FocusZone::Sidebar);
        remote.move_sidebar(4, 5);
        assert_eq!(remote.sidebar_index(), 4);
        remote.move_sidebar(1, 5);
        assert_eq!(remote.sidebar_index(), 4);
        remote.focus_content();
        assert_eq!(remote.zone(), FocusZone::Content);
    }

    #[test]
    fn empty_sidebar_never_underflows() {
        let mut remote = RemoteControl::default();
        remote.move_sidebar(1, 0);
        remote.reconcile_sidebar(0);
        assert_eq!(remote.sidebar_index(), 0);
    }
}
