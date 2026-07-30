//! `DeathScreen` (M82) — the first consumer of [`crate::screen`].
//!
//! Everything here is `net/minecraft/client/gui/screens/DeathScreen.java`;
//! nothing here belongs in the framework module, which is the point of the
//! split. A sibling screen (`award_stats`, `server_links`) is a module of this
//! shape beside this one.
//!
//! # The layout, transcribed
//!
//! ```java
//! // init()
//! this.exitButtons.add(this.addRenderableWidget(Button.builder(message, …)
//!     .bounds(this.width / 2 - 100, this.height / 4 + 72, 200, 20).build()));
//! this.exitToTitleButton = this.addRenderableWidget(Button.builder(…)
//!     .bounds(this.width / 2 - 100, this.height / 4 + 96, 200, 20).build());
//! this.setButtonsActive(false);
//!
//! // visitText()
//! int middleLine = this.width / 2;
//! output.defaultParameters(normalParameters.withScale(2.0F));
//! output.accept(TextAlignment.CENTER, middleLine / 2, 30, this.title);
//! output.defaultParameters(normalParameters);
//! if (this.causeOfDeath != null) output.accept(CENTER, middleLine, 85, this.causeOfDeath);
//! output.accept(CENTER, middleLine, 100, this.deathScore);
//! ```
//!
//! **The title's anchor is divided twice and it is not a typo.**
//! `withScale(2.0F)` scales the *pose*, so every coordinate in that block is
//! in half-size units; `middleLine / 2` is `(width / 2) / 2`, and the run's
//! left edge is `anchor - font.width(title) / 2` in those units before the
//! pose doubles it. Both divisions truncate, so the screen-space left edge is
//! `2 * (width / 4 - w / 2)` and **not** `width / 2 - w`: for an odd-width
//! title the two differ by a pixel. That is the whole reason
//! [`title_left`] does the arithmetic in this order rather than the tidy one.
//!
//! # Three things that read backwards
//!
//! * **`shouldCloseOnEsc()` is false.** The death screen cannot be dismissed;
//!   Esc does nothing at all. And `Gui.setScreen(null)` re-opens it — with
//!   `causeOfDeath = null` — while `player.isDeadOrDying()`, so there is no
//!   path out of it except its own two buttons.
//! * **`isPauseScreen()` is false**, unlike every other screen (`Screen`'s
//!   default is true). The world keeps ticking behind it.
//! * **The buttons start dead and the guard is one second.** `init()` ends
//!   with `setButtonsActive(false)` and `tick()` re-enables them at
//!   `delayTicker == 20` — an exact equality against a counter that starts at
//!   0 and is incremented *before* the test, so the twentieth tick is the one
//!   that arms them. It is an anti-misclick guard: without it the click that
//!   killed you respawns you.

use crate::screen::{Backdrop, Screen, ScreenKind, Widget, WidgetId, BUTTON_HEIGHT, BUTTON_WIDTH};

/// The respawn button — `deathScreen.respawn`, or `deathScreen.spectate` in
/// hardcore. Its `onPress` is `player.respawn()` followed by
/// `button.active = false`, so it cannot be pressed twice.
pub const RESPAWN: WidgetId = 0;
/// The "Title Screen" button. Rewo has no title screen; see
/// [`DeathScreen::build`].
pub const TITLE_SCREEN: WidgetId = 1;

/// `DeathScreen.tick`'s `delayTicker == 20`.
pub const BUTTON_DELAY_TICKS: u32 = 20;

/// `extractDeathBackground` — `fillGradient(0, 0, width, height, 1615855616,
/// -1602211792)`.
///
/// Those two decimal literals are `0x60500000` and `0xA0803030`: a dark red at
/// 96/255 alpha over a warmer, more opaque `0x803030` at 160/255. **The alpha
/// changes down the screen as well as the colour**, which a "tint the screen
/// red" reading would miss, and it is not
/// `Screen.extractTransparentBackground`'s neutral `0x101010` pair — the death
/// screen replaces `extractBackground` outright, so it gets neither the blur
/// nor the menu texture nor the neutral dim.
pub const BACKDROP: Backdrop = Backdrop::argb(0x6050_0000, 0xA080_3030);

/// `TITLE_SCALE`.
pub const TITLE_SCALE: i32 = 2;

/// The state a death screen is built from.
///
/// `causeOfDeath` is nullable and the two producers differ: the packet path
/// passes `packet.message()`, and `Gui.setScreen(null)` re-opening the screen
/// passes **null**, so the message is lost on a re-open. Rewo keeps the
/// message on the session, so a rebuild (a window resize) keeps it — a
/// deliberate deviation, recorded because it is one: reproducing the loss
/// would mean throwing away state Rewo already holds in order to be wrong in
/// the same way.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeathScreen {
    pub cause_of_death: Option<String>,
    pub hardcore: bool,
    /// `player.getScore()` — `Player.DATA_SCORE_ID`, metadata index 18.
    pub score: i32,
}

/// The four strings the screen needs, already resolved through the language
/// map by the caller.
///
/// Passed in rather than looked up here so this module stays free of the asset
/// pipeline and can be unit-tested without a jar.
#[derive(Clone, Debug, PartialEq)]
pub struct DeathLabels {
    /// `deathScreen.title` / `deathScreen.title.hardcore`.
    pub title: String,
    /// `deathScreen.respawn` / `deathScreen.spectate`.
    pub respawn: String,
    /// `deathScreen.titleScreen`.
    pub title_screen: String,
    /// `deathScreen.score.value` — the **template**, `%s` and all.
    ///
    /// Not substituted here, and that is the point:
    /// `Component.translatable(key, literal(score).withStyle(YELLOW))` keeps
    /// the template and its argument apart until the text is laid out,
    /// because the argument carries a style the template does not. Flattening
    /// it to `"Score: 0"` at this layer would lose the colour with no way to
    /// recover it — the renderer would have nothing to split on.
    pub score_template: String,
}

/// The four lang keys, in the two pairs the hardcore flag chooses between.
pub const KEY_TITLE: &str = "deathScreen.title";
pub const KEY_TITLE_HARDCORE: &str = "deathScreen.title.hardcore";
pub const KEY_RESPAWN: &str = "deathScreen.respawn";
pub const KEY_SPECTATE: &str = "deathScreen.spectate";
pub const KEY_TITLE_SCREEN: &str = "deathScreen.titleScreen";
pub const KEY_SCORE_VALUE: &str = "deathScreen.score.value";

impl DeathScreen {
    /// Which of each pair the hardcore flag selects.
    pub fn title_key(&self) -> &'static str {
        if self.hardcore {
            KEY_TITLE_HARDCORE
        } else {
            KEY_TITLE
        }
    }

    /// `Component.translatable(hardcore ? "deathScreen.spectate" : "deathScreen.respawn")`.
    ///
    /// Note the pairing is the **opposite way round** from the title's: the
    /// hardcore *title* is the second key and the hardcore *button* is the
    /// first. Reading `hardcore ? a : b` off one and applying it to the other
    /// swaps both.
    pub fn respawn_key(&self) -> &'static str {
        if self.hardcore {
            KEY_SPECTATE
        } else {
            KEY_RESPAWN
        }
    }

    /// Resolve the four strings against a language map.
    pub fn labels(&self, lang: &rewo_data::lang::Language) -> DeathLabels {
        DeathLabels {
            title: lang.or_key(self.title_key()).to_string(),
            respawn: lang.or_key(self.respawn_key()).to_string(),
            title_screen: lang.or_key(KEY_TITLE_SCREEN).to_string(),
            score_template: lang.or_key(KEY_SCORE_VALUE).to_string(),
        }
    }

    /// `DeathScreen.init` — the two buttons, both inactive.
    ///
    /// The "Title Screen" button is built and rendered even though Rewo has no
    /// title screen, because it is half of the screen's chrome and because
    /// leaving it out would make the delay guard meaningless (with one button
    /// there is nothing to mis-click onto). What it *does* on press is the
    /// app's business: vanilla's `exitToTitleScreen` is
    /// `level.disconnect(...)` + `disconnectWithSavingScreen()` +
    /// `setScreen(new TitleScreen())`, and Rewo's equivalent of the last step
    /// is leaving the session. **The `ConfirmScreen` in between is not
    /// reproduced** — it is a second screen with a `BooleanConsumer`, and
    /// vanilla's own hardcore branch skips it, so Rewo takes the hardcore
    /// branch always. Named rather than silently dropped.
    pub fn build(&self, labels: &DeathLabels, gui_w: i32, gui_h: i32) -> Screen {
        let x = gui_w / 2 - 100;
        let mut respawn = Widget::button(
            RESPAWN,
            x,
            gui_h / 4 + 72,
            BUTTON_WIDTH,
            BUTTON_HEIGHT,
            labels.respawn.clone(),
        );
        let mut title = Widget::button(
            TITLE_SCREEN,
            x,
            gui_h / 4 + 96,
            BUTTON_WIDTH,
            BUTTON_HEIGHT,
            labels.title_screen.clone(),
        );
        // `this.setButtonsActive(false)` is the last line of `init()`.
        respawn.active = false;
        title.active = false;
        Screen::new(ScreenKind::Death, gui_w, gui_h)
            .with_widgets(vec![respawn, title])
            .with_backdrop(BACKDROP)
            // `shouldCloseOnEsc()` / `isPauseScreen()`, both overridden.
            .with_close_on_esc(false)
            .with_pause(false)
    }

    /// `DeathScreen.tick` — advance the clock and arm the buttons at exactly
    /// [`BUTTON_DELAY_TICKS`].
    ///
    /// ```java
    /// this.delayTicker++;
    /// if (this.delayTicker == 20) this.setButtonsActive(true);
    /// ```
    ///
    /// The equality is transcribed as `>=` here for one reason and it changes
    /// nothing: `Screen.ticks` is saturating and monotonic, and the respawn
    /// button's own `onPress` clears `active` afterwards — under `==` a
    /// pressed-then-still-open screen would stay dead, which is what vanilla
    /// wants, and [`Self::tick`] is only ever called on a screen that has not
    /// been pressed. The observable boundary — dead at 19 ticks, live at 20 —
    /// is identical.
    pub fn tick(screen: &mut Screen) {
        screen.tick();
        if screen.ticks >= BUTTON_DELAY_TICKS {
            for w in &mut screen.widgets {
                w.active = true;
            }
        }
    }
}

/// The title's left edge in GUI space, given `Font.width(title)`.
///
/// See the module docs: two truncating divisions, then the pose's `×2`.
pub fn title_left(gui_w: i32, font_width: i32) -> i32 {
    TITLE_SCALE * (gui_w / 2 / 2 - font_width / 2)
}

/// The title's baseline-top in GUI space — `30` in half-size units.
pub fn title_top() -> i32 {
    TITLE_SCALE * 30
}

/// `accept(CENTER, middleLine, 85, causeOfDeath)`.
pub fn cause_pos(gui_w: i32, font_width: i32) -> (i32, i32) {
    (gui_w / 2 - font_width / 2, 85)
}

/// `accept(CENTER, middleLine, 100, deathScore)`.
pub fn score_pos(gui_w: i32, font_width: i32) -> (i32, i32) {
    (gui_w / 2 - font_width / 2, 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::ButtonSprite;

    fn labels() -> DeathLabels {
        DeathLabels {
            title: "You Died!".into(),
            respawn: "Respawn".into(),
            title_screen: "Title Screen".into(),
            score_template: "Score: %s".into(),
        }
    }

    #[test]
    fn the_buttons_sit_where_the_decompile_puts_them() {
        let s = DeathScreen::default().build(&labels(), 320, 240);
        let r = s.widget(RESPAWN).unwrap();
        // 320/2 - 100 = 60 ; 240/4 + 72 = 132
        assert_eq!((r.x, r.y, r.width, r.height), (60, 132, 200, 20));
        let t = s.widget(TITLE_SCREEN).unwrap();
        // 240/4 + 96 = 156 — 24 below, i.e. the 20-px button plus 4 of gap.
        assert_eq!((t.x, t.y, t.width, t.height), (60, 156, 200, 20));
        assert_eq!(t.y - r.bottom(), 4);
    }

    #[test]
    fn both_buttons_start_dead() {
        let s = DeathScreen::default().build(&labels(), 320, 240);
        assert!(s.widgets.iter().all(|w| !w.active));
        assert!(s.widgets.iter().all(|w| w.visible));
    }

    /// The boundary the anti-misclick guard is: 19 ticks dead, 20 alive.
    #[test]
    fn the_buttons_arm_on_the_twentieth_tick_and_not_the_nineteenth() {
        let mut s = DeathScreen::default().build(&labels(), 320, 240);
        for _ in 0..(BUTTON_DELAY_TICKS - 1) {
            DeathScreen::tick(&mut s);
        }
        assert_eq!(s.ticks, 19);
        assert!(
            s.widgets.iter().all(|w| !w.active),
            "one tick short of the guard"
        );
        DeathScreen::tick(&mut s);
        assert_eq!(s.ticks, 20);
        assert!(s.widgets.iter().all(|w| w.active));
    }

    #[test]
    fn a_click_on_a_dead_button_does_nothing_and_is_not_even_consumed() {
        use crate::screen::MouseResult;
        let mut s = DeathScreen::default().build(&labels(), 320, 240);
        assert_eq!(s.mouse_clicked(160.0, 140.0, 0), MouseResult::Ignored);
        for _ in 0..BUTTON_DELAY_TICKS {
            DeathScreen::tick(&mut s);
        }
        assert_eq!(s.mouse_clicked(160.0, 140.0, 0), MouseResult::Pressed(RESPAWN));
    }

    #[test]
    fn hovering_a_dead_button_still_shows_the_disabled_sprite() {
        let s = DeathScreen::default().build(&labels(), 320, 240);
        let r = s.widget(RESPAWN).unwrap();
        assert!(r.is_hovered(Some((160.0, 140.0))));
        assert_eq!(
            r.sprite(true, false),
            ButtonSprite::Disabled,
            "the guard would be invisible if hover overrode it"
        );
    }

    #[test]
    fn the_two_hardcore_pairs_run_in_opposite_directions() {
        let normal = DeathScreen::default();
        assert_eq!(normal.title_key(), KEY_TITLE);
        assert_eq!(normal.respawn_key(), KEY_RESPAWN);
        let hard = DeathScreen {
            hardcore: true,
            ..Default::default()
        };
        assert_eq!(hard.title_key(), KEY_TITLE_HARDCORE);
        assert_eq!(hard.respawn_key(), KEY_SPECTATE);
    }

    #[test]
    fn esc_cannot_dismiss_the_death_screen() {
        use crate::screen::KeyResult;
        let mut s = DeathScreen::default().build(&labels(), 320, 240);
        assert!(!s.close_on_esc);
        assert_eq!(s.key_pressed(256, false), KeyResult::Ignored);
        assert!(!s.pause, "isPauseScreen() is false — the world keeps ticking");
    }

    /// The double truncation. `width/2/2 - w/2` then `×2` is not `width/2 - w`
    /// whenever `Font.width(title)` is odd.
    #[test]
    fn the_title_anchor_truncates_twice() {
        assert_eq!(title_left(320, 50), 2 * (80 - 25));
        assert_eq!(title_left(320, 51), 2 * (80 - 25), "51/2 == 25");
        assert_ne!(
            title_left(320, 51),
            320 / 2 - 51,
            "the tidy form is a pixel off"
        );
        assert_eq!(title_top(), 60);
        // Odd GUI widths truncate on the first division too.
        assert_eq!(title_left(321, 50), 2 * (321 / 2 / 2 - 25));
    }

    #[test]
    fn the_cause_and_score_lines_are_centred_at_the_unscaled_middle() {
        assert_eq!(cause_pos(320, 40), (140, 85));
        assert_eq!(score_pos(320, 40), (140, 100));
        assert_eq!(score_pos(320, 41).0, 160 - 20, "integer division truncates");
    }

    #[test]
    fn the_backdrop_is_the_two_argb_literals_from_the_decompile() {
        assert_eq!(BACKDROP.top[3], 96.0 / 255.0);
        assert_eq!(BACKDROP.bottom[3], 160.0 / 255.0);
        assert_eq!(BACKDROP.top[0], 80.0 / 255.0);
        assert_eq!(BACKDROP.bottom, [128.0 / 255.0, 48.0 / 255.0, 48.0 / 255.0, 160.0 / 255.0]);
        assert_ne!(
            BACKDROP.top, BACKDROP.bottom,
            "a flat fill would lose both the colour and the alpha ramp"
        );
    }

    /// The template survives the lookup unsubstituted, so the renderer still
    /// has a `%s` to split the yellow value out of.
    #[test]
    fn the_score_line_keeps_its_template_rather_than_flattening_it() {
        let lang = rewo_data::lang::Language::from_map(
            [(KEY_SCORE_VALUE.to_string(), "Score: %s".to_string())]
                .into_iter()
                .collect(),
        );
        let d = DeathScreen {
            score: 1234,
            ..Default::default()
        };
        assert_eq!(d.labels(&lang).score_template, "Score: %s");
        // A missing key falls back to the key itself, which is what
        // `Language.or_key` does everywhere else and what vanilla renders for
        // an unknown translation.
        let empty = rewo_data::lang::Language::from_map(Default::default());
        assert_eq!(
            DeathScreen::default().labels(&empty).score_template,
            KEY_SCORE_VALUE
        );
    }
}
