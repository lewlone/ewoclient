//! The recipe book's ghost recipe — the preview laid over a menu's slots
//! (M103).
//!
//! M93y decoded `place_ghost_recipe` into `PlaySession::ghost_recipe` and
//! nothing consumed it. This is the consumer: which slot each ingredient goes
//! in, and what is drawn over it.
//!
//! # The item is sandwiched between two washes of DIFFERENT colours
//!
//! `GhostSlots.extractRenderState` fills, draws the item, then fills again:
//!
//! ```java
//! graphics.fill(x, y, x + 16, y + 16, 822018048);   // 0x30FF0000 — RED
//! graphics.fakeItem(itemStack, x, y);
//! graphics.fill(x, y, x + 16, y + 16, 822083583);   // 0x30FFFFFF — WHITE
//! ```
//!
//! Both alpha 48, and they are **not the same colour** — a red tint behind and a
//! white veil in front, which together give the ghost its washed-out look.
//! Reading either as "a grey wash", or drawing one and not the other, changes
//! what it looks like entirely.
//!
//! # Only a result slot gets a count, and only some get the big wash
//!
//! `itemDecorations` runs for the result and nothing else, so an input ghost
//! never shows a number. And `isResultSlotBig` widens the result's wash to
//! 24x24 at `(x - 4, y - 4)` — **true by default**, false only for
//! `InventoryScreen`. So the player's own 2x2 result gets the plain 16x16 and
//! every other screen's gets the big one.

/// `0x30FF0000` — drawn **under** the item.
pub const WASH_UNDER: u32 = 0x30FF_0000;
/// `0x30FFFFFF` — drawn **over** it.
pub const WASH_OVER: u32 = 0x30FF_FFFF;

/// One ghosted slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ghost {
    /// The menu slot index it covers.
    pub slot: usize,
    /// The item ids this ingredient stands for, in order. The render cycles
    /// through them.
    pub items: Vec<i32>,
    pub is_result: bool,
}

impl Ghost {
    /// `GhostSlot.getItem(index)` — `items.get(index % size)`.
    ///
    /// A **one**-level cycle, unlike `RecipeButton.getDisplayStack`'s two-level
    /// one (M95): a ghost has no notion of "which recipe", only "which member of
    /// this ingredient".
    pub fn item(&self, index: i32) -> Option<i32> {
        if self.items.is_empty() {
            return None;
        }
        let n = self.items.len() as i32;
        self.items
            .get((index - n * index.div_euclid(n)) as usize)
            .copied()
    }
}

/// The wash rect for a ghost, in the menu's own coordinates.
///
/// `(dx, dy, size)` relative to the slot's top-left.
pub fn wash_rect(is_result: bool, big_result: bool) -> (i32, i32, i32) {
    if is_result && big_result {
        (-4, -4, 24)
    } else {
        (0, 0, 16)
    }
}

/// `AbstractRecipeBookScreen.isBiggerResultSlot` — **true by default**, and
/// false only for `InventoryScreen`.
///
/// So the player's own 2x2 result draws the plain 16x16 wash and a crafting
/// table's or a furnace's draws 24x24. The name suggests the special case is the
/// big one; the override says otherwise.
pub fn big_result_slot(player_inventory: bool) -> bool {
    !player_inventory
}

/// `PlaceRecipeHelper.placeRecipe` — where a `recipeW x recipeH` shape lands in
/// a `gridW x gridH` grid (M103).
///
/// Returns the grid index for each ingredient, in the order they are consumed;
/// an ingredient list shorter than the shape simply stops.
///
/// **The centring test is strict and in floats**: `recipeHeight < gridHeight /
/// 2.0F`. For a 3-tall grid that is `< 1.5`, so a **1**-tall recipe is centred
/// and a **2**-tall one is not — it stays top-left.
///
/// The strictness itself is **unobservable on the grids Minecraft has**: `<` and
/// `<=` differ only when the grid's dimension is even *and* the recipe is
/// exactly half of it, and for a 2x2 the resulting `startPos` is 0 either way
/// while a 3x3's half is never an integer. A mutation to `<=` therefore survived
/// every witness until one was written on a 4x4, where the two readings finally
/// diverge. Transcribed strictly for fidelity, and the test below says why the
/// fixture is not a Minecraft grid.
pub fn place_recipe(
    grid_w: usize,
    grid_h: usize,
    recipe_w: usize,
    recipe_h: usize,
    count: usize,
) -> Vec<usize> {
    let mut out = Vec::with_capacity(count);
    let mut grid_index = 0usize;
    let mut taken = 0usize;
    let mut y = 0usize;
    while y < grid_h {
        // The row skip: when the shape is centred vertically and this row is
        // above where it starts, the whole row is stepped over — and vanilla
        // then advances `gridYPos` a SECOND time via the for-loop, so the skip
        // costs one row and processing continues on the next.
        let center_v = (recipe_h as f32) < (grid_h as f32) / 2.0;
        let start_v = ((grid_h as f32) / 2.0 - (recipe_h as f32) / 2.0).floor() as usize;
        let mut row = y;
        if center_v && start_v > y {
            grid_index += grid_w;
            row = y + 1;
        }
        let mut x = 0usize;
        while x < grid_w {
            if taken >= count {
                return out;
            }
            let center_h = (recipe_w as f32) < (grid_w as f32) / 2.0;
            let start_h = ((grid_w as f32) / 2.0 - (recipe_w as f32) / 2.0).floor() as usize;
            let (total_w, add) = if center_h {
                (start_h + recipe_w, start_h <= x && x < start_h + recipe_w)
            } else {
                (recipe_w, x < recipe_w)
            };
            if add {
                out.push(grid_index);
                taken += 1;
            } else if total_w == x {
                // Past the shape's right edge: skip the rest of the row.
                grid_index += grid_w - x;
                break;
            }
            grid_index += 1;
            x += 1;
        }
        y = row + 1;
    }
    out
}

/// A menu's ghostable geometry (M103).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GhostMenu {
    /// `getResultSlot()`'s menu index.
    pub result: usize,
    /// `getInputGridSlots()`'s first index, and the grid's shape. `None` for a
    /// furnace, whose inputs are named individually rather than gridded.
    pub grid: Option<(usize, usize, usize)>,
    /// The furnace's `(ingredient, fuel)` slots.
    pub furnace: Option<(usize, usize)>,
}

/// The crafting family: result 0, then a `w x h` grid from slot 1.
pub const fn crafting_menu(w: usize, h: usize) -> GhostMenu {
    GhostMenu { result: 0, grid: Some((1, w, h)), furnace: None }
}

/// `AbstractFurnaceMenu`: ingredient 0, fuel 1, **result 2**.
pub const FURNACE_MENU: GhostMenu =
    GhostMenu { result: 2, grid: None, furnace: Some((0, 1)) };

/// One ingredient, already resolved to the items it stands for.
pub type Resolved = Vec<i32>;

/// Where a ghost recipe's ingredients land (M103).
///
/// `fillGhostRecipe` sets the **result first**, then the inputs — and the two
/// families differ in more than their slots:
///
/// * **shaped crafting** places its shape through [`place_recipe`], so a small
///   recipe is centred in a big grid;
/// * **shapeless crafting** fills the first `min(ingredients, slots)` input
///   slots in order, with no centring at all;
/// * a **furnace** ghosts its ingredient always and its **fuel only if the fuel
///   slot is EMPTY** — so a furnace that already has coal in it shows no fuel
///   ghost. That guard is easy to miss and its absence is a ghost that covers
///   the fuel you already put there.
/// * anything else (a stonecutter or smithing display) ghosts **the result
///   alone**, because `fillGhostRecipe`'s switch has no case for it.
pub fn layout(
    menu: GhostMenu,
    result: Resolved,
    inputs: &[Resolved],
    shape: Option<(usize, usize)>,
    fuel_slot_empty: bool,
) -> Vec<Ghost> {
    let mut out = Vec::new();
    // `setSlot` skips an ingredient that resolves to nothing, so an unresolvable
    // display leaves its slot un-ghosted rather than washing an empty cell.
    if !result.is_empty() {
        out.push(Ghost { slot: menu.result, items: result, is_result: true });
    }
    if let Some((first, gw, gh)) = menu.grid {
        match shape {
            Some((rw, rh)) => {
                for (n, cell) in place_recipe(gw, gh, rw, rh, inputs.len())
                    .into_iter()
                    .enumerate()
                {
                    if !inputs[n].is_empty() {
                        out.push(Ghost {
                            slot: first + cell,
                            items: inputs[n].clone(),
                            is_result: false,
                        });
                    }
                }
            }
            None => {
                // `min(ingredients.size(), inputSlots.size())` — a shapeless
                // recipe with more ingredients than the grid has slots is
                // truncated, not rejected.
                for (i, ing) in inputs.iter().take(gw * gh).enumerate() {
                    if !ing.is_empty() {
                        out.push(Ghost {
                            slot: first + i,
                            items: ing.clone(),
                            is_result: false,
                        });
                    }
                }
            }
        }
    } else if let Some((ing_slot, fuel)) = menu.furnace {
        if let Some(i) = inputs.first().filter(|v| !v.is_empty()) {
            out.push(Ghost { slot: ing_slot, items: i.clone(), is_result: false });
        }
        if fuel_slot_empty {
            if let Some(f) = inputs.get(1).filter(|v| !v.is_empty()) {
                out.push(Ghost { slot: fuel, items: f.clone(), is_result: false });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(id: i32) -> Resolved {
        vec![id]
    }

    /// The result is ghosted first and is the only slot marked as one.
    #[test]
    fn the_result_is_ghosted_first_and_alone_carries_the_flag() {
        let g = layout(crafting_menu(3, 3), one(99), &[one(1)], Some((1, 1)), true);
        assert_eq!(g[0].slot, 0, "the result slot, first");
        assert!(g[0].is_result);
        assert!(g[1..].iter().all(|x| !x.is_result));
    }

    /// A shaped recipe goes through `place_recipe`, so a 1x1 centres.
    #[test]
    fn a_shaped_recipe_is_placed_and_centred() {
        let g = layout(crafting_menu(3, 3), one(99), &[one(1)], Some((1, 1)), true);
        // Grid slot 4 is menu slot 1 + 4 = 5.
        assert_eq!(g[1].slot, 5);
    }

    /// A SHAPELESS recipe is not centred — it fills the first slots in order,
    /// which is the difference the two branches exist for.
    #[test]
    fn a_shapeless_recipe_fills_the_first_slots_without_centring() {
        let ing = [one(1), one(2), one(3)];
        let g = layout(crafting_menu(3, 3), one(99), &ing, None, true);
        assert_eq!(
            g[1..].iter().map(|x| x.slot).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "in order from the first input slot"
        );
        // The same three as a 3x1 SHAPE land in the middle ROW instead: 1 is
        // under 3/2, so the helper centres it vertically. This witness first
        // expected 1,2,3 for both and was wrong about the shaped case — a
        // 3-wide, 1-tall recipe is centred on the vertical axis even though it
        // spans the horizontal one.
        let shaped = layout(crafting_menu(3, 3), one(99), &ing, Some((3, 1)), true);
        assert_eq!(
            shaped[1..].iter().map(|x| x.slot).collect::<Vec<_>>(),
            vec![4, 5, 6],
            "grid cells 3,4,5 — the middle row"
        );
        // A 1x1 shape centres on both axes; shapeless never centres at all.
        assert_eq!(layout(crafting_menu(3, 3), one(99), &[one(1)], Some((1, 1)), true)[1].slot, 5);
        assert_eq!(layout(crafting_menu(3, 3), one(99), &[one(1)], None, true)[1].slot, 1);
    }

    /// A shapeless recipe with more ingredients than slots is TRUNCATED.
    #[test]
    fn a_shapeless_recipe_longer_than_the_grid_is_truncated() {
        let ing: Vec<Resolved> = (0..7).map(one).collect();
        let g = layout(crafting_menu(2, 2), one(99), &ing, None, true);
        assert_eq!(g.len(), 1 + 4, "the result plus four of the seven");
    }

    /// A furnace ghosts its fuel ONLY when the fuel slot is empty.
    #[test]
    fn a_furnace_ghosts_its_fuel_only_into_an_empty_fuel_slot() {
        let ing = [one(1), one(2)];
        let empty = layout(FURNACE_MENU, one(99), &ing, None, true);
        assert_eq!(
            empty.iter().map(|g| g.slot).collect::<Vec<_>>(),
            vec![2, 0, 1],
            "result 2, ingredient 0, fuel 1"
        );
        let full = layout(FURNACE_MENU, one(99), &ing, None, false);
        assert_eq!(
            full.iter().map(|g| g.slot).collect::<Vec<_>>(),
            vec![2, 0],
            "no fuel ghost over fuel you already have"
        );
    }

    /// An ingredient that resolves to nothing leaves its slot un-ghosted rather
    /// than washing an empty cell — `setSlot`'s own `if (!entries.isEmpty())`.
    #[test]
    fn an_unresolvable_ingredient_is_skipped_rather_than_washed() {
        let ing = [Vec::new(), one(2)];
        let g = layout(crafting_menu(2, 2), one(99), &ing, None, true);
        assert_eq!(g.len(), 2, "the result and the second ingredient");
        assert_eq!(g[1].slot, 2, "and the second keeps ITS slot, not the first's");
        // A result that resolves to nothing is skipped too.
        let no_result = layout(crafting_menu(2, 2), Vec::new(), &[one(1)], None, true);
        assert!(no_result.iter().all(|x| !x.is_result));
    }

    /// A display with no case in `fillGhostRecipe`'s switch — a stonecutter or
    /// smithing one — ghosts the RESULT alone.
    #[test]
    fn a_display_with_no_input_case_ghosts_the_result_alone() {
        let menu = GhostMenu { result: 3, grid: None, furnace: None };
        let g = layout(menu, one(99), &[one(1), one(2)], None, true);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].slot, 3);
        assert!(g[0].is_result);
    }

    /// The two washes are different colours, both at alpha 48.
    #[test]
    fn the_two_washes_are_red_under_and_white_over() {
        assert_eq!(WASH_UNDER, 0x30FF_0000);
        assert_eq!(WASH_OVER, 0x30FF_FFFF);
        assert_ne!(WASH_UNDER, WASH_OVER, "not one colour drawn twice");
        assert_eq!(WASH_UNDER >> 24, 48);
        assert_eq!(WASH_OVER >> 24, 48);
        // Under is red, over is white — the pair, not a grey.
        assert_eq!(WASH_UNDER & 0x00FF_FFFF, 0x00FF_0000);
        assert_eq!(WASH_OVER & 0x00FF_FFFF, 0x00FF_FFFF);
    }

    /// Only the result gets the big wash, and only when the screen is not the
    /// player's own inventory.
    #[test]
    fn only_a_result_slot_on_a_non_inventory_screen_gets_the_big_wash() {
        assert_eq!(wash_rect(true, true), (-4, -4, 24));
        assert_eq!(wash_rect(true, false), (0, 0, 16), "the player's inventory");
        assert_eq!(wash_rect(false, true), (0, 0, 16), "an input, never big");
        assert_eq!(wash_rect(false, false), (0, 0, 16));
        // `isBiggerResultSlot` is TRUE by default.
        assert!(big_result_slot(false));
        assert!(!big_result_slot(true));
    }

    /// A ghost cycles through its ingredient's members, one level.
    #[test]
    fn a_ghost_cycles_through_its_own_members_only() {
        let g = Ghost { slot: 1, items: vec![10, 11, 12], is_result: false };
        assert_eq!(g.item(0), Some(10));
        assert_eq!(g.item(1), Some(11));
        assert_eq!(g.item(2), Some(12));
        assert_eq!(g.item(3), Some(10), "and wraps");
        // A single-member ingredient never changes.
        let one = Ghost { slot: 1, items: vec![7], is_result: false };
        for i in 0..5 {
            assert_eq!(one.item(i), Some(7));
        }
        // An empty one shows nothing rather than panicking.
        let none = Ghost { slot: 1, items: Vec::new(), is_result: false };
        assert_eq!(none.item(0), None);
    }

    /// The row skip advances the row a SECOND time — vanilla's `gridYPos++`
    /// inside the loop, on top of the for-loop's own.
    ///
    /// Like the strictness above, this is unobservable on Minecraft's grids: the
    /// extra advance only matters when the shape is at least 2 tall AND centred,
    /// which needs `gridHeight >= 5`. A 6-tall grid shows it — without the extra
    /// advance the skip fires twice and every cell shifts by a row.
    #[test]
    fn the_row_skip_advances_the_row_twice_and_a_six_tall_grid_shows_it() {
        // 1 wide, 2 tall, into 1x6: `2 < 3` so it centres, and
        // `startPos = floor(3 - 1) = 2`.
        //
        // The shape lands on rows **1 and 2**, not 2 and 3 — the skip advances
        // ONE row and then processes, rather than jumping to `startPos`. So a
        // shape whose computed start is 2 begins at row 1. This witness first
        // expected [2, 3] on that reasoning and was wrong; vanilla's loop does
        // the same thing, since `gridYPos++` fires once per row that satisfies
        // the guard and the column loop runs immediately after.
        assert_eq!(place_recipe(1, 6, 1, 2, 2), vec![1, 2]);
        // Without the second advance the guard fires again at row 1 and
        // everything shifts down.
    }

    /// `<` versus `<=` in the centring test — indistinguishable on every grid
    /// Minecraft has, and different on a 4x4.
    ///
    /// The fixture is deliberately not a Minecraft grid: `<` and `<=` differ
    /// only when the dimension is even AND the recipe is exactly half of it, so
    /// a 2x2 gives `startPos` 0 either way and a 3x3's half (1.5) is never an
    /// integer. Without this, a mutation to `<=` survives — which it did.
    #[test]
    fn the_centring_test_is_strict_and_a_four_by_four_shows_it() {
        // A 2-tall recipe in a 4-tall grid: `2 < 2.0` is FALSE, so no centring
        // and the shape sits at the top-left. Under `<=` it would centre with
        // `startPos = floor(2 - 1) = 1`, skipping the first row.
        assert_eq!(place_recipe(4, 4, 2, 2, 4), vec![0, 1, 4, 5]);
        // And a 1-tall recipe in the same grid IS centred: `1 < 2.0`,
        // `startPos = floor(2 - 0.5) = 1`.
        assert_eq!(place_recipe(4, 1, 1, 1, 1), vec![1], "1x1 into 4 wide");
        // The half-size case on a 2x2, where the two readings coincide because
        // the computed start is 0 — this is why no Minecraft grid can tell them
        // apart.
        assert_eq!(place_recipe(2, 2, 1, 1, 1), vec![0]);
    }

    /// A shape that fills the grid lands in order, one ingredient per cell.
    #[test]
    fn a_full_shape_fills_the_grid_in_order() {
        assert_eq!(place_recipe(3, 3, 3, 3, 9), (0..9).collect::<Vec<_>>());
        assert_eq!(place_recipe(2, 2, 2, 2, 4), vec![0, 1, 2, 3]);
    }

    /// A 1x1 recipe in a 3x3 grid is CENTRED — cell 4.
    #[test]
    fn a_one_by_one_recipe_centres_in_a_three_by_three_grid() {
        assert_eq!(place_recipe(3, 3, 1, 1, 1), vec![4]);
    }

    /// A 2-tall recipe in a 3-tall grid is NOT centred: the test is strict
    /// (`2 < 1.5` is false), so it stays at the top-left.
    #[test]
    fn a_two_tall_recipe_in_a_three_tall_grid_is_not_centred() {
        // 2x2 into 3x3: rows 0 and 1, columns 0 and 1 — cells 0,1,3,4.
        assert_eq!(place_recipe(3, 3, 2, 2, 4), vec![0, 1, 3, 4]);
        // Whereas a 1-wide, 1-tall one IS centred, which is the boundary.
        assert_eq!(place_recipe(3, 3, 1, 1, 1), vec![4]);
    }

    /// A 2x2 shape in a 2x2 grid is not centred either — the grid has no room.
    #[test]
    fn a_shape_the_size_of_its_grid_is_never_centred() {
        assert_eq!(place_recipe(2, 2, 2, 2, 4), vec![0, 1, 2, 3]);
        assert_eq!(place_recipe(3, 3, 3, 3, 9), (0..9).collect::<Vec<_>>());
    }

    /// A shape wider than it is tall skips the rest of each row.
    #[test]
    fn a_narrow_shape_skips_the_rest_of_each_row() {
        // 2 wide, 2 tall, into a 3x3: cells 0,1 then 3,4.
        assert_eq!(place_recipe(3, 3, 2, 2, 4), vec![0, 1, 3, 4]);
        // 1 wide, 3 tall into 3x3: `1 < 1.5` so it centres horizontally at
        // column 1 — cells 1, 4, 7.
        assert_eq!(place_recipe(3, 3, 1, 3, 3), vec![1, 4, 7]);
    }

    /// Fewer ingredients than the shape simply stops — the iterator runs out.
    #[test]
    fn fewer_ingredients_than_the_shape_stops_early() {
        assert_eq!(place_recipe(3, 3, 3, 3, 4), vec![0, 1, 2, 3]);
        assert_eq!(place_recipe(3, 3, 3, 3, 0), Vec::<usize>::new());
    }

    /// Every index is inside the grid, for every shape that fits it.
    #[test]
    fn no_placement_ever_leaves_the_grid() {
        for gw in 1..=3usize {
            for gh in 1..=3usize {
                for rw in 1..=gw {
                    for rh in 1..=gh {
                        for n in 0..=(rw * rh) {
                            for i in place_recipe(gw, gh, rw, rh, n) {
                                assert!(i < gw * gh, "{gw}x{gh} shape {rw}x{rh} n={n} -> {i}");
                            }
                        }
                    }
                }
            }
        }
    }
}
