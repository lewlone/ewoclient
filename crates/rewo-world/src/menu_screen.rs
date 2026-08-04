//! What each menu type's screen looks like: its texture, its panel size, its
//! label positions, and how its background is blitted.
//!
//! The companion to [`crate::menu_layout`], which says where the *slots* are.
//! Split because they come from different places in vanilla — the slot list is
//! built in `*Menu`'s constructor (server-shared code) and the geometry in
//! `*Screen`'s (client-only) — and because a menu's slots are protocol state
//! while its screen is presentation.
//!
//! # `lectern` is not a container screen at all
//!
//! `LecternScreen extends BookViewScreen`, not `AbstractContainerScreen`. It
//! draws a book, has no slot grid, and never shows the player's inventory —
//! which is exactly why [`crate::menu_layout`]'s lectern entry is one slot with
//! no `addStandardInventorySlots`. The two findings are the same fact seen from
//! either side. So this is **24 container screens and one book viewer**, and
//! `SCREENS[17]` is `None` rather than a container definition that would draw a
//! slot grid vanilla does not have.
//!
//! # Two kinds of title override, and only one of them is a number
//!
//! `AbstractContainerScreen`'s constructor sets `titleLabelX = 8`,
//! `titleLabelY = 6`, `inventoryLabelX = 8` and `inventoryLabelY = imageHeight
//! - 94`. Six screens override the title's x, and they do it in **two
//! different ways**:
//!
//! * `dispenser`, `crafter_3x3`, `brewing_stand` and **the three furnaces**
//!   compute `(imageWidth - font.width(title)) / 2` — a *centring*, which
//!   depends on the rendered width of a title the server chose, so it cannot be
//!   stored as a constant.
//!
//!   **The furnaces are inherited, and that is how they were missed.** M87f
//!   surveyed each `*Screen.java` individually and recorded six; the three
//!   furnaces set nothing of their own, because `AbstractFurnaceScreen.init`
//!   does it for them. A survey that does not follow `extends` sees a base
//!   class's overrides as absent from every subclass. `tools/check_menu_layouts.py`
//!   now walks the chain and grades this table against it.
//! * `anvil` (60), `crafting` (29) and `smithing` (44, and a `titleLabelY` of
//!   15) are plain literals.
//!
//! Storing the first three as whatever they happen to measure for the vanilla
//! English title would put a custom-named dispenser's title in the wrong place
//! and nowhere else, which is the kind of wrong that survives a screenshot.
//!
//! `merchant` is the only screen that moves `inventoryLabelX` (to 107), because
//! its player inventory sits at `left = 108` rather than 8.
//!
//! # The chest family's background is two blits, not one
//!
//! `ContainerScreen` draws `generic_54.png` twice: the top `rows * 18 + 17` px
//! from `v = 0`, then 96 px from `v = 126` for the player's half. One sheet
//! serves all six row counts because the second blit skips whatever rows the
//! first did not use. Every other container screen is a single full-size blit.
//!
//! # The sheet size is a per-blit argument, and one screen is not 256 wide
//!
//! `blit(pipeline, texture, x, y, u, v, w, h, texWidth, texHeight)` takes the
//! sheet's dimensions per call. Twenty-one of the twenty-two container
//! backgrounds pass `256, 256` and `MerchantScreen` passes **`512, 256`** —
//! which it has to, since its panel is 276 px wide. Treating 256 as a constant
//! makes the merchant's `u1` run to `276 / 256 = 1.078`, and the sampler then
//! wraps or clamps: the right-hand third of the trade screen paints as a
//! repeat of its own left edge, which looks like a texture bug rather than an
//! arithmetic one.

/// Where a screen puts its title's x.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleX {
    /// `AbstractContainerScreen`'s default, 8.
    Default,
    /// A literal override.
    Fixed(i32),
    /// `(imageWidth - font.width(title)) / 2`, resolved at draw time against
    /// the title actually being rendered.
    Centered,
}

/// How a screen paints its background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    /// One `blit(texture, leftPos, topPos, 0, 0, imageWidth, imageHeight)`.
    Whole,
    /// `ContainerScreen`'s pair: `(w, rows * 18 + 17)` from `v = 0`, then
    /// `(w, 96)` from `v = 126` at `y + rows * 18 + 17`.
    ChestRows(u8),
}

/// One menu type's screen geometry.
#[derive(Debug, Clone, Copy)]
pub struct MenuScreen {
    /// Path under `assets/minecraft/`, as vanilla writes it.
    pub texture: &'static str,
    pub image_w: i32,
    pub image_h: i32,
    pub title_x: TitleX,
    /// `titleLabelY`, 6 unless overridden (only `smithing`, at 15).
    pub title_y: i32,
    /// `inventoryLabelX`, 8 unless overridden (only `merchant`, at 107).
    pub inventory_label_x: i32,
    pub background: Background,
    /// The `texWidth`/`texHeight` this screen's blit declares.
    ///
    /// **Not a constant.** `blit(..., u, v, w, h, texWidth, texHeight)` takes
    /// them per call, and while 21 of the 22 container backgrounds pass
    /// `256, 256`, `MerchantScreen` passes **`512, 256`** — which it must,
    /// because its panel is 276 px wide and a 256-wide sheet cannot supply
    /// that. A global 256 makes the merchant's UVs run to 1.078 and the
    /// sampler wrap or clamp, painting the right-hand third of the trade
    /// screen with a repeat of its own left edge.
    pub sheet_w: f32,
    pub sheet_h: f32,
}

impl MenuScreen {
    /// `inventoryLabelY`, which is always `imageHeight - 94`.
    ///
    /// `ContainerScreen` and `HopperScreen` both assign this explicitly, but
    /// the base constructor has already computed the identical value — they are
    /// redundant restatements, not overrides, and reading them as overrides
    /// invites storing a second copy that can drift.
    pub const fn inventory_label_y(&self) -> i32 {
        self.image_h - 94
    }

    /// `titleLabelX`, given the rendered width of the title.
    ///
    /// Takes the width rather than storing an x so [`TitleX::Centered`] stays
    /// honest for a server-chosen name.
    pub const fn title_x(&self, title_width: i32) -> i32 {
        match self.title_x {
            TitleX::Default => 8,
            TitleX::Fixed(x) => x,
            TitleX::Centered => (self.image_w - title_width) / 2,
        }
    }
}

const fn chest(rows: u8) -> Option<MenuScreen> {
    Some(MenuScreen {
        texture: "textures/gui/container/generic_54.png",
        image_w: 176,
        image_h: 114 + (rows as i32) * 18,
        title_x: TitleX::Default,
        title_y: 6,
        inventory_label_x: 8,
        background: Background::ChestRows(rows),
        sheet_w: 256.0,
        sheet_h: 256.0,
    })
}

/// A default-geometry container screen: 176x166, one blit, default labels.
const fn plain(texture: &'static str) -> Option<MenuScreen> {
    Some(MenuScreen {
        texture,
        image_w: 176,
        image_h: 166,
        title_x: TitleX::Default,
        title_y: 6,
        inventory_label_x: 8,
        background: Background::Whole,
        sheet_w: 256.0,
        sheet_h: 256.0,
    })
}

/// As [`plain`], with the title centred.
const fn centered(texture: &'static str) -> Option<MenuScreen> {
    Some(MenuScreen {
        title_x: TitleX::Centered,
        ..*plain(texture).as_ref().unwrap()
    })
}

/// As [`plain`], with the title at a literal x.
const fn titled(texture: &'static str, x: i32) -> Option<MenuScreen> {
    Some(MenuScreen {
        title_x: TitleX::Fixed(x),
        ..*plain(texture).as_ref().unwrap()
    })
}

/// Screen geometry per `minecraft:menu` registry id, parallel to
/// [`crate::menu_layout::REGISTRY`].
///
/// `None` means the menu has no `AbstractContainerScreen` — only `lectern`,
/// which is a `BookViewScreen`.
pub static SCREENS: &[Option<MenuScreen>] = &[
    chest(1),
    chest(2),
    chest(3),
    chest(4),
    chest(5),
    chest(6),
    centered("textures/gui/container/dispenser.png"),
    centered("textures/gui/container/crafter.png"),
    titled("textures/gui/container/anvil.png", 60),
    Some(MenuScreen {
        texture: "textures/gui/container/beacon.png",
        image_w: 230,
        image_h: 219,
        title_x: TitleX::Default,
        title_y: 6,
        inventory_label_x: 8,
        background: Background::Whole,
        sheet_w: 256.0,
        sheet_h: 256.0,
    }),
    centered("textures/gui/container/blast_furnace.png"),
    centered("textures/gui/container/brewing_stand.png"),
    titled("textures/gui/container/crafting_table.png", 29),
    plain("textures/gui/container/enchanting_table.png"),
    centered("textures/gui/container/furnace.png"),
    plain("textures/gui/container/grindstone.png"),
    Some(MenuScreen {
        texture: "textures/gui/container/hopper.png",
        image_w: 176,
        image_h: 133,
        title_x: TitleX::Default,
        title_y: 6,
        inventory_label_x: 8,
        background: Background::Whole,
        sheet_w: 256.0,
        sheet_h: 256.0,
    }),
    // lectern -- a BookViewScreen, not a container screen.
    None,
    plain("textures/gui/container/loom.png"),
    Some(MenuScreen {
        texture: "textures/gui/container/villager.png",
        image_w: 276,
        image_h: 166,
        title_x: TitleX::Default,
        title_y: 6,
        // The only screen that moves this, because its player inventory is at
        // left = 108 rather than 8.
        inventory_label_x: 107,
        background: Background::Whole,
        // The one screen that is not 256x256; see `sheet_w`.
        sheet_w: 512.0,
        sheet_h: 256.0,
    }),
    Some(MenuScreen {
        texture: "textures/gui/container/shulker_box.png",
        image_w: 176,
        image_h: 167,
        title_x: TitleX::Default,
        title_y: 6,
        inventory_label_x: 8,
        background: Background::Whole,
        sheet_w: 256.0,
        sheet_h: 256.0,
    }),
    Some(MenuScreen {
        texture: "textures/gui/container/smithing.png",
        image_w: 176,
        image_h: 166,
        title_x: TitleX::Fixed(44),
        title_y: 15,
        inventory_label_x: 8,
        background: Background::Whole,
        sheet_w: 256.0,
        sheet_h: 256.0,
    }),
    centered("textures/gui/container/smoker.png"),
    plain("textures/gui/container/cartography_table.png"),
    plain("textures/gui/container/stonecutter.png"),
];

/// The screen for a `minecraft:menu` id, or `None` for an unknown id or for
/// `lectern`.
pub fn screen_of(protocol_id: i32) -> Option<&'static MenuScreen> {
    usize::try_from(protocol_id)
        .ok()
        .and_then(|i| SCREENS.get(i))
        .and_then(|s| s.as_ref())
}

/// The two blits `ContainerScreen.extractBackground` makes, as
/// `(dst_y_offset, v, height)` pairs against the panel's top-left.
///
/// Returns one entry for every other screen.
pub fn background_blits(s: &MenuScreen) -> Vec<(i32, f32, i32)> {
    match s.background {
        Background::Whole => vec![(0, 0.0, s.image_h)],
        Background::ChestRows(rows) => {
            let top = rows as i32 * 18 + 17;
            vec![(0, 0.0, top), (top, 126.0, 96)]
        }
    }
}

/// One background quad: where it goes in GUI pixels relative to the panel's
/// top-left, and which part of the 256x256 sheet it samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelQuad {
    pub dx: i32,
    pub dy: i32,
    pub w: i32,
    pub h: i32,
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

/// The background quads for a screen, ready to hand to a textured quad
/// emitter.
///
/// Vanilla's call is
/// `blit(pipeline, texture, x, y, u, v, w, h, texWidth, texHeight)` with
/// `texWidth`/`texHeight` **always 256** for these sheets, so the UVs are
/// `u / 256` and `(u + w) / 256`. The source rect is the same size as the
/// destination in every case — nothing here scales — which is why a chest
/// needs two quads rather than one stretched one: the middle of
/// `generic_54.png` is rows the panel does not want.
pub fn background_quads(s: &MenuScreen) -> Vec<PanelQuad> {
    background_blits(s)
        .into_iter()
        .map(|(dy, v, h)| PanelQuad {
            dx: 0,
            dy,
            w: s.image_w,
            h,
            u0: 0.0,
            v0: v / s.sheet_h,
            u1: s.image_w as f32 / s.sheet_w,
            v1: (v + h as f32) / s.sheet_h,
        })
        .collect()
}

/// One progress overlay a furnace draws: where it goes in GUI pixels relative
/// to the panel, and which pixels of its 14x14 or 24x16 sprite it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressBlit {
    pub dx: i32,
    pub dy: i32,
    pub w: i32,
    pub h: i32,
    /// Source origin within the sprite.
    pub sx: i32,
    pub sy: i32,
}

/// `AbstractFurnaceScreen.extractBackground`'s two overlays (M91).
///
/// Returns `(lit, burn)`. The lit flame is `None` unless the furnace is lit —
/// vanilla guards it on `isLit()`, where the burn arrow is drawn
/// **unconditionally** and simply comes out zero-width.
///
/// ```java
/// if (menu.isLit()) {
///     int h = Mth.ceil(getLitProgress() * 13.0F) + 1;
///     blitSprite(litProgressSprite, 14, 14, 0, 14 - h, xo + 56, yo + 36 + 14 - h, 14, h);
/// }
/// int w = Mth.ceil(getBurnProgress() * 24.0F);
/// blitSprite(burnProgressSprite, 24, 16, 0, 0, xo + 79, yo + 34, w, 16);
/// ```
///
/// **The flame grows upward from a fixed bottom edge**, which is why both its
/// source `y` and its destination `y` move together: the sprite is sampled from
/// `14 - h` down, and drawn at `36 + 14 - h`, so its bottom stays at `y = 50`
/// and the top rises. Anchoring it at a fixed top instead would make the flame
/// shrink downward, which reads as burning *up*.
///
/// **`ceil`, not round or truncate**, and the `+ 1` on the flame: a furnace
/// with a hair of fuel left still shows one pixel of flame, and an arrow at
/// any progress at all shows one pixel of arrow.
pub fn furnace_progress(lit: bool, lit_progress: f32, burn_progress: f32) -> (Option<ProgressBlit>, ProgressBlit) {
    let flame = lit.then(|| {
        let h = (lit_progress * 13.0).ceil() as i32 + 1;
        ProgressBlit {
            dx: 56,
            dy: 36 + 14 - h,
            w: 14,
            h,
            sx: 0,
            sy: 14 - h,
        }
    });
    let arrow = ProgressBlit {
        dx: 79,
        dy: 34,
        w: (burn_progress * 24.0).ceil() as i32,
        h: 16,
        sx: 0,
        sy: 0,
    };
    (flame, arrow)
}

/// `BrewingStandScreen`'s bubble column heights, one per animation frame.
///
/// A **table**, not a formula — the gaps are 5, 4, 4, 5, 5, 6, which no
/// arithmetic produces — and its last entry is **0**, so one frame in seven
/// draws no bubbles at all. Reading the guard as "bubbles are always visible
/// while brewing" loses that blink, which is the animation's whole character.
pub const BUBBLE_LENGTHS: [i32; 7] = [29, 24, 20, 16, 11, 6, 0];

/// How long a brew takes, in ticks. `BrewingStandBlockEntity`'s
/// `BREWING_TIME_SECONDS * 20`.
pub const BREW_TICKS_TOTAL: i32 = 400;

/// `BrewingStandScreen.extractBackground`'s three overlays (M92).
///
/// ```java
/// int fuelLength = Mth.clamp((18 * fuel + 20 - 1) / 20, 0, 18);
/// if (fuelLength > 0) blitSprite(FUEL_LENGTH, 18, 4, 0, 0, xo + 60, yo + 44, fuelLength, 4);
/// int tickCount = menu.getBrewingTicks();
/// if (tickCount > 0) {
///     int length = (int)(28.0F * (1.0F - tickCount / 400.0F));
///     if (length > 0) blitSprite(BREW_PROGRESS, 9, 28, 0, 0, xo + 97, yo + 16, 9, length);
///     length = BUBBLELENGTHS[tickCount / 2 % 7];
///     if (length > 0) blitSprite(BUBBLES, 12, 29, 0, 29 - length, xo + 63, yo + 14 + 29 - length, 12, length);
/// }
/// ```
///
/// # Three things here invert
///
/// 1. **The brew timer counts DOWN.** `getBrewingTicks` is the ticks
///    *remaining* out of 400, so the arrow's length is `28 * (1 - t/400)`: it
///    is empty when brewing starts and full just before the potion pops.
///    Treating the field as elapsed time runs the arrow backwards, which looks
///    like a plausible animation and is exactly wrong.
/// 2. **The arrow grows DOWNWARD and the bubbles grow UPWARD.** The arrow's
///    destination `y` is a fixed `16` and only its height changes; the
///    bubbles' source and destination `y` move together (`29 - length`), so
///    their *bottom* edge is pinned at `14 + 29 = 43` and the top rises — the
///    same shape as M91's furnace flame, and the reason to state it separately
///    is that the two are one function apart and differ.
/// 3. **Nothing is drawn at all when `tickCount == 0`.** Both the arrow and
///    the bubbles are inside that guard, so an idle stand shows a bare panel;
///    only the fuel bar survives it. Hoisting either out paints a stopped
///    animation on an idle stand.
///
/// The fuel bar is the odd one out in a fourth way: it grows **rightward**
/// from a fixed left edge, so its source rect never moves.
///
/// Returns `(fuel, brew, bubbles)`, each `None` where vanilla's guard skips
/// the blit — which is a real state, not an optimisation.
pub fn brewing_progress(
    fuel: i32,
    ticks: i32,
) -> (Option<ProgressBlit>, Option<ProgressBlit>, Option<ProgressBlit>) {
    // Integer ceiling division by 20: `(18 * fuel + 19) / 20`, written in
    // vanilla as `+ 20 - 1`. Clamped on BOTH sides, so a corrupt negative fuel
    // reads as empty rather than as a huge negative width.
    let fuel_len = ((18 * fuel + 20 - 1) / 20).clamp(0, 18);
    let fuel_bar = (fuel_len > 0).then_some(ProgressBlit {
        dx: 60,
        dy: 44,
        w: fuel_len,
        h: 4,
        sx: 0,
        sy: 0,
    });

    if ticks <= 0 {
        return (fuel_bar, None, None);
    }

    // `(int)` on a float — truncation toward zero, not `ceil` like the
    // furnace's. The two screens genuinely differ.
    let arrow_h = (28.0 * (1.0 - ticks as f32 / BREW_TICKS_TOTAL as f32)) as i32;
    let brew = (arrow_h > 0).then_some(ProgressBlit {
        dx: 97,
        dy: 16,
        w: 9,
        h: arrow_h,
        sx: 0,
        sy: 0,
    });

    // `tickCount / 2 % 7` — integer divide first, so each frame is held for
    // two ticks and the cycle is 14 ticks long.
    let bubble_h = BUBBLE_LENGTHS[(ticks / 2 % 7) as usize];
    let bubbles = (bubble_h > 0).then_some(ProgressBlit {
        dx: 63,
        dy: 14 + 29 - bubble_h,
        w: 12,
        h: bubble_h,
        sx: 0,
        sy: 29 - bubble_h,
    });

    (fuel_bar, brew, bubbles)
}

// -- the enchanting table (M92) ---------------------------------------------

/// One enchanting-table offer row's visual state.
///
/// **Three states, not two.** A reading that splits rows into "offer" and "no
/// offer" collapses the first two, and they differ by a whole sprite: an empty
/// row draws its background and *nothing else*, while an unaffordable one
/// draws the same background **plus its level numeral**. So a table with no
/// item in it and a table you cannot afford look different, and conflating
/// them makes an empty table sprout three numerals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnchantRow {
    /// `cost == 0` — no offer at all. Background only.
    Empty,
    /// A real offer the player cannot take: the disabled background *and* the
    /// disabled numeral, plus the name and cost dimmed.
    Unaffordable { cost: i32 },
    /// A real offer, cursor elsewhere.
    Available { cost: i32 },
    /// A real offer with the cursor over it.
    Hovered { cost: i32 },
}

impl EnchantRow {
    /// The offer's level cost, or `None` for an empty row.
    pub fn cost(self) -> Option<i32> {
        match self {
            EnchantRow::Empty => None,
            EnchantRow::Unaffordable { cost }
            | EnchantRow::Available { cost }
            | EnchantRow::Hovered { cost } => Some(cost),
        }
    }

    /// Whether this row draws its level numeral, and whether the numeral is
    /// the greyed variant.
    pub fn numeral(self) -> Option<bool> {
        match self {
            EnchantRow::Empty => None,
            EnchantRow::Unaffordable { .. } => Some(true),
            EnchantRow::Available { .. } | EnchantRow::Hovered { .. } => Some(false),
        }
    }
}

/// The three rows' states (M92).
///
/// # The affordability rule, and where its inputs come from
///
/// ```java
/// if ((goldCount < i + 1 || player.experienceLevel < cost) && !player.hasInfiniteMaterials())
/// ```
///
/// **The lapis requirement is the row INDEX plus one, not the cost.** Row 0
/// needs one lapis and row 2 needs three, whatever they charge in levels —
/// reading it as "the cost in lapis" makes an expensive top row look
/// unaffordable with a full stack sitting in the slot.
///
/// The three inputs come from three different places, which is the reason this
/// takes them as parameters rather than reading a menu:
///
/// * `costs` is `container_set_data` slots 0..=2 — the only part that is,
/// * `lapis` is `getGoldCount()`, which is **the COUNT of the stack in menu
///   slot 1**, so it arrives through `container_set_content`, and
/// * `xp_level` and `creative` are the local player's, from `set_experience`
///   (M79) and `player_abilities` (M75).
///
/// So "the enchanting table's data packet drives its rows" is only a third
/// true, and a client that wired the costs alone would grey every row the
/// moment it had no lapis *value* to check against.
///
/// `hasInfiniteMaterials()` is `abilities.instabuild` — creative, which skips
/// both halves at once.
pub fn enchant_rows(
    costs: [i32; 3],
    lapis: i32,
    xp_level: i32,
    creative: bool,
    mouse_gui: Option<(f64, f64)>,
) -> [EnchantRow; 3] {
    std::array::from_fn(|i| {
        let cost = costs[i];
        if cost == 0 {
            return EnchantRow::Empty;
        }
        if (lapis < i as i32 + 1 || xp_level < cost) && !creative {
            return EnchantRow::Unaffordable { cost };
        }
        match mouse_gui {
            Some((x, y)) if enchant_row_hovered(i, x, y) => EnchantRow::Hovered { cost },
            _ => EnchantRow::Available { cost },
        }
    })
}

/// Row `i`'s 108x19 background, in GUI pixels relative to the panel.
pub fn enchant_row_rect(i: usize) -> ProgressBlit {
    ProgressBlit {
        dx: 60,
        dy: 14 + 19 * i as i32,
        w: 108,
        h: 19,
        sx: 0,
        sy: 0,
    }
}

/// Row `i`'s 16x16 level numeral — **one pixel in and one down** from the row,
/// not flush with it.
pub fn enchant_level_rect(i: usize) -> ProgressBlit {
    ProgressBlit {
        dx: 61,
        dy: 15 + 19 * i as i32,
        w: 16,
        h: 16,
        sx: 0,
        sy: 0,
    }
}

/// Whether the cursor is over row `i` for the purposes of the **highlight and
/// the click**, which share one test:
///
/// ```java
/// double xx = x - (xo + 60), yy = y - (yo + 14 + 19 * i);
/// xx >= 0 && yy >= 0 && xx < 108 && yy < 19
/// ```
///
/// A bare rect with no bleed — unlike a slot's, which is `isHovering`'s 18x18.
pub fn enchant_row_hovered(i: usize, gui_x: f64, gui_y: f64) -> bool {
    let (x, y) = (gui_x - 60.0, gui_y - (14 + 19 * i as i32) as f64);
    x >= 0.0 && y >= 0.0 && x < 108.0 && y < 19.0
}

/// Whether the cursor is over row `i` for the purposes of its **tooltip**,
/// which is a *different rectangle* — and this is not a slip in either place.
///
/// `extractRenderState` uses `isHovering(60, 14 + 19 * i, 108, 17, ...)`, whose
/// body applies a one-pixel bleed on every side to a box declared **17 tall**,
/// giving `[59, 169) x [13 + 19i, 32 + 19i)`; the highlight's own test is a
/// bare `[60, 168) x [14 + 19i, 33 + 19i)`. They agree over most of the row and
/// disagree at its edges: the bottom row of pixels highlights without offering
/// a tooltip, and the row above the top offers a tooltip without highlighting.
pub fn enchant_tooltip_hovered(i: usize, gui_x: f64, gui_y: f64) -> bool {
    let (top, h) = ((14 + 19 * i as i32) as f64, 17.0);
    gui_x >= 59.0 && gui_x < 60.0 + 108.0 + 1.0 && gui_y >= top - 1.0 && gui_y < top + h + 1.0
}

/// Where row `i`'s cost numeral is drawn, given its rendered width.
///
/// `leftPosText + 86 - font.width(costText)` with `leftPosText = 60 + 20`, so
/// it is **right-aligned** against x = 166 and a two-digit cost starts further
/// left than a one-digit one. The `+ 7` on y is on top of the row's own `+ 2`.
pub fn enchant_cost_pos(i: usize, cost_width: i32) -> (i32, i32) {
    (80 + 86 - cost_width, 16 + 19 * i as i32 + 7)
}

/// The four text colours `EnchantmentScreen` uses, as packed `0xRRGGBB`.
///
/// **`col` does double duty in vanilla and is reassigned before the cost text
/// is drawn**, so the name and the cost in the same row are different colours,
/// and the cost's colour does *not* track the hover. Reading the variable once
/// gets one of the two wrong wherever they differ.
///
/// The disabled name is `(0x685E4A & 0xFEFEFE) >> 1` — the low bit is masked
/// off *before* the shift so the halving cannot bleed between channels.
pub const ENCHANT_NAME_AVAILABLE: u32 = 0x685E4A;
pub const ENCHANT_NAME_HOVERED: u32 = 0xFFFF80;
pub const ENCHANT_NAME_DISABLED: u32 = 0x342F25;
pub const ENCHANT_COST_ENABLED: u32 = 0x80FF20;
pub const ENCHANT_COST_DISABLED: u32 = 0x407F10;

impl EnchantRow {
    /// This row's cost-text colour, which depends only on affordability.
    pub fn cost_color(self) -> Option<u32> {
        match self {
            EnchantRow::Empty => None,
            EnchantRow::Unaffordable { .. } => Some(ENCHANT_COST_DISABLED),
            // Not hover-dependent: `col` is reassigned to the same value in
            // both arms of the enabled branch, after the name has used it.
            EnchantRow::Available { .. } | EnchantRow::Hovered { .. } => {
                Some(ENCHANT_COST_ENABLED)
            }
        }
    }

    /// This row's name-text colour, which *does* depend on the hover.
    pub fn name_color(self) -> Option<u32> {
        match self {
            EnchantRow::Empty => None,
            EnchantRow::Unaffordable { .. } => Some(ENCHANT_NAME_DISABLED),
            EnchantRow::Available { .. } => Some(ENCHANT_NAME_AVAILABLE),
            EnchantRow::Hovered { .. } => Some(ENCHANT_NAME_HOVERED),
        }
    }
}

/// `EnchantmentMenu.clickMenuButton` — whether pressing row `i` is legal
/// (M92f).
///
/// **This is not the same predicate the row is drawn with**, and the two are
/// worth keeping apart even though they almost always agree:
///
/// ```java
/// if (buttonId >= 0 && buttonId < costs.length) {
///    int enchantmentCost = buttonId + 1;
///    if ((currency.isEmpty() || currency.getCount() < enchantmentCost) && !hasInfiniteMaterials())
///       return false;
///    if (costs[buttonId] <= 0 || item.isEmpty()
///        || (experienceLevel < enchantmentCost || experienceLevel < costs[buttonId]) && !hasInfiniteMaterials())
///       return false;
///    ...
///    return true;
/// }
/// return false;
/// ```
///
/// Three differences from [`enchant_rows`]'s test:
///
/// * it requires **slot 0 to be non-empty**, which the render never asks —
///   the render leans on `costs[i]` being 0 without an item, which is true but
///   is an invariant maintained somewhere else entirely (`slotsChanged`);
/// * it tests the level against **both** `buttonId + 1` and `costs[i]`, where
///   the render tests only the second. The first is implied — `slotsChanged`
///   zeroes any cost below `i + 1` — but again only by an invariant in another
///   method, so transcribing one and deriving the other would couple them;
/// * `costs[buttonId] <= 0` rather than `== 0`, so a negative cost is also
///   refused.
///
/// Vanilla sends the packet **only when this returns true**: the client asks
/// its own menu whether the press is legal before telling the server. Rewo has
/// no server-side menu to ask, so this *is* that gate.
pub fn enchant_click_allowed(
    row: usize,
    costs: [i32; 3],
    lapis: i32,
    has_item: bool,
    xp_level: i32,
    creative: bool,
) -> bool {
    if row >= costs.len() {
        return false;
    }
    let enchantment_cost = row as i32 + 1;
    if (lapis < enchantment_cost) && !creative {
        return false;
    }
    if costs[row] <= 0
        || !has_item
        || ((xp_level < enchantment_cost || xp_level < costs[row]) && !creative)
    {
        return false;
    }
    true
}

/// The enchanting row a GUI-space cursor would press, if any.
///
/// The rect is the *highlight's*, not the tooltip's — `mouseClicked` and
/// `extractBackground` share one test and `extractRenderState` uses another
/// (see [`enchant_tooltip_hovered`]). Pressing is the highlight's.
pub fn enchant_click_row(gui_x: f64, gui_y: f64) -> Option<usize> {
    (0..3).find(|&i| enchant_row_hovered(i, gui_x, gui_y))
}

// -- the beacon (M92) --------------------------------------------------------

/// The six effects a beacon can grant, in `BeaconBlockEntity.BEACON_EFFECTS`
/// order — which is also tier order, and is what the screen lays out.
///
/// ```java
/// List.of(List.of(SPEED, HASTE),
///         List.of(RESISTANCE, JUMP_BOOST),
///         List.of(STRENGTH),
///         List.of(REGENERATION))
/// ```
///
/// A closed enum rather than a registry id because the *set* is fixed in code
/// (`BEACON_EFFECTS` is a literal, and `VALID_EFFECTS` is derived from it), even
/// though the ids that reach the wire are the `minecraft:mob_effect` registry's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeaconEffect {
    Speed,
    Haste,
    Resistance,
    JumpBoost,
    Strength,
    Regeneration,
}

impl BeaconEffect {
    /// The registry name, for resolving an id against the datagen report.
    pub const fn name(self) -> &'static str {
        match self {
            BeaconEffect::Speed => "minecraft:speed",
            BeaconEffect::Haste => "minecraft:haste",
            BeaconEffect::Resistance => "minecraft:resistance",
            BeaconEffect::JumpBoost => "minecraft:jump_boost",
            BeaconEffect::Strength => "minecraft:strength",
            BeaconEffect::Regeneration => "minecraft:regeneration",
        }
    }

    /// All six, in `BEACON_EFFECTS` flattened order.
    pub const ALL: [BeaconEffect; 6] = [
        BeaconEffect::Speed,
        BeaconEffect::Haste,
        BeaconEffect::Resistance,
        BeaconEffect::JumpBoost,
        BeaconEffect::Strength,
        BeaconEffect::Regeneration,
    ];
}

/// `BEACON_EFFECTS.get(tier)` — the effects a given tier offers.
pub const fn beacon_tier_effects(tier: usize) -> &'static [BeaconEffect] {
    match tier {
        0 => &[BeaconEffect::Speed, BeaconEffect::Haste],
        1 => &[BeaconEffect::Resistance, BeaconEffect::JumpBoost],
        2 => &[BeaconEffect::Strength],
        3 => &[BeaconEffect::Regeneration],
        _ => &[],
    }
}

/// What a beacon button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeaconButtonKind {
    /// A power button: its effect, whether it is in the primary column, and
    /// the tier that gates it.
    Power {
        effect: BeaconEffect,
        primary: bool,
        tier: i32,
    },
    /// The secondary column's last slot — the "primary effect at level II"
    /// upgrade, which shows **whatever the primary currently is**.
    Upgrade,
    Confirm,
    Cancel,
}

/// One beacon button: where it sits relative to the panel, and what it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeaconButton {
    pub x: i32,
    pub y: i32,
    pub kind: BeaconButtonKind,
}

/// Every beacon button is 22x22 (`BeaconScreenButton`'s constructor).
pub const BEACON_BUTTON: i32 = 22;

/// The beacon's buttons, in `BeaconScreen.init`'s own order.
///
/// # An invisible button moves a visible one
///
/// The secondary column's count is `BEACON_EFFECTS.get(3).size() + 1` — the
/// **`+ 1` is the upgrade button**, which is `visible = false` unless a primary
/// effect has been chosen. It is still counted into
/// `totalWidth = count * 22 + (count - 1) * 2`, and every button in the column
/// is placed at `167 + c * 24 - totalWidth / 2`. So dropping the `+ 1` because
/// the button is usually invisible halves the total width and slides
/// regeneration 12 px right, onto the panel's painted recess. The layout of
/// what you can see depends on something you usually cannot.
///
/// # The two columns do not share their y arithmetic
///
/// The primary column steps `22 + tier * 25`, but the secondary column is a
/// **fixed 47** — not `22 + 3 * 25`. Deriving it from the tier puts it 50 px
/// below the panel's row.
pub fn beacon_buttons() -> Vec<BeaconButton> {
    let mut out = Vec::new();
    // The primary column: tiers 0..=2, centred on x = 76.
    for tier in 0..=2usize {
        let effects = beacon_tier_effects(tier);
        let count = effects.len() as i32;
        let total_w = count * 22 + (count - 1) * 2;
        for (c, &effect) in effects.iter().enumerate() {
            out.push(BeaconButton {
                x: 76 + c as i32 * 24 - total_w / 2,
                y: 22 + tier as i32 * 25,
                kind: BeaconButtonKind::Power {
                    effect,
                    primary: true,
                    tier: tier as i32,
                },
            });
        }
    }
    // The secondary column: tier 3's effects PLUS the upgrade slot, all at
    // y = 47 and centred on x = 167.
    let effects = beacon_tier_effects(3);
    let count = effects.len() as i32 + 1;
    let total_w = count * 22 + (count - 1) * 2;
    for (c, &effect) in effects.iter().enumerate() {
        out.push(BeaconButton {
            x: 167 + c as i32 * 24 - total_w / 2,
            y: 47,
            kind: BeaconButtonKind::Power {
                effect,
                primary: false,
                tier: 3,
            },
        });
    }
    out.push(BeaconButton {
        x: 167 + (count - 1) * 24 - total_w / 2,
        y: 47,
        kind: BeaconButtonKind::Upgrade,
    });
    out.push(BeaconButton { x: 164, y: 107, kind: BeaconButtonKind::Confirm });
    out.push(BeaconButton { x: 190, y: 107, kind: BeaconButtonKind::Cancel });
    out
}

/// Which of the four chrome sprites a button draws, or that it draws nothing.
///
/// The priority is vanilla's own `if / else if` chain and it is **not** the
/// order the states are named in: `active` is tested first, so a *selected*
/// button whose tier is out of reach still paints as disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeaconButtonState {
    /// `visible = false` — the upgrade button with no primary chosen.
    Hidden,
    Disabled,
    Selected,
    Highlighted,
    Normal,
}

/// The beacon screen's live state, as the buttons read it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BeaconChoice {
    /// `menu.getLevels()` — the pyramid's height, 0..=4.
    pub levels: i32,
    /// The screen's own `primary` / `secondary` fields, which start as the
    /// menu's decoded data slots and then track the player's clicks.
    pub primary: Option<BeaconEffect>,
    pub secondary: Option<BeaconEffect>,
    /// `menu.hasPayment()` — whether the payment slot holds anything.
    pub has_payment: bool,
}

/// One button's state (M92).
///
/// * A power button is `active` when **`tier < levels`**, a strict comparison:
///   a level-1 beacon lights only tier 0, and a level-4 one lights all four.
/// * `selected` compares against `primary` **or** `secondary` depending on the
///   button's own column, so the same effect can be lit in one column and not
///   the other.
/// * Confirm is active on `hasPayment() && primary != null` — it does **not**
///   require a secondary, which is what lets you take a single effect.
/// * Cancel is unconditionally active (`updateStatus` leaves it alone).
/// * The upgrade button is hidden entirely while there is no primary, and
///   otherwise **borrows the primary's effect**, so it is the one button whose
///   icon changes as you click elsewhere.
pub fn beacon_button_state(
    b: BeaconButton,
    s: BeaconChoice,
    hovered: bool,
) -> BeaconButtonState {
    let (active, selected) = match b.kind {
        BeaconButtonKind::Power { effect, primary, tier } => {
            let chosen = if primary { s.primary } else { s.secondary };
            (tier < s.levels, chosen == Some(effect))
        }
        BeaconButtonKind::Upgrade => {
            let Some(p) = s.primary else {
                return BeaconButtonState::Hidden;
            };
            // `setEffect(primary)` then `super.updateStatus(levels)`, so it
            // gates on tier 3 like any other secondary-column button.
            (3 < s.levels, s.secondary == Some(p))
        }
        BeaconButtonKind::Confirm => (s.has_payment && s.primary.is_some(), false),
        BeaconButtonKind::Cancel => (true, false),
    };
    if !active {
        BeaconButtonState::Disabled
    } else if selected {
        BeaconButtonState::Selected
    } else if hovered {
        BeaconButtonState::Highlighted
    } else {
        BeaconButtonState::Normal
    }
}

/// What the upgrade button's icon shows — the *primary* effect, not its own.
pub fn beacon_upgrade_effect(s: BeaconChoice) -> Option<BeaconEffect> {
    s.primary
}

/// Whether the cursor is over a 22x22 button, in panel-relative GUI pixels.
///
/// `AbstractWidget.isMouseOver` is a plain half-open rect with no bleed —
/// unlike a slot's 18x18 test, and unlike the enchanting row's.
pub fn beacon_button_hovered(b: BeaconButton, gui_x: f64, gui_y: f64) -> bool {
    gui_x >= b.x as f64
        && gui_x < (b.x + BEACON_BUTTON) as f64
        && gui_y >= b.y as f64
        && gui_y < (b.y + BEACON_BUTTON) as f64
}

/// The 18x18 icon inside a 22x22 button — **inset by 2 on both axes**, so it
/// leaves a 2 px frame on every side.
pub fn beacon_icon_rect(b: BeaconButton) -> ProgressBlit {
    ProgressBlit { dx: b.x + 2, dy: b.y + 2, w: 18, h: 18, sx: 0, sy: 0 }
}

/// The button's own 22x22 chrome rect.
pub fn beacon_button_rect(b: BeaconButton) -> ProgressBlit {
    ProgressBlit {
        dx: b.x,
        dy: b.y,
        w: BEACON_BUTTON,
        h: BEACON_BUTTON,
        sx: 0,
        sy: 0,
    }
}

/// Where `extractBackground` draws the five payment-item icons, as
/// `(item name, x)` at a shared `y = 109`.
///
/// **The spacing is irregular and it is vanilla's own**: the literals are
/// `20`, `41`, `41 + 22`, `42 + 44`, `42 + 66`, so the gaps run 21, 22, 23, 22.
/// A regular pitch reads as obviously right and misplaces three of the five.
pub const BEACON_PAYMENT_ICONS: [(&str, i32); 5] = [
    ("minecraft:netherite_ingot", 20),
    ("minecraft:emerald", 41),
    ("minecraft:diamond", 63),
    ("minecraft:gold_ingot", 86),
    ("minecraft:iron_ingot", 108),
];

/// The y every payment icon shares.
pub const BEACON_PAYMENT_ICON_Y: i32 = 109;

#[cfg(test)]
mod tests {
    use super::*;

    /// This table's texture paths, deduplicated, in first-appearance order.
    fn distinct_textures() -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for s in SCREENS.iter().flatten() {
            if !out.contains(&s.texture) {
                out.push(s.texture);
            }
        }
        out
    }

    #[test]
    fn every_screens_texture_is_one_the_asset_bake_loads() {
        // The two lists live in different crates and neither can see the
        // other: `rewo-data` has no `rewo-world` dependency, so its loader
        // list is written independently of this table. That makes this a real
        // cross-check rather than a restatement -- a sheet named here and not
        // there is a container that opens and paints nothing, and a sheet
        // named there and not here is dead weight in the atlas.
        //
        // The two spell paths differently on purpose, each in its own crate's
        // idiom: vanilla's `Identifier` form here (`textures/gui/...`) and the
        // jar-relative form the loader takes there (`gui/...`).
        let mut mine: Vec<String> = distinct_textures()
            .iter()
            .map(|t| t.trim_start_matches("textures/").to_string())
            .collect();
        let mut theirs: Vec<String> = rewo_data::assets::MENU_BACKGROUND_TEXTURES
            .iter()
            .map(|t| t.to_string())
            .collect();
        mine.sort();
        theirs.sort();
        assert_eq!(mine, theirs);
    }

    #[test]
    fn the_chest_family_shares_one_sheet() {
        // 25 menu types, 24 container screens, 19 sheets: the six chests share
        // generic_54.png, which is why the bake list is not one per menu.
        assert_eq!(SCREENS.len(), 25);
        assert_eq!(SCREENS.iter().flatten().count(), 24);
        assert_eq!(distinct_textures().len(), 19);
        let chest_sheets: std::collections::HashSet<_> =
            (0..6).map(|i| screen_of(i).unwrap().texture).collect();
        assert_eq!(chest_sheets.len(), 1, "all six chests are one sheet");
    }

    // -- M91: the furnace's progress overlays -------------------------------

    #[test]
    fn an_unlit_furnace_draws_no_flame_but_still_draws_the_arrow() {
        // Vanilla guards the flame on isLit() and draws the arrow
        // unconditionally — at zero progress it is simply zero-width.
        let (flame, arrow) = furnace_progress(false, 0.0, 0.0);
        assert!(flame.is_none());
        assert_eq!(arrow.w, 0);
        assert_eq!((arrow.dx, arrow.dy, arrow.h), (79, 34, 16));
    }

    #[test]
    fn the_flame_grows_upward_from_a_fixed_bottom_edge() {
        // Its source y and destination y move together, so the bottom stays
        // put and the top rises. Anchoring at a fixed top instead makes the
        // flame shrink downward, which reads as burning up.
        let bottoms: Vec<i32> = [0.0f32, 0.25, 0.5, 1.0]
            .iter()
            .map(|&p| {
                let f = furnace_progress(true, p, 0.0).0.unwrap();
                f.dy + f.h
            })
            .collect();
        assert_eq!(bottoms, vec![50, 50, 50, 50], "the bottom edge never moves");
        // ...and the height really does grow.
        let hs: Vec<i32> = [0.0f32, 0.5, 1.0]
            .iter()
            .map(|&p| furnace_progress(true, p, 0.0).0.unwrap().h)
            .collect();
        assert_eq!(hs, vec![1, 8, 14]);
    }

    #[test]
    fn the_source_row_tracks_the_height_so_the_sprite_is_not_stretched() {
        for p in [0.0f32, 0.3, 0.7, 1.0] {
            let f = furnace_progress(true, p, 0.0).0.unwrap();
            assert_eq!(f.sy, 14 - f.h, "sampled from the sprite's bottom up");
            assert_eq!(f.sy + f.h, 14, "and always reaching its bottom edge");
        }
    }

    #[test]
    fn ceil_means_a_sliver_of_progress_still_shows_a_pixel() {
        // `Mth.ceil`, not round or truncate. A furnace with a hair of fuel
        // left shows one pixel of flame; an arrow at any progress at all shows
        // one pixel of arrow. Truncation would show nothing until 1/24th.
        assert_eq!(furnace_progress(false, 0.0, 0.001).1.w, 1);
        assert_eq!(furnace_progress(false, 0.0, 1.0 / 24.0).1.w, 1);
        assert_eq!(furnace_progress(false, 0.0, 1.0).1.w, 24);
        // The flame's `+ 1` is on top of the ceil, so even zero is one pixel.
        assert_eq!(furnace_progress(true, 0.0, 0.0).0.unwrap().h, 1);
    }

    #[test]
    fn a_full_flame_is_the_whole_sprite() {
        let f = furnace_progress(true, 1.0, 0.0).0.unwrap();
        assert_eq!((f.sx, f.sy, f.w, f.h), (0, 0, 14, 14));
        assert_eq!((f.dx, f.dy), (56, 36));
    }

    // -- M92: the brewing stand's three overlays ----------------------------

    #[test]
    fn the_brew_arrow_counts_down_from_four_hundred() {
        // `getBrewingTicks` is the ticks REMAINING, so the arrow is empty when
        // brewing starts and full just before the potion pops. Reading the
        // field as elapsed time runs the animation backwards, which looks
        // plausible and is exactly inverted.
        let started = brewing_progress(0, 400).1;
        assert!(started.is_none(), "at t=400 the arrow is 0 tall, so not drawn");
        let nearly_done = brewing_progress(0, 1).1.expect("almost finished");
        assert_eq!(nearly_done.h, 27);
        // ...and it is monotonic in the direction that matters.
        let mut last = 0;
        for t in [400, 300, 200, 100, 1] {
            let h = brewing_progress(0, t).1.map_or(0, |b| b.h);
            assert!(h >= last, "t={t}: the arrow must grow as ticks fall");
            last = h;
        }
    }

    #[test]
    fn the_arrow_grows_downward_but_the_bubbles_grow_upward() {
        // The two are one function apart and they differ. The arrow's dy is a
        // fixed 16 and only its height changes; the bubbles' source and
        // destination y move together so their BOTTOM edge is pinned.
        for t in [1, 50, 150, 399] {
            if let Some(a) = brewing_progress(0, t).1 {
                assert_eq!(a.dy, 16, "the arrow's top never moves");
                assert_eq!(a.sy, 0, "so it always samples from the sprite's top");
            }
        }
        let bottoms: Vec<i32> = (0..14)
            .filter_map(|t| brewing_progress(0, 400 - t).2)
            .map(|b| b.dy + b.h)
            .collect();
        assert!(!bottoms.is_empty());
        assert!(
            bottoms.iter().all(|&b| b == 43),
            "the bubbles' bottom edge is pinned at 14 + 29: {bottoms:?}"
        );
    }

    #[test]
    fn the_bubbles_source_row_tracks_their_height() {
        // M91's furnace-flame witness has this half and the brewing one was
        // written without it, which a mutation found: pinning `sy` to 0 while
        // the destination still rises samples the TOP of the bubble column and
        // draws it at the bottom, so the art slides instead of filling.
        for t in 1..15 {
            let Some(b) = brewing_progress(0, t).2 else { continue };
            assert_eq!(b.sy, 29 - b.h, "t={t}: sampled from the sprite's bottom up");
            assert_eq!(b.sy + b.h, 29, "t={t}: and always reaching its bottom edge");
        }
    }

    #[test]
    fn one_bubble_frame_in_seven_is_blank() {
        // BUBBLELENGTHS ends in 0. A guard read as "bubbles are always visible
        // while brewing" loses the blink, which is the animation's character.
        assert_eq!(BUBBLE_LENGTHS[6], 0);
        let frames: Vec<bool> = (0..7)
            .map(|f| brewing_progress(0, 400 - f * 2).2.is_some())
            .collect();
        assert_eq!(frames.iter().filter(|v| !**v).count(), 1, "{frames:?}");
    }

    #[test]
    fn each_bubble_frame_is_held_for_two_ticks() {
        // `tickCount / 2 % 7` divides FIRST, so the cycle is 14 ticks and not
        // 7. Taking the modulo first would flicker at twice the speed.
        let h = |t: i32| brewing_progress(0, t).2.map_or(0, |b| b.h);
        // The pairs held together start on EVEN ticks, because the divide
        // truncates: 2/2 and 3/2 are both frame 1. This witness was written
        // pairing (1,2) first and failed, and the failure was the finding —
        // that pair straddles a frame boundary.
        for pair in (2..14).step_by(2) {
            assert_eq!(h(pair), h(pair + 1), "ticks {pair} and {} differ", pair + 1);
        }
        assert_eq!(h(2), h(16), "and the cycle repeats after 14 ticks");
        assert_ne!(h(2), h(4), "while adjacent frames really are different");
    }

    #[test]
    fn the_arrow_truncates_where_the_furnace_ceils() {
        // The two screens genuinely differ: `(int)(28.0F * ...)` here against
        // `Mth.ceil` in AbstractFurnaceScreen. At one tick remaining the
        // arrow's exact height is 27.93 and vanilla shows 27; at 399 it is
        // 0.07 and vanilla shows NOTHING, where a ceil would show a pixel.
        assert!(brewing_progress(0, 399).1.is_none(), "0.07 truncates to 0");
        assert_eq!(brewing_progress(0, 1).1.unwrap().h, 27, "27.93 truncates to 27");
        // The furnace, for contrast, is the other way at the same fraction.
        assert_eq!(furnace_progress(false, 0.0, 0.001).1.w, 1);
    }

    #[test]
    fn the_fuel_bar_ceils_so_one_charge_shows_one_pixel() {
        // `(18 * fuel + 20 - 1) / 20` is a ceiling divide. Truncating would
        // show an empty bar for one blaze powder charge (18/20 = 0), so a
        // player would think fuelling had failed.
        assert!(brewing_progress(0, 0).0.is_none(), "no fuel, no bar");
        assert_eq!(brewing_progress(1, 0).0.unwrap().w, 1);
        assert_eq!(brewing_progress(20, 0).0.unwrap().w, 18, "full");
        assert_eq!(brewing_progress(10, 0).0.unwrap().w, 9, "half");
        // Clamped on BOTH sides: a corrupt value cannot produce a negative or
        // over-wide quad.
        assert!(brewing_progress(-5, 0).0.is_none());
        assert_eq!(brewing_progress(9999, 0).0.unwrap().w, 18);
    }

    #[test]
    fn an_idle_stand_keeps_its_fuel_bar_and_drops_both_animations() {
        // Both the arrow and the bubbles live inside `if (tickCount > 0)`.
        // Hoisting either out paints a stopped animation on an idle stand.
        let (fuel, brew, bubbles) = brewing_progress(20, 0);
        assert!(fuel.is_some(), "fuel is outside the guard");
        assert!(brew.is_none());
        assert!(bubbles.is_none());
    }

    #[test]
    fn the_fuel_bar_never_moves_only_its_width_changes() {
        // The third growth direction on one screen: rightward from a fixed
        // left edge, so unlike the bubbles its source rect stays put.
        for f in 1..=20 {
            let b = brewing_progress(f, 0).0.unwrap();
            assert_eq!((b.dx, b.dy, b.sx, b.sy, b.h), (60, 44, 0, 0, 4));
        }
    }

    // -- M92: the enchanting table's three rows -----------------------------

    /// Rows with no cursor anywhere.
    fn rows(costs: [i32; 3], lapis: i32, xp: i32) -> [EnchantRow; 3] {
        enchant_rows(costs, lapis, xp, false, None)
    }

    #[test]
    fn an_empty_row_draws_no_numeral_but_an_unaffordable_one_does() {
        // The distinction a two-state reading loses. `cost == 0` blits the
        // disabled background and returns; an unaffordable offer blits the
        // same background AND its numeral. Conflating them makes an empty
        // table sprout three numerals.
        let empty = rows([0, 0, 0], 64, 30);
        assert_eq!(empty, [EnchantRow::Empty; 3]);
        assert!(empty.iter().all(|r| r.numeral().is_none()));

        let poor = rows([5, 10, 15], 64, 0);
        assert!(matches!(poor[0], EnchantRow::Unaffordable { cost: 5 }));
        assert_eq!(poor[0].numeral(), Some(true), "greyed, but drawn");
    }

    #[test]
    fn the_lapis_requirement_is_the_row_index_not_the_cost() {
        // `goldCount < i + 1` — row 0 needs one lapis and row 2 needs three,
        // whatever they charge in LEVELS. Reading it as "the cost in lapis"
        // greys an expensive top row with a full stack in the slot.
        let one = rows([1, 2, 3], 1, 30);
        assert!(matches!(one[0], EnchantRow::Available { .. }), "1 lapis buys row 0");
        assert!(matches!(one[1], EnchantRow::Unaffordable { .. }), "but not row 1");
        assert!(matches!(one[2], EnchantRow::Unaffordable { .. }));
        // ...and a costly row is affordable on one lapis if the levels are there.
        let costly = rows([30, 0, 0], 1, 30);
        assert!(matches!(costly[0], EnchantRow::Available { .. }));
    }

    #[test]
    fn either_half_of_the_affordability_test_alone_disables_a_row() {
        // It is an OR of two independent shortages, so a witness that only
        // ever varies one of them cannot tell an `&&` from an `||`.
        assert!(matches!(rows([10, 0, 0], 0, 30)[0], EnchantRow::Unaffordable { .. }), "no lapis");
        assert!(matches!(rows([10, 0, 0], 64, 9)[0], EnchantRow::Unaffordable { .. }), "no levels");
        assert!(matches!(rows([10, 0, 0], 64, 10)[0], EnchantRow::Available { .. }), "exactly enough");
    }

    #[test]
    fn creative_mode_skips_both_halves_at_once() {
        // `&& !hasInfiniteMaterials()` wraps the WHOLE test, so instabuild
        // makes an offer available with no lapis and no levels.
        let broke = enchant_rows([30, 30, 30], 0, 0, true, None);
        assert!(broke.iter().all(|r| matches!(r, EnchantRow::Available { .. })));
        // ...but it does not conjure an offer that is not there.
        assert_eq!(enchant_rows([0, 0, 0], 0, 0, true, None), [EnchantRow::Empty; 3]);
    }

    #[test]
    fn only_an_affordable_row_can_be_hovered() {
        // The hover test sits inside the enabled branch, so the cursor over an
        // unaffordable row changes nothing at all.
        let at = |c: [i32; 3], lapis, xp| enchant_rows(c, lapis, xp, false, Some((100.0, 20.0)));
        assert!(matches!(at([10, 0, 0], 64, 30)[0], EnchantRow::Hovered { .. }));
        assert!(matches!(at([10, 0, 0], 0, 30)[0], EnchantRow::Unaffordable { .. }));
        assert!(matches!(at([0, 0, 0], 64, 30)[0], EnchantRow::Empty));
        // The cursor is over row 0's band only.
        assert!(matches!(at([10, 10, 10], 64, 30)[1], EnchantRow::Available { .. }));
    }

    #[test]
    fn the_rows_tile_at_a_nineteen_pixel_pitch_with_the_numeral_inset() {
        for i in 0..3 {
            let r = enchant_row_rect(i);
            assert_eq!((r.dx, r.w, r.h), (60, 108, 19));
            assert_eq!(r.dy, 14 + 19 * i as i32);
            let n = enchant_level_rect(i);
            // One in and one down, not flush: `leftPos + 1, yo + 15 + 19 * i`.
            assert_eq!((n.dx - r.dx, n.dy - r.dy), (1, 1), "row {i}");
            assert_eq!((n.w, n.h), (16, 16));
        }
        // The pitch is 19, so consecutive rows leave no gap and no overlap.
        assert_eq!(enchant_row_rect(1).dy - enchant_row_rect(0).dy, 19);
    }

    #[test]
    fn the_highlight_and_the_tooltip_use_different_rectangles() {
        // Not a slip in either place: the click/highlight test is a bare
        // 108x19 and the tooltip's is `isHovering(.., 108, 17, ..)`, whose
        // body bleeds one pixel on every side of a box declared two rows
        // shorter. They agree in the middle and disagree at the edges.
        assert!(enchant_row_hovered(0, 100.0, 32.0), "the highlight reaches y=32");
        assert!(!enchant_tooltip_hovered(0, 100.0, 32.0), "the tooltip stops at 31");
        assert!(enchant_tooltip_hovered(0, 100.0, 13.0), "the tooltip reaches up to 13");
        assert!(!enchant_row_hovered(0, 100.0, 13.0), "the highlight starts at 14");
        assert!(enchant_tooltip_hovered(0, 59.0, 20.0), "and one pixel left of the row");
        assert!(!enchant_row_hovered(0, 59.0, 20.0));
        // Over the middle they agree, which is why the difference is easy to
        // miss and why the witness probes the edges.
        for x in [60.0, 100.0, 167.0] {
            assert!(enchant_row_hovered(0, x, 20.0));
            assert!(enchant_tooltip_hovered(0, x, 20.0));
        }
    }

    #[test]
    fn the_cost_text_is_right_aligned_so_a_wider_number_starts_further_left() {
        // `leftPosText + 86 - font.width(costText)`, against a fixed right
        // edge at 166.
        let (x1, y) = enchant_cost_pos(0, 6); // one digit
        let (x2, _) = enchant_cost_pos(0, 12); // two digits
        assert_eq!(x1 + 6, x2 + 12, "both end at the same x");
        assert_eq!(x1 + 6, 166);
        assert_eq!(y, 23, "16 + 7");
        assert_eq!(enchant_cost_pos(2, 6).1, 61, "16 + 38 + 7");
    }

    #[test]
    fn the_cost_colour_ignores_the_hover_but_the_name_colour_does_not() {
        // `col` does double duty: the name reads it, then it is REASSIGNED
        // before the cost text is drawn. So a hovered row's name is pale
        // yellow while its cost stays the same green as an unhovered one.
        let hov = EnchantRow::Hovered { cost: 5 };
        let avail = EnchantRow::Available { cost: 5 };
        assert_ne!(hov.name_color(), avail.name_color(), "the name tracks hover");
        assert_eq!(hov.cost_color(), avail.cost_color(), "the cost does not");
        assert_eq!(hov.cost_color(), Some(ENCHANT_COST_ENABLED));
        // And the disabled name is the available one halved per channel with
        // the low bit masked off first.
        assert_eq!(
            EnchantRow::Unaffordable { cost: 5 }.name_color(),
            Some((ENCHANT_NAME_AVAILABLE & 0xFEFEFE) >> 1)
        );
        assert_eq!(EnchantRow::Empty.cost_color(), None);
    }

    // -- M92f: the enchanting table's click gate ----------------------------

    #[test]
    fn the_click_gate_requires_an_item_where_the_render_gate_does_not() {
        // `clickMenuButton` tests `item.isEmpty()` outright. The render leans
        // on `costs[i]` being 0 without an item — true, but maintained by
        // `slotsChanged`, a different method. Transcribing one and deriving
        // the other would couple them across that seam.
        assert!(enchant_click_allowed(0, [5, 0, 0], 64, true, 30, false));
        assert!(
            !enchant_click_allowed(0, [5, 0, 0], 64, false, 30, false),
            "no item in slot 0 refuses the press even with a live cost"
        );
    }

    #[test]
    fn the_click_gate_checks_the_level_against_the_row_index_too() {
        // `experienceLevel < enchantmentCost || experienceLevel < costs[i]`.
        // The first is normally implied (a cost below `i + 1` is zeroed), so
        // this probes a cost that breaks the invariant — which is what tells
        // the two conditions apart at all.
        assert!(
            !enchant_click_allowed(2, [0, 0, 1], 64, true, 1, false),
            "row 2 needs level 3 even for a cost of 1"
        );
        assert!(enchant_click_allowed(2, [0, 0, 1], 64, true, 3, false));
    }

    #[test]
    fn the_click_gates_lapis_requirement_is_the_row_index_like_the_renders() {
        assert!(!enchant_click_allowed(2, [0, 0, 5], 2, true, 30, false), "needs 3");
        assert!(enchant_click_allowed(2, [0, 0, 5], 3, true, 30, false));
    }

    #[test]
    fn creative_skips_both_shortages_but_not_the_item_or_the_cost() {
        // `hasInfiniteMaterials()` guards the two resource tests and NEITHER
        // the empty-item test nor the zero-cost one, so instabuild cannot
        // enchant an empty slot.
        assert!(enchant_click_allowed(2, [0, 0, 5], 0, true, 0, true));
        assert!(!enchant_click_allowed(2, [0, 0, 5], 0, false, 0, true), "still needs an item");
        assert!(!enchant_click_allowed(0, [0, 0, 0], 64, true, 30, true), "and a live offer");
    }

    #[test]
    fn a_negative_cost_is_refused_rather_than_treated_as_an_offer() {
        // `costs[buttonId] <= 0`, not `== 0`.
        assert!(!enchant_click_allowed(0, [-1, 0, 0], 64, true, 30, false));
    }

    #[test]
    fn an_out_of_range_row_is_refused() {
        assert!(!enchant_click_allowed(3, [5, 5, 5], 64, true, 30, true));
    }

    #[test]
    fn the_press_uses_the_highlights_rectangle_not_the_tooltips() {
        // Two rects on one screen (see the M92b witness); the press shares the
        // highlight's.
        assert_eq!(enchant_click_row(100.0, 20.0), Some(0));
        assert_eq!(enchant_click_row(100.0, 39.0), Some(1));
        assert_eq!(enchant_click_row(100.0, 13.0), None, "the tooltip's row, not the press's");
        assert_eq!(enchant_click_row(59.0, 20.0), None);
        assert_eq!(enchant_click_row(200.0, 20.0), None);
    }

    // -- M92: the beacon's buttons ------------------------------------------

    fn button_of(kind: BeaconButtonKind) -> BeaconButton {
        beacon_buttons()
            .into_iter()
            .find(|b| b.kind == kind)
            .unwrap_or_else(|| panic!("no {kind:?}"))
    }

    #[test]
    fn an_invisible_button_moves_a_visible_one() {
        // The secondary column counts the upgrade slot into its total width
        // even though it is usually invisible. Dropping the `+ 1` because "the
        // button isn't there" halves the width and slides regeneration right.
        let regen = button_of(BeaconButtonKind::Power {
            effect: BeaconEffect::Regeneration,
            primary: false,
            tier: 3,
        });
        // count = 2 -> total = 46 -> 167 + 0 - 23.
        assert_eq!(regen.x, 144);
        // What it would be with count = 1 (total 22): 167 - 11 = 156.
        assert_ne!(regen.x, 156, "the +1 for the upgrade slot is load-bearing");
        assert_eq!(button_of(BeaconButtonKind::Upgrade).x, 168, "167 + 24 - 23");
    }

    #[test]
    fn the_two_columns_do_not_share_their_y_arithmetic() {
        // The primary column steps 22 + tier * 25; the secondary is a FIXED
        // 47, not 22 + 3 * 25 = 97.
        let ys: Vec<i32> = (0..=2)
            .map(|t| {
                button_of(BeaconButtonKind::Power {
                    effect: beacon_tier_effects(t as usize)[0],
                    primary: true,
                    tier: t,
                })
                .y
            })
            .collect();
        assert_eq!(ys, vec![22, 47, 72]);
        let regen = button_of(BeaconButtonKind::Power {
            effect: BeaconEffect::Regeneration,
            primary: false,
            tier: 3,
        });
        assert_eq!(regen.y, 47);
        assert_ne!(regen.y, 22 + 3 * 25, "not derived from the tier");
    }

    #[test]
    fn a_single_effect_tier_centres_rather_than_left_aligning() {
        // Tier 2 has one effect, so total = 22 and the button lands at
        // 76 - 11 = 65 — centred on the column, not at its left edge.
        let strength = button_of(BeaconButtonKind::Power {
            effect: BeaconEffect::Strength,
            primary: true,
            tier: 2,
        });
        assert_eq!(strength.x, 65);
        // ...and a two-effect tier straddles the same centre.
        let speed = button_of(BeaconButtonKind::Power {
            effect: BeaconEffect::Speed,
            primary: true,
            tier: 0,
        });
        let haste = button_of(BeaconButtonKind::Power {
            effect: BeaconEffect::Haste,
            primary: true,
            tier: 0,
        });
        assert_eq!((speed.x, haste.x), (53, 77));
        assert_eq!((speed.x + haste.x + BEACON_BUTTON) / 2, 76, "centred on 76");
    }

    #[test]
    fn a_power_button_lights_only_when_its_tier_is_strictly_below_the_level() {
        // `tier < levels`. A level-1 beacon lights tier 0 alone.
        let speed = button_of(BeaconButtonKind::Power {
            effect: BeaconEffect::Speed,
            primary: true,
            tier: 0,
        });
        let strength = button_of(BeaconButtonKind::Power {
            effect: BeaconEffect::Strength,
            primary: true,
            tier: 2,
        });
        let at = |levels| BeaconChoice { levels, ..Default::default() };
        assert_eq!(beacon_button_state(speed, at(0), false), BeaconButtonState::Disabled);
        assert_eq!(beacon_button_state(speed, at(1), false), BeaconButtonState::Normal);
        assert_eq!(beacon_button_state(strength, at(2), false), BeaconButtonState::Disabled);
        assert_eq!(beacon_button_state(strength, at(3), false), BeaconButtonState::Normal);
    }

    #[test]
    fn disabled_beats_selected_which_beats_hovered() {
        // Vanilla's chain tests `!active` FIRST, so a selected button out of
        // tier range still paints disabled — the states are not independent
        // flags and their order is not the order they are named in.
        let speed = button_of(BeaconButtonKind::Power {
            effect: BeaconEffect::Speed,
            primary: true,
            tier: 0,
        });
        let chosen = |levels| BeaconChoice {
            levels,
            primary: Some(BeaconEffect::Speed),
            ..Default::default()
        };
        assert_eq!(beacon_button_state(speed, chosen(0), true), BeaconButtonState::Disabled);
        assert_eq!(beacon_button_state(speed, chosen(4), true), BeaconButtonState::Selected);
        assert_eq!(
            beacon_button_state(speed, BeaconChoice { levels: 4, ..Default::default() }, true),
            BeaconButtonState::Highlighted
        );
    }

    #[test]
    fn the_same_effect_can_be_selected_in_one_column_and_not_the_other() {
        // `selected` compares against primary OR secondary by the button's own
        // column, so the two columns do not shadow each other.
        let regen_secondary = button_of(BeaconButtonKind::Power {
            effect: BeaconEffect::Regeneration,
            primary: false,
            tier: 3,
        });
        let s = BeaconChoice {
            levels: 4,
            primary: Some(BeaconEffect::Regeneration),
            secondary: None,
            has_payment: true,
        };
        assert_eq!(
            beacon_button_state(regen_secondary, s, false),
            BeaconButtonState::Normal,
            "the PRIMARY being regeneration must not light the secondary button"
        );
    }

    #[test]
    fn the_upgrade_button_is_hidden_until_a_primary_is_chosen() {
        let up = button_of(BeaconButtonKind::Upgrade);
        let none = BeaconChoice { levels: 4, ..Default::default() };
        assert_eq!(beacon_button_state(up, none, false), BeaconButtonState::Hidden);
        assert_eq!(beacon_upgrade_effect(none), None);
        let with = BeaconChoice {
            levels: 4,
            primary: Some(BeaconEffect::Haste),
            ..Default::default()
        };
        assert_ne!(beacon_button_state(up, with, false), BeaconButtonState::Hidden);
        // It BORROWS the primary's icon — the one button whose art changes as
        // you click elsewhere.
        assert_eq!(beacon_upgrade_effect(with), Some(BeaconEffect::Haste));
    }

    #[test]
    fn confirm_needs_a_payment_and_a_primary_but_no_secondary() {
        // Requiring a secondary would make a single-effect beacon unusable.
        let confirm = button_of(BeaconButtonKind::Confirm);
        let cancel = button_of(BeaconButtonKind::Cancel);
        let s = |pay: bool, prim: bool| BeaconChoice {
            levels: 4,
            primary: prim.then_some(BeaconEffect::Speed),
            secondary: None,
            has_payment: pay,
        };
        assert_eq!(beacon_button_state(confirm, s(false, true), false), BeaconButtonState::Disabled);
        assert_eq!(beacon_button_state(confirm, s(true, false), false), BeaconButtonState::Disabled);
        assert_eq!(beacon_button_state(confirm, s(true, true), false), BeaconButtonState::Normal);
        // Cancel never disables — `updateStatus` leaves it alone.
        assert_eq!(beacon_button_state(cancel, s(false, false), false), BeaconButtonState::Normal);
    }

    #[test]
    fn the_icon_is_inset_two_pixels_inside_its_button() {
        for b in beacon_buttons() {
            let chrome = beacon_button_rect(b);
            let icon = beacon_icon_rect(b);
            assert_eq!((chrome.w, chrome.h), (22, 22));
            assert_eq!((icon.w, icon.h), (18, 18));
            assert_eq!((icon.dx - chrome.dx, icon.dy - chrome.dy), (2, 2));
        }
    }

    #[test]
    fn the_button_hover_box_has_no_bleed() {
        // Unlike a slot's 18x18 test and unlike the enchanting row's tooltip
        // box: `AbstractWidget.isMouseOver` is a plain half-open rect.
        let b = BeaconButton { x: 50, y: 30, kind: BeaconButtonKind::Cancel };
        assert!(beacon_button_hovered(b, 50.0, 30.0));
        assert!(beacon_button_hovered(b, 71.9, 51.9));
        assert!(!beacon_button_hovered(b, 49.0, 40.0), "no one-pixel bleed left");
        assert!(!beacon_button_hovered(b, 72.0, 40.0), "nor right");
        assert!(!beacon_button_hovered(b, 60.0, 52.0), "nor below");
    }

    #[test]
    fn the_payment_icon_spacing_is_irregular() {
        // 20, 41, 63, 86, 108 — gaps of 21, 22, 23, 22, straight out of
        // vanilla's literals (`41`, `41 + 22`, `42 + 44`, `42 + 66`). A
        // regular pitch reads as obviously right and misplaces three of five.
        let xs: Vec<i32> = BEACON_PAYMENT_ICONS.iter().map(|(_, x)| *x).collect();
        assert_eq!(xs, vec![20, 41, 63, 86, 108]);
        let gaps: Vec<i32> = xs.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(gaps, vec![21, 22, 23, 22]);
        assert!(gaps.iter().any(|g| *g != gaps[0]), "not a constant pitch");
    }

    #[test]
    fn each_effect_gets_exactly_one_button_and_only_one_column_offers_it() {
        // FIVE power buttons in the primary column (tiers 0..=2 hold 2, 2, 1)
        // and one in the secondary, plus the upgrade slot, confirm and cancel:
        // nine. This witness was written asserting eight and failed, and the
        // miscount pointed at the asymmetry below.
        let b = beacon_buttons();
        assert_eq!(b.len(), 9);
        let powers = b
            .iter()
            .filter(|b| matches!(b.kind, BeaconButtonKind::Power { .. }))
            .count();
        assert_eq!(powers, 6, "5 primary + 1 secondary");

        // The columns are built from DISJOINT tiers — 0..=2 primary, 3
        // secondary — so no effect is offered in both. Strength can only ever
        // be a primary and regeneration can only ever be a secondary, which is
        // the game rule the layout encodes.
        let column_of = |e: BeaconEffect| {
            b.iter().find_map(|b| match b.kind {
                BeaconButtonKind::Power { effect, primary, .. } if effect == e => Some(primary),
                _ => None,
            })
        };
        assert_eq!(column_of(BeaconEffect::Strength), Some(true));
        assert_eq!(column_of(BeaconEffect::Regeneration), Some(false));
        // Every effect in the flattened table appears exactly once.
        for e in BeaconEffect::ALL {
            assert_eq!(
                b.iter()
                    .filter(|b| matches!(b.kind, BeaconButtonKind::Power { effect, .. } if effect == e))
                    .count(),
                1,
                "{e:?}"
            );
        }
    }

    #[test]
    fn there_is_one_screen_slot_per_menu_type() {
        assert_eq!(SCREENS.len(), crate::menu_layout::REGISTRY.len());
    }

    #[test]
    fn only_the_lectern_has_no_container_screen() {
        let missing: Vec<_> = SCREENS
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_none())
            .map(|(i, _)| crate::menu_layout::REGISTRY[i].name)
            .collect();
        assert_eq!(missing, vec!["lectern"]);
    }

    #[test]
    fn screen_and_layout_agree_on_panel_size() {
        // Worth being precise about what this does and does not prove. It is
        // NOT two independent sources: both tables took the panel size from
        // the `*Screen` constructors, because that is the only place it
        // exists. So this catches a *transcription* slip -- 176 typed in one
        // table and 177 in the other -- and cannot catch a misreading of
        // vanilla, which would be identical in both. The layout table carries
        // the size at all only because its consumers had it to hand before
        // this module existed; if one of them is ever wrong, it is wrong here
        // too.
        for (i, s) in SCREENS.iter().enumerate() {
            let Some(s) = s else { continue };
            let l = &crate::menu_layout::REGISTRY[i];
            assert_eq!(s.image_w, l.image_w, "{} width", l.name);
            assert_eq!(s.image_h, l.image_h, "{} height", l.name);
        }
    }

    #[test]
    fn the_chest_family_is_two_blits_that_tile_exactly() {
        for rows in 1..=6u8 {
            let s = screen_of(rows as i32 - 1).unwrap();
            let b = background_blits(s);
            assert_eq!(b.len(), 2, "a chest is two blits");
            let (y0, v0, h0) = b[0];
            let (y1, v1, h1) = b[1];
            assert_eq!((y0, v0), (0, 0.0));
            assert_eq!(h0, rows as i32 * 18 + 17, "the top is rows*18 + 17");
            assert_eq!(y1, h0, "the second blit starts where the first ends");
            assert_eq!((v1, h1), (126.0, 96), "the player's half is 96px from v=126");
            // NOT `== image_h`. This assertion was written as "the two exactly
            // fill the panel" and failed, and the failure was correct: vanilla
            // paints `rows * 18 + 17 + 96 = rows * 18 + 113` of a panel it
            // declares `114 + rows * 18` tall, so **the bottom pixel row of a
            // chest panel is never painted by the background**. Both numbers
            // are literals in the decompile (`ContainerScreen`'s constructor
            // and its two blits), so this is vanilla's arithmetic and not a
            // transcription slip -- and a "fix" that stretched the second blit
            // to close the gap would sample a row of `generic_54.png` vanilla
            // never samples.
            assert_eq!(
                y1 + h1,
                s.image_h - 1,
                "the background stops one pixel short of the declared height"
            );
        }
    }

    #[test]
    fn every_other_screen_is_one_full_height_blit() {
        for (i, s) in SCREENS.iter().enumerate() {
            let Some(s) = s else { continue };
            if matches!(s.background, Background::ChestRows(_)) {
                continue;
            }
            let b = background_blits(s);
            assert_eq!(b.len(), 1, "{i}");
            assert_eq!(b[0], (0, 0.0, s.image_h), "{i}");
        }
    }

    #[test]
    fn a_quads_source_rect_is_the_same_size_as_its_destination() {
        // Nothing here scales. If a source rect were ever a different size
        // from its destination the sheet would be stretched, and the tell is
        // subtle -- a one-row difference reads as a slightly soft panel edge,
        // not as an obvious stretch.
        for s in SCREENS.iter().flatten() {
            for q in background_quads(s) {
                let src_w = (q.u1 - q.u0) * s.sheet_w;
                let src_h = (q.v1 - q.v0) * s.sheet_h;
                assert!((src_w - q.w as f32).abs() < 1e-4, "{} w", s.texture);
                assert!((src_h - q.h as f32).abs() < 1e-4, "{} h", s.texture);
            }
        }
    }

    #[test]
    fn a_chests_two_quads_sample_a_gap_in_the_sheet() {
        // The reason a chest cannot be one stretched quad: the second blit
        // starts at v = 126 while the first ended at v = rows*18 + 17, so for
        // anything under six rows there is a band of generic_54.png between
        // them that the panel deliberately skips. At six rows the two meet
        // (108 + 17 = 125) and the gap closes to a single row.
        let three = screen_of(2).unwrap();
        let q = background_quads(three);
        assert_eq!(q.len(), 2);
        let first_end_v = q[0].v1 * three.sheet_h;
        let second_start_v = q[1].v0 * three.sheet_h;
        assert_eq!(first_end_v, 71.0, "3 rows: 3*18 + 17");
        assert_eq!(second_start_v, 126.0);
        assert!(second_start_v > first_end_v, "the skipped band is real");

        let six = screen_of(5).unwrap();
        let q6 = background_quads(six);
        assert_eq!(q6[0].v1 * six.sheet_h, 125.0, "6 rows: 6*18 + 17");
        assert_eq!(q6[1].v0 * six.sheet_h, 126.0, "still one row apart, never before");
    }

    #[test]
    fn a_quad_never_samples_outside_the_sheet() {
        for s in SCREENS.iter().flatten() {
            for q in background_quads(s) {
                assert!(q.u1 <= 1.0 + 1e-6, "{} u {}", s.texture, q.u1);
                assert!(q.v1 <= 1.0 + 1e-6, "{} v {}", s.texture, q.v1);
            }
        }
        // This assertion FAILED when the sheet size was a global 256, and the
        // failure was the finding: `MerchantScreen` blits `512, 256`, because
        // a 276 px panel cannot come off a 256-wide texture. With a global
        // 256 its u1 is 276/256 = 1.078 and the sampler wraps or clamps,
        // repeating the left edge across the right-hand third of the trade
        // screen. So the sheet is per-screen, and this is the witness.
        let merchant = screen_of(19).unwrap();
        assert_eq!(merchant.image_w, 276);
        assert_eq!(merchant.sheet_w, 512.0);
        assert!((background_quads(merchant)[0].u1 - 276.0 / 512.0).abs() < 1e-6);
    }

    #[test]
    fn a_centered_title_moves_with_its_width_and_a_fixed_one_does_not() {
        // The distinction that cannot be flattened: three screens compute
        // (imageWidth - font.width(title)) / 2, so storing whatever the
        // vanilla English title happens to measure would misplace a
        // custom-named container's title and nothing else.
        let dispenser = screen_of(6).unwrap();
        assert_eq!(dispenser.title_x(0), 88);
        assert_eq!(dispenser.title_x(60), 58);
        let anvil = screen_of(8).unwrap();
        assert_eq!(anvil.title_x(0), 60);
        assert_eq!(anvil.title_x(60), 60, "a fixed title does not move");
    }

    #[test]
    fn the_inventory_label_is_always_ninety_four_above_the_bottom() {
        for s in SCREENS.iter().flatten() {
            assert_eq!(s.inventory_label_y(), s.image_h - 94);
        }
        // The chest's, spot-checked against the value ContainerScreen assigns.
        assert_eq!(screen_of(5).unwrap().inventory_label_y(), 222 - 94);
    }

    #[test]
    fn merchant_is_the_only_screen_that_moves_the_inventory_label_x() {
        let moved: Vec<_> = SCREENS
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_some_and(|s| s.inventory_label_x != 8))
            .map(|(i, _)| crate::menu_layout::REGISTRY[i].name)
            .collect();
        assert_eq!(moved, vec!["merchant"]);
    }
}

// -- CrafterScreen (M93j) ---------------------------------------------------

/// `CrafterScreen.extractDisabledSlot` —
/// `blitSprite(DISABLED_SLOT, cs.x - 1, cs.y - 1, 18, 18)`.
///
/// **A third slot geometry**, and none of the three is the obvious one. The
/// icon is 16x16 at the slot's own position; M35's hover highlight is 24x24 at
/// `slot - 4`, *bracketing* the icon; this cover is 18x18 at `slot - 1`, which
/// is the recess the sheet paints rather than either of those. Reusing the
/// highlight's numbers puts a 24px cover over an 18px hole.
///
/// It **replaces** the slot's normal render rather than layering over it:
/// `extractSlot` calls this *instead of* `super.extractSlot`, so a disabled
/// slot shows no item. That is the opposite of the toggle itself, which is
/// additive (M93i) — the two halves of the same feature compose the two
/// different ways, and swapping them either hides an enabled slot's item or
/// paints the cover under one.
pub fn crafter_disabled_cover(slot_x: i32, slot_y: i32) -> ProgressBlit {
    ProgressBlit {
        dx: slot_x - 1,
        dy: slot_y - 1,
        w: CRAFTER_COVER_SIZE,
        h: CRAFTER_COVER_SIZE,
        sx: 0,
        sy: 0,
    }
}

/// The disabled-slot cover's edge, in GUI pixels.
pub const CRAFTER_COVER_SIZE: i32 = 18;

/// `CrafterScreen.extractRedstone`'s arrow — 16x16 at panel-relative (97, 35).
///
/// **Vanilla writes this one in SCREEN coordinates**, alone in the class:
///
/// ```java
/// int xo = this.width / 2 + 9;
/// int yo = this.height / 2 - 48;
/// ```
///
/// where every other blit is `leftPos + ...`. The two agree *only* because the
/// crafter's panel is the standard 176x166: `leftPos = width/2 - 88`, so
/// `width/2 + 9 == leftPos + 97`, and `topPos = height/2 - 83`, so
/// `height/2 - 48 == topPos + 35`. Recorded as panel-relative because that is
/// what Rewo's overlay seam speaks, and with the derivation written down
/// because a screen with a non-standard panel size would make the two forms
/// diverge and this constant silently wrong.
pub fn crafter_redstone() -> ProgressBlit {
    ProgressBlit {
        dx: 97,
        dy: 35,
        w: 16,
        h: 16,
        sx: 0,
        sy: 0,
    }
}

#[cfg(test)]
mod m93j_crafter {
    use super::*;

    /// The cover is 18x18 at `slot - 1` — **not** the icon's 16x16 at the slot,
    /// and not M35's highlight at `slot - 4` / 24x24.
    #[test]
    fn the_cover_is_a_third_slot_geometry() {
        // CrafterMenu's grid slot 0 is at (26, 17).
        let c = crafter_disabled_cover(26, 17);
        assert_eq!((c.dx, c.dy, c.w, c.h), (25, 16, 18, 18));
        // Explicitly NOT the highlight's numbers, which bracket the icon and
        // would put a 24px cover over an 18px hole.
        assert_ne!((c.dx, c.dy, c.w, c.h), (26 - 4, 17 - 4, 24, 24));
        // ...and not the icon's, which would leave the recess showing.
        assert_ne!((c.dx, c.dy, c.w, c.h), (26, 17, 16, 16));
        // The offset is uniform, so the last grid slot lands the same way.
        let c8 = crafter_disabled_cover(26 + 2 * 18, 17 + 2 * 18);
        assert_eq!((c8.dx, c8.dy), (25 + 36, 16 + 36));
    }

    /// Vanilla writes the arrow in SCREEN coordinates; this records the
    /// panel-relative equivalent, and the two agree only for a 176x166 panel.
    #[test]
    fn the_redstone_arrow_is_the_screen_form_resolved_against_a_176x166_panel() {
        let r = crafter_redstone();
        assert_eq!((r.dx, r.dy, r.w, r.h), (97, 35, 16, 16));
        // The derivation, so the constant is checkable rather than magic:
        //   leftPos = (width - 176) / 2 = width/2 - 88, and vanilla's
        //   xo = width/2 + 9, so xo - leftPos = 97.
        //   topPos = (height - 166) / 2 = height/2 - 83, and yo = height/2 - 48,
        //   so yo - topPos = 35.
        let (image_w, image_h) = (176i32, 166i32);
        for (width, height) in [(854i32, 480i32), (1280, 720), (640, 400)] {
            let left = (width - image_w) / 2;
            let top = (height - image_h) / 2;
            assert_eq!(width / 2 + 9 - left, r.dx, "at {width}x{height}");
            assert_eq!(height / 2 - 48 - top, r.dy, "at {width}x{height}");
        }
    }
}

/// What pressing a beacon button does (M93l).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeaconPress {
    /// Nothing: the button is disabled or hidden, or it is already selected.
    None,
    /// The screen's new `(primary, secondary)`.
    Select(BeaconChoice),
    /// Send `set_beacon` with the current choice, then close the container.
    Confirm,
    /// Close the container, sending nothing.
    Cancel,
}

/// `BeaconPowerButton.onPress` and its two siblings (M93l).
///
/// ```java
/// if (!this.isSelected()) {
///    if (this.isPrimary) {
///       primary = this.effect;
///       if (!Objects.equals(secondary, this.effect)) secondary = null;
///    } else {
///       secondary = this.effect;
///    }
///    updateButtons();
/// }
/// ```
///
/// # Choosing a primary CLEARS the secondary — except when they match
///
/// That guard reads backwards at first. Picking a new primary discards
/// whatever secondary you had, because a secondary is only meaningful
/// alongside the primary it was chosen with — **unless** the secondary is
/// already the same effect, which is the "primary at level II" double and is
/// still valid. Inverting the condition keeps exactly the choices that should
/// be discarded and discards the one that should be kept.
///
/// # The upgrade button is an ordinary secondary button
///
/// `BeaconUpgradePowerButton extends BeaconPowerButton` with `isPrimary =
/// false` and `tier = 3`, and `updateStatus` re-points its effect at the
/// current primary. So pressing it takes the secondary branch with
/// `effect = primary` — it is not a fourth kind of press.
///
/// # A press only happens on an ACTIVE, VISIBLE button
///
/// `AbstractWidget.onClick` is reached through `mouseClicked`, which requires
/// both. Gating here rather than at the call site keeps the button's own
/// `updateStatus` rules — `tier < levels`, `hasPayment() && primary != null` —
/// as the single source for both what is drawn and what responds.
pub fn beacon_press(b: BeaconButton, s: BeaconChoice) -> BeaconPress {
    match beacon_button_state(b, s, false) {
        BeaconButtonState::Disabled | BeaconButtonState::Hidden => return BeaconPress::None,
        _ => {}
    }
    match b.kind {
        BeaconButtonKind::Confirm => BeaconPress::Confirm,
        BeaconButtonKind::Cancel => BeaconPress::Cancel,
        BeaconButtonKind::Power { effect, primary, .. } => beacon_select(s, effect, primary),
        // The upgrade button borrows the primary and presses as a secondary.
        BeaconButtonKind::Upgrade => match s.primary {
            Some(p) => beacon_select(s, p, false),
            None => BeaconPress::None,
        },
    }
}

fn beacon_select(s: BeaconChoice, effect: BeaconEffect, primary: bool) -> BeaconPress {
    let chosen = if primary { s.primary } else { s.secondary };
    // `if (!this.isSelected())` — pressing the lit button is inert.
    if chosen == Some(effect) {
        return BeaconPress::None;
    }
    let mut next = s;
    if primary {
        next.primary = Some(effect);
        if next.secondary != Some(effect) {
            next.secondary = None;
        }
    } else {
        next.secondary = Some(effect);
    }
    BeaconPress::Select(next)
}

#[cfg(test)]
mod m93l_beacon_press {
    use super::*;

    fn power(effect: BeaconEffect, primary: bool, tier: i32) -> BeaconButton {
        BeaconButton { x: 0, y: 0, kind: BeaconButtonKind::Power { effect, primary, tier } }
    }
    fn lit() -> BeaconChoice {
        BeaconChoice { levels: 4, has_payment: true, ..Default::default() }
    }
    fn effects(primary: bool) -> (BeaconEffect, BeaconEffect) {
        // Two distinct tier-0 effects from the real table, so the fixture
        // cannot drift from the layout.
        let t0 = beacon_tier_effects(0);
        let other = beacon_tier_effects(1);
        let _ = primary;
        (t0[0], other[0])
    }

    #[test]
    fn choosing_a_primary_clears_a_DIFFERENT_secondary_and_keeps_a_matching_one() {
        // THE guard that reads backwards. A secondary is only meaningful
        // alongside the primary it was chosen with — except when it IS that
        // primary, which is the "primary at level II" double.
        let (a, b) = effects(true);
        // secondary = b, choose primary = a  ->  b is discarded.
        let s = BeaconChoice { primary: None, secondary: Some(b), ..lit() };
        let BeaconPress::Select(next) = beacon_press(power(a, true, 0), s) else {
            panic!("a fresh primary must select");
        };
        assert_eq!(next.primary, Some(a));
        assert_eq!(next.secondary, None, "a different secondary is cleared");

        // secondary = a, choose primary = a  ->  a is KEPT.
        let s = BeaconChoice { primary: None, secondary: Some(a), ..lit() };
        let BeaconPress::Select(next) = beacon_press(power(a, true, 0), s) else {
            panic!("must select");
        };
        assert_eq!(next.primary, Some(a));
        assert_eq!(next.secondary, Some(a), "a MATCHING secondary survives");
    }

    #[test]
    fn pressing_the_lit_button_is_inert() {
        let (a, _) = effects(true);
        let s = BeaconChoice { primary: Some(a), ..lit() };
        assert_eq!(beacon_press(power(a, true, 0), s), BeaconPress::None);
        // ...but the same button in the OTHER column is a different selection.
        assert!(matches!(
            beacon_press(power(a, false, 0), s),
            BeaconPress::Select(_)
        ));
    }

    #[test]
    fn a_disabled_or_hidden_button_does_not_press() {
        let (a, _) = effects(true);
        // tier 3 on a level-1 beacon: `tier < levels` is false.
        let weak = BeaconChoice { levels: 1, has_payment: true, ..Default::default() };
        assert_eq!(beacon_press(power(a, true, 3), weak), BeaconPress::None);
        // Confirm needs BOTH a payment and a primary.
        let confirm = BeaconButton { x: 0, y: 0, kind: BeaconButtonKind::Confirm };
        assert_eq!(
            beacon_press(confirm, BeaconChoice { primary: Some(a), has_payment: false, ..lit() }),
            BeaconPress::None
        );
        assert_eq!(
            beacon_press(confirm, BeaconChoice { primary: None, ..lit() }),
            BeaconPress::None
        );
        assert_eq!(
            beacon_press(confirm, BeaconChoice { primary: Some(a), ..lit() }),
            BeaconPress::Confirm
        );
        // Cancel is unconditional — it does not even need a primary.
        let cancel = BeaconButton { x: 0, y: 0, kind: BeaconButtonKind::Cancel };
        assert_eq!(beacon_press(cancel, BeaconChoice::default()), BeaconPress::Cancel);
    }

    #[test]
    fn the_upgrade_button_presses_as_a_SECONDARY_holding_the_primary() {
        // It is a BeaconPowerButton with isPrimary = false whose effect is
        // re-pointed at the primary, not a fourth kind of press.
        let (a, _) = effects(true);
        let up = BeaconButton { x: 0, y: 0, kind: BeaconButtonKind::Upgrade };
        let s = BeaconChoice { primary: Some(a), ..lit() };
        let BeaconPress::Select(next) = beacon_press(up, s) else {
            panic!("the upgrade must select");
        };
        assert_eq!(next.primary, Some(a), "the primary is untouched");
        assert_eq!(next.secondary, Some(a), "the secondary becomes the primary");
        // Hidden with no primary, and inert once already doubled.
        assert_eq!(beacon_press(up, BeaconChoice { primary: None, ..lit() }), BeaconPress::None);
        assert_eq!(beacon_press(up, next), BeaconPress::None);
    }
}

// -- LoomScreen's pattern grid (M93o) ---------------------------------------

/// The pattern grid's origin, relative to the panel.
pub const LOOM_GRID_X: i32 = 60;
pub const LOOM_GRID_Y: i32 = 13;
/// Its cell pitch **and** its hit size — vanilla uses 14 for both, so the
/// cells tile with no gap and no bleed.
pub const LOOM_CELL: i32 = 14;
/// `for (row 0..4) for (column 0..4)` — the visible window, not the total.
pub const LOOM_COLS: i32 = 4;
pub const LOOM_ROWS: i32 = 4;

/// Which pattern index the cursor is over, given the first visible row.
///
/// ```java
/// double xx = event.x() - (xo + column * 14);
/// double yy = event.y() - (yo + row * 14);
/// int index = (row + this.startRow) * 4 + column;
/// if (xx >= 0 && yy >= 0 && xx < 14 && yy < 14 && menu.clickMenuButton(player, index))
/// ```
///
/// **The bounds test and the range test are separate**, and vanilla puts the
/// range test inside `clickMenuButton` — so a cell past the end of the list is
/// *hit* and then *rejected*, and the loop carries on rather than stopping.
/// Returning the index here and letting the caller range-check keeps that
/// split, which matters because a rejected cell must not consume the click.
pub fn loom_cell_at(gui_x: f64, gui_y: f64, start_row: i32) -> Option<i32> {
    let dx = gui_x - LOOM_GRID_X as f64;
    let dy = gui_y - LOOM_GRID_Y as f64;
    if dx < 0.0 || dy < 0.0 {
        return None;
    }
    let (col, row) = ((dx / LOOM_CELL as f64) as i32, (dy / LOOM_CELL as f64) as i32);
    if col >= LOOM_COLS || row >= LOOM_ROWS {
        return None;
    }
    Some((row + start_row) * LOOM_COLS + col)
}

/// `LoomMenu.clickMenuButton` — `buttonId >= 0 && buttonId < selectable.size()`.
///
/// The client runs this **before** sending, and only sends when it passes, so
/// an out-of-range cell produces no packet at all.
pub fn loom_button_accepted(index: i32, selectable: usize) -> bool {
    // `try_from` rather than `index >= 0 && (index as usize) < selectable`.
    //
    // Deleting the non-negative half is an **equivalent mutant in Rust, and
    // only in Rust**: `(-1i32) as usize` wraps to `usize::MAX`, which fails
    // the upper bound for every representable `selectable`, so no input
    // distinguishes the two. It is load-bearing in Java, where `<` does not
    // wrap — vanilla's `buttonId >= 0` is a real guard.
    //
    // A mutation survived here and was shown equivalent rather than witnessed
    // (M93g's precedent). The rewrite is kept anyway: it states the intent
    // without leaning on the wrap, so a future `selectable` that is not a
    // `usize` cannot quietly make the two readings differ.
    usize::try_from(index).is_ok_and(|i| i < selectable)
}

/// `LoomScreen.displayPatterns` — whether the grid is shown at all.
///
/// ```java
/// displayPatterns = !bannerStack.isEmpty() && !dyeStack.isEmpty()
///                   && !hasMaxPatterns && !menu.getSelectablePatterns().isEmpty();
/// ```
///
/// **A dye is required to show the grid**, which is easy to miss: a banner
/// alone offers nothing, because a loom stamps one dyed layer at a time.
pub fn loom_display_patterns(
    has_banner: bool,
    has_dye: bool,
    has_max_patterns: bool,
    selectable: usize,
) -> bool {
    has_banner && has_dye && !has_max_patterns && selectable != 0
}

/// `LoomScreen.canScroll` — `displayPatterns && selectable.size() > 16`.
///
/// **Strictly greater**, so exactly 16 patterns fill the window and do not
/// scroll; the 32-entry no-item-required set does.
pub fn loom_can_scroll(display_patterns: bool, selectable: usize) -> bool {
    display_patterns && selectable > (LOOM_COLS * LOOM_ROWS) as usize
}

#[cfg(test)]
mod m93o_loom_grid {
    use super::*;

    #[test]
    fn the_cells_tile_with_no_gap_and_no_bleed() {
        // Pitch and hit size are both 14, so cell N ends exactly where N+1
        // begins. A 12px hit on a 14px pitch would leave dead seams; a 16px
        // one would make neighbours overlap.
        assert_eq!(loom_cell_at(60.0, 13.0, 0), Some(0));
        assert_eq!(loom_cell_at(73.9, 26.9, 0), Some(0), "the far corner of cell 0");
        assert_eq!(loom_cell_at(74.0, 13.0, 0), Some(1), "and 74 is already cell 1");
        assert_eq!(loom_cell_at(60.0, 27.0, 0), Some(4), "one row down is +4");
        // Just outside on either axis is a miss, not a clamp.
        assert_eq!(loom_cell_at(59.9, 13.0, 0), None);
        assert_eq!(loom_cell_at(60.0, 12.9, 0), None);
        // The window is 4x4; the fifth column and row are outside it.
        assert_eq!(loom_cell_at(60.0 + 4.0 * 14.0, 13.0, 0), None);
        assert_eq!(loom_cell_at(60.0, 13.0 + 4.0 * 14.0, 0), None);
    }

    #[test]
    fn scrolling_moves_the_index_by_whole_rows() {
        // `(row + startRow) * 4 + column`, so a row of scroll is 4 indices.
        assert_eq!(loom_cell_at(60.0, 13.0, 0), Some(0));
        assert_eq!(loom_cell_at(60.0, 13.0, 1), Some(4));
        assert_eq!(loom_cell_at(60.0, 13.0, 4), Some(16));
        // The COLUMN is unaffected by the scroll.
        assert_eq!(loom_cell_at(60.0 + 14.0, 13.0, 3), Some(13));
    }

    #[test]
    fn a_cell_past_the_end_is_hit_and_then_rejected() {
        // The two tests are separate in vanilla — the bounds test finds the
        // cell, `clickMenuButton` refuses the index — and keeping them apart
        // is what lets a rejected cell fall through instead of consuming the
        // click.
        assert_eq!(loom_cell_at(60.0 + 3.0 * 14.0, 13.0, 0), Some(3));
        assert!(!loom_button_accepted(3, 3), "index 3 needs a 4th pattern");
        assert!(loom_button_accepted(3, 4));
        assert!(!loom_button_accepted(-1, 4), "and negative is rejected too");
    }

    #[test]
    fn the_grid_needs_a_dye_and_not_just_a_banner() {
        // Easy to miss: a loom stamps one DYED layer at a time, so a banner
        // alone shows nothing.
        assert!(!loom_display_patterns(true, false, false, 32));
        assert!(!loom_display_patterns(false, true, false, 32));
        assert!(loom_display_patterns(true, true, false, 32));
        // A full banner offers nothing more...
        assert!(!loom_display_patterns(true, true, true, 32));
        // ...and neither does an empty selectable set.
        assert!(!loom_display_patterns(true, true, false, 0));
    }

    #[test]
    fn sixteen_patterns_fill_the_window_without_scrolling() {
        // `> 16`, strictly — exactly one screenful does not scroll.
        assert!(!loom_can_scroll(true, 16));
        assert!(loom_can_scroll(true, 17));
        assert!(loom_can_scroll(true, 32), "the no-item-required set does");
        // ...and a hidden grid never scrolls whatever its size.
        assert!(!loom_can_scroll(false, 32));
    }
}
