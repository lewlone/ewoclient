//! The screen framework (M82) — `Screen`, `AbstractWidget` and the input
//! routing they share.
//!
//! Before this, Rewo had exactly one screen (the inventory, M35–M43) and its
//! state was `open: bool` plus a mouse position. Three clientbound packets are
//! blocked behind a real framework — `player_combat_kill` (68, the death
//! screen), `award_stats` (3, the statistics screen) and `server_links` (137,
//! which renders on the pause and disconnect screens) — so this module is the
//! part all three share, transcribed from `Screen`, `AbstractContainerEventHandler`,
//! `ContainerEventHandler`, `AbstractWidget`, `AbstractButton` and
//! `WidgetSprites` in the 26.2 decompile.
//!
//! # There is no stack — vanilla has one screen slot
//!
//! The obvious model for "a screen framework" is a stack, and vanilla does not
//! have one. `Gui.screen` is a **single field** and `Gui.setScreen` *replaces*
//! it, calling `removed()` on the outgoing screen and `added()` + `init()` on
//! the incoming one. The nesting that looks like a stack — the death screen's
//! "Title Screen" button opening a `ConfirmScreen` — is a replacement carrying
//! a **callback**: `DeathScreen.handleExitToTitleScreen` builds a
//! `TitleConfirmScreen` whose `BooleanConsumer` either quits or respawns, and
//! nothing anywhere pops back to the `DeathScreen` instance. So [`Screens`] is
//! one slot, and a screen that wants a "back" target carries it itself.
//!
//! # Where a new screen registers
//!
//! [`ScreenKind`] is the registry: one variant per screen, and it is a **tag**,
//! not the screen's content. A new screen is
//!
//! 1. a variant here,
//! 2. a module beside this one that owns its layout and builds a [`Screen`]
//!    (see [`crate::death_screen`]), and
//! 3. two arms in the app — one that rebuilds it on resize, one that acts on
//!    the [`WidgetId`] a press returns.
//!
//! Nothing about a particular screen belongs in this module.
//!
//! # What is deliberately not here
//!
//! **Arrow-key focus navigation.** `ContainerEventHandler.handleArrowNavigation`
//! is a directional search (`nextFocusPathInDirection`, with a
//! `nextFocusPathVaguelyInDirection` fallback when the strict pass finds
//! nothing) and half of it is worse than none: a partial transcription would
//! move focus somewhere plausible and wrong. Tab navigation reaches the same
//! set on every screen Rewo has, so [`Screen::key_pressed`] implements Tab
//! exactly and leaves the four arrow keys inert. Named here rather than
//! silently omitted.

/// Which screen is up.
///
/// A tag the app dispatches on — see the module docs. Deliberately carries no
/// per-screen data, so adding a screen cannot widen this type.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum ScreenKind {
    /// The player inventory (M35).
    Inventory,
    /// The death screen (M82).
    Death,
}

/// A widget's identity within its screen. The screen's own module owns the
/// constants; the framework only hands the number back.
pub type WidgetId = u32;

/// `AbstractButton.SPRITES.get(active, hoveredOrFocused)`.
///
/// **The three-argument `WidgetSprites` constructor is the load-bearing
/// detail.** `AbstractButton` builds
/// `new WidgetSprites(button, button_disabled, button_highlighted)`, and that
/// overload is
///
/// ```text
/// WidgetSprites(enabled, disabled, focused) -> this(enabled, disabled, focused, disabled)
///                                                                              ^^^^^^^^
/// ```
///
/// so `disabledFocused` is **`button_disabled`, not a highlighted variant**.
/// Hovering a disabled button therefore changes nothing at all — the obvious
/// reading ("hover highlights, active or not") is wrong, and it is wrong in a
/// direction a screenshot would not settle, because the disabled sprite is
/// already dim.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonSprite {
    /// `widget/button`.
    Enabled,
    /// `widget/button_disabled`.
    Disabled,
    /// `widget/button_highlighted`.
    Highlighted,
}

/// `AbstractWidget.WithInactiveMessage.defaultInactiveMessage` —
/// `Style.EMPTY.withColor(-6250336)`, i.e. `0xFFA0A0A0`.
///
/// An inactive button does not merely draw a dimmer sprite: `getMessage()` is
/// overridden to return a **grey-styled copy** of the label, so the text is a
/// second, independent tell.
pub const INACTIVE_LABEL: [f32; 3] = [160.0 / 255.0, 160.0 / 255.0, 160.0 / 255.0];

/// `ARGB.white(opacity)` — the collector's default text colour when a
/// component carries no style colour of its own.
pub const DEFAULT_LABEL: [f32; 3] = [1.0, 1.0, 1.0];

/// `Button.DEFAULT_HEIGHT`.
pub const BUTTON_HEIGHT: i32 = 20;
/// `Button.BIG_WIDTH` — and the width of the three `widget/button*` sprites.
pub const BUTTON_WIDTH: i32 = 200;

/// One `AbstractWidget`, flattened to the parts a button needs.
///
/// Rewo has no text fields, sliders or lists yet, so this is the whole widget
/// model: a rectangle, the two flags every `AbstractWidget` carries, and a
/// label. When a sibling milestone needs a second widget *shape* the honest
/// move is a `kind` field here, not a parallel type — the routing below is
/// written against `AbstractWidget`, not against buttons.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub id: WidgetId,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// `AbstractWidget.active`.
    pub active: bool,
    /// `AbstractWidget.visible`.
    pub visible: bool,
    pub message: String,
}

impl Widget {
    /// `Button.builder(message, …).bounds(x, y, width, height).build()`.
    pub fn button(
        id: WidgetId,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id,
            x,
            y,
            width,
            height,
            active: true,
            visible: true,
            message: message.into(),
        }
    }

    /// `AbstractWidget.getRight`.
    pub fn right(&self) -> i32 {
        self.x + self.width
    }

    /// `AbstractWidget.getBottom`.
    pub fn bottom(&self) -> i32 {
        self.y + self.height
    }

    /// `AbstractWidget.isActive` — `visible && active`, in that order and both
    /// of them. A screen that hides a widget must not still route clicks to it.
    pub fn is_active(&self) -> bool {
        self.visible && self.active
    }

    /// `AbstractWidget.areCoordinatesInRectangle` — **half-open on the far
    /// edges**:
    ///
    /// ```text
    /// x >= getX() && y >= getY() && x < getRight() && y < getBottom()
    /// ```
    ///
    /// so a click at exactly `x + width` misses and one at `x + width - 1`
    /// hits. Closing the interval would make two edge-to-edge widgets both
    /// claim the shared column.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x as f64
            && y >= self.y as f64
            && x < self.right() as f64
            && y < self.bottom() as f64
    }

    /// `AbstractWidget.isMouseOver` — `isActive() && areCoordinatesInRectangle`.
    ///
    /// This is what `ContainerEventHandler.getChildAt` uses, so **an inactive
    /// widget is not found at all** and the click falls through the screen
    /// rather than being swallowed.
    pub fn is_mouse_over(&self, x: f64, y: f64) -> bool {
        self.is_active() && self.contains(x, y)
    }

    /// `AbstractWidget.isHovered`, as assigned in `extractRenderState`:
    ///
    /// ```java
    /// this.isHovered = graphics.containsPointInScissor(mouseX, mouseY)
    ///               && this.areCoordinatesInRectangle(mouseX, mouseY);
    /// ```
    ///
    /// **It does not test `isActive()`**, unlike [`Self::is_mouse_over`]. The
    /// asymmetry is real and observable only through the cursor icon
    /// (`handleCursor` asks `isHovered()` and then picks `POINTING_HAND` or
    /// `NOT_ALLOWED`); it does not reach the sprite, because
    /// [`ButtonSprite`]'s table collapses disabled-and-hovered back to
    /// disabled.
    pub fn is_hovered(&self, mouse: Option<(f64, f64)>) -> bool {
        self.visible && mouse.is_some_and(|(x, y)| self.contains(x, y))
    }

    /// `SPRITES.get(this.active, isHoveredOrFocused())`.
    ///
    /// Note the first argument is the raw `active` field, **not** `isActive()`
    /// — an invisible-but-active widget would ask for the enabled sprite. It
    /// never gets drawn, so the distinction is inert; it is transcribed rather
    /// than tidied because tidying it is the kind of edit that is right until
    /// a widget starts toggling `visible`.
    pub fn sprite(&self, hovered: bool, focused: bool) -> ButtonSprite {
        match (self.active, hovered || focused) {
            (true, true) => ButtonSprite::Highlighted,
            (true, false) => ButtonSprite::Enabled,
            // `disabledFocused == disabled` — see [`ButtonSprite`].
            (false, _) => ButtonSprite::Disabled,
        }
    }

    /// The colour `getMessage()` resolves to: white while active, `0xA0A0A0`
    /// once `active` goes false.
    pub fn label_color(&self) -> [f32; 3] {
        if self.active {
            DEFAULT_LABEL
        } else {
            INACTIVE_LABEL
        }
    }

    /// `AbstractButton.extractDefaultLabel` →
    /// `extractScrollingStringOverContents(output, getMessage(), 2)` →
    /// `acceptScrollingWithDefaultCenter(message, left, right, top, bottom)`.
    ///
    /// Returns the label's `(anchor_x, top_y)` in the screen's own space, for
    /// a label that fits (Rewo has no scrolling label). Both come from
    /// `defaultScrollingHelper`:
    ///
    /// ```java
    /// int textTop = (top + bottom - lineHeight) / 2 + 1;      // lineHeight = 9
    /// int textX   = Mth.clamp(centerX, left + lineWidth / 2, right - lineWidth / 2);
    /// this.accept(TextAlignment.CENTER, textX, textTop, message);
    /// ```
    ///
    /// with `left = x + 2`, `right = x + width - 2` (`TEXT_MARGIN`), and
    /// `centerX = (left + right) / 2`. The `+ 1` on `textTop` is not
    /// decorative: without it a 20-px button's 9-px line sits a pixel high.
    pub fn label_anchor(&self, line_width: i32) -> (i32, i32) {
        const LINE_HEIGHT: i32 = 9;
        const TEXT_MARGIN: i32 = 2;
        let left = self.x + TEXT_MARGIN;
        let right = self.right() - TEXT_MARGIN;
        let top = self.y;
        let bottom = self.bottom();
        let text_top = (top + bottom - LINE_HEIGHT) / 2 + 1;
        let center_x = (left + right) / 2;
        let lo = left + line_width / 2;
        let hi = right - line_width / 2;
        // `Mth.clamp(value, min, max)`. A label wider than the button makes
        // `lo > hi`; vanilla takes the scrolling branch there instead, so this
        // clamp is only ever reached with `lo <= hi`.
        (center_x.clamp(lo.min(hi), hi.max(lo)), text_top)
    }
}

/// What a click did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseResult {
    /// A widget was pressed. The app acts on the id.
    Pressed(WidgetId),
    /// The click landed on a widget but did not press it, and the screen
    /// still consumed it.
    ///
    /// This is `ContainerEventHandler.mouseClicked`'s real contract: it
    /// returns **true whenever `getChildAt` found something**, whatever the
    /// child's own `mouseClicked` answered. A right-click on a button is
    /// therefore eaten by the screen and never reaches the world.
    Consumed,
    /// No widget was under the cursor. The screen's own `mouseClicked` (which
    /// the app supplies — the inventory's slot handling is exactly this) gets
    /// the click.
    Ignored,
}

/// What a key press did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyResult {
    /// The focused widget was activated by Enter / Space / KP-Enter.
    Pressed(WidgetId),
    /// Esc on a screen whose `shouldCloseOnEsc()` is true.
    Close,
    /// Focus moved (Tab). Consumed by the screen.
    Handled,
    /// Nothing here wanted it.
    Ignored,
}

/// GLFW key codes, which are what `KeyEvent.key()` carries.
mod keys {
    pub const ESCAPE: i32 = 256;
    pub const ENTER: i32 = 257;
    pub const TAB: i32 = 258;
    pub const SPACE: i32 = 32;
    pub const KP_ENTER: i32 = 335;
}

/// `InputWithModifiers.isSelection` — `ENTER || SPACE || KP_ENTER`.
///
/// Distinct from `isConfirmation`, which drops SPACE. A button takes the
/// former.
pub fn is_selection(key: i32) -> bool {
    key == keys::ENTER || key == keys::SPACE || key == keys::KP_ENTER
}

/// The vertical gradient a screen paints behind itself.
///
/// `Screen.extractBackground` has three cases and a screen may replace all of
/// them, which is what the death screen does. Both stops are sRGB + straight
/// alpha, and **`col1` is the top**: `ColoredRectangleRenderState.buildVertices`
/// writes `col1` at `y0`, `col2` at `y1`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Backdrop {
    pub top: [f32; 4],
    pub bottom: [f32; 4],
}

impl Backdrop {
    /// Unpack an `ARGB` int the way `fillGradient`'s callers write them.
    pub const fn argb(top: u32, bottom: u32) -> Self {
        Self {
            top: unpack(top),
            bottom: unpack(bottom),
        }
    }

    /// `Screen.extractTransparentBackground` — the in-world menu dim,
    /// `0xC0101010` → `0xD0101010`. Already shipped for the inventory in
    /// `rewo_gpu::container`; restated here so a new screen can ask for it by
    /// name.
    pub const TRANSPARENT: Self = Self::argb(0xC010_1010, 0xD010_1010);
}

const fn unpack(argb: u32) -> [f32; 4] {
    [
        ((argb >> 16) & 0xFF) as f32 / 255.0,
        ((argb >> 8) & 0xFF) as f32 / 255.0,
        (argb & 0xFF) as f32 / 255.0,
        ((argb >> 24) & 0xFF) as f32 / 255.0,
    ]
}

/// One open screen: its widgets, its focus, and its own tick clock.
#[derive(Clone, Debug)]
pub struct Screen {
    pub kind: ScreenKind,
    pub widgets: Vec<Widget>,
    pub backdrop: Option<Backdrop>,
    /// `Screen.shouldCloseOnEsc` — **true by default, and the death screen
    /// overrides it to false.** A screen that returns false cannot be
    /// dismissed at all; it must be left through one of its own buttons.
    pub close_on_esc: bool,
    /// `Screen.isPauseScreen`. Not acted on by Rewo (there is no single-player
    /// pause), decoded because the two screens disagree — the inventory is a
    /// pause screen and the death screen explicitly is not.
    pub pause: bool,
    /// The GUI-space size this screen's widgets were laid out for, so the app
    /// can notice a resize and rebuild — `Screen.resize` → `repositionElements`
    /// → `rebuildWidgets`.
    pub width: i32,
    pub height: i32,
    /// The screen's own tick counter, incremented by [`Self::tick`].
    ///
    /// Vanilla screens keep their own (`DeathScreen.delayTicker`); one counter
    /// here serves all of them, because `Screen.tick()` is called once per
    /// client tick for whatever screen is up and every use so far is "how long
    /// have I been open".
    pub ticks: u32,
    focused: Option<usize>,
}

impl Screen {
    pub fn new(kind: ScreenKind, width: i32, height: i32) -> Self {
        Self {
            kind,
            widgets: Vec::new(),
            backdrop: None,
            close_on_esc: true,
            pause: true,
            width,
            height,
            ticks: 0,
            focused: None,
        }
    }

    pub fn with_widgets(mut self, widgets: Vec<Widget>) -> Self {
        self.widgets = widgets;
        self
    }

    pub fn with_backdrop(mut self, backdrop: Backdrop) -> Self {
        self.backdrop = Some(backdrop);
        self
    }

    pub fn with_close_on_esc(mut self, close_on_esc: bool) -> Self {
        self.close_on_esc = close_on_esc;
        self
    }

    pub fn with_pause(mut self, pause: bool) -> Self {
        self.pause = pause;
        self
    }

    /// `Screen.tick()` — once per client tick, 20 Hz.
    pub fn tick(&mut self) {
        self.ticks = self.ticks.saturating_add(1);
    }

    pub fn widget(&self, id: WidgetId) -> Option<&Widget> {
        self.widgets.iter().find(|w| w.id == id)
    }

    pub fn widget_mut(&mut self, id: WidgetId) -> Option<&mut Widget> {
        self.widgets.iter_mut().find(|w| w.id == id)
    }

    /// The currently focused widget's id, if any.
    ///
    /// `Screen.setInitialFocus` only focuses anything when
    /// `minecraft.getLastInputType().isKeyboard()`, so a screen opened with
    /// the mouse starts with **nothing** focused. Rewo opens every screen from
    /// a key or a packet and then drives it with the mouse; starting unfocused
    /// matches what a player sees.
    pub fn focused(&self) -> Option<WidgetId> {
        self.focused.and_then(|i| self.widgets.get(i)).map(|w| w.id)
    }

    /// `ContainerEventHandler.getChildAt` — the **first** child whose
    /// `isMouseOver` is true, in insertion order. Not the topmost, not the
    /// nearest.
    fn child_at(&self, x: f64, y: f64) -> Option<usize> {
        self.widgets.iter().position(|w| w.is_mouse_over(x, y))
    }

    /// The widget the cursor is over for *rendering* purposes — `isHovered`,
    /// which ignores `active`.
    pub fn hovered(&self, mouse: Option<(f64, f64)>) -> Option<WidgetId> {
        self.widgets
            .iter()
            .find(|w| w.is_hovered(mouse))
            .map(|w| w.id)
    }

    /// `Screen.mouseClicked` → `ContainerEventHandler.mouseClicked` →
    /// `AbstractWidget.mouseClicked` → `AbstractButton.onClick`.
    ///
    /// `button` is the GLFW button number; `isValidClickButton` is
    /// `button == 0`, so only the left button presses. The focus assignment is
    /// vanilla's: a widget that handled the click and answers
    /// `shouldTakeFocusAfterInteraction()` (the default, `true`) becomes the
    /// focused one — which is why clicking a button and then pressing Enter
    /// presses it again.
    pub fn mouse_clicked(&mut self, x: f64, y: f64, button: u8) -> MouseResult {
        let Some(i) = self.child_at(x, y) else {
            return MouseResult::Ignored;
        };
        // `AbstractWidget.mouseClicked`: `isActive()` is already true (that is
        // what `getChildAt` tested) and so is `isMouseOver`, so the only
        // remaining gate is the button number.
        if button == 0 {
            self.set_focused(Some(i));
            MouseResult::Pressed(self.widgets[i].id)
        } else {
            MouseResult::Consumed
        }
    }

    /// `Screen.keyPressed`, in its order.
    ///
    /// 1. Esc, if `shouldCloseOnEsc()`.
    /// 2. `ContainerEventHandler.keyPressed` — the focused widget only, and
    ///    `AbstractButton.keyPressed` requires `isActive()` before
    ///    `isSelection()`.
    /// 3. Tab (258) → `TabNavigation(!hasShiftDown())`.
    ///
    /// Vanilla's final statement is `return false;` even after a successful
    /// navigation, which reads like a bug and is not one: nothing above the
    /// screen consumes key presses, so the return value is only read by
    /// `Screen`'s own callers. Rewo reports [`KeyResult::Handled`] instead,
    /// because Rewo *does* have something above the screen — the world input —
    /// and letting Tab fall through to it would move the hotbar.
    pub fn key_pressed(&mut self, key: i32, shift: bool) -> KeyResult {
        if key == keys::ESCAPE && self.close_on_esc {
            return KeyResult::Close;
        }
        if let Some(i) = self.focused {
            if let Some(w) = self.widgets.get(i) {
                if w.is_active() && is_selection(key) {
                    return KeyResult::Pressed(w.id);
                }
            }
        }
        if key == keys::TAB {
            self.cycle_focus(!shift);
            return KeyResult::Handled;
        }
        KeyResult::Ignored
    }

    /// `ContainerEventHandler.handleTabNavigation`, plus the retry
    /// `Screen.keyPressed` wraps it in.
    ///
    /// ```java
    /// int index = sortedChildren.indexOf(focus);
    /// if (focus != null && index >= 0) newIndex = index + (forward ? 1 : 0);
    /// else if (forward)               newIndex = 0;
    /// else                            newIndex = sortedChildren.size();
    /// ```
    ///
    /// The asymmetric `+ (forward ? 1 : 0)` is not a typo: a backward
    /// `ListIterator.previous()` from index `i` yields element `i - 1`, so
    /// both directions step by one. And the wrap is not in this function — it
    /// is `Screen.keyPressed` retrying after `clearFocus()` when the first
    /// pass finds nothing, which is why tabbing off the end lands on the first
    /// widget rather than staying put.
    ///
    /// `AbstractWidget.nextFocusPath` returns null for an inactive widget and
    /// for the already-focused one, so both are skipped.
    fn cycle_focus(&mut self, forward: bool) {
        let n = self.widgets.len();
        if n == 0 {
            return;
        }
        let focus = self.focused;
        let start = match focus {
            Some(i) if forward => i + 1,
            Some(i) => i,
            None if forward => 0,
            None => n,
        };
        if let Some(next) = self.scan(start, forward, focus) {
            self.set_focused(Some(next));
            return;
        }
        // `clearFocus(); focusPath = super.nextFocusPath(navigationEvent);`
        self.set_focused(None);
        let restart = if forward { 0 } else { n };
        if let Some(next) = self.scan(restart, forward, None) {
            self.set_focused(Some(next));
        }
    }

    /// The `ListIterator` walk: forward yields `start, start+1, …`, backward
    /// yields `start-1, start-2, …`.
    fn scan(&self, start: usize, forward: bool, skip: Option<usize>) -> Option<usize> {
        let n = self.widgets.len();
        if forward {
            (start..n).find(|&i| self.widgets[i].is_active() && Some(i) != skip)
        } else {
            (0..start.min(n))
                .rev()
                .find(|&i| self.widgets[i].is_active() && Some(i) != skip)
        }
    }

    /// `AbstractContainerEventHandler.setFocused`.
    fn set_focused(&mut self, index: Option<usize>) {
        self.focused = index;
    }

    /// `Screen.clearFocus`.
    pub fn clear_focus(&mut self) {
        self.focused = None;
    }

    /// Focus a widget by id — the seam `Screen.setInitialFocus(GuiEventListener)`
    /// gives a screen that wants a default button.
    pub fn focus(&mut self, id: WidgetId) {
        self.focused = self.widgets.iter().position(|w| w.id == id);
    }
}

/// The one screen slot — `Minecraft.screen` / `Gui.screen`.
///
/// See the module docs for why this is not a stack.
#[derive(Default, Debug)]
pub struct Screens {
    current: Option<Screen>,
}

impl Screens {
    /// `Gui.setScreen(screen)`. Replaces whatever was there.
    pub fn open(&mut self, screen: Screen) {
        self.current = Some(screen);
    }

    /// `Gui.setScreen(null)`.
    ///
    /// Vanilla's `setScreen(null)` is *not* unconditionally "no screen": if the
    /// player `isDeadOrDying()` it substitutes a fresh `DeathScreen` (or
    /// respawns), which is what makes the death screen inescapable. That rule
    /// belongs to the caller that knows whether the player is dead, not here.
    pub fn close(&mut self) {
        self.current = None;
    }

    pub fn kind(&self) -> Option<ScreenKind> {
        self.current.as_ref().map(|s| s.kind)
    }

    pub fn is(&self, kind: ScreenKind) -> bool {
        self.kind() == Some(kind)
    }

    pub fn is_open(&self) -> bool {
        self.current.is_some()
    }

    pub fn current(&self) -> Option<&Screen> {
        self.current.as_ref()
    }

    pub fn current_mut(&mut self) -> Option<&mut Screen> {
        self.current.as_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_buttons() -> Screen {
        Screen::new(ScreenKind::Death, 320, 240).with_widgets(vec![
            Widget::button(0, 60, 100, BUTTON_WIDTH, BUTTON_HEIGHT, "A"),
            Widget::button(1, 60, 124, BUTTON_WIDTH, BUTTON_HEIGHT, "B"),
        ])
    }

    #[test]
    fn the_hit_rectangle_is_half_open_on_the_far_edges() {
        let w = Widget::button(0, 10, 20, 200, 20, "x");
        assert!(w.contains(10.0, 20.0), "the near edges are inclusive");
        assert!(w.contains(209.999, 39.999));
        assert!(!w.contains(210.0, 30.0), "x == right misses");
        assert!(!w.contains(100.0, 40.0), "y == bottom misses");
        assert!(!w.contains(9.999, 30.0));
    }

    #[test]
    fn an_inactive_widget_is_not_moused_over_but_is_still_hovered() {
        let mut w = Widget::button(0, 0, 0, 200, 20, "x");
        w.active = false;
        assert!(!w.is_mouse_over(5.0, 5.0), "isMouseOver tests isActive()");
        assert!(
            w.is_hovered(Some((5.0, 5.0))),
            "isHovered does not — the asymmetry is vanilla's"
        );
        w.visible = false;
        assert!(!w.is_hovered(Some((5.0, 5.0))), "invisible is never hovered");
    }

    /// The `WidgetSprites` three-argument constructor, in all four states.
    #[test]
    fn a_disabled_button_shows_the_same_sprite_hovered_or_not() {
        let mut w = Widget::button(0, 0, 0, 200, 20, "x");
        assert_eq!(w.sprite(false, false), ButtonSprite::Enabled);
        assert_eq!(w.sprite(true, false), ButtonSprite::Highlighted);
        assert_eq!(w.sprite(false, true), ButtonSprite::Highlighted);
        w.active = false;
        assert_eq!(w.sprite(false, false), ButtonSprite::Disabled);
        assert_eq!(
            w.sprite(true, false),
            ButtonSprite::Disabled,
            "disabledFocused == disabled"
        );
        assert_eq!(w.sprite(false, true), ButtonSprite::Disabled);
    }

    #[test]
    fn an_inactive_label_is_grey() {
        let mut w = Widget::button(0, 0, 0, 200, 20, "x");
        assert_eq!(w.label_color(), [1.0, 1.0, 1.0]);
        w.active = false;
        assert_eq!(w.label_color(), INACTIVE_LABEL);
        assert_eq!(w.label_color(), [160.0 / 255.0; 3]);
    }

    #[test]
    fn the_label_sits_one_pixel_below_the_true_vertical_centre() {
        let w = Widget::button(0, 60, 100, 200, 20, "x");
        // (100 + 120 - 9) / 2 + 1 = 105 + 1
        assert_eq!(w.label_anchor(30).1, 106);
        // centre: ((62) + (258)) / 2 = 160
        assert_eq!(w.label_anchor(30).0, 160);
    }

    #[test]
    fn only_the_left_button_presses_and_the_screen_eats_the_rest() {
        let mut s = two_buttons();
        assert_eq!(s.mouse_clicked(70.0, 105.0, 0), MouseResult::Pressed(0));
        assert_eq!(
            s.mouse_clicked(70.0, 105.0, 1),
            MouseResult::Consumed,
            "a right click on a widget is still consumed by the screen"
        );
        assert_eq!(
            s.mouse_clicked(5.0, 5.0, 0),
            MouseResult::Ignored,
            "nothing under the cursor falls through"
        );
    }

    #[test]
    fn an_inactive_button_is_invisible_to_the_click_router() {
        let mut s = two_buttons();
        s.widget_mut(0).unwrap().active = false;
        assert_eq!(
            s.mouse_clicked(70.0, 105.0, 0),
            MouseResult::Ignored,
            "getChildAt uses isMouseOver, which is gated on isActive()"
        );
    }

    #[test]
    fn a_click_focuses_what_it_pressed() {
        let mut s = two_buttons();
        assert_eq!(s.focused(), None, "a screen opens unfocused");
        s.mouse_clicked(70.0, 128.0, 0);
        assert_eq!(s.focused(), Some(1));
    }

    #[test]
    fn tab_walks_forward_and_wraps_through_a_cleared_focus() {
        let mut s = two_buttons();
        assert_eq!(s.key_pressed(258, false), KeyResult::Handled);
        assert_eq!(s.focused(), Some(0));
        s.key_pressed(258, false);
        assert_eq!(s.focused(), Some(1));
        s.key_pressed(258, false);
        assert_eq!(s.focused(), Some(0), "off the end, clearFocus() then retry");
    }

    #[test]
    fn shift_tab_walks_backward_from_the_end() {
        let mut s = two_buttons();
        s.key_pressed(258, true);
        assert_eq!(s.focused(), Some(1), "unfocused + backward starts at size");
        s.key_pressed(258, true);
        assert_eq!(s.focused(), Some(0));
        s.key_pressed(258, true);
        assert_eq!(s.focused(), Some(1));
    }

    #[test]
    fn tab_skips_an_inactive_widget() {
        let mut s = two_buttons();
        s.widget_mut(0).unwrap().active = false;
        s.key_pressed(258, false);
        assert_eq!(s.focused(), Some(1));
        s.key_pressed(258, false);
        assert_eq!(s.focused(), Some(1), "the only candidate stays");
    }

    #[test]
    fn enter_space_and_keypad_enter_press_the_focused_button() {
        for key in [257, 32, 335] {
            let mut s = two_buttons();
            s.focus(1);
            assert_eq!(s.key_pressed(key, false), KeyResult::Pressed(1), "{key}");
        }
        let mut s = two_buttons();
        s.focus(1);
        assert_eq!(
            s.key_pressed(69, false),
            KeyResult::Ignored,
            "E is not a selection key"
        );
    }

    #[test]
    fn an_inactive_focused_button_cannot_be_pressed_by_the_keyboard() {
        let mut s = two_buttons();
        s.focus(0);
        s.widget_mut(0).unwrap().active = false;
        assert_eq!(s.key_pressed(257, false), KeyResult::Ignored);
    }

    #[test]
    fn esc_closes_only_when_the_screen_allows_it() {
        let mut s = two_buttons();
        assert_eq!(s.key_pressed(256, false), KeyResult::Close);
        let mut s = two_buttons().with_close_on_esc(false);
        assert_eq!(
            s.key_pressed(256, false),
            KeyResult::Ignored,
            "shouldCloseOnEsc() == false means Esc does nothing at all"
        );
    }

    #[test]
    fn the_arrow_keys_are_deliberately_inert() {
        let mut s = two_buttons();
        for key in [262, 263, 264, 265] {
            assert_eq!(s.key_pressed(key, false), KeyResult::Ignored, "{key}");
            assert_eq!(s.focused(), None);
        }
    }

    #[test]
    fn the_backdrop_unpacks_argb_with_col1_on_top() {
        let b = Backdrop::argb(0x6050_0000, 0xA080_3030);
        assert_eq!(b.top, [80.0 / 255.0, 0.0, 0.0, 96.0 / 255.0]);
        assert_eq!(
            b.bottom,
            [128.0 / 255.0, 48.0 / 255.0, 48.0 / 255.0, 160.0 / 255.0]
        );
        assert_eq!(
            Backdrop::TRANSPARENT.top,
            [16.0 / 255.0, 16.0 / 255.0, 16.0 / 255.0, 192.0 / 255.0]
        );
    }

    #[test]
    fn the_slot_holds_one_screen_and_a_second_open_replaces_it() {
        let mut s = Screens::default();
        assert!(!s.is_open());
        s.open(Screen::new(ScreenKind::Inventory, 320, 240));
        assert!(s.is(ScreenKind::Inventory));
        s.open(Screen::new(ScreenKind::Death, 320, 240));
        assert!(s.is(ScreenKind::Death));
        assert!(!s.is(ScreenKind::Inventory), "there is no stack to pop back to");
        s.close();
        assert_eq!(s.kind(), None);
    }
}
