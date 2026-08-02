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
//! * `dispenser`, `crafter_3x3` and `brewing_stand` compute
//!   `(imageWidth - font.width(title)) / 2` — a *centring*, which depends on
//!   the rendered width of a title the server chose, so it cannot be stored as
//!   a constant.
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
    }),
    plain("textures/gui/container/blast_furnace.png"),
    centered("textures/gui/container/brewing_stand.png"),
    titled("textures/gui/container/crafting_table.png", 29),
    plain("textures/gui/container/enchanting_table.png"),
    plain("textures/gui/container/furnace.png"),
    plain("textures/gui/container/grindstone.png"),
    Some(MenuScreen {
        texture: "textures/gui/container/hopper.png",
        image_w: 176,
        image_h: 133,
        title_x: TitleX::Default,
        title_y: 6,
        inventory_label_x: 8,
        background: Background::Whole,
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
    }),
    Some(MenuScreen {
        texture: "textures/gui/container/shulker_box.png",
        image_w: 176,
        image_h: 167,
        title_x: TitleX::Default,
        title_y: 6,
        inventory_label_x: 8,
        background: Background::Whole,
    }),
    Some(MenuScreen {
        texture: "textures/gui/container/smithing.png",
        image_w: 176,
        image_h: 166,
        title_x: TitleX::Fixed(44),
        title_y: 15,
        inventory_label_x: 8,
        background: Background::Whole,
    }),
    plain("textures/gui/container/smoker.png"),
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

#[cfg(test)]
mod tests {
    use super::*;

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
