//! `OverlayRecipeComponent` (M104) — the "which of these?" popup a right-click
//! opens over a recipe cell that holds more than one recipe.
//!
//! M98 made the book clickable and recorded the gap in
//! [`crate::recipe_book_screen::BookAction::Recipe`]'s own doc: a right-click on
//! a multi-recipe cell was reported and did nothing. This is what reads it.
//!
//! # The three clamps round three different ways, and only one of them works
//!
//! `init` nudges the panel back on screen in whole button widths:
//!
//! ```java
//! float rightPos = this.x + Math.min(total, maxRow) * 25;
//! float maxLeftPos = centerX + 50;
//! if (rightPos > maxLeftPos)
//!    this.x = (int)(this.x - buttonWidth * (int)((rightPos - maxLeftPos) / buttonWidth));
//!
//! float bottomPos = this.y + rows * 25;
//! float maxBottomPos = centerY + 50;
//! if (bottomPos > maxBottomPos)
//!    this.y = (int)(this.y - buttonWidth * Mth.ceil((bottomPos - maxBottomPos) / buttonWidth));
//!
//! float topPos = this.y;
//! float maxTopPos = centerY - 100;
//! if (topPos < maxTopPos)
//!    this.y = (int)(this.y - buttonWidth * Mth.ceil((topPos - maxTopPos) / buttonWidth));
//! ```
//!
//! Three clamps, one step size, **three different roundings** — and the
//! difference is not stylistic:
//!
//! * the **horizontal** one truncates with a C-style `(int)` cast, so a positive
//!   quotient floors: an overlay overhanging by 1..24 px is not moved at all,
//!   and one overhanging by 38 px moves 25 and still overhangs by 13;
//! * the **bottom** one takes `Mth.ceil` of a *positive* quotient, so it rounds
//!   up and always clears its bound — the only clamp that is guaranteed to;
//! * the **top** one takes `Mth.ceil` of a *negative* one. `Mth.ceil` is
//!   `(int)Math.ceil(v)`, a true ceiling, so `ceil(-0.6) == 0` and the clamp is
//!   a **complete no-op** for any deficit under one button width.
//!
//! The same function over-corrects at the bottom and under-corrects at the top,
//! decided by nothing but the sign of its argument. Reaching for a symmetric
//! "clamp into the box" here is the natural thing and diverges on the whole
//! right-hand column of the book.
//!
//! **The order matters too**: `topPos` is read *after* the bottom clamp may
//! already have moved `y`, so the two are sequential rather than a pair of
//! independent bounds.
//!
//! # It opens on a right-click and accepts only left-clicks
//!
//! `RecipeBookPage.mouseClicked` opens it on `event.button() == 1` over a cell
//! that is `!isOnlyOption()`; `OverlayRecipeComponent.mouseClicked` opens with
//! `if (event.button() != 0) return false`. So a *second* right-click closes it.
//! See [`click`] for the rest of that asymmetry — in particular that a click
//! which hits nothing is still consumed.
//!
//! # Ground truth (bundled 26.2 decompile)
//!
//! `net/minecraft/client/gui/screens/recipebook/OverlayRecipeComponent.java`,
//! `RecipeBookPage.java`, `net/minecraft/recipebook/PlaceRecipeHelper.java`.

/// `BUTTON_SIZE` — the pitch the overlay's buttons tile at, and separately the
/// step every clamp in [`origin`] moves by.
///
/// Two roles for one number, arriving by two routes: the pitch is the literal
/// `25` in `init`, the step is `buttonWidth`, which is
/// `RecipeButton.getWidth()`. They are equal, and nothing in vanilla makes them
/// equal — [`origin`] therefore takes the step as a parameter rather than
/// reading this.
pub const BUTTON_PITCH: i32 = 25;

/// The button *widget* is **24**, one less than its pitch
/// (`super(x, y, 24, 24, EMPTY)`), so the overlay's buttons have a 1-px gutter
/// where the book's own 25-px cells abut with none.
pub const BUTTON_SIZE: i32 = 24;

/// `MAX_ROW` / `MAX_ROW_LARGE` — 4 buttons per row, or 5 once there are more
/// than 16. See [`max_row`].
pub const MAX_ROW: usize = 4;
pub const MAX_ROW_LARGE: usize = 5;

/// The first button's offset inside the panel, and the panel's total margin.
///
/// **These do not agree, and the disagreement is vanilla's.** Buttons start at
/// `x + 4, y + 5` and the panel is `cols * 25 + 8` by `rows * 25 + 8`, so the
/// padding is 4 left / 5 right and 5 top / 4 bottom — the panel is not
/// symmetric about its own grid. (`extractRenderState` declares a local
/// `int border = 4;` and never reads it.)
pub const PAD_X: i32 = 4;
pub const PAD_Y: i32 = 5;
/// `width * 25 + 8` — the 8 that makes the padding asymmetric.
pub const PANEL_MARGIN: i32 = 8;

/// `ITEM_RENDER_SCALE`. An ingredient is drawn at `16 * 0.375 == 6` px.
pub const ITEM_SCALE: f32 = 0.375;

/// The ingredient grid's pitch inside a button, from
/// `createGridPos(x, y) = Pos(3 + x * 7, 3 + y * 7)`.
pub const INGREDIENT_PITCH: i32 = 7;
pub const INGREDIENT_ORIGIN: i32 = 3;

/// The panel's nine-slice, from `overlay_recipe.png.mcmeta`:
/// `{"type": "nine_slice", "width": 32, "height": 32, "border": 4}`.
pub const PANEL_SHEET: i32 = 32;
pub const PANEL_BORDER: i32 = 4;

/// `total <= 16 ? 4 : 5`.
///
/// The threshold is on the **total**, not on the row count, so 16 recipes make
/// four rows of four and 17 make four rows of five (the last holding one).
pub fn max_row(total: usize) -> usize {
    if total <= 16 { MAX_ROW } else { MAX_ROW_LARGE }
}

/// `(int)Math.ceil((float)total / maxRow)`.
pub fn rows(total: usize) -> usize {
    let m = max_row(total);
    total.div_ceil(m)
}

/// How many columns the panel is wide — `Math.min(total, maxRow)`.
pub fn cols(total: usize) -> usize {
    total.min(max_row(total))
}

/// The panel's size in GUI pixels: `cols * 25 + 8` by `rows * 25 + 8`.
pub fn panel_size(total: usize) -> (i32, i32) {
    (
        cols(total) as i32 * BUTTON_PITCH + PANEL_MARGIN,
        rows(total) as i32 * BUTTON_PITCH + PANEL_MARGIN,
    )
}

/// Where the panel's top-left lands, from the clicked cell's corner.
///
/// `button` is the cell's `(getX(), getY())`; `centre` is what
/// `RecipeBookPage.mouseClicked` passes, which is **not the book's centre**:
///
/// ```java
/// this.overlay.init(…, xo + imageWidth / 2, yo + 13 + imageHeight / 2, button.getWidth());
/// ```
///
/// The x half is the plain centre and the y half carries a bare **`+ 13`**.
/// Only the vertical bound is nudged, and nothing in the call names the 13 —
/// deriving `centre` as the panel's midpoint puts every bound 13 px too high.
///
/// `step` is `RecipeButton.getWidth()`, always 25. See the module docs for why
/// the three clamps below round three different ways.
pub fn origin(
    button: (i32, i32),
    total: usize,
    centre: (i32, i32),
    step: f32,
) -> (i32, i32) {
    let (mut x, mut y) = button;

    // Horizontal: a C-style truncation, so a positive quotient FLOORS.
    let right = x as f32 + (cols(total) as i32 * BUTTON_PITCH) as f32;
    let max_left = centre.0 as f32 + 50.0;
    if right > max_left {
        x = (x as f32 - step * ((right - max_left) / step) as i32 as f32) as i32;
    }

    // Bottom: `Mth.ceil` of a POSITIVE quotient — rounds up, always clears.
    let bottom = y as f32 + (rows(total) as i32 * BUTTON_PITCH) as f32;
    let max_bottom = centre.1 as f32 + 50.0;
    if bottom > max_bottom {
        y = (y as f32 - step * ((bottom - max_bottom) / step).ceil()) as i32;
    }

    // Top: `Mth.ceil` of a NEGATIVE quotient — rounds toward zero, so a
    // deficit under one step moves nothing. Reads the y the bottom clamp left.
    let top = y as f32;
    let max_top = centre.1 as f32 - 100.0;
    if top < max_top {
        y = (y as f32 - step * ((top - max_top) / step).ceil()) as i32;
    }

    (x, y)
}

/// Button `i`'s top-left, relative to the panel's own origin.
pub fn button_pos(i: usize, total: usize) -> (i32, i32) {
    let m = max_row(total);
    (
        PAD_X + BUTTON_PITCH * (i % m) as i32,
        PAD_Y + BUTTON_PITCH * (i / m) as i32,
    )
}

/// Button `i`'s top-left in the same space the panel's origin is in.
pub fn button_origin(panel: (i32, i32), i: usize, total: usize) -> (i32, i32) {
    let (dx, dy) = button_pos(i, total);
    (panel.0 + dx, panel.1 + dy)
}

/// The order the buttons are built in: **every craftable recipe first, then
/// every uncraftable one**, and the uncraftable half is empty while filtering.
///
/// ```java
/// List<RecipeDisplayEntry> craftable = collection.getSelectedRecipes(CRAFTABLE);
/// List<RecipeDisplayEntry> unCraftable = isFiltering ? List.of() : getSelectedRecipes(NOT_CRAFTABLE);
/// … boolean canCraft = i < craftables;
/// ```
///
/// So the overlay is **sorted**, where the cell it opened from is not: the book
/// keeps a collection's recipes in the order the server sent them, and this
/// promotes the ones you can make. `canCraft` is then read off the index rather
/// than looked up again, which is why the two lists cannot simply be
/// concatenated in the other order.
/// Vanilla builds the two halves with two filtered passes over the collection's
/// own `entries` list, so **within** each half the collection's order survives —
/// which is what a stable partition of one flagged list gives, and why this
/// takes one list rather than two.
///
/// Generic over the payload because the caller carries more per recipe than an
/// id (its shape and its resolved ingredients), and a second copy of this
/// ordering in the app would be a second thing to drift.
pub fn promote<T: Clone>(recipes: &[(T, bool)], filtering: bool) -> Vec<(T, bool)> {
    let mut out: Vec<(T, bool)> = recipes.iter().filter(|(_, c)| *c).cloned().collect();
    if !filtering {
        out.extend(recipes.iter().filter(|(_, c)| !*c).cloned());
    }
    out
}

/// One button of an open overlay, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    /// The `RecipeDisplayId` this button places.
    pub recipe: i32,
    pub craftable: bool,
    /// Its ingredient grid — a position and the items that position cycles
    /// through. Ingredients that resolved to nothing are already dropped, so
    /// this is what draws rather than what the recipe declares.
    pub slots: Vec<(Pos, Vec<i32>)>,
}

/// An open overlay. **A snapshot, not a view.**
///
/// Vanilla resolves everything in `init` — the entry lists, each button's
/// craftable flag, each ingredient's positions — and nothing refreshes it
/// afterwards. `updateCollections` rebuilds the page's cells and leaves the
/// overlay alone, and `updateStackedContents` reaches it only through that. So
/// crafting something while the overlay is up does **not** re-sort it or
/// re-grey a button; only closing and reopening does.
///
/// That is why this is stored rather than recomputed from the session each
/// frame: a recomputed overlay would resort itself under the cursor, which
/// looks tidier and is not what vanilla does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    /// The panel's top-left in book pixels, after [`origin`]'s three clamps.
    pub origin: (i32, i32),
    /// Which family of button art — the MENU's kind, not the recipes'.
    pub furnace: bool,
    /// In [`promote`]'s order: craftable first.
    pub buttons: Vec<Button>,
}

impl Open {
    pub fn total(&self) -> usize {
        self.buttons.len()
    }

    /// The craftable flags, in button order — what [`Open::buttons`] gives the
    /// chrome.
    pub fn craftable_flags(&self) -> Vec<bool> {
        self.buttons.iter().map(|b| b.craftable).collect()
    }

    /// Which button the cursor is over, in book pixels.
    pub fn hovered(&self, bx: i32, by: i32) -> Option<usize> {
        hit(bx - self.origin.0, by - self.origin.1, self.total())
    }

    /// Resolve a click in book pixels.
    pub fn click_at(&self, bx: i32, by: i32, right: bool) -> Click {
        click(bx - self.origin.0, by - self.origin.1, self.total(), right)
    }
}

/// One ingredient's place inside a button, and which of the recipe's
/// ingredients it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    /// Offset from the button's `(getX() + 2, getY() + 2)` grid origin.
    pub x: i32,
    pub y: i32,
    /// Index into the display's own ingredient list.
    pub ingredient: usize,
}

/// The display shapes the two button classes recognise.
///
/// Shape-neutral because `rewo-net` depends on `rewo-world` and not the other
/// way round, so `RecipeDisplay` cannot be named here. The caller maps its own
/// display onto this and resolves the ingredient indices afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Shaped { w: usize, h: usize, ingredients: usize },
    Shapeless { ingredients: usize },
    Furnace,
    /// A stonecutter or smithing display. Both `calculateIngredientsPositions`
    /// switches fall through to a `default:` that adds nothing.
    Other,
}

/// Where each ingredient sits inside a 24x24 button.
///
/// **`furnace_menu` is the MENU's kind, not the display's** — `RecipeBookPage`
/// picks the button class once, from `menu instanceof AbstractFurnaceMenu`, and
/// each class then ignores every display it does not match. So a furnace book
/// showing a shaped display draws an empty button, and so does a crafting book
/// showing a furnace one. Dispatching on the display instead would draw
/// ingredients in both cases and never be wrong in a way anyone could see on
/// vanilla, because the two never mix there.
///
/// The two crafting arms do not agree on how to place a shape:
///
/// * **shaped** goes through [`crate::ghost_slots::place_recipe`], so a 1x1
///   recipe centres in the 3x3;
/// * **shapeless** does not — it is a bare `i % 3, i / 3`, with the 3 written as
///   a literal rather than taken from the grid.
///
/// An ingredient whose `resolveForStacks` comes back empty is skipped by the
/// caller; neither arm derives a position from a running counter, so dropping
/// one does not shift the rest.
pub fn grid_positions(furnace_menu: bool, shape: Shape) -> Vec<Pos> {
    let grid = |x: usize, y: usize, ingredient: usize| Pos {
        x: INGREDIENT_ORIGIN + x as i32 * INGREDIENT_PITCH,
        y: INGREDIENT_ORIGIN + y as i32 * INGREDIENT_PITCH,
        ingredient,
    };
    if furnace_menu {
        // `OverlaySmeltingRecipeButton` — the one ingredient, dead centre.
        return match shape {
            Shape::Furnace => vec![grid(1, 1, 0)],
            _ => Vec::new(),
        };
    }
    match shape {
        Shape::Shaped { w, h, ingredients } => crate::ghost_slots::place_recipe(3, 3, w, h, ingredients)
            .into_iter()
            .enumerate()
            // `gridIndex` and `(gridXPos, gridYPos)` stay in lockstep through
            // both of `placeRecipe`'s skips, so the callback's coordinates are
            // recoverable from the index alone — see the test below, which is
            // what makes reusing M103's index-returning port sound.
            .map(|(n, cell)| grid(cell % 3, cell / 3, n))
            .collect(),
        Shape::Shapeless { ingredients } => (0..ingredients).map(|i| grid(i % 3, i / 3, i)).collect(),
        Shape::Furnace | Shape::Other => Vec::new(),
    }
}

/// An ingredient's top-left and size, in GUI pixels, given its button's corner.
///
/// ```java
/// translate(gridPosX + pos.x, gridPosY + pos.y);  scale(0.375F);  translate(-8, -8);
/// ```
///
/// with `gridPos = getX() + 2, getY() + 2`. The transforms compose on the
/// coordinate, so the trailing `-8` is scaled too: a point `p` in the item's own
/// `0..16` space lands at `grid + pos + 0.375 * (p - 8)`. **`pos` is therefore
/// the ingredient's CENTRE**, and its corner is 3 px up and left of it — reading
/// `pos` as a top-left offsets every ingredient by half its own size.
pub fn item_rect(button: (i32, i32), pos: Pos) -> (f32, f32, f32) {
    let size = 16.0 * ITEM_SCALE;
    (
        (button.0 + 2 + pos.x) as f32 - size / 2.0,
        (button.1 + 2 + pos.y) as f32 - size / 2.0,
        size,
    )
}

/// Which of an ingredient's items shows this frame.
///
/// `ingredients.get(currentIndex % ingredients.size())` — a **one**-level cycle
/// on the shared clock, where `RecipeButton.getDisplayStack` one screen over
/// uses a two-level one (`% entryCount` and `/ entryCount`). Same clock, so a
/// cell and the overlay it opens can be showing different items on the same
/// frame, which is correct and looks like a bug.
pub fn select_ingredient(count: usize, cycle: i32) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(cycle.rem_euclid(count as i32) as usize)
}

/// Which button a click at panel-relative `(px, py)` lands on.
///
/// The buttons are 24 wide on a 25 pitch, so the 25th column and row of each
/// cell fall between two buttons and hit nothing — unlike the book's own grid,
/// where a click can never fall between cells.
pub fn hit(px: i32, py: i32, total: usize) -> Option<usize> {
    (0..total).find(|&i| {
        let (bx, by) = button_pos(i, total);
        px >= bx && py >= by && px < bx + BUTTON_SIZE && py < by + BUTTON_SIZE
    })
}

/// What a click does while the overlay is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    /// Left click on button `i` — place that recipe, and the overlay closes
    /// with the page's own click handling.
    Select(usize),
    /// Anything else: the overlay shuts. **The click is still consumed** —
    /// `RecipeBookPage.mouseClicked` returns `true` on both paths, so it never
    /// reaches the page's arrows, its cells, the search box, the filter, the
    /// tabs, or the menu underneath.
    Close,
}

/// Resolve a click while the overlay is up.
///
/// ```java
/// if (this.overlay.isVisible()) {
///    if (this.overlay.mouseClicked(event, doubleClick)) { … } else { this.overlay.setVisible(false); }
///    return true;
/// }
/// ```
///
/// and `OverlayRecipeComponent.mouseClicked` opens `if (event.button() != 0)
/// return false`. **A right-click therefore closes it**, which is the same
/// button that opened it — so right-clicking a multi-recipe cell twice is open
/// then shut, not open then re-open. The left/right asymmetry is doubled up:
/// each `OverlayRecipeButton` is an `AbstractWidget` whose default
/// `isValidClickButton` is also `button == 0`.
pub fn click(px: i32, py: i32, total: usize, right: bool) -> Click {
    if right {
        return Click::Close;
    }
    match hit(px, py, total) {
        Some(i) => Click::Select(i),
        None => Click::Close,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cell the book would put at column `col`, row `row`, in book pixels.
    fn cell(col: i32, row: i32) -> (i32, i32) {
        crate::recipe_book_screen::grid_slot((row * 5 + col) as usize)
    }

    /// What `RecipeBookPage.mouseClicked` passes for a book at origin 0.
    fn centre() -> (i32, i32) {
        (
            crate::recipe_book_screen::IMAGE_W / 2,
            13 + crate::recipe_book_screen::IMAGE_H / 2,
        )
    }

    #[test]
    fn the_row_width_steps_up_after_sixteen_not_after_four_rows() {
        assert_eq!(max_row(16), 4, "sixteen is four rows of four");
        assert_eq!(max_row(17), 5);
        assert_eq!(rows(16), 4);
        // Seventeen is FOUR rows again, of five — the threshold is on the
        // total, so growing by one recipe widens the panel instead of
        // deepening it.
        assert_eq!(rows(17), 4);
        assert_eq!(cols(16), 4);
        assert_eq!(cols(17), 5);
    }

    #[test]
    fn the_panel_pads_four_one_side_and_five_the_other() {
        // Two buttons wide, one row: 2*25+8 by 1*25+8.
        assert_eq!(panel_size(2), (58, 33));
        let (w, h) = panel_size(2);
        let (bx, by) = button_pos(1, 2);
        // Left margin 4, right margin 5.
        assert_eq!(button_pos(0, 2).0, 4);
        assert_eq!(w - (bx + BUTTON_SIZE), 5, "right margin is 5, not 4");
        // Top margin 5, bottom margin 4 — the other way round.
        assert_eq!(by, 5);
        assert_eq!(h - (by + BUTTON_SIZE), 4, "bottom margin is 4, not 5");
    }

    #[test]
    fn a_left_column_overlay_is_not_moved_at_all() {
        // Column 0, two recipes: right edge at 11 + 50 = 61, bound at 73 + 50.
        let (x, y) = origin(cell(0, 0), 2, centre(), 25.0);
        assert_eq!((x, y), cell(0, 0), "well inside every bound");
    }

    /// The horizontal clamp's `(int)` truncation, and what it leaves behind.
    #[test]
    fn the_horizontal_clamp_floors_and_so_under_corrects() {
        let c = centre();
        let bound = c.0 + 50; // 123
        // Column 4, two recipes: right edge 111 + 50 = 161, overhanging by 38.
        let (x, _) = origin(cell(4, 0), 2, c, 25.0);
        assert_eq!(x, 111 - 25, "38 / 25 truncates to ONE step, not two");
        let right = x + cols(2) as i32 * BUTTON_PITCH;
        assert!(
            right > bound,
            "and so the overlay still overhangs its bound by {}",
            right - bound
        );
        assert_eq!(right - bound, 13);
    }

    /// The partner: a `ceil` there would clear the bound, so the witness above
    /// is measuring the rounding and not merely the presence of a clamp.
    #[test]
    fn a_ceiling_would_have_cleared_the_horizontal_bound() {
        let c = centre();
        let (x, _) = origin(cell(4, 0), 2, c, 25.0);
        let ceiled = 111 - 25 * ((161.0f32 - 123.0) / 25.0).ceil() as i32;
        assert_ne!(x, ceiled, "the two roundings must disagree here");
        assert!(ceiled + cols(2) as i32 * BUTTON_PITCH <= c.0 + 50);
    }

    /// An overhang under one step moves nothing whatsoever.
    #[test]
    fn a_sub_step_overhang_is_left_alone() {
        let c = centre();
        // Column 3 (x = 86), two recipes: right edge 136, bound 123, over by 13.
        let (x, _) = origin(cell(3, 0), 2, c, 25.0);
        assert_eq!(x, 86, "13 / 25 truncates to zero steps");
    }

    #[test]
    fn the_bottom_clamp_ceils_and_so_always_clears() {
        let c = centre();
        let bound = c.1 + 50; // 146
        // Bottom row (y = 106), two rows deep: bottom edge 156, over by 10.
        let (_, y) = origin(cell(0, 3), 5, c, 25.0);
        assert_eq!(y, 106 - 25, "a 10 px overhang still costs a whole step");
        assert!(y + rows(5) as i32 * BUTTON_PITCH <= bound);
    }

    /// `Mth.ceil` is a true ceiling, so the top clamp's negative quotient
    /// rounds toward zero and a deficit under one step does nothing.
    #[test]
    fn the_top_clamp_is_a_no_op_below_one_step() {
        let c = centre();
        let max_top = c.1 - 100; // -4
        // 26 recipes: maxRow 5, six rows. From the top row (y = 31) the bottom
        // clamp lifts it to 31 - 25*ceil((181-146)/25) = 31 - 50 = -19.
        let (_, y) = origin(cell(0, 0), 26, c, 25.0);
        assert_eq!(y, -19);
        assert!(y < max_top, "it is above its own top bound…");
        assert_eq!(
            y - max_top,
            -15,
            "…by 15 px, and ceil(-15/25) == 0 leaves it there"
        );
    }

    /// And the partner: with a deficit past one step it does move — so the
    /// witness above is measuring the rounding, not a clamp that never fires.
    #[test]
    fn the_top_clamp_does_fire_past_one_step() {
        let c = centre();
        // 31 recipes: maxRow 5, seven rows. Bottom clamp: 31 - 25*ceil((206-146)/25)
        // = 31 - 75 = -44, which is 40 below the -4 bound: ceil(-40/25) == -1.
        let (_, y) = origin(cell(0, 0), 31, c, 25.0);
        assert_eq!(y, -44 + 25, "one step back down, still 15 px short");
    }

    /// The `+ 13` in `centerY` is **inert**, and this proves it exhaustively.
    ///
    /// This witness was written the other way round — asserting that dropping
    /// the 13 moves the overlay — and failed, because the fixture could not
    /// express the claim: a two-deep overlay on the bottom row lands at 81
    /// under both bounds. It is not the fixture. **No fixture can**, and the
    /// reason is arithmetic:
    ///
    /// every cell's `y` is `31 + 25r`, so `y ≡ 6 (mod 25)`; the two candidate
    /// bounds are 146 and 133, so the overflows are `≡ 10` and `≡ 23 (mod 25)`
    /// — and since the second is exactly the first plus 13, the two always sit
    /// in the *same* `ceil` bucket. A bound that moves by less than the
    /// quantisation step cannot change a quantised answer. The top clamp is
    /// worse off still: whenever it fires at all it lands on `-19` under either
    /// bound.
    ///
    /// So `+ 13` joins `extractRenderState`'s unread `int border = 4;` as
    /// something vanilla computes and never spends — transcribed for fidelity,
    /// because its inertness is a property of `step == 25` and of the book's
    /// own 25-px grid, and a change to either would wake it up.
    #[test]
    fn the_thirteen_in_the_vertical_centre_can_never_change_the_answer() {
        let real = centre();
        let naive = (
            crate::recipe_book_screen::IMAGE_W / 2,
            crate::recipe_book_screen::IMAGE_H / 2,
        );
        assert_eq!(real.1 - naive.1, 13, "the offset is real and only on y");
        assert_eq!(real.0, naive.0);
        for i in 0..crate::recipe_book_screen::ITEMS_PER_PAGE {
            let c = crate::recipe_book_screen::grid_slot(i);
            for total in 1..=64usize {
                assert_eq!(
                    origin(c, total, real, 25.0),
                    origin(c, total, naive, 25.0),
                    "cell {i}, {total} recipes"
                );
            }
        }
    }

    /// And the partner, so the witness above is not merely reporting that
    /// `centre` is unused: a bound moved by a **whole** step does change it.
    #[test]
    fn a_bound_moved_by_a_whole_step_does_change_the_answer() {
        let real = centre();
        let shifted = (real.0, real.1 - BUTTON_PITCH);
        assert_ne!(
            origin(cell(0, 3), 5, real, 25.0),
            origin(cell(0, 3), 5, shifted, 25.0)
        );
    }

    #[test]
    fn craftable_recipes_are_promoted_ahead_of_the_rest() {
        // The collection's own order interleaves them; the overlay does not.
        let all = [(7, true), (3, false), (8, true)];
        assert_eq!(
            promote(&all, false),
            vec![(7, true), (8, true), (3, false)],
            "craftable first, and stable within each half"
        );
        // Filtering drops the uncraftable half entirely rather than greying it.
        assert_eq!(promote(&all, true), vec![(7, true), (8, true)]);
    }

    /// The identity that lets M103's index-returning `place_recipe` serve a
    /// caller that needs `placeRecipe`'s `(gridXPos, gridYPos)`.
    ///
    /// `gridIndex` advances with `gridXPos` inside a row, by a whole row on the
    /// centring skip (which also bumps `gridYPos`), and by `gridWidth -
    /// gridXPos` on the right-edge break (where the outer loop bumps
    /// `gridYPos`). So `gridIndex == gridYPos * gridWidth + gridXPos` at every
    /// callback, and `% 3` / `/ 3` recover the pair.
    #[test]
    fn a_shaped_recipe_centres_the_way_the_grid_index_says() {
        // 1x1 into 3x3 centres: `startPos = floor(1.5 - 0.5) = 1`.
        assert_eq!(
            grid_positions(false, Shape::Shaped { w: 1, h: 1, ingredients: 1 }),
            vec![Pos { x: 3 + 7, y: 3 + 7, ingredient: 0 }]
        );
        // 3x3 fills in reading order, no centring.
        let full = grid_positions(false, Shape::Shaped { w: 3, h: 3, ingredients: 9 });
        assert_eq!(full.len(), 9);
        assert_eq!(full[0], Pos { x: 3, y: 3, ingredient: 0 });
        assert_eq!(full[8], Pos { x: 3 + 14, y: 3 + 14, ingredient: 8 });
    }

    /// An **asymmetric** shape, because neither fixture above can see a
    /// transposed x/y: a filled 3x3 is symmetric, and a centred 1x1 lands on
    /// the diagonal. A 2-wide 1-tall recipe centres vertically and not
    /// horizontally, so its two cells are `(0, 1)` and `(1, 1)` — and reading
    /// the grid index the other way round moves the first to `(1, 0)`.
    #[test]
    fn a_shape_that_centres_on_one_axis_only_pins_which_way_round_it_is() {
        assert_eq!(
            crate::ghost_slots::place_recipe(3, 3, 2, 1, 2),
            vec![3, 4],
            "row 0 is skipped, so the pair is the middle row's first two cells"
        );
        assert_eq!(
            grid_positions(false, Shape::Shaped { w: 2, h: 1, ingredients: 2 }),
            vec![
                Pos { x: 3, y: 3 + 7, ingredient: 0 },
                Pos { x: 3 + 7, y: 3 + 7, ingredient: 1 },
            ]
        );
    }

    /// Shapeless does NOT centre — the two arms of one switch disagree.
    #[test]
    fn a_shapeless_recipe_fills_from_the_corner_where_a_shaped_one_centres() {
        let shapeless = grid_positions(false, Shape::Shapeless { ingredients: 1 });
        let shaped = grid_positions(false, Shape::Shaped { w: 1, h: 1, ingredients: 1 });
        assert_eq!(
            shapeless,
            vec![Pos { x: 3, y: 3, ingredient: 0 }],
            "top-left, not the middle"
        );
        assert_ne!(shapeless, shaped);
    }

    /// The class is chosen by the MENU, and each ignores what it does not match.
    #[test]
    fn the_button_class_follows_the_menu_and_not_the_display() {
        assert_eq!(
            grid_positions(true, Shape::Furnace),
            vec![Pos { x: 3 + 7, y: 3 + 7, ingredient: 0 }],
            "a furnace book centres its single ingredient"
        );
        assert!(
            grid_positions(true, Shape::Shaped { w: 3, h: 3, ingredients: 9 }).is_empty(),
            "a furnace book draws nothing for a shaped display"
        );
        assert!(
            grid_positions(false, Shape::Furnace).is_empty(),
            "and a crafting book draws nothing for a furnace one"
        );
        assert!(grid_positions(false, Shape::Other).is_empty());
    }

    #[test]
    fn an_ingredient_is_centred_on_its_position_not_cornered_at_it() {
        // Button at (0, 0), ingredient at grid (0, 0): pos (3, 3), grid origin
        // (2, 2), so the CENTRE is (5, 5) and the corner is (2, 2).
        let (x, y, s) = item_rect((0, 0), Pos { x: 3, y: 3, ingredient: 0 });
        assert_eq!(s, 6.0);
        assert_eq!((x, y), (2.0, 2.0));
        // Reading `pos` as a top-left would put it at (5, 5) — half a size off.
        assert_ne!((x, y), (5.0, 5.0));
    }

    #[test]
    fn the_ingredient_cycle_has_one_level_where_the_cells_has_two() {
        assert_eq!(select_ingredient(3, 0), Some(0));
        assert_eq!(select_ingredient(3, 4), Some(1));
        // Past the count it wraps rather than advancing an outer index — with
        // `getDisplayStack`'s two-level cycle, index 3 of a 3-list would move
        // the *offset* on and pick a different form.
        assert_eq!(select_ingredient(3, 3), Some(0));
        assert_eq!(select_ingredient(0, 7), None, "an empty ingredient shows nothing");
    }

    #[test]
    fn a_click_lands_on_a_button_but_not_in_the_gutter_between_two() {
        let (bx, by) = button_pos(1, 4);
        assert_eq!(hit(bx, by, 4), Some(1));
        assert_eq!(hit(bx + BUTTON_SIZE - 1, by + BUTTON_SIZE - 1, 4), Some(1));
        // The 25th column is past this button and before the next: the buttons
        // are 24 wide on a 25 pitch, so there IS a gap.
        assert_eq!(hit(bx + BUTTON_SIZE, by, 4), None);
        assert_eq!(hit(bx, by + BUTTON_SIZE, 4), None);
        // Inside the panel's own margin, left of every button.
        assert_eq!(hit(0, 0, 4), None);
    }

    #[test]
    fn a_right_click_closes_the_overlay_its_own_button_opened() {
        let (bx, by) = button_pos(2, 4);
        assert_eq!(click(bx, by, 4, false), Click::Select(2));
        assert_eq!(
            click(bx, by, 4, true),
            Click::Close,
            "the button that opens it does not select in it"
        );
        // And a left click that misses closes it too.
        assert_eq!(click(0, 0, 4, false), Click::Close);
    }
}
