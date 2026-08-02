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
