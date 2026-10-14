//! The sounds a screen plays by itself (M186) — `SimpleSoundInstance.forUI`.
//!
//! Every one of these is a **client-side** sound: no packet carries it, and a
//! client that forgets to play it is silent where vanilla clicks. Before M186
//! Rewo's screens played none of them (the "UI-sound family" `REWO_PLAN.md`
//! §0.0 carried as open since M172).
//!
//! The sites, from the 26.2 decompile (`grep -rn "forUI(" net/minecraft/client`):
//!
//! | Sound | Where vanilla plays it |
//! |---|---|
//! | `ui.button.click` | `AbstractWidget.playDownSound` — every active widget a left click lands on, unless the widget overrides it (below); `AbstractButton.keyPressed` on Enter/Space; `TabManager.setCurrentTab(tab, playSound = true)` when the tab CHANGES; `AbstractSliderButton.onRelease` |
//! | `item.book.page_turn` | `PageButton.playDownSound`, only when `playTurnSound` — true for a held book (`BookViewScreen(BookAccess)`), **false for the lectern** (`LecternScreen` calls the no-argument `BookViewScreen()`; the server plays the lectern's turn) |
//! | `ui.stonecutter.select_recipe` | `StonecutterScreen.mouseClicked`, after `clickMenuButton` accepted the cell |
//! | `ui.loom.select_pattern` | `LoomScreen.mouseClicked`, the same shape |
//!
//! The overrides that play **nothing** on press: `AbstractSliderButton` (it
//! plays on *release* instead), `TabButton` (the tab manager plays it, and only
//! for a change), `PageButton` without `playTurnSound`, and the passive
//! widgets (`ImageWidget`, `StringWidget`, which are inactive anyway).
//!
//! Not here: toasts (Rewo has none), the sound-options preview
//! (`SoundPreviewHandler`, a title-screen feature), and the crafter's slot
//! toggle, which is `player.playSound(...)` — a positioned world sound at the
//! player, not a UI one.
//!
//! All four go through the two-argument `forUI(sound, pitch)` with pitch 1.0,
//! which is volume **0.25** — see `rewo_net::sound_instance::SoundInstance::for_ui`.

/// One client-side UI sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiSound {
    /// `SoundEvents.UI_BUTTON_CLICK`.
    ButtonClick,
    /// `SoundEvents.BOOK_PAGE_TURN`.
    PageTurn,
    /// `SoundEvents.UI_STONECUTTER_SELECT_RECIPE`.
    StonecutterSelect,
    /// `SoundEvents.UI_LOOM_SELECT_PATTERN`.
    LoomSelect,
}

impl UiSound {
    pub const ALL: [UiSound; 4] = [
        UiSound::ButtonClick,
        UiSound::PageTurn,
        UiSound::StonecutterSelect,
        UiSound::LoomSelect,
    ];

    /// The sound event's identifier, as `SoundEvents.java` registers it.
    pub fn event(self) -> &'static str {
        match self {
            UiSound::ButtonClick => "minecraft:ui.button.click",
            UiSound::PageTurn => "minecraft:item.book.page_turn",
            UiSound::StonecutterSelect => "minecraft:ui.stonecutter.select_recipe",
            UiSound::LoomSelect => "minecraft:ui.loom.select_pattern",
        }
    }

    /// The `forUI` pitch. Every site passes `1.0F`.
    pub fn pitch(self) -> f32 {
        1.0
    }
}

/// The sounds a screen asked for since the app last drained them.
///
/// A queue rather than a direct call because the click handlers live in
/// `rewo-ui` and the app's input arms, neither of which owns the sound engine.
/// The app drains it once per frame, so a click is heard within a frame of the
/// press — vanilla plays on the press itself, and a frame is below what a
/// listener can separate from it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiSounds {
    queued: Vec<UiSound>,
}

impl UiSounds {
    /// Bound on what one frame can queue. A frame never legitimately holds
    /// more than a handful of presses; the cap only stops a stuck drain from
    /// growing without limit.
    pub const MAX_QUEUED: usize = 64;

    pub fn play(&mut self, sound: UiSound) {
        if self.queued.len() < Self::MAX_QUEUED {
            self.queued.push(sound);
        }
    }

    pub fn take(&mut self) -> Vec<UiSound> {
        std::mem::take(&mut self.queued)
    }

    pub fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    pub fn queued(&self) -> &[UiSound] {
        &self.queued
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_is_a_namespaced_id_and_distinct() {
        let mut seen = std::collections::HashSet::new();
        for s in UiSound::ALL {
            assert!(s.event().starts_with("minecraft:"), "{s:?}");
            assert!(seen.insert(s.event()), "{s:?} shares an id");
            assert_eq!(s.pitch(), 1.0);
        }
    }

    #[test]
    fn the_queue_drains_in_press_order_and_caps() {
        let mut q = UiSounds::default();
        q.play(UiSound::ButtonClick);
        q.play(UiSound::PageTurn);
        assert_eq!(q.take(), vec![UiSound::ButtonClick, UiSound::PageTurn]);
        assert!(q.is_empty());
        for _ in 0..UiSounds::MAX_QUEUED + 10 {
            q.play(UiSound::ButtonClick);
        }
        assert_eq!(q.queued().len(), UiSounds::MAX_QUEUED);
    }
}
