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
    /// The statistics screen (M84).
    Stats,
    /// `PauseScreen(showPauseMenu = true)` — Esc with a session behind it
    /// (M85).
    Pause,
    /// `ServerLinksDialogScreen` for the built-in `Dialogs.SERVER_LINKS`
    /// (M85). Reached from [`Self::Pause`], never opened directly.
    ServerLinks,
    /// `DisconnectedScreen` (M85) — **the one screen with no session behind
    /// it.** Every other variant here is opened while a world is loaded.
    Disconnected,
    /// `OptionsSubScreen` and its root (M157). One variant for all three
    /// pages, because vanilla has one screen SLOT and the pages replace one
    /// another in it — see [`crate::options_screen::OptionsPage`], which is
    /// what distinguishes them.
    Options,
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
/// `Font.lineHeight`. The 9 that appears in every `StringWidget`,
/// `MultiLineTextWidget` and `defaultScrollingHelper`.
pub const LINE_HEIGHT: i32 = 9;

/// One GUI sheet this framework can blit.
///
/// A mirror of the sprite *names* `rewo_gpu::screen` packs, for the same
/// crate-boundary reason [`ButtonSprite`] is one: the world crate decides
/// *which* sprite a widget shows and the gpu crate owns *where it lives*.
/// The list grows when a screen needs a sheet, which is the honest signal that
/// a screen has arrived.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sprite {
    /// `widget/tab` â€” an unselected tab.
    Tab,
    /// `widget/tab_highlighted`.
    TabHighlighted,
    /// `widget/tab_selected`.
    TabSelected,
    /// `widget/tab_selected_highlighted`.
    TabSelectedHighlighted,
    /// `statistics/header` â€” the sort button's resting background.
    StatHeader,
    /// `container/slot` â€” an 18Ã—18 slot, and the sort button's *hovered*
    /// background (see [`WidgetSprites`]).
    Slot,
    /// The six `statistics/<column>` icons, in `ItemStatisticsList`'s own
    /// column order: mined, broken, crafted, used, picked_up, dropped.
    StatColumn(u8),
    /// `statistics/sort_up` / `statistics/sort_down`.
    SortUp,
    SortDown,
}

/// `WidgetSprites`, the record, verbatim.
///
/// ```java
/// public record WidgetSprites(Identifier enabled, Identifier disabled,
///                             Identifier enabledFocused, Identifier disabledFocused)
/// ```
///
/// **The field names lie at one of its two call sites, and that is the point of
/// modelling it rather than hard-coding each widget's four sprites.**
/// `MenuTabBar.MenuTabButton` calls `SPRITES.get(this.isSelected(), â€¦)` â€” so
/// its `enabled` slot holds `tab_selected` and its `disabledFocused` slot holds
/// `tab_highlighted`, a *brighter* sprite than `disabled`. That is the exact
/// opposite of the death screen's three-argument case, where
/// `disabledFocused == disabled` and hovering a dead button changes nothing.
/// One record, two call sites, opposite meanings â€” [`WidgetKind::Sprites`]
/// therefore carries the first argument explicitly instead of assuming it is
/// `active`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WidgetSprites {
    pub enabled: Sprite,
    pub disabled: Sprite,
    pub enabled_focused: Sprite,
    pub disabled_focused: Sprite,
}

impl WidgetSprites {
    /// `WidgetSprites(sprite, focused)` â†’ `(sprite, sprite, focused, focused)`.
    ///
    /// The two-argument overload, which is what the statistics sort buttons
    /// take: `new WidgetSprites(HEADER_SPRITE, SLOT_SPRITE)`. So hovering one
    /// replaces `statistics/header` with the plain `container/slot` â€” the
    /// hover state is the *less* decorated sheet, which reads backwards.
    pub const fn two(sprite: Sprite, focused: Sprite) -> Self {
        Self {
            enabled: sprite,
            disabled: sprite,
            enabled_focused: focused,
            disabled_focused: focused,
        }
    }

    /// The four-argument constructor, in its declared order.
    pub const fn four(
        enabled: Sprite,
        disabled: Sprite,
        enabled_focused: Sprite,
        disabled_focused: Sprite,
    ) -> Self {
        Self {
            enabled,
            disabled,
            enabled_focused,
            disabled_focused,
        }
    }

    /// `get(enabled, focused)`.
    pub fn get(&self, enabled: bool, focused: bool) -> Sprite {
        match (enabled, focused) {
            (true, true) => self.enabled_focused,
            (true, false) => self.enabled,
            (false, true) => self.disabled_focused,
            (false, false) => self.disabled,
        }
    }
}

/// What shape of `AbstractWidget` this is.
///
/// M82 left the note that "when a sibling milestone needs a second widget
/// *shape* the honest move is a `kind` field here, not a parallel type"; M85 is
/// that milestone. The routing below is written against `AbstractWidget` and is
/// unchanged — what a kind decides is only how the *app* draws it.
#[derive(Clone, Debug, PartialEq)]
pub enum WidgetKind {
    /// `Button` — a nine-sliced sprite plus a centred label.
    Button,
    /// `StringWidget` — one line of text, no chrome.
    ///
    /// **Its constructor sets `this.active = false`**, which is why a label is
    /// invisible to [`Screen::mouse_clicked`]'s `getChildAt` and skipped by
    /// Tab, with no special case anywhere in the routing. `centered` is
    /// `MultiLineTextWidget.setCentered`'s single-line analogue: vanilla's
    /// `StringWidget` draws at `getX()` and its *frame cell* does the
    /// centring, so this flag is only set where the screen wants the text
    /// centred inside a wider widget rect.
    Label { centered: bool },
    /// `MultiLineTextWidget` — pre-wrapped, `9 * lines` tall.
    ///
    /// One widget rather than one per line, because that is what the layout
    /// measures: `getHeight()` is `lineCount * 9` and `getWidth()` is the
    /// widest line, and splitting it into N widgets would let the layout put a
    /// gap between them.
    MultiLabel { lines: Vec<String>, centered: bool },
    /// **Geometry with nothing drawn** — see [`Widget::reserved`].
    Reserved,
    /// Anything drawn from a `WidgetSprites` record: a tab, an image button
    /// (M84).
    Sprites {
        sprites: WidgetSprites,
        /// The **first** argument to `WidgetSprites.get`. `isSelected()` for a
        /// tab, `isActive()` for an `ImageButton`. See [`WidgetSprites`].
        first: bool,
        /// A second sheet blitted over the first at the same rect — the sort
        /// button's column icon over its background.
        overlay: Option<Sprite>,
        /// Whether the widget draws its `getMessage()`. A tab does; a
        /// statistics sort button does not (its message is its tooltip).
        label: bool,
    },
}

/// One `AbstractWidget`, flattened to the parts the screens Rewo has need.
///
/// A rectangle, the two flags every `AbstractWidget` carries, a label, and a
/// [`WidgetKind`].
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
    pub kind: WidgetKind,
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
            kind: WidgetKind::Button,
        }
    }

    /// `new StringWidget(message, font)` — `active = false`, height 9.
    pub fn label(id: WidgetId, x: i32, y: i32, width: i32, message: impl Into<String>) -> Self {
        Self {
            id,
            x,
            y,
            width,
            height: LINE_HEIGHT,
            active: false,
            visible: true,
            message: message.into(),
            kind: WidgetKind::Label { centered: false },
        }
    }

    /// `new MultiLineTextWidget(message, font).setCentered(centered)`, with the
    /// wrap already done by the caller (which owns the font metrics).
    pub fn multi_label(
        id: WidgetId,
        x: i32,
        y: i32,
        width: i32,
        lines: Vec<String>,
        centered: bool,
    ) -> Self {
        Self {
            id,
            x,
            y,
            width,
            height: LINE_HEIGHT * lines.len() as i32,
            active: false,
            visible: true,
            message: lines.join(" "),
            kind: WidgetKind::MultiLabel { lines, centered },
        }
    }

    /// A widget whose **geometry is real and whose rendering is deliberately
    /// absent** — it occupies its cell in the layout so everything around it
    /// lands where vanilla puts it, and nothing draws inside it.
    ///
    /// Two places need this and they share one reason: the widget's *action* is
    /// a subsystem Rewo does not have, so drawing a working-looking control
    /// would be a lie about what pressing it does, while omitting the widget
    /// entirely would move every sibling.
    ///
    /// * `PauseScreen`'s four-icon row — bug report and feedback are
    ///   `ConfirmLinkScreen.confirmLink` (a browser), Friends is Realms auth,
    ///   and player reporting is the chat-report subsystem.
    /// * `DialogScreen`'s warning `ImageButton` — it opens
    ///   `DialogScreen.WarningScreen`, a `ConfirmScreen` with a
    ///   `BooleanConsumer`, which is exactly the nesting M82 declined to build
    ///   for the death screen's "Title Screen" button.
    ///
    /// `active = false`, so it is click-through and Tab-skipped like a label.
    pub fn reserved(id: WidgetId, x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            id,
            x,
            y,
            width,
            height,
            active: false,
            visible: true,
            message: String::new(),
            kind: WidgetKind::Reserved,
        }
    }

    /// The same widget drawn from a `WidgetSprites` record instead of the
    /// `widget/button` family (M84).
    pub fn with_kind(mut self, kind: WidgetKind) -> Self {
        self.kind = kind;
        self
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
    /// once `active` goes false — **and only for a widget that greys at all.**
    ///
    /// The greying is not `AbstractWidget`'s, it is
    /// `AbstractWidget.WithInactiveMessage.getMessage()`'s, and exactly three
    /// classes extend it: `AbstractButton`, `AbstractSliderButton` and
    /// `TabButton`. `AbstractStringWidget` — the parent of both `StringWidget`
    /// and `MultiLineTextWidget` — extends `AbstractWidget` directly, so a
    /// label draws white however its `active` flag reads.
    ///
    /// That flag is not even a choice for them: `StringWidget`'s constructor
    /// ends `this.active = false;` so the widget cannot take focus, and Rewo's
    /// [`Widget::label`] / [`Widget::multi_label`] copy it. Reading it as
    /// "grey" therefore greyed **every** dialog title and disconnect reason.
    ///
    /// Nothing caught it for three milestones because the text pass was handed
    /// `160/255` in a slot the sRGB attachment treats as linear, so the grey
    /// stored as **208** and `serverlinkshot`'s `> 200` "is it white" probes
    /// were satisfied by it. M130's colour-space fix dropped it to 160 and both
    /// witnesses went red — the first bug had been hiding the second.
    pub fn label_color(&self) -> [f32; 3] {
        match self.kind {
            WidgetKind::Label { .. } | WidgetKind::MultiLabel { .. } => DEFAULT_LABEL,
            _ if self.active => DEFAULT_LABEL,
            _ => INACTIVE_LABEL,
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

/// `Screen.extractMenuBackground` — the tiled 32-px texture a *menu* screen
/// paints instead of the in-game dim (M85).
///
/// `Screen.extractBackground` has three cases and M82 needed only the first:
///
/// ```java
/// if (this.isInGameUi())            this.extractTransparentBackground(graphics);   // the gradient
/// else {
///    if (this.minecraft.level == null) this.extractPanorama(graphics, a);
///    this.extractBlurredBackground(graphics);
///    this.extractMenuBackground(graphics);                                         // this
/// }
/// ```
///
/// **The pause screen and the disconnect screen both take the second branch**,
/// so neither of them draws the gradient [`Backdrop`] the inventory and the
/// death screen do.
///
/// `in_world` selects the texture: `minecraft.level == null ? MENU_BACKGROUND :
/// INWORLD_MENU_BACKGROUND` — and that same null test is what decides whether a
/// panorama is drawn under it. So the disconnect screen, which by definition
/// has no level, is the one that gets the plain `menu_background.png`.
///
/// **In 26.2 the two sheets are byte-identical and both are uniform.** Each is
/// 16×16 of `rgba(0, 0, 0, 64)` — a flat 25 % black wash, no pattern at all. So
/// the whole tiling apparatus (a 16×16 file, a declared size of 32, one full
/// wrap per 32 screen pixels) is currently *unobservable*: any tile size, and
/// either sheet, produce the same pixels. It is transcribed rather than
/// collapsed to a fill because a resource pack — or a version — that gives
/// either file a pattern makes every one of those numbers visible at once, and
/// because the flag is what a future panorama would branch on. The gate says
/// so: it grades the composite and the coverage, which are the two properties
/// a uniform texture still has, and does **not** claim to distinguish the
/// sheets.
///
/// **The blur and the panorama are not reproduced.** The blur is a
/// backdrop-blur pass Rewo's screen framework has not got; the panorama is a
/// rotating cube-map of a purpose-built scene. Both are named rather than
/// approximated. Because the wash is 25 % rather than opaque, what shows
/// through an in-world pause screen is the *unblurred* world — the one place
/// the missing blur is visible.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MenuBackground {
    /// `minecraft.level != null` — true while a world is loaded.
    pub in_world: bool,
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
    /// `Screen.extractMenuBackground` — see [`MenuBackground`]. Mutually
    /// exclusive with [`Self::backdrop`] in vanilla, because
    /// `extractBackground`'s two branches are an if/else.
    pub menu_background: Option<MenuBackground>,
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
            menu_background: None,
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

    /// `extractBackground`'s else branch — the tiled menu texture. See
    /// [`MenuBackground`].
    pub fn with_menu_background(mut self, in_world: bool) -> Self {
        self.menu_background = Some(MenuBackground { in_world });
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

/// `AbstractScrollArea.SCROLLBAR_WIDTH`.
pub const SCROLLBAR_WIDTH: i32 = 6;
/// `AbstractScrollArea.SCROLLBAR_MIN_HEIGHT`.
pub const SCROLLBAR_MIN_HEIGHT: i32 = 32;
/// `AbstractSelectionList.Entry.CONTENT_PADDING`.
pub const CONTENT_PADDING: i32 = 2;

/// `AbstractSelectionList` + `AbstractScrollArea`, as far as the geometry goes
/// (M84).
///
/// The rows themselves are the screen's business â€” this holds only their
/// heights, because that is all the scroll model needs. Three lists share it on
/// the statistics screen and they have nothing else in common.
///
/// # The three constants that are not where they look
///
/// * **`x` is always 0.** `AbstractSelectionList`'s constructor is
///   `super(0, y, width, height, â€¦)` â€” a list spans the whole screen and the
///   *rows* are centred inside it by [`Self::row_left`]. Passing the screen's
///   own left edge would centre the rows over the wrong span at any width.
/// * **The scroll rate is `defaultEntryHeight / 2`, an integer division**, from
///   `AbstractScrollArea.defaultSettings(defaultEntryHeight / 2)`. So a
///   14-px-row list scrolls 7 px a notch and a 9-px one would scroll 4, not
///   4.5.
/// * **The scrollbar is not at the list's right edge.**
///   `AbstractSelectionList` overrides `scrollBarX()` to
///   `getRowRight() + scrollbarWidth() + 2` â€” beside the *rows*, with a gap of
///   one whole scrollbar width plus two. `AbstractScrollArea`'s own
///   `getRight() - scrollbarWidth()` is what a container widget uses, and it
///   would put the bar hard against the window edge on a wide screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScrollList {
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// `getRowWidth()`.
    pub row_width: i32,
    /// `AbstractScrollArea.scrollAmount`, always clamped to
    /// `[0, max_scroll()]`.
    scroll: f64,
    /// `defaultEntryHeight / 2`.
    pub scroll_rate: i32,
    /// Each row's height, in order. `defaultEntryHeight` for every row on
    /// every list Rewo has; kept per-row because `addEntry(entry, height)`
    /// exists and `contentHeight()` sums them.
    pub rows: Vec<i32>,
}

impl ScrollList {
    /// `AbstractSelectionList(minecraft, width, height, y, defaultEntryHeight)`.
    pub fn new(width: i32, height: i32, y: i32, default_entry_height: i32, row_width: i32) -> Self {
        Self {
            y,
            width,
            height,
            row_width,
            scroll: 0.0,
            scroll_rate: default_entry_height / 2,
            rows: Vec::new(),
        }
    }

    /// `contentHeight()` â€” the rows plus **4**, which is
    /// `getFirstEntryY()`'s 2 at the top and a matching 2 at the bottom.
    pub fn content_height(&self) -> i32 {
        self.rows.iter().sum::<i32>() + 4
    }

    /// `maxScrollAmount()`.
    pub fn max_scroll(&self) -> i32 {
        (self.content_height() - self.height).max(0)
    }

    /// `scrollable()` â€” strictly greater than zero, so a list that exactly
    /// fills its box draws no scrollbar at all.
    pub fn scrollable(&self) -> bool {
        self.max_scroll() > 0
    }

    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// `setScrollAmount` â€” `Mth.clamp(v, 0, maxScrollAmount())`.
    pub fn set_scroll(&mut self, v: f64) {
        self.scroll = v.clamp(0.0, self.max_scroll() as f64);
    }

    /// `mouseScrolled` â€” `setScrollAmount(scrollAmount() - scrollY * scrollRate())`.
    ///
    /// Note the **minus**: a positive `scrollY` (wheel away from you) moves the
    /// list *up*, toward row 0.
    pub fn mouse_scrolled(&mut self, scroll_y: f64) {
        self.set_scroll(self.scroll - scroll_y * self.scroll_rate as f64);
    }

    /// `getFirstEntryY()`.
    pub fn first_entry_y(&self) -> i32 {
        self.y + 2
    }

    /// A row's top in screen space.
    ///
    /// `addEntry` sets `entry.y = getNextY()`, which is
    /// `getFirstEntryY() - (int) scrollAmount + Î£ heights`. The cast is a
    /// **truncation**, and `scrollAmount` is clamped non-negative, so it floors.
    pub fn row_top(&self, row: usize) -> i32 {
        let above: i32 = self.rows.iter().take(row).sum();
        self.first_entry_y() - self.scroll as i32 + above
    }

    pub fn row_height(&self, row: usize) -> i32 {
        self.rows.get(row).copied().unwrap_or(0)
    }

    pub fn row_bottom(&self, row: usize) -> i32 {
        self.row_top(row) + self.row_height(row)
    }

    /// `getRowLeft()` â€” `getX() + width / 2 - getRowWidth() / 2`, with
    /// `getX() == 0`.
    pub fn row_left(&self) -> i32 {
        self.width / 2 - self.row_width / 2
    }

    pub fn row_right(&self) -> i32 {
        self.row_left() + self.row_width
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height
    }

    /// `AbstractWidget.isMouseOver` on the *list*, which
    /// `extractWidgetRenderState` gates the row search on. A row scrolled past
    /// the list's own box is still `isMouseOver` itself â€” only this gate stops
    /// it being hovered.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= 0.0 && y >= self.y as f64 && x < self.width as f64 && y < self.bottom() as f64
    }

    /// `getEntryAtPosition` â€” the first row whose rectangle contains the
    /// point, **without** the list gate. Half-open on the far edges, like every
    /// other hit rect here.
    pub fn row_at_unclipped(&self, x: f64, y: f64) -> Option<usize> {
        let (l, r) = (self.row_left() as f64, self.row_right() as f64);
        if x < l || x >= r {
            return None;
        }
        (0..self.rows.len())
            .find(|&i| y >= self.row_top(i) as f64 && y < self.row_bottom(i) as f64)
    }

    /// The row under the cursor as the renderer sees it: the list gate, then
    /// the row search.
    pub fn row_at(&self, x: f64, y: f64) -> Option<usize> {
        if !self.contains(x, y) {
            return None;
        }
        self.row_at_unclipped(x, y)
    }

    /// `scrollerHeight()` â€” `clamp(heightÂ² / contentHeight, 32, height - 8)`.
    pub fn scroller_height(&self) -> i32 {
        let ch = self.content_height().max(1);
        let raw = ((self.height as f32 * self.height as f32) / ch as f32) as i32;
        raw.clamp(SCROLLBAR_MIN_HEIGHT, self.height - 8)
    }

    /// `AbstractSelectionList.scrollBarX()` â€” beside the rows, not at the
    /// right edge. See the type's docs.
    pub fn scroll_bar_x(&self) -> i32 {
        self.row_right() + SCROLLBAR_WIDTH + 2
    }

    /// `scrollBarY()`.
    pub fn scroll_bar_y(&self) -> i32 {
        let max = self.max_scroll();
        if max == 0 {
            self.y
        } else {
            self.y
                .max((self.scroll as i32) * (self.height - self.scroller_height()) / max + self.y)
        }
    }

    /// `isOverScrollbar` â€” **inclusive** on the right edge (`x <=`) where
    /// every other hit test here is half-open, and half-open on the bottom.
    pub fn over_scrollbar(&self, x: f64, y: f64) -> bool {
        x >= self.scroll_bar_x() as f64
            && x <= (self.scroll_bar_x() + SCROLLBAR_WIDTH) as f64
            && y >= self.y as f64
            && y < self.bottom() as f64
    }

    /// A row's content rect â€” `Entry.getContentX/Y/Width/Height`, inset by
    /// [`CONTENT_PADDING`] on all four sides.
    pub fn content_rect(&self, row: usize) -> (i32, i32, i32, i32) {
        (
            self.row_left() + CONTENT_PADDING,
            self.row_top(row) + CONTENT_PADDING,
            self.row_width - 2 * CONTENT_PADDING,
            self.row_height(row) - 2 * CONTENT_PADDING,
        )
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

    /// …but only a **button** greys. `AbstractStringWidget` extends
    /// `AbstractWidget`, not `AbstractWidget.WithInactiveMessage`, so a label
    /// draws white — and `StringWidget`'s own constructor sets
    /// `this.active = false`, which is the state this asserts over.
    #[test]
    fn a_string_widget_does_not_grey_although_it_is_inactive() {
        let label = Widget::label(0, 4, 4, 100, "title");
        let multi = Widget::multi_label(1, 4, 4, 100, vec!["a".into(), "b".into()], true);
        assert!(!label.active, "StringWidget's constructor sets active = false");
        assert!(!multi.active);
        assert_eq!(label.label_color(), DEFAULT_LABEL);
        assert_eq!(multi.label_color(), DEFAULT_LABEL);
        // The mutation this exists to catch: reading `active` for every kind,
        // which greys every dialog title and every disconnect reason.
        assert_ne!(label.label_color(), INACTIVE_LABEL);
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

    // ---- M84: the two framework extensions the statistics screen needed ----

    /// The same record, two call sites, opposite meanings for the same slot.
    #[test]
    fn the_tab_sprites_put_a_brighter_sheet_in_the_disabled_focused_slot() {
        // `MenuTabButton.SPRITES`, in its declared order.
        let tab = WidgetSprites::four(
            Sprite::TabSelected,
            Sprite::Tab,
            Sprite::TabSelectedHighlighted,
            Sprite::TabHighlighted,
        );
        assert_eq!(tab.get(true, false), Sprite::TabSelected);
        assert_eq!(tab.get(true, true), Sprite::TabSelectedHighlighted);
        assert_eq!(tab.get(false, false), Sprite::Tab);
        assert_eq!(
            tab.get(false, true),
            Sprite::TabHighlighted,
            "`disabledFocused` is a *highlight* here â€” the three-argument \
             constructor's rule (disabledFocused == disabled) is not a \
             property of the record"
        );
    }

    /// `new WidgetSprites(HEADER_SPRITE, SLOT_SPRITE)` â€” the hovered sheet is
    /// the *plainer* one.
    #[test]
    fn the_sort_buttons_hover_sheet_is_the_bare_slot() {
        let s = WidgetSprites::two(Sprite::StatHeader, Sprite::Slot);
        assert_eq!(s.get(true, false), Sprite::StatHeader);
        assert_eq!(s.get(true, true), Sprite::Slot);
        assert_eq!(
            s.get(false, false),
            Sprite::StatHeader,
            "the two-argument overload puts the same sheet in enabled and \
             disabled, so an inactive sort button looks identical"
        );
        assert_eq!(s.get(false, true), Sprite::Slot);
    }

    fn general_list() -> ScrollList {
        // `GeneralStatisticsList`: width 320, content height 100, y 33, row
        // height 14, row width 280.
        let mut l = ScrollList::new(320, 100, 33, 14, 280);
        l.rows = vec![14; 20];
        l
    }

    #[test]
    fn the_scroll_rate_is_half_the_row_height_truncated() {
        assert_eq!(ScrollList::new(320, 100, 33, 14, 280).scroll_rate, 7);
        assert_eq!(ScrollList::new(320, 100, 33, 22, 280).scroll_rate, 11);
        assert_eq!(
            ScrollList::new(320, 100, 33, 9, 280).scroll_rate,
            4,
            "an integer division, so 9/2 is 4 and not 4.5"
        );
    }

    #[test]
    fn the_content_height_is_the_rows_plus_four() {
        let l = general_list();
        assert_eq!(l.content_height(), 20 * 14 + 4);
        assert_eq!(l.max_scroll(), 20 * 14 + 4 - 100);
        let mut short = general_list();
        short.rows = vec![14; 2];
        assert_eq!(short.max_scroll(), 0, "32 of content in a 100-px box");
        assert!(!short.scrollable());
    }

    /// The clamp, sampled **on** both ends.
    #[test]
    fn the_scroll_clamps_to_zero_and_to_max() {
        let mut l = general_list();
        let max = l.max_scroll() as f64;
        l.set_scroll(-1.0);
        assert_eq!(l.scroll(), 0.0);
        l.set_scroll(max);
        assert_eq!(l.scroll(), max);
        l.set_scroll(max + 1.0);
        assert_eq!(l.scroll(), max, "one past the end is the end");
        // The wheel's sign: away from you scrolls toward row 0.
        l.set_scroll(50.0);
        l.mouse_scrolled(1.0);
        assert_eq!(l.scroll(), 43.0, "50 - 1 * 7");
        l.mouse_scrolled(-1.0);
        assert_eq!(l.scroll(), 50.0);
    }

    /// The row `y` uses `(int) scrollAmount`, a truncation.
    #[test]
    fn a_fractional_scroll_truncates_the_row_positions() {
        let mut l = general_list();
        assert_eq!(l.row_top(0), 35, "y + 2");
        assert_eq!(l.row_top(1), 49);
        l.set_scroll(7.9);
        assert_eq!(l.row_top(0), 28, "35 - 7, not 35 - 8");
    }

    #[test]
    fn the_rows_are_centred_in_the_list_and_the_bar_sits_beside_them() {
        let l = general_list();
        assert_eq!(l.row_left(), 320 / 2 - 280 / 2);
        assert_eq!(l.row_right(), 20 + 280);
        assert_eq!(
            l.scroll_bar_x(),
            300 + 6 + 2,
            "getRowRight() + scrollbarWidth() + 2 â€” a whole bar width of gap"
        );
        assert_ne!(
            l.scroll_bar_x(),
            l.width - SCROLLBAR_WIDTH,
            "the AbstractScrollArea default would sit at the window edge"
        );
    }

    /// The row hit rect and the list gate are two separate tests, and the
    /// boundary between them is where a scrolled-past row lives.
    #[test]
    fn a_row_scrolled_above_the_list_is_still_its_own_rect_but_not_hovered() {
        let mut l = general_list();
        l.set_scroll(20.0);
        // Row 0 now spans y 15..29, entirely above the list's y = 33.
        assert_eq!(l.row_top(0), 15);
        assert!(l.row_at_unclipped(160.0, 20.0) == Some(0));
        assert!(
            l.row_at(160.0, 20.0).is_none(),
            "the list's own isMouseOver gate is what clips it"
        );
        // Row 2 straddles the top edge; the part inside the list hits.
        assert_eq!(l.row_at(160.0, 45.0), Some(2));
    }

    #[test]
    fn the_row_hit_rect_is_half_open_on_both_axes() {
        let l = general_list();
        assert_eq!(l.row_at(20.0, 35.0), Some(0), "the near corner is inside");
        assert_eq!(l.row_at(19.0, 35.0), None, "one left of getRowLeft()");
        assert_eq!(l.row_at(299.0, 35.0), Some(0));
        assert_eq!(l.row_at(300.0, 35.0), None, "x == getRowRight() misses");
        assert_eq!(l.row_at(160.0, 48.0), Some(0));
        assert_eq!(l.row_at(160.0, 49.0), Some(1), "y == bottom is the next row");
    }

    /// `isOverScrollbar` is `x <= scrollBarX() + width`, inclusive â€” the one
    /// hit test in this module that is not half-open.
    #[test]
    fn the_scrollbar_hit_test_is_inclusive_on_its_right_edge() {
        let l = general_list();
        let x0 = l.scroll_bar_x();
        assert!(l.over_scrollbar(x0 as f64, 40.0));
        assert!(l.over_scrollbar((x0 + SCROLLBAR_WIDTH) as f64, 40.0));
        assert!(!l.over_scrollbar((x0 + SCROLLBAR_WIDTH + 1) as f64, 40.0));
        assert!(!l.over_scrollbar(x0 as f64, l.bottom() as f64), "bottom is not");
    }

    #[test]
    fn the_scroller_is_clamped_between_thirty_two_and_the_box_less_eight() {
        let l = general_list();
        // 100Â² / 284 = 35
        assert_eq!(l.scroller_height(), 35);
        let mut tall = general_list();
        tall.rows = vec![14; 400];
        assert_eq!(
            tall.scroller_height(),
            SCROLLBAR_MIN_HEIGHT,
            "a very long list floors at 32"
        );
        // At scroll 0 the bar is at the list's own top; at max it is at the
        // bottom of its travel.
        let mut l = general_list();
        assert_eq!(l.scroll_bar_y(), l.y);
        l.set_scroll(l.max_scroll() as f64);
        assert_eq!(l.scroll_bar_y(), l.y + l.height - l.scroller_height());
    }

    #[test]
    fn a_rows_content_is_inset_by_two_on_every_side() {
        let l = general_list();
        let (x, y, w, h) = l.content_rect(0);
        assert_eq!((x, y, w, h), (22, 37, 276, 10));
        assert_eq!(x + w, l.row_right() - CONTENT_PADDING);
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
