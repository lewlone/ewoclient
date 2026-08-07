//! The recipe book's screen model — tabs, collections, filtering, pagination
//! and geometry (M93z).
//!
//! M93y decoded the four packets and recorded the book itself as the subsystem
//! that had to follow. This is its *model*: what a tab contains, what the
//! filter hides, how the page paginates, and where everything sits. The render
//! is separate and not here.
//!
//! # It is positioned against the WINDOW, not against a panel
//!
//! Every other screen Rewo draws is panel-relative — `container_panel` centres
//! a sheet and everything is an offset inside it. The recipe book is not:
//! `getXOrigin` is `(width - 147) / 2 - xOffset`, so it is centred on the
//! *window* and then pushed left by 86 to sit beside the menu, and `xOffset`
//! collapses to **0** on a narrow window, where the book covers the menu
//! instead of flanking it.

/// `IMAGE_WIDTH` / `IMAGE_HEIGHT`.
pub const IMAGE_W: i32 = 147;
pub const IMAGE_H: i32 = 166;
/// `OFFSET_X_POSITION` — how far left of centre the book sits beside a menu.
/// **Zero when the window is too narrow**, which is what makes the book cover
/// the menu on a small screen rather than hang off the edge.
pub const OFFSET_X: i32 = 86;

/// `ITEMS_PER_PAGE`, and the grid it fills.
pub const ITEMS_PER_PAGE: usize = 20;
pub const GRID_COLS: i32 = 5;
pub const GRID_ROWS: i32 = 4;
/// `setPosition(xo + 11 + 25 * (i % 5), yo + 31 + 25 * (i / 5))`.
pub const GRID_X: i32 = 11;
pub const GRID_Y: i32 = 31;
pub const GRID_PITCH: i32 = 25;

/// The page arrows — `ImageButton(xo + 93 | xo + 38, yo + 137, 12, 17)`.
pub const PAGE_FORWARD_X: i32 = 93;
pub const PAGE_BACK_X: i32 = 38;
pub const PAGE_ARROW_Y: i32 = 137;
pub const PAGE_ARROW_W: i32 = 12;
pub const PAGE_ARROW_H: i32 = 17;

/// The search field — `EditBox(font, xo + 25, yo + 13, 81, 9 + 5)`, max 50.
///
/// The height is written `9 + 5`, the font's line height plus padding, and the
/// max length is the same 50 the anvil's field uses (M93t).
pub const SEARCH_X: i32 = 25;
pub const SEARCH_Y: i32 = 13;
pub const SEARCH_W: i32 = 81;
pub const SEARCH_H: i32 = 14;
pub const SEARCH_MAX_LENGTH: usize = 50;

/// The field's own text geometry, derived from its rect (M100).
///
/// `EditBox` defaults to **bordered** and the book never calls
/// `setBordered(false)`, which decides all three:
///
/// * `textX = getX() + 4` — the text is inset four pixels, not flush.
/// * `textY = getY() + (height - 8) / 2` — vertically centred, so **3** for a
///   14-tall field. Not `getY()`, which is the unbordered case.
/// * `getInnerWidth() = width - 8` — so the visible text is clipped to **73**
///   px, not the field's 81. Eight, not four: the inset is taken off both ends.
pub const SEARCH_TEXT_X: i32 = SEARCH_X + 4;
pub const SEARCH_TEXT_Y: i32 = SEARCH_Y + (SEARCH_H - 8) / 2;
pub const SEARCH_INNER_W: i32 = SEARCH_W - 8;

/// `gui.recipebook.search_hint`, drawn when the field is empty **and
/// unfocused**.
pub const SEARCH_HINT: &str = "Search...";

/// `SEARCH_HINT_STYLE` is `GRAY, ITALIC` — `ChatFormatting.GRAY` is
/// `0xAAAAAA`. The italic is not reproduced: Rewo's bitmap font pass has no
/// slant, and the colour is the part that distinguishes a hint from real text.
pub const SEARCH_HINT_COLOR: [f32; 3] = [0.666_666_7, 0.666_666_7, 0.666_666_7];

/// The field's own background sprite — `SPRITES.get(isActive(), isFocused())`.
///
/// **The third meaning of `WidgetSprites::get` on this one screen**, and the
/// only one that is the record's plain reading: a tab passes `selected` as
/// *focused* (M94) and the filter passes `filtering` as *enabled* (M94), while
/// this passes exactly what the names say. Assuming one convention across the
/// screen is wrong twice out of three times.
pub fn search_sprite_focused(focused: bool) -> bool {
    focused
}

/// The filter toggle — `create(xo + 110, yo + 12, 26, 16, …)`.
pub const FILTER_X: i32 = 110;
pub const FILTER_Y: i32 = 12;
pub const FILTER_W: i32 = 26;
pub const FILTER_H: i32 = 16;

/// The tab column — `xPosTab = getXOrigin() - 30`, `yPosTab = getYOrigin() + 3`,
/// pitch 27.
pub const TAB_DX: i32 = -30;
pub const TAB_DY: i32 = 3;
pub const TAB_PITCH: i32 = 27;

/// `getXOrigin` — **window**-relative, and pushed left by `OFFSET_X` unless the
/// window is too narrow to flank the menu.
pub fn x_origin(width: i32, too_narrow: bool) -> i32 {
    (width - IMAGE_W) / 2 - if too_narrow { 0 } else { OFFSET_X }
}

/// `getYOrigin`.
pub fn y_origin(height: i32) -> i32 {
    (height - IMAGE_H) / 2
}

/// Where recipe button `i` of a page sits, relative to the book's origin.
pub fn grid_slot(i: usize) -> (i32, i32) {
    let i = i as i32;
    (
        GRID_X + GRID_PITCH * (i % GRID_COLS),
        GRID_Y + GRID_PITCH * (i / GRID_COLS),
    )
}

/// `totalPages = (int) Math.ceil(size / 20.0)`.
///
/// **Zero collections give zero pages**, not one — so an empty book has no
/// page to be on, and `current_page` must be clamped by
/// [`clamp_page`] rather than assumed valid.
pub fn total_pages(collections: usize) -> usize {
    collections.div_ceil(ITEMS_PER_PAGE)
}

/// `if (this.totalPages <= this.currentPage || resetPage) this.currentPage = 0;`
///
/// Note the comparison is `<=`, so a page index equal to the count resets —
/// and the reset is to **0**, not to the last page. Shrinking the list under a
/// reader therefore sends them to the front rather than to the new end.
pub fn clamp_page(current: usize, collections: usize, reset: bool) -> usize {
    if reset || total_pages(collections) <= current {
        0
    } else {
        current
    }
}

/// Which collection indices a page shows — `startOffset = 20 * currentPage`,
/// then 20 buttons of which the ones past the end are hidden.
pub fn page_range(page: usize, collections: usize) -> std::ops::Range<usize> {
    let start = (ITEMS_PER_PAGE * page).min(collections);
    start..(start + ITEMS_PER_PAGE).min(collections)
}

/// Where a tab's button sits, relative to the book's origin.
///
/// `index` is its position among the **visible** tabs — `updateTabs` makes
/// every search tab visible unconditionally and asks `updateVisibility` for the
/// rest, then lays out only the ones that survive.
pub fn tab_position(index: i32) -> (i32, i32) {
    (TAB_DX, TAB_DY + TAB_PITCH * index)
}

/// `Inventory.items` as menu slots — armour, storage, hotbar and offhand.
///
/// **5..46**, so neither the player menu's 2x2 crafting grid (1..5) nor its
/// craft result (slot 0). The grid arrives through
/// `fillCraftSlotsStackedContents` instead, and the result through nothing at
/// all — counting it would let a recipe read as craftable from its own output.
pub const PLAYER_ITEM_SLOTS: std::ops::Range<usize> = 5..46;

/// Which of an open menu's slots feed `fillCraftSlotsStackedContents` (M102).
///
/// `RecipeBookMenu` declares it abstract and the two families answer
/// differently — and **not just with different ranges**:
///
/// * `AbstractCraftingMenu` contributes `craftSlots`, the **input grid only**.
///   Its result slot is excluded, which matters: a crafted item sitting in the
///   output would otherwise count as an ingredient for itself.
/// * `AbstractFurnaceMenu` contributes the whole `container` — **including the
///   result slot** — because a furnace's container is three slots and
///   `fillStackedContents` walks all of them.
///
/// And the two use **different accounting**: the crafting container calls
/// `accountSimpleStack`, which gates on `isUsableForCrafting`, while the furnace
/// block entity calls bare `accountStack`, which does not. So a damaged pickaxe
/// in a furnace's fuel slot counts and the same pickaxe on a crafting grid does
/// not. Four lines apart in the decompile, and inverting either half is
/// invisible without a fixture that has a damaged stack in the right place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CraftSlots {
    /// Menu slot indices, inclusive-exclusive.
    pub range: std::ops::Range<usize>,
    /// Whether `isUsableForCrafting` gates them — true for the crafting family,
    /// false for the furnace family.
    pub gated: bool,
}

/// The craft slots of the menu a book is open over, or `None` for a menu with
/// no book at all.
///
/// The player's own inventory is `InventoryMenu`, whose grid is **2x2 at menu
/// slots 1..5** — the result is slot 0. A crafting table is 3x3 at 1..10.
pub fn craft_slots(book: BookType, player_inventory: bool) -> Option<CraftSlots> {
    if player_inventory {
        return Some(CraftSlots { range: 1..5, gated: true });
    }
    Some(match book {
        // `CraftingMenu`: result 0, grid 1..=9.
        BookType::Crafting => CraftSlots { range: 1..10, gated: true },
        // `AbstractFurnaceMenu`: the container is ingredient 0, fuel 1,
        // result 2 — and all three are contributed.
        BookType::Furnace | BookType::BlastFurnace | BookType::Smoker => {
            CraftSlots { range: 0..3, gated: false }
        }
    })
}

/// One tab of one book — its icon(s) and the categories it shows (M95).
///
/// **This replaces M93z's `Tab`, which was wrong.** That enum was the four
/// `SearchRecipeBookCategory` values, and those are *the search tab of each of
/// the four books*, not the tabs within one book. `includedCategories()` is
/// what the **search tab** contains. Each book has its own hand-written tab
/// list: crafting **five**, furnace four, blast furnace three, smoker two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookTab {
    /// The item(s) drawn on the tab. One icon or two — `TabInfo` has a
    /// three-argument constructor for the pairs, and the pair is drawn at
    /// +3/+14 against a single icon's +9.
    pub primary: &'static str,
    pub secondary: Option<&'static str>,
    /// The categories this tab shows. A **search** tab lists the book's whole
    /// set; a category tab exactly one.
    pub categories: &'static [&'static str],
    /// Whether this is the search tab — the one `updateTabs` makes visible
    /// unconditionally, where a category tab's visibility depends on having a
    /// collection with `hasAnySelected`.
    ///
    /// **Explicit, not derived from the category count.** Vanilla's
    /// discriminator is the *type* — `SearchRecipeBookCategory` against
    /// `RecipeBookCategory` — and a count heuristic gets the **smoker** wrong,
    /// because its search tab includes exactly one category (`smoker_food`),
    /// the same one its single category tab does. A rule that works for three
    /// books out of four is worse than no rule.
    pub search: bool,
}

impl BookTab {
    pub fn is_search(&self) -> bool {
        self.search
    }
}

/// `CraftingRecipeBookComponent.TABS` — **five**, the first of them search.
///
/// The search tab's icon is a **compass**: `TabInfo(SearchRecipeBookCategory)`
/// is `new ItemStack(Items.COMPASS)`, and all four books' search tabs share it.
pub const CRAFTING_TABS: &[BookTab] = &[
    BookTab {
        primary: "compass",
        secondary: None,
        search: true,
        categories: &[
            "minecraft:crafting_equipment",
            "minecraft:crafting_building_blocks",
            "minecraft:crafting_misc",
            "minecraft:crafting_redstone",
        ],
    },
    BookTab {
        primary: "iron_axe",
        secondary: Some("golden_sword"),
        search: false,
        categories: &["minecraft:crafting_equipment"],
    },
    BookTab {
        primary: "bricks",
        secondary: None,
        search: false,
        categories: &["minecraft:crafting_building_blocks"],
    },
    BookTab {
        primary: "lava_bucket",
        secondary: Some("apple"),
        search: false,
        categories: &["minecraft:crafting_misc"],
    },
    BookTab {
        primary: "redstone",
        secondary: None,
        search: false,
        categories: &["minecraft:crafting_redstone"],
    },
];

/// `FurnaceScreen.TABS` — four.
pub const FURNACE_TABS: &[BookTab] = &[
    BookTab {
        primary: "compass",
        secondary: None,
        search: true,
        categories: &[
            "minecraft:furnace_food",
            "minecraft:furnace_blocks",
            "minecraft:furnace_misc",
        ],
    },
    BookTab {
        primary: "porkchop",
        secondary: None,
        search: false,
        categories: &["minecraft:furnace_food"],
    },
    BookTab {
        primary: "stone",
        secondary: None,
        search: false,
        categories: &["minecraft:furnace_blocks"],
    },
    BookTab {
        primary: "lava_bucket",
        secondary: Some("emerald"),
        search: false,
        categories: &["minecraft:furnace_misc"],
    },
];

/// `BlastFurnaceScreen.TABS` — three.
pub const BLAST_FURNACE_TABS: &[BookTab] = &[
    BookTab {
        primary: "compass",
        secondary: None,
        search: true,
        categories: &[
            "minecraft:blast_furnace_blocks",
            "minecraft:blast_furnace_misc",
        ],
    },
    BookTab {
        primary: "redstone_ore",
        secondary: None,
        search: false,
        categories: &["minecraft:blast_furnace_blocks"],
    },
    BookTab {
        primary: "iron_shovel",
        secondary: Some("golden_leggings"),
        search: false,
        categories: &["minecraft:blast_furnace_misc"],
    },
];

/// `SmokerScreen.TABS` — two.
pub const SMOKER_TABS: &[BookTab] = &[
    BookTab {
        primary: "compass",
        secondary: None,
        search: true,
        categories: &["minecraft:smoker_food"],
    },
    BookTab {
        primary: "porkchop",
        secondary: None,
        search: false,
        categories: &["minecraft:smoker_food"],
    },
];

/// The four books, in `RecipeBookSettings`' positional order (M93y).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BookType {
    #[default]
    Crafting,
    Furnace,
    BlastFurnace,
    Smoker,
}

impl BookType {
    pub const ALL: [BookType; 4] = [
        BookType::Crafting,
        BookType::Furnace,
        BookType::BlastFurnace,
        BookType::Smoker,
    ];

    /// `RecipeBookSettings`' index — and `RecipeBookType`'s ordinal.
    pub fn index(self) -> usize {
        match self {
            BookType::Crafting => 0,
            BookType::Furnace => 1,
            BookType::BlastFurnace => 2,
            BookType::Smoker => 3,
        }
    }

    pub fn tabs(self) -> &'static [BookTab] {
        match self {
            BookType::Crafting => CRAFTING_TABS,
            BookType::Furnace => FURNACE_TABS,
            BookType::BlastFurnace => BLAST_FURNACE_TABS,
            BookType::Smoker => SMOKER_TABS,
        }
    }

    /// Whether this book's filter toggle uses the furnace art.
    ///
    /// `CraftingRecipeBookComponent` and `FurnaceRecipeBookComponent` are the
    /// only two subclasses, and all three furnaces share the second — so this
    /// is "not crafting", not "is a furnace by name".
    pub fn furnace_family(self) -> bool {
        self != BookType::Crafting
    }
}

/// The three registry categories **no tab contains**.
///
/// `minecraft:recipe_book_category` has 13 entries and the four tabs cover
/// 10. A stonecutter, smithing or campfire recipe is in `allCollections` and
/// reachable from no tab at all — those screens have their own UI rather than
/// a book page, so a "which tab is this in" lookup must be allowed to answer
/// nothing.
pub const CATEGORIES_WITHOUT_A_TAB: [&str; 3] = [
    "minecraft:stonecutter",
    "minecraft:smithing",
    "minecraft:campfire",
];

/// Which of a book's tabs a category belongs to, or `None`.
///
/// The **search tab is skipped**: it contains every category the book has, so
/// including it would make this answer 0 for everything. The question this
/// asks is "which category tab", which is what a collection needs.
pub fn category_tab_of(book: BookType, category: &str) -> Option<usize> {
    book.tabs()
        .iter()
        .enumerate()
        .find(|(_, t)| !t.is_search() && t.categories.contains(&category))
        .map(|(i, _)| i)
}

/// One `RecipeCollection` — the recipes that share a cell in the grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    /// The recipe ids, in the order they were added.
    pub recipes: Vec<i32>,
    pub category: String,
}

/// `ClientRecipeBook.categorizeAndGroupRecipes` (M93z).
///
/// ```java
/// if (groupId.isEmpty()) result.computeIfAbsent(category, …).add(List.of(entry));
/// else { … multiItemGroups.get(category, groupId) … }
/// ```
///
/// Two rules, and the second is the interesting one:
///
/// * a recipe with **no group is its own collection** — a singleton;
/// * recipes sharing a `(category, group)` pair are **one** collection, whose
///   position in the category is that of its **first-seen** member. Later
///   members append to the list already placed, so they do not move it.
///
/// **The input order is a `HashMap`'s**, so vanilla's own collection order is
/// not stable between runs. That makes ordering here *not* a wire contract —
/// unlike M93s's stonecutter list, where the index a click sends made the
/// order load-bearing. This preserves insertion order because a stable book is
/// better than an arbitrary one, not because vanilla guarantees it.
pub fn collections(entries: &[(i32, Option<i32>, &str)]) -> Vec<Collection> {
    let mut out: Vec<Collection> = Vec::new();
    // (category, group) -> index into `out`.
    let mut groups: std::collections::HashMap<(String, i32), usize> = Default::default();
    for &(id, group, category) in entries {
        match group {
            None => out.push(Collection {
                recipes: vec![id],
                category: category.to_string(),
            }),
            Some(g) => {
                let key = (category.to_string(), g);
                match groups.get(&key) {
                    Some(&i) => out[i].recipes.push(id),
                    None => {
                        groups.insert(key, out.len());
                        out.push(Collection {
                            recipes: vec![id],
                            category: category.to_string(),
                        });
                    }
                }
            }
        }
    }
    out
}

/// What one collection contributes to the page, for [`visible_collections`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollectionState {
    /// `hasAnySelected()` — at least one recipe fits the open menu.
    pub any_selected: bool,
    /// `hasCraftable()` — at least one can be made from what you hold.
    pub has_craftable: bool,
    /// Whether the search matched it. `true` when the query is empty, since
    /// vanilla skips the search stage entirely then.
    pub matches_search: bool,
}

/// `RecipeBookComponent.updateCollections`' three stages, in order.
///
/// ```java
/// collection.removeIf(c -> !c.hasAnySelected());
/// if (!searchTarget.isEmpty()) collection.removeIf(e -> !set.contains(e));
/// if (isFiltering) collection.removeIf(c -> !c.hasCraftable());
/// ```
///
/// **The first stage is unconditional**, which is the part a reader drops: the
/// filter button toggles only the *third*, so "show all recipes" still hides
/// every collection with nothing selected. A book that showed those would list
/// furnace recipes in a crafting table.
///
/// The order of the other two does not change the result — they are both
/// `removeIf` — but it is kept because the search stage is skipped entirely on
/// an empty query rather than matching everything, and that distinction shows
/// up in [`CollectionState::matches_search`]'s documented default.
pub fn visible_collections(states: &[CollectionState], filtering: bool) -> Vec<usize> {
    states
        .iter()
        .enumerate()
        .filter(|(_, s)| s.any_selected && s.matches_search && (!filtering || s.has_craftable))
        .map(|(i, _)| i)
        .collect()
}

/// The search query as vanilla forms it — `searchBox.getValue().toLowerCase(Locale.ROOT)`.
///
/// `Locale.ROOT` rather than the default locale, which is what stops a Turkish
/// user's dotless ı from breaking a search for "iron".
pub fn search_query(raw: &str) -> String {
    raw.to_lowercase()
}

/// `AbstractRecipeBookScreen.init` — `this.widthTooNarrow = this.width < 379`.
///
/// A hard threshold in GUI pixels, not a computed fit. Below it the book stops
/// flanking the menu and covers it instead ([`x_origin`]), and the screen stops
/// forwarding clicks to the menu underneath.
pub const WIDTH_TOO_NARROW_BELOW: i32 = 379;

pub fn width_too_narrow(width: i32) -> bool {
    width < WIDTH_TOO_NARROW_BELOW
}

/// `RecipeBookComponent.updateScreenPosition` — **opening the book MOVES the
/// menu**.
///
/// ```java
/// if (isVisible() && !widthTooNarrow) leftPos = 177 + (width - imageWidth - 200) / 2;
/// else                               leftPos = (width - imageWidth) / 2;
/// ```
///
/// This is the reason the book's render is not a pure addition. Every
/// panel-relative thing — slot hit-testing, the icons drawn into slots, the
/// hover box — is measured from `leftPos`, so drawing the book's pixels without
/// this shift leaves the menu centred while the book overlaps it, and every
/// click in the menu lands on the wrong slot.
///
/// **`topPos` does not move.** Only the horizontal position changes.
///
/// Note the shifted form is *not* "centre the pair": it is a literal 177 plus
/// the centring of a 200-wide notional block. For a 176-wide menu on an 800-wide
/// screen that is 177 + 212 = 389 against a centred 312 — 77 px right.
pub fn screen_left(width: i32, image_w: i32, book_visible: bool, too_narrow: bool) -> i32 {
    if book_visible && !too_narrow {
        177 + (width - image_w - 200) / 2
    } else {
        (width - image_w) / 2
    }
}

/// `RecipeBookTabButton` — 35x27.
pub const TAB_W: i32 = 35;
pub const TAB_H: i32 = 27;

/// A selected tab is drawn **2 px further left**, so it sticks out of the
/// column. Its icon moves with it (`moveLeft = selected ? -2 : 0`) — the sprite
/// and the icon shift together, and shifting only one of them is a plausible
/// half-transcription that reads as a misplaced icon.
pub fn tab_x_shift(selected: bool) -> i32 {
    if selected { -2 } else { 0 }
}

/// A tab's sprite tracks **selection, not hover**.
///
/// ```java
/// Identifier sprite = this.sprites.get(true, this.selected);
/// ```
///
/// `WidgetSprites.get(enabled, focused)` — `enabled` is a hard-coded `true`, so
/// the disabled slots are unreachable, and the *focused* argument receives
/// `selected`. The two-argument `WidgetSprites(tab, tab_selected)` constructor
/// fills `(enabled, disabled, focused, disabledFocused)` as
/// `(tab, tab, tab_selected, tab_selected)`, so the result is `tab_selected`
/// exactly when selected. **A tab does not respond to the cursor at all** —
/// `handleCursor` is even overridden to suppress the pointer while selected.
pub fn tab_selected_sprite(selected: bool) -> bool {
    selected
}

/// Where a tab's icons sit, relative to the tab's own unshifted origin.
///
/// One icon sits at **+9**, two at **+3** and **+14** — the single icon is
/// *not* the average of the pair (which would be 8.5), so a centred derivation
/// is half a pixel off and rounds wrong in one direction.
pub fn tab_icon_offsets(has_secondary: bool, selected: bool) -> Vec<(i32, i32)> {
    let dx = tab_x_shift(selected);
    if has_secondary {
        vec![(3 + dx, 5), (14 + dx, 5)]
    } else {
        vec![(9 + dx, 5)]
    }
}

/// The recipe button's four background sprites, as a 2x2 matrix.
///
/// `hasCraftable` x `hasMultipleRecipes`, and **`hasMultipleRecipes` counts the
/// SELECTED entries** (`selectedEntries.size() > 1`), not every recipe in the
/// collection — so filtering can change a slot's chrome without changing what
/// it makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotSprite {
    Craftable,
    ManyCraftable,
    Uncraftable,
    ManyUncraftable,
}

pub fn slot_sprite(craftable: bool, multiple: bool) -> SlotSprite {
    match (craftable, multiple) {
        (true, true) => SlotSprite::ManyCraftable,
        (true, false) => SlotSprite::Craftable,
        (false, true) => SlotSprite::ManyUncraftable,
        (false, false) => SlotSprite::Uncraftable,
    }
}

/// Where the item(s) go inside a 25x25 recipe button — `(back, front)`.
///
/// The "a stack of recipes" look is **two items**, not one sprite: when the
/// collection has several recipes *and they all share a result display*,
/// vanilla draws a copy at `offset + 1`, then **decrements `offset`** and draws
/// the real one. So the pair is (5, 3) — the shadow down-right of centre and
/// the front one up-left of it. With a single recipe there is no back copy and
/// the item sits at 4.
///
/// Reading `offset--` as applying to the *back* copy gives (4, 4): two items
/// exactly on top of each other, which renders as one and hides the effect.
pub fn recipe_item_offsets(multiple_same_result: bool) -> (Option<i32>, i32) {
    if multiple_same_result {
        (Some(5), 3)
    } else {
        (None, 4)
    }
}

/// `1.0F + 0.1F * sin(animationTime / 15.0F * PI)` — the highlight pulse.
///
/// One formula, **two different applications**: a recipe button scales
/// `(squeeze, squeeze)` and a tab scales `(1.0, squeeze)`. Both pivot on
/// `(x + 8, y + 12)`. A tab that pulsed uniformly would widen into its
/// neighbour.
pub const ANIMATION_TIME: f32 = 15.0;

pub fn squeeze(animation_time: f32) -> f32 {
    1.0 + 0.1 * (animation_time / ANIMATION_TIME * std::f32::consts::PI).sin()
}

/// `updateArrowButtons` — `(forward, back)`.
///
/// Both are gated on `totalPages > 1`, which is redundant for `forward`
/// (`currentPage < totalPages - 1` already implies it for a clamped page) and
/// **not** for `back` on an empty book, where `totalPages` is 0.
pub fn page_arrows_visible(page: usize, total: usize) -> (bool, bool) {
    (
        total > 1 && page + 1 < total,
        total > 1 && page > 0,
    )
}

/// The `x/y` page counter, drawn **only when there is more than one page**.
///
/// `xo - width / 2 + 73`, `yo + 141`. Centred on 73 rather than the panel's
/// true centre of 73.5, and the text width is halved with integer division
/// first — so the label sits half a pixel left of centre by construction.
pub const PAGE_LABEL_CENTRE_X: i32 = 73;
pub const PAGE_LABEL_Y: i32 = 141;

pub fn page_label_x(text_width: i32) -> i32 {
    PAGE_LABEL_CENTRE_X - text_width / 2
}

/// The counter's translation key. Its value in `en_us.json` is **`%s/%s`** —
/// no spaces, so a three-page book's first page reads `1/3` and not `1 / 3`.
pub const PAGE_LABEL_KEY: &str = "gui.recipebook.page";

/// The counter's text, or `None` on the frames it is not drawn at all.
///
/// `if (this.totalPages > 1)` — a book that fits on one page shows **no**
/// counter, rather than a permanent `1/1`. That gate is here rather than at the
/// caller because it is the same fact as the two arrows' visibility
/// ([`page_arrows_visible`], also `totalPages > 1`): a page with nothing to
/// page to says nothing about paging.
///
/// `page` is the **0-based** current page, as it is everywhere else in this
/// module; vanilla passes `currentPage + 1` into the format and leaves
/// `totalPages` alone, so only one of the two arguments is converted. Reading
/// the `+ 1` as a 1-based convention and applying it to both gives `1/4` on a
/// three-page book — a wrong number that still counts up correctly and so
/// survives casual use.
///
/// `template` is the resolved format string, taken from the language map by the
/// caller. Passing the **key itself** when the map has no entry is not a
/// fallback invented here: `Language.getOrDefault` returns the key, and
/// `decomposeTemplate` on a string with no specifiers yields it unchanged, so
/// vanilla renders the bare key too.
pub fn page_label(page: usize, total: usize, template: &str) -> Option<String> {
    (total > 1).then(|| {
        rewo_data::lang::format(template, &[&(page + 1).to_string(), &total.to_string()])
    })
}

/// The filter toggle's sprite, as an index into a four-entry
/// `(enabled, disabled, enabled_highlighted, disabled_highlighted)` group.
///
/// `withSprite((button, filtering) -> textures.get(filtering, button.isHoveredOrFocused()))`
/// — so `WidgetSprites`' **`enabled` slot carries "is filtering"**, not "is the
/// button usable". The names line up (`filter_enabled` / `filter_disabled`),
/// which is what makes the mis-reading easy: taking `enabled` as the widget's
/// own state gives a button that never changes when you toggle it.
///
/// The crafting and furnace families have **different art** — `filter_*` versus
/// `furnace_filter_*` — so the group base differs per menu.
pub fn filter_sprite_offset(filtering: bool, hovered: bool) -> usize {
    match (filtering, hovered) {
        (true, false) => 0,
        (false, false) => 1,
        (true, true) => 2,
        (false, true) => 3,
    }
}

/// One quad of the book's chrome, in book-relative GUI pixels.
///
/// The sprite is named **semantically** rather than as an atlas index: the
/// index is `rewo_data::assets`' business and its order is append-only, so
/// keeping the geometry free of it means the atlas can grow without touching
/// this file — and, more usefully, a test here can assert *which sprite* a
/// state picks without knowing where it was packed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BookQuad {
    pub sprite: BookSprite,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Which pixels of the sprite this quad takes, or `None` for the whole of
    /// it stretched to `(w, h)` — which is every quad the book draws except the
    /// which-of-these overlay's panel (M104).
    ///
    /// That panel is the list's one nine-slice, so it arrives as several quads
    /// sharing a sprite and differing only here. Carried as an `Option` rather
    /// than always spelling out `(0, 0, w, h)` so the existing quads keep
    /// saying what they mean: they have no source rect *because* they have no
    /// choice about it.
    pub src: Option<(i32, i32, i32, i32)>,
}

impl BookQuad {
    /// A quad taking the whole of its sprite — the ordinary case.
    pub const fn whole(sprite: BookSprite, x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { sprite, x, y, w, h, src: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookSprite {
    /// The 147x166 panel itself, sampled from `(1, 1)` of `recipe_book.png`.
    Panel,
    Tab { selected: bool },
    Slot(SlotSprite),
    PageForward { hovered: bool },
    PageBackward { hovered: bool },
    Filter { furnace: bool, filtering: bool, hovered: bool },
    /// The which-of-these overlay's nine-sliced backing (M104).
    OverlayPanel,
    /// One of its buttons. `furnace` follows the MENU, not the recipe — see
    /// [`crate::recipe_overlay::grid_positions`].
    OverlayButton { furnace: bool, craftable: bool, hovered: bool },
}

/// What a page of the book needs to know to draw itself.
#[derive(Debug, Clone, Copy)]
pub struct BookView {
    /// How many tabs are visible. `updateTabs` makes every
    /// `SearchRecipeBookCategory` visible unconditionally, so for the four
    /// search tabs this is always 4 — but it is a parameter because the
    /// *ghost* categories a modded book could add are not.
    pub tabs: usize,
    pub selected_tab: usize,
    pub page: usize,
    pub total_pages: usize,
    /// How many recipe buttons this page shows — at most [`ITEMS_PER_PAGE`].
    pub shown: usize,
    pub filtering: bool,
    /// The crafting family and the furnace family have different filter art.
    pub furnace_family: bool,
}

/// Every quad the book's chrome draws, in vanilla's own order: the panel, then
/// the tabs, then the page's recipe slots, then the arrows.
///
/// **The filter toggle and the search box are widgets on the screen, not part
/// of this list's ordering contract** — vanilla adds them between the tabs and
/// the page, but they are separate `AbstractWidget`s whose relative order among
/// themselves is fixed by `initVisuals` rather than by a draw loop. The filter
/// is emitted last here so a caller that cannot resolve hover for it can drop
/// the final quad without disturbing anything else.
pub fn book_chrome(view: BookView, slots: &[(bool, bool)], hover: BookHover) -> Vec<BookQuad> {
    let mut out = Vec::with_capacity(2 + view.tabs + view.shown + 2);
    out.push(BookQuad::whole(BookSprite::Panel, 0, 0, IMAGE_W, IMAGE_H));

    for i in 0..view.tabs {
        let selected = i == view.selected_tab;
        let (tx, ty) = tab_position(i as i32);
        out.push(BookQuad::whole(
            BookSprite::Tab { selected },
            tx + tab_x_shift(selected),
            ty,
            TAB_W,
            TAB_H,
        ));
    }

    for (i, &(craftable, multiple)) in slots.iter().take(view.shown).enumerate() {
        let (sx, sy) = grid_slot(i);
        out.push(BookQuad::whole(
            BookSprite::Slot(slot_sprite(craftable, multiple)),
            sx,
            sy,
            SLOT_SIZE,
            SLOT_SIZE,
        ));
    }

    let (fwd, back) = page_arrows_visible(view.page, view.total_pages);
    if fwd {
        out.push(BookQuad::whole(
            BookSprite::PageForward { hovered: hover.page_forward },
            PAGE_FORWARD_X,
            PAGE_ARROW_Y,
            PAGE_ARROW_W,
            PAGE_ARROW_H,
        ));
    }
    if back {
        out.push(BookQuad::whole(
            BookSprite::PageBackward { hovered: hover.page_backward },
            PAGE_BACK_X,
            PAGE_ARROW_Y,
            PAGE_ARROW_W,
            PAGE_ARROW_H,
        ));
    }

    out.push(BookQuad::whole(
        BookSprite::Filter {
            furnace: view.furnace_family,
            filtering: view.filtering,
            hovered: hover.filter,
        },
        FILTER_X,
        FILTER_Y,
        FILTER_W,
        FILTER_H,
    ));
    out
}

/// The which-of-these overlay's chrome, in book-relative GUI pixels (M104).
///
/// A separate list from [`book_chrome`] rather than a tail of it, because
/// `RecipeBookPage.extractRenderState` calls `graphics.nextStratum()` before
/// the overlay — it is a layer above every cell, arrow and tab, and merging the
/// two would leave that ordering to whoever happened to append last.
///
/// `craftable` is one flag per button **in the overlay's own promoted order**
/// ([`crate::recipe_overlay::entries`]), not the collection's.
pub fn overlay_chrome(
    origin: (i32, i32),
    craftable: &[bool],
    furnace: bool,
    hovered: Option<usize>,
) -> Vec<BookQuad> {
    use crate::recipe_overlay as ro;
    let total = craftable.len();
    let (pw, ph) = ro::panel_size(total);
    let mut out: Vec<BookQuad> = crate::nine_slice::quads(
        (origin.0, origin.1, pw, ph),
        (ro::PANEL_SHEET, ro::PANEL_SHEET),
        crate::nine_slice::Border::all(ro::PANEL_BORDER),
    )
    .into_iter()
    .map(|q| BookQuad {
        sprite: BookSprite::OverlayPanel,
        x: q.dx,
        y: q.dy,
        w: q.w,
        h: q.h,
        src: Some((q.sx, q.sy, q.sw, q.sh)),
    })
    .collect();
    for (i, &c) in craftable.iter().enumerate() {
        let (bx, by) = ro::button_origin(origin, i, total);
        out.push(BookQuad::whole(
            BookSprite::OverlayButton { furnace, craftable: c, hovered: hovered == Some(i) },
            bx,
            by,
            ro::BUTTON_SIZE,
            ro::BUTTON_SIZE,
        ));
    }
    out
}

/// Which of the book's three hoverable widgets the cursor is over.
///
/// The tabs are absent on purpose: a tab's sprite reads `selected`, never
/// hover ([`tab_selected_sprite`]), so a hover flag for one would have nowhere
/// to go.
#[derive(Debug, Clone, Copy, Default)]
pub struct BookHover {
    pub page_forward: bool,
    pub page_backward: bool,
    pub filter: bool,
}

/// Where every ITEM the book draws goes, in book-relative GUI pixels (M95).
///
/// Separate from [`book_chrome`] because these are items, not sprites: they go
/// through the GUI item pass, which draws a model rather than an atlas rect.
/// The caller resolves *which* item — a tab's is a registry name from
/// [`BookTab`], a slot's comes off the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookIcon {
    pub x: i32,
    pub y: i32,
    pub kind: BookIconKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookIconKind {
    /// Tab `index`'s first icon.
    TabPrimary(usize),
    /// Its second, on the four tabs that have one.
    TabSecondary(usize),
    /// Recipe slot `index`. `back` is the SHADOW copy a multi-recipe
    /// collection draws behind the real one.
    Slot { index: usize, back: bool },
}

/// Every item position the book draws, in vanilla's own order: the tabs' icons
/// (each after its own tab sprite), then the page's slots.
///
/// `multi` says, per visible slot, whether the collection has several recipes
/// **and they all share a result display** — which is the pair of conditions
/// that draws the shadow copy. Either alone draws one item.
pub fn book_icons(view: BookView, tabs: &[BookTab], multi: &[bool]) -> Vec<BookIcon> {
    let mut out = Vec::new();
    for (i, tab) in tabs.iter().enumerate().take(view.tabs) {
        let (tx, ty) = tab_position(i as i32);
        for (n, (dx, dy)) in tab_icon_offsets(tab.secondary.is_some(), i == view.selected_tab)
            .into_iter()
            .enumerate()
        {
            out.push(BookIcon {
                x: tx + dx,
                y: ty + dy,
                kind: if n == 0 {
                    BookIconKind::TabPrimary(i)
                } else {
                    BookIconKind::TabSecondary(i)
                },
            });
        }
    }
    for index in 0..view.shown {
        let (sx, sy) = grid_slot(index);
        let (back, front) = recipe_item_offsets(multi.get(index).copied().unwrap_or(false));
        if let Some(b) = back {
            out.push(BookIcon {
                x: sx + b,
                y: sy + b,
                kind: BookIconKind::Slot { index, back: true },
            });
        }
        out.push(BookIcon {
            x: sx + front,
            y: sy + front,
            kind: BookIconKind::Slot { index, back: false },
        });
    }
    out
}

/// What the cursor is over, in the book (M98).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookHit {
    PageForward,
    PageBackward,
    /// A visible recipe cell, by its index on the page.
    Slot(usize),
    /// The search field, or the magnifier left of it — vanilla treats a click
    /// on the icon as a click on the box.
    Search,
    Filter,
    Tab(usize),
}

/// The magnifier icon's rect, in book coordinates.
///
/// `ScreenRectangle.of(HORIZONTAL, xo + 8, searchBox.getY(), searchBox.getX() -
/// getXOrigin(), searchBox.getHeight())` — so x 8, and a width of **25**, which
/// is the search box's own x. It therefore **overlaps the box** rather than
/// abutting it, and the overlap is harmless only because both resolve to the
/// same hit.
pub const MAGNIFIER_X: i32 = 8;
pub const MAGNIFIER_W: i32 = SEARCH_X;

fn inside(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32) -> bool {
    // `AbstractWidget.clicked` — inclusive of the top-left, exclusive of the
    // bottom-right.
    px >= x && py >= y && px < x + w && py < y + h
}

/// What a click at book-relative `(bx, by)` lands on, in **vanilla's
/// resolution order** (M98).
///
/// `RecipeBookComponent.mouseClicked` tests, in order: the **page** (its arrows,
/// then its recipe buttons), then the search box and its magnifier, then the
/// filter toggle, then the tabs. That order is a contract, not the draw order —
/// and it is why a recipe cell wins over anything that overlapped it.
///
/// **A selected tab's hit rect does not move with its sprite.** The 2 px
/// leftward shift is applied at draw time only (`xPos = getX(); if (selected)
/// xPos -= 2`), while `clicked` tests `getX()` — so the leftmost two columns of
/// a selected tab are painted and not clickable. Shifting the rect too would be
/// the natural "fix" and would diverge.
pub fn book_hit(bx: i32, by: i32, view: BookView, tab_count: usize) -> Option<BookHit> {
    let (fwd, back) = page_arrows_visible(view.page, view.total_pages);
    if fwd && inside(bx, by, PAGE_FORWARD_X, PAGE_ARROW_Y, PAGE_ARROW_W, PAGE_ARROW_H) {
        return Some(BookHit::PageForward);
    }
    if back && inside(bx, by, PAGE_BACK_X, PAGE_ARROW_Y, PAGE_ARROW_W, PAGE_ARROW_H) {
        return Some(BookHit::PageBackward);
    }
    for index in 0..view.shown {
        let (sx, sy) = grid_slot(index);
        if inside(bx, by, sx, sy, SLOT_SIZE, SLOT_SIZE) {
            return Some(BookHit::Slot(index));
        }
    }
    if inside(bx, by, MAGNIFIER_X, SEARCH_Y, MAGNIFIER_W, SEARCH_H)
        || inside(bx, by, SEARCH_X, SEARCH_Y, SEARCH_W, SEARCH_H)
    {
        return Some(BookHit::Search);
    }
    if inside(bx, by, FILTER_X, FILTER_Y, FILTER_W, FILTER_H) {
        return Some(BookHit::Filter);
    }
    for i in 0..tab_count.min(view.tabs) {
        let (tx, ty) = tab_position(i as i32);
        if inside(bx, by, tx, ty, TAB_W, TAB_H) {
            return Some(BookHit::Tab(i));
        }
    }
    None
}

/// The book's own screen state — what only a click can change (M98).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BookState {
    pub selected_tab: usize,
    pub page: usize,
}

/// What a press does to the search field's focus (M99).
///
/// `None` leaves it alone; `Some(v)` sets it. A **pure function of the hit**,
/// which is the point: the focus itself lives in the `EditBox` and nowhere else.
///
/// An earlier cut kept a `search_focused` flag on [`BookState`] *as well*, and
/// mirrored it into the widget — two places holding one fact, which is how a
/// field comes to look focused and swallow nothing (`can_consume_input` gates
/// every keystroke on the widget's own flag). A test caught that, and a mutation
/// deleting the mirror survived because nothing could drive it. With one owner
/// there is no mirror to delete.
///
/// The rule: the **page**'s own widgets return early in
/// `RecipeBookComponent.mouseClicked`, so they never reach the
/// `setFocused(false)` in the else-branch — everything below it unfocuses,
/// unconditionally, before the filter and the tabs are even tested.
pub fn focus_change(hit: Option<BookHit>) -> Option<bool> {
    match hit? {
        BookHit::PageForward | BookHit::PageBackward | BookHit::Slot(_) => None,
        BookHit::Search => Some(true),
        BookHit::Filter | BookHit::Tab(_) => Some(false),
    }
}

/// What a press changed, for the caller to act on (M98).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookAction {
    /// The filter toggled — the caller flips it and tells the server.
    ToggleFilter,
    /// A recipe cell was clicked with the given button. `right` opens the
    /// which-of-these overlay in vanilla; Rewo has no overlay, so a right-click
    /// on a multi-recipe cell is reported and does nothing.
    Recipe { index: usize, right: bool },
    /// The tab or page moved; nothing leaves the client.
    Navigated,
}

impl BookState {
    /// Apply a press. Returns `None` when the click missed the book entirely,
    /// which is what tells the caller to let it fall through to the menu.
    ///
    /// **Switching tabs resets the page** — `onTabButtonPress` calls
    /// `updateCollections(true, …)`, whose `resetPage` is the `clamp_page`
    /// argument M93z modelled. Re-selecting the tab already selected does
    /// nothing at all: the guard is `selectedTab != button`.
    /// The page is not clamped here: `book_hit` only reports an arrow that is
    /// **visible**, and `page_arrows_visible` is what gates that — so a
    /// forward press cannot arrive on the last page.
    pub fn press(&mut self, hit: Option<BookHit>, right: bool) -> Option<BookAction> {
        let hit = hit?;
        Some(match hit {
            BookHit::PageForward => {
                self.page += 1;
                BookAction::Navigated
            }
            BookHit::PageBackward => {
                self.page = self.page.saturating_sub(1);
                BookAction::Navigated
            }
            BookHit::Slot(index) => BookAction::Recipe { index, right },
            // The focus itself is [`focus_change`]'s business, not this
            // method's — see its docs for why that split exists.
            BookHit::Search => BookAction::Navigated,
            BookHit::Filter => BookAction::ToggleFilter,
            BookHit::Tab(i) => {
                if i != self.selected_tab {
                    self.selected_tab = i;
                    self.page = 0;
                }
                BookAction::Navigated
            }
        })
    }
}

/// A recipe button is 25x25/// A recipe button is 25x25 — the same as the grid pitch, so the buttons abut
/// with no gap and a click can never fall between two.
pub const SLOT_SIZE: i32 = 25;

/// Where the panel is sampled from `recipe_book.png`: **`(1, 1)`**.
pub const PANEL_SOURCE: (i32, i32) = (1, 1);

#[cfg(test)]
mod tests {
    use super::*;

    fn view(shown: usize, total: usize, page: usize) -> BookView {
        BookView {
            tabs: 4,
            selected_tab: 0,
            page,
            total_pages: total,
            shown,
            filtering: false,
            furnace_family: false,
        }
    }

    fn full_view(shown: usize, total: usize, page: usize) -> BookView {
        BookView {
            tabs: CRAFTING_TABS.len(),
            selected_tab: 0,
            page,
            total_pages: total,
            shown,
            filtering: false,
            furnace_family: false,
        }
    }

    #[test]
    fn a_click_outside_everything_misses_the_book() {
        let v = full_view(20, 1, 0);
        // Bare panel between the search row and the grid.
        assert_eq!(book_hit(70, 29, v, CRAFTING_TABS.len()), None);
        // Well outside the panel on the right, past the tabs' side.
        assert_eq!(book_hit(200, 80, v, CRAFTING_TABS.len()), None);
    }

    #[test]
    fn a_recipe_cell_is_hit_over_its_own_25_by_25() {
        let v = full_view(20, 1, 0);
        let (sx, sy) = grid_slot(7);
        assert_eq!(book_hit(sx, sy, v, 5), Some(BookHit::Slot(7)), "top-left is in");
        assert_eq!(
            book_hit(sx + SLOT_SIZE - 1, sy + SLOT_SIZE - 1, v, 5),
            Some(BookHit::Slot(7)),
            "bottom-right corner is in"
        );
        // Exclusive on the far edges — the next cell's territory.
        assert_eq!(book_hit(sx + SLOT_SIZE, sy, v, 5), Some(BookHit::Slot(8)));
    }

    /// A cell past the end of the page is not clickable, even though its
    /// geometry exists.
    #[test]
    fn an_invisible_cell_is_not_clickable() {
        let (sx, sy) = grid_slot(19);
        assert_eq!(
            book_hit(sx + 12, sy + 12, full_view(20, 1, 0), 5),
            Some(BookHit::Slot(19))
        );
        assert_eq!(book_hit(sx + 12, sy + 12, full_view(5, 3, 2), 5), None);
    }

    /// An arrow is clickable only where it is drawn — the same
    /// `page_arrows_visible` gate.
    #[test]
    fn an_arrow_is_clickable_only_when_it_is_drawn() {
        let mid = full_view(20, 3, 1);
        let first = full_view(20, 3, 0);
        let one = full_view(3, 1, 0);
        let f = (PAGE_FORWARD_X + 6, PAGE_ARROW_Y + 8);
        let b = (PAGE_BACK_X + 6, PAGE_ARROW_Y + 8);
        assert_eq!(book_hit(f.0, f.1, mid, 5), Some(BookHit::PageForward));
        assert_eq!(book_hit(b.0, b.1, mid, 5), Some(BookHit::PageBackward));
        assert_eq!(book_hit(b.0, b.1, first, 5), None, "no page to go back to");
        assert_eq!(book_hit(f.0, f.1, one, 5), None, "one page, no arrows");
    }

    /// The magnifier counts as the search box, and it OVERLAPS it — 8..33
    /// against the box's 25..106.
    #[test]
    fn the_magnifier_counts_as_the_search_box() {
        let v = full_view(20, 1, 0);
        assert_eq!(book_hit(MAGNIFIER_X, SEARCH_Y + 7, v, 5), Some(BookHit::Search));
        assert_eq!(book_hit(SEARCH_X + 40, SEARCH_Y + 7, v, 5), Some(BookHit::Search));
        assert!(MAGNIFIER_X + MAGNIFIER_W > SEARCH_X, "they overlap by construction");
        // Just left of the magnifier is nothing.
        assert_eq!(book_hit(MAGNIFIER_X - 1, SEARCH_Y + 7, v, 5), None);
    }

    /// A selected tab's HIT RECT does not move with its sprite: the 2 px shift
    /// is applied at draw time only, so a selected tab's leftmost two columns
    /// are painted and not clickable.
    #[test]
    fn a_selected_tabs_hit_rect_does_not_follow_its_sprite() {
        let mut v = full_view(20, 1, 0);
        v.selected_tab = 0;
        let (tx, ty) = tab_position(0);
        assert_eq!(book_hit(tx, ty + 13, v, 5), Some(BookHit::Tab(0)));
        // The two columns the shift paints into are NOT hits.
        assert_eq!(book_hit(tx - 1, ty + 13, v, 5), None);
        assert_eq!(book_hit(tx + tab_x_shift(true), ty + 13, v, 5), None);
    }

    #[test]
    fn each_tab_is_hit_at_its_own_pitch() {
        let v = full_view(20, 1, 0);
        for i in 0..CRAFTING_TABS.len() {
            let (tx, ty) = tab_position(i as i32);
            assert_eq!(book_hit(tx + 17, ty + 13, v, CRAFTING_TABS.len()), Some(BookHit::Tab(i)));
        }
        // A sixth tab does not exist on a crafting book.
        let (tx, ty) = tab_position(5);
        assert_eq!(book_hit(tx + 17, ty + 13, v, CRAFTING_TABS.len()), None);
    }

    #[test]
    fn the_filter_toggle_is_hit_over_its_own_rect() {
        let v = full_view(20, 1, 0);
        assert_eq!(book_hit(FILTER_X, FILTER_Y, v, 5), Some(BookHit::Filter));
        assert_eq!(
            book_hit(FILTER_X + FILTER_W - 1, FILTER_Y + FILTER_H - 1, v, 5),
            Some(BookHit::Filter)
        );
        assert_eq!(book_hit(FILTER_X + FILTER_W, FILTER_Y, v, 5), None);
    }

    /// Switching tabs RESETS the page — `onTabButtonPress` passes
    /// `resetPage = true`. Re-selecting the tab you are on does nothing, since
    /// the guard is `selectedTab != button`.
    #[test]
    fn switching_tabs_resets_the_page_and_reselecting_does_nothing() {
        let mut st = BookState { selected_tab: 0, page: 2 };
        assert_eq!(st.press(Some(BookHit::Tab(0)), false), Some(BookAction::Navigated));
        assert_eq!(st.page, 2, "re-selecting the same tab leaves the page alone");
        assert_eq!(st.press(Some(BookHit::Tab(3)), false), Some(BookAction::Navigated));
        assert_eq!((st.selected_tab, st.page), (3, 0));
    }

    #[test]
    fn the_arrows_move_the_page_by_one() {
        let mut st = BookState { selected_tab: 0, page: 1 };
        st.press(Some(BookHit::PageForward), false);
        assert_eq!(st.page, 2);
        st.press(Some(BookHit::PageBackward), false);
        assert_eq!(st.page, 1);
    }

    /// A click anywhere in the book but the PAGE unfocuses the search box —
    /// `mouseClicked`'s else-branch calls `setFocused(false)` unconditionally
    /// before it goes on to the filter and the tabs, and the page path returns
    /// early without reaching it.
    #[test]
    fn only_a_click_on_the_page_leaves_the_search_focused() {
        for hit in [BookHit::PageForward, BookHit::PageBackward, BookHit::Slot(0)] {
            assert_eq!(focus_change(Some(hit)), None, "{hit:?} leaves focus alone");
        }
        for hit in [BookHit::Filter, BookHit::Tab(1)] {
            assert_eq!(focus_change(Some(hit)), Some(false), "{hit:?} unfocuses");
        }
        assert_eq!(focus_change(Some(BookHit::Search)), Some(true));
        // A miss leaves it alone too — the whole method returns before the
        // else-branch when nothing in the book is hit.
        assert_eq!(focus_change(None), None);
    }

    #[test]
    fn a_miss_returns_no_action_so_the_click_falls_through() {
        let mut st = BookState::default();
        assert_eq!(st.press(None, false), None);
        assert_eq!(st, BookState::default(), "and changes nothing");
    }

    #[test]
    fn a_recipe_click_reports_its_index_and_button() {
        let mut st = BookState::default();
        assert_eq!(
            st.press(Some(BookHit::Slot(4)), false),
            Some(BookAction::Recipe { index: 4, right: false })
        );
        assert_eq!(
            st.press(Some(BookHit::Slot(4)), true),
            Some(BookAction::Recipe { index: 4, right: true })
        );
    }

    #[test]
    fn the_filter_reports_a_toggle_rather_than_flipping_itself() {
        // The flag lives in the server-synced settings, not in `BookState` —
        // so pressing it asks the caller to flip and tell the server, which is
        // what `toggleFiltering` + `sendUpdateSettings` do in that order.
        let mut st = BookState::default();
        assert_eq!(st.press(Some(BookHit::Filter), false), Some(BookAction::ToggleFilter));
    }

    #[test]
    fn every_tab_draws_its_icons_over_its_own_sprite() {
        let mut v = view(0, 1, 0);
        v.tabs = 5;
        let icons = book_icons(v, CRAFTING_TABS, &[]);
        // Five tabs; two of the five carry a pair, so seven icons.
        assert_eq!(icons.len(), 7);
        assert_eq!(
            icons.iter().filter(|i| matches!(i.kind, BookIconKind::TabSecondary(_))).count(),
            2,
            "equipment and misc"
        );
        // Tab 0 is selected, so its icon rides the 2 px shift with its sprite.
        let (tx, ty) = tab_position(0);
        assert_eq!(icons[0].x, tx + 9 - 2);
        assert_eq!(icons[0].y, ty + 5);
        // Tab 1 is not, and carries a PAIR at +3/+14.
        let (t1x, t1y) = tab_position(1);
        assert_eq!(icons[1].x, t1x + 3);
        assert_eq!(icons[2].x, t1x + 14);
        assert_eq!((icons[1].y, icons[2].y), (t1y + 5, t1y + 5));
    }

    /// The shadow copy exists only when a collection has SEVERAL recipes AND
    /// they share a result display. Either alone draws one item.
    #[test]
    fn only_a_multi_recipe_slot_with_one_result_draws_two_items() {
        let mut v = view(3, 1, 0);
        v.tabs = 0;
        let icons = book_icons(v, &[], &[false, true, false]);
        let slots: Vec<_> = icons
            .iter()
            .filter_map(|i| match i.kind {
                BookIconKind::Slot { index, back } => Some((index, back, i.x, i.y)),
                _ => None,
            })
            .collect();
        // Four: one each for slots 0 and 2, two for slot 1.
        assert_eq!(slots.len(), 4);
        assert_eq!(slots.iter().filter(|s| s.1).count(), 1, "one shadow");
        let (sx, sy) = grid_slot(1);
        assert!(slots.contains(&(1, true, sx + 5, sy + 5)), "the shadow at +5");
        assert!(slots.contains(&(1, false, sx + 3, sy + 3)), "the front at +3");
        // A single-recipe slot sits at +4, between the two.
        let (zx, zy) = grid_slot(0);
        assert!(slots.contains(&(0, false, zx + 4, zy + 4)));
    }

    /// A page shorter than 20 draws icons only for the slots it has — the same
    /// `shown` the chrome respects, so the two cannot disagree about how many
    /// cells there are.
    #[test]
    fn the_icons_and_the_chrome_agree_about_how_many_slots_there_are() {
        let v = view(5, 3, 2);
        let cells = book_chrome(v, &vec![(true, false); ITEMS_PER_PAGE], BookHover::default())
            .into_iter()
            .filter(|q| matches!(q.sprite, BookSprite::Slot(_)))
            .count();
        let items = book_icons(v, &[], &vec![false; ITEMS_PER_PAGE])
            .into_iter()
            .filter(|i| matches!(i.kind, BookIconKind::Slot { .. }))
            .count();
        assert_eq!(cells, 5);
        assert_eq!(items, 5);
    }

    /// An icon sits INSIDE its 25x25 cell, whichever offset it takes.
    #[test]
    fn every_slot_icon_lands_inside_its_own_cell() {
        let mut v = view(ITEMS_PER_PAGE, 1, 0);
        v.tabs = 0;
        for multi in [false, true] {
            for i in book_icons(v, &[], &vec![multi; ITEMS_PER_PAGE]) {
                let BookIconKind::Slot { index, .. } = i.kind else { continue };
                let (sx, sy) = grid_slot(index);
                assert!(i.x >= sx && i.x + 16 <= sx + SLOT_SIZE, "slot {index} x");
                assert!(i.y >= sy && i.y + 16 <= sy + SLOT_SIZE, "slot {index} y");
            }
        }
    }

    #[test]
    fn the_panel_is_sampled_from_one_one_not_the_sheets_corner() {
        assert_eq!(PANEL_SOURCE, (1, 1));
        let q = book_chrome(view(0, 1, 0), &[], BookHover::default());
        assert_eq!(q[0].sprite, BookSprite::Panel);
        assert_eq!((q[0].w, q[0].h), (IMAGE_W, IMAGE_H));
        // The panel is first — everything else draws over it.
        assert!(q[1..].iter().all(|b| b.sprite != BookSprite::Panel));
    }

    #[test]
    fn only_the_selected_tab_wears_the_selected_sprite_and_sticks_out() {
        let mut v = view(0, 1, 0);
        v.selected_tab = 2;
        let q = book_chrome(v, &[], BookHover::default());
        let tabs: Vec<_> = q
            .iter()
            .filter(|b| matches!(b.sprite, BookSprite::Tab { .. }))
            .collect();
        assert_eq!(tabs.len(), 4);
        for (i, t) in tabs.iter().enumerate() {
            let sel = i == 2;
            assert_eq!(t.sprite, BookSprite::Tab { selected: sel });
            assert_eq!(t.x, TAB_DX + tab_x_shift(sel), "tab {i}");
            assert_eq!(t.y, TAB_DY + TAB_PITCH * i as i32);
        }
        // The selected one is the ONLY one further left.
        assert_eq!(tabs.iter().filter(|t| t.x == TAB_DX - 2).count(), 1);
    }

    #[test]
    fn the_slots_tile_with_no_gap_so_a_click_cannot_fall_between_them() {
        assert_eq!(SLOT_SIZE, GRID_PITCH);
        let slots = vec![(true, false); 20];
        let q = book_chrome(view(20, 1, 0), &slots, BookHover::default());
        let cells: Vec<_> = q
            .iter()
            .filter(|b| matches!(b.sprite, BookSprite::Slot(_)))
            .collect();
        assert_eq!(cells.len(), 20);
        // Row 0 is contiguous: each cell's right edge is the next one's left.
        for i in 0..4 {
            assert_eq!(cells[i].x + cells[i].w, cells[i + 1].x);
        }
        assert_eq!(cells[4].y + cells[4].h, cells[5].y, "and so are the rows");
    }

    #[test]
    fn a_short_page_draws_only_the_slots_it_has() {
        let slots = vec![(true, false); 20];
        let q = book_chrome(view(5, 3, 2), &slots, BookHover::default());
        assert_eq!(
            q.iter().filter(|b| matches!(b.sprite, BookSprite::Slot(_))).count(),
            5,
            "the last page of 45 collections"
        );
    }

    #[test]
    fn each_slots_chrome_follows_its_own_state() {
        let slots = [(true, false), (true, true), (false, false), (false, true)];
        let q = book_chrome(view(4, 1, 0), &slots, BookHover::default());
        let got: Vec<_> = q
            .iter()
            .filter_map(|b| match b.sprite {
                BookSprite::Slot(s) => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(
            got,
            vec![
                SlotSprite::Craftable,
                SlotSprite::ManyCraftable,
                SlotSprite::Uncraftable,
                SlotSprite::ManyUncraftable
            ]
        );
    }

    #[test]
    fn the_arrows_appear_only_where_there_is_a_page_to_go_to() {
        let has = |q: &[BookQuad], f: fn(&BookSprite) -> bool| q.iter().any(|b| f(&b.sprite));
        let fwd = |s: &BookSprite| matches!(s, BookSprite::PageForward { .. });
        let back = |s: &BookSprite| matches!(s, BookSprite::PageBackward { .. });

        let one = book_chrome(view(3, 1, 0), &[], BookHover::default());
        assert!(!has(&one, fwd) && !has(&one, back), "a single page has neither");

        let first = book_chrome(view(20, 3, 0), &[], BookHover::default());
        assert!(has(&first, fwd) && !has(&first, back));

        let last = book_chrome(view(5, 3, 2), &[], BookHover::default());
        assert!(!has(&last, fwd) && has(&last, back));
    }

    #[test]
    fn hover_reaches_the_arrows_and_the_filter_and_nothing_else() {
        let hover = BookHover { page_forward: true, page_backward: false, filter: true };
        let q = book_chrome(view(20, 3, 1), &[], hover);
        assert!(q.contains(&BookQuad::whole(
            BookSprite::PageForward { hovered: true },
            PAGE_FORWARD_X,
            PAGE_ARROW_Y,
            PAGE_ARROW_W,
            PAGE_ARROW_H,
        )));
        assert!(q
            .iter()
            .any(|b| b.sprite == BookSprite::PageBackward { hovered: false }));
        assert!(q
            .iter()
            .any(|b| matches!(b.sprite, BookSprite::Filter { hovered: true, .. })));
    }

    #[test]
    fn the_filter_carries_both_the_family_and_the_filter_state() {
        let mut v = view(0, 1, 0);
        v.filtering = true;
        v.furnace_family = true;
        let q = book_chrome(v, &[], BookHover::default());
        let f = q.iter().find(|b| matches!(b.sprite, BookSprite::Filter { .. })).unwrap();
        assert_eq!(
            f.sprite,
            BookSprite::Filter { furnace: true, filtering: true, hovered: false }
        );
        assert_eq!((f.x, f.y, f.w, f.h), (FILTER_X, FILTER_Y, FILTER_W, FILTER_H));
        // A crafting book with the same filter state picks DIFFERENT art.
        let mut c = view(0, 1, 0);
        c.filtering = true;
        let cq = book_chrome(c, &[], BookHover::default());
        let cf = cq.iter().find(|b| matches!(b.sprite, BookSprite::Filter { .. })).unwrap();
        assert_ne!(f.sprite, cf.sprite);
    }

    #[test]
    fn opening_the_book_MOVES_the_menu_it_sits_beside() {
        // Centred while the book is shut…
        assert_eq!(screen_left(800, 176, false, false), (800 - 176) / 2);
        // …and shoved right while it is open, by a literal 177 plus the
        // centring of a 200-wide block. NOT the centring of the pair.
        assert_eq!(screen_left(800, 176, true, false), 177 + (800 - 176 - 200) / 2);
        assert_eq!(screen_left(800, 176, true, false) - screen_left(800, 176, false, false), 77);
        // A narrow window keeps the menu centred and lets the book cover it.
        assert_eq!(screen_left(360, 176, true, true), (360 - 176) / 2);
        // Which is exactly the case `x_origin` stops offsetting for, so the
        // two agree about the same window.
        assert!(width_too_narrow(360) && !width_too_narrow(379));
        assert_eq!(x_origin(360, width_too_narrow(360)), (360 - 147) / 2);
    }

    /// The threshold is a literal, and one below it is narrow.
    #[test]
    fn the_narrow_threshold_is_379_exclusive() {
        assert!(width_too_narrow(378));
        assert!(!width_too_narrow(379));
        assert!(!width_too_narrow(380));
    }

    /// The book and the menu are BOTH window-centred, and the gap between them
    /// is still not constant, because 147 is odd and a menu's width is even.
    /// That is why the book is placed from the window rather than expressed as
    /// an offset from the panel.
    #[test]
    fn the_book_to_panel_gap_changes_with_window_PARITY() {
        let gap = |w: i32| x_origin(w, false) - screen_left(w, 176, true, false);
        // One pixel apart on adjacent widths — an offset baked against either
        // one is wrong on the other.
        assert_ne!(gap(800), gap(801));
        assert_eq!((gap(800) - gap(801)).abs(), 1);
    }

    #[test]
    fn a_selected_tab_sticks_out_and_takes_its_icon_with_it() {
        assert_eq!(tab_x_shift(true), -2);
        assert_eq!(tab_x_shift(false), 0);
        // The icon moves by the same amount — not by zero, and not by double.
        assert_eq!(tab_icon_offsets(false, false), vec![(9, 5)]);
        assert_eq!(tab_icon_offsets(false, true), vec![(7, 5)]);
        assert_eq!(
            tab_icon_offsets(false, true)[0].0 - tab_icon_offsets(false, false)[0].0,
            tab_x_shift(true)
        );
    }

    /// A single icon sits at 9; a pair at 3 and 14. 9 is NOT their midpoint,
    /// so a "centre one icon between where two would go" derivation is wrong.
    #[test]
    fn one_tab_icon_is_not_the_midpoint_of_two() {
        let pair = tab_icon_offsets(true, false);
        assert_eq!(pair, vec![(3, 5), (14, 5)]);
        let midpoint = (pair[0].0 + pair[1].0 + 16) / 2 - 8; // centre of the two 16px icons
        assert_eq!(tab_icon_offsets(false, false)[0].0, 9);
        assert_ne!(tab_icon_offsets(false, false)[0].0, midpoint);
    }

    /// A tab responds to SELECTION, never to the cursor.
    #[test]
    fn a_tabs_sprite_tracks_selection_and_not_hover() {
        assert!(tab_selected_sprite(true));
        assert!(!tab_selected_sprite(false));
        // There is no hover input at all — the signature cannot express one,
        // which is the point: `get(true, selected)` leaves hover unread.
    }

    #[test]
    fn the_slot_chrome_is_a_two_by_two_matrix() {
        assert_eq!(slot_sprite(true, false), SlotSprite::Craftable);
        assert_eq!(slot_sprite(true, true), SlotSprite::ManyCraftable);
        assert_eq!(slot_sprite(false, false), SlotSprite::Uncraftable);
        assert_eq!(slot_sprite(false, true), SlotSprite::ManyUncraftable);
        // All four are distinct — a collapsed pair would draw the wrong chrome
        // for half the states.
        let all = [
            slot_sprite(true, false),
            slot_sprite(true, true),
            slot_sprite(false, false),
            slot_sprite(false, true),
        ];
        for i in 0..4 {
            for j in (i + 1)..4 {
                assert_ne!(all[i], all[j]);
            }
        }
    }

    /// The stacked look is two items at DIFFERENT offsets. (4, 4) would render
    /// as one item and hide the effect entirely.
    #[test]
    fn a_multi_recipe_slot_draws_two_items_that_do_not_coincide() {
        assert_eq!(recipe_item_offsets(false), (None, 4));
        let (back, front) = recipe_item_offsets(true);
        assert_eq!((back, front), (Some(5), 3));
        assert_ne!(back.unwrap(), front);
        // The front one moves UP-LEFT of where a single item sits, and the
        // back one down-right of it.
        assert!(front < 4 && back.unwrap() > 4);
    }

    #[test]
    fn the_pulse_peaks_at_half_and_is_flat_at_both_ends() {
        assert!((squeeze(0.0) - 1.0).abs() < 1e-6);
        assert!((squeeze(ANIMATION_TIME) - 1.0).abs() < 1e-6, "sin(PI) is 0");
        assert!((squeeze(ANIMATION_TIME / 2.0) - 1.1).abs() < 1e-6, "the peak");
        // It only ever grows.
        assert!(squeeze(ANIMATION_TIME * 0.25) > 1.0);
    }

    #[test]
    fn an_arrow_hides_at_the_end_it_points_at() {
        assert_eq!(page_arrows_visible(0, 1), (false, false), "one page, neither");
        assert_eq!(page_arrows_visible(0, 3), (true, false), "at the front");
        assert_eq!(page_arrows_visible(1, 3), (true, true), "in the middle");
        assert_eq!(page_arrows_visible(2, 3), (false, true), "at the end");
        // An empty book has zero pages, and `total > 1` is what keeps the back
        // arrow off it — `page > 0` alone would too, but only by luck of the
        // clamp.
        assert_eq!(page_arrows_visible(0, 0), (false, false));
    }

    #[test]
    fn the_page_label_is_centred_on_73_not_on_the_panel() {
        // 147 wide, so the true centre is 73.5 — vanilla uses 73.
        assert_eq!(page_label_x(0), 73);
        assert_eq!(page_label_x(20), 63);
        assert_ne!(PAGE_LABEL_CENTRE_X * 2, IMAGE_W, "73*2 != 147, deliberately");
        // The width is halved with integer division, so an odd label is not
        // symmetric about the centre either.
        assert_eq!(page_label_x(21), 73 - 10);
    }

    /// The real `en_us.json` value, so the tests below read what the client
    /// actually shows rather than a plausible stand-in. Written out rather than
    /// loaded because that is the point of the first assertion: **no spaces**.
    const PAGE_TEMPLATE: &str = "%s/%s";

    #[test]
    fn a_one_page_book_shows_no_counter_at_all() {
        // `if (this.totalPages > 1)`. Not a permanent `1/1`, and not an empty
        // string — the text is never laid out, so nothing occupies the row.
        assert_eq!(page_label(0, 1, PAGE_TEMPLATE), None);
        assert_eq!(page_label(0, 0, PAGE_TEMPLATE), None, "an empty book");
        assert_eq!(page_label(0, 2, PAGE_TEMPLATE).as_deref(), Some("1/2"));
        // The same threshold the arrows use, and for the same reason: for any
        // page inside its own book, `fwd || back` is false only when the page
        // is both the first and the last — which is `total <= 1` exactly. So
        // "the counter is drawn" and "some arrow is drawn" are the same
        // predicate, and asserting the equality catches either one drifting.
        //
        // Written WITHOUT a `|| total > 1` third term: an earlier draft had one,
        // which made the right-hand side equal to the gate under test and the
        // arrows irrelevant to it.
        for total in 0..6 {
            for page in 0..total.max(1) {
                let (fwd, back) = page_arrows_visible(page, total);
                assert_eq!(
                    page_label(page, total, PAGE_TEMPLATE).is_some(),
                    fwd || back,
                    "counter and arrows disagree on page {page} of {total}"
                );
            }
        }
    }

    #[test]
    fn only_the_current_page_is_converted_to_1_based() {
        // `Component.translatable(key, currentPage + 1, totalPages)` — the `+1`
        // is on the FIRST argument only. Applying it to both gives "1/4" here,
        // which still counts up correctly and so survives casual use.
        assert_eq!(page_label(0, 3, PAGE_TEMPLATE).as_deref(), Some("1/3"));
        assert_eq!(page_label(1, 3, PAGE_TEMPLATE).as_deref(), Some("2/3"));
        assert_eq!(page_label(2, 3, PAGE_TEMPLATE).as_deref(), Some("3/3"));
    }

    #[test]
    fn the_separator_is_a_bare_slash_with_no_spaces() {
        // `'gui.recipebook.page' = '%s/%s'`. A spaced " / " is what the label
        // looks like it should be and is 2 px wider, which moves the centred x.
        let label = page_label(0, 3, PAGE_TEMPLATE).unwrap();
        assert_eq!(label, "1/3");
        assert!(!label.contains(' '));
    }

    #[test]
    fn a_missing_translation_renders_the_bare_key_rather_than_nothing() {
        // `Language.getOrDefault` returns the key, and `decomposeTemplate` on a
        // string with no specifiers yields it unchanged. So this is vanilla's
        // own behaviour, not a fallback invented here — and it is visible,
        // which is what makes a missing key get fixed.
        assert_eq!(
            page_label(0, 3, PAGE_LABEL_KEY).as_deref(),
            Some(PAGE_LABEL_KEY)
        );
        // …and the gate still applies: a one-page book shows nothing even when
        // the key is missing.
        assert_eq!(page_label(0, 1, PAGE_LABEL_KEY), None);
    }

    /// `WidgetSprites`' `enabled` slot carries "is filtering", not "is the
    /// button usable". Read the other way the button never changes on click.
    #[test]
    fn the_filter_sprite_is_keyed_on_FILTERING_and_hover_is_the_second_axis() {
        assert_eq!(filter_sprite_offset(true, false), 0);
        assert_eq!(filter_sprite_offset(false, false), 1);
        assert_eq!(filter_sprite_offset(true, true), 2);
        assert_eq!(filter_sprite_offset(false, true), 3);
        // Toggling the filter must move the sprite at BOTH hover states —
        // the failure mode of reading the axes the wrong way round is that one
        // of these two pairs collapses.
        assert_ne!(
            filter_sprite_offset(true, false),
            filter_sprite_offset(false, false)
        );
        assert_ne!(
            filter_sprite_offset(true, true),
            filter_sprite_offset(false, true)
        );
    }

    #[test]
    fn the_book_is_positioned_against_the_WINDOW_and_slides_when_narrow() {
        // Centred, then pushed left by 86 to flank the menu…
        assert_eq!(x_origin(800, false), (800 - 147) / 2 - 86);
        // …and NOT pushed on a narrow window, where it covers the menu.
        assert_eq!(x_origin(800, true), (800 - 147) / 2);
        assert_eq!(x_origin(800, false) + OFFSET_X, x_origin(800, true));
        // Y never moves.
        assert_eq!(y_origin(600), (600 - 166) / 2);
    }

    #[test]
    fn the_grid_is_five_wide_and_four_tall() {
        assert_eq!(grid_slot(0), (11, 31));
        assert_eq!(grid_slot(4), (11 + 4 * 25, 31), "the end of row 0");
        assert_eq!(grid_slot(5), (11, 31 + 25), "and 5 wraps");
        assert_eq!(grid_slot(19), (11 + 4 * 25, 31 + 3 * 25), "the last of 20");
        assert_eq!(GRID_COLS * GRID_ROWS, ITEMS_PER_PAGE as i32);
    }

    #[test]
    fn an_empty_book_has_ZERO_pages_rather_than_one() {
        assert_eq!(total_pages(0), 0);
        assert_eq!(total_pages(1), 1);
        assert_eq!(total_pages(20), 1);
        assert_eq!(total_pages(21), 2, "ceil, not floor");
        assert_eq!(total_pages(40), 2);
    }

    #[test]
    fn a_shrinking_list_sends_the_reader_to_the_FRONT_not_the_end() {
        // `totalPages <= currentPage` — note the `<=`, so an index equal to
        // the count resets too.
        assert_eq!(clamp_page(3, 100, false), 3, "still in range");
        assert_eq!(clamp_page(5, 100, false), 0, "5 pages, index 5 is out");
        assert_eq!(clamp_page(3, 20, false), 0, "the list shrank");
        // …and the reset is to 0, not to the new LAST page. The fixture has to
        // have more than one page left for the two to differ at all: the first
        // draft shrank to 20 collections, where the last page IS page 0, so it
        // could not express its own claim.
        assert_eq!(total_pages(100), 5);
        assert_eq!(clamp_page(9, 100, false), 0);
        assert_ne!(clamp_page(9, 100, false), total_pages(100) - 1);
        assert_eq!(clamp_page(2, 100, true), 0, "reset wins regardless");
    }

    #[test]
    fn a_page_shows_at_most_twenty_and_the_last_one_is_short() {
        assert_eq!(page_range(0, 45), 0..20);
        assert_eq!(page_range(1, 45), 20..40);
        assert_eq!(page_range(2, 45), 40..45, "the short last page");
        assert_eq!(page_range(0, 0), 0..0);
        // A page past the end is empty rather than a panic.
        assert_eq!(page_range(9, 5), 5..5);
    }

    /// M93z modelled the four `SearchRecipeBookCategory` values as "the tabs".
    /// They are the SEARCH TAB OF EACH OF THE FOUR BOOKS. A crafting book has
    /// five tabs, and the first of them is the search tab whose contents are
    /// that enum's `includedCategories()`.
    /// The player's items are 5..46 — NOT the whole 46-slot menu, which would
    /// double-count the 2x2 grid and add the craft result.
    #[test]
    fn the_players_item_slots_exclude_the_grid_and_the_result() {
        assert_eq!(PLAYER_ITEM_SLOTS, 5..46);
        assert!(!PLAYER_ITEM_SLOTS.contains(&0), "the craft RESULT");
        for grid in 1..5 {
            assert!(!PLAYER_ITEM_SLOTS.contains(&grid), "grid slot {grid}");
        }
        assert!(PLAYER_ITEM_SLOTS.contains(&5), "armour starts here");
        assert!(PLAYER_ITEM_SLOTS.contains(&45), "and the offhand ends it");
        // The grid arrives through the craft-slot fill instead, so between the
        // two every slot but 0 is accounted exactly once.
        let grid = craft_slots(BookType::Crafting, true).unwrap();
        assert_eq!(grid.range, 1..5);
        for s in 0..46usize {
            let n = usize::from(PLAYER_ITEM_SLOTS.contains(&s)) + usize::from(grid.range.contains(&s));
            assert_eq!(n, usize::from(s != 0), "slot {s} accounted {n} times");
        }
    }

    /// The two families differ in RANGE and in GATING, and the furnace's
    /// includes its result slot.
    #[test]
    fn the_craft_slots_differ_by_family_in_both_range_and_gating() {
        let craft = craft_slots(BookType::Crafting, false).unwrap();
        assert_eq!(craft.range, 1..10, "a 3x3 grid, result at 0 excluded");
        assert!(craft.gated, "accountSimpleStack");

        for b in [BookType::Furnace, BookType::BlastFurnace, BookType::Smoker] {
            let f = craft_slots(b, false).unwrap();
            assert_eq!(f.range, 0..3, "the whole container, {b:?}");
            assert!(f.range.contains(&2), "INCLUDING the result slot");
            assert!(!f.gated, "bare accountStack — no isUsableForCrafting");
        }
        // The player's own inventory is the crafting family's 2x2.
        let player = craft_slots(BookType::Crafting, true).unwrap();
        assert_eq!(player.range, 1..5);
        assert!(player.gated);
    }

    #[test]
    fn a_book_has_its_own_tabs_and_the_first_is_SEARCH() {
        assert_eq!(BookType::Crafting.tabs().len(), 5, "NOT 4");
        assert_eq!(BookType::Furnace.tabs().len(), 4);
        assert_eq!(BookType::BlastFurnace.tabs().len(), 3);
        assert_eq!(BookType::Smoker.tabs().len(), 2);
        for b in BookType::ALL {
            assert!(b.tabs()[0].is_search(), "{b:?} tab 0 is the search tab");
            assert_eq!(b.tabs()[0].primary, "compass", "every search tab is a compass");
            // ...and no OTHER tab is a search tab.
            assert!(b.tabs()[1..].iter().all(|t| !t.is_search()));
        }
    }

    /// The search tab's list is `SearchRecipeBookCategory`'s, in its own
    /// hand-written order — equipment FIRST, where the registry's id order is
    /// building_blocks, redstone, equipment, misc.
    #[test]
    fn the_search_tabs_categories_do_not_follow_the_registrys_id_order() {
        let search = &BookType::Crafting.tabs()[0];
        assert_eq!(search.categories[0], "minecraft:crafting_equipment");
        assert_eq!(search.categories.len(), 4);
        assert_eq!(
            BookType::Smoker.tabs()[0].categories,
            ["minecraft:smoker_food"]
        );
    }

    /// The search tab is the union of the book's category tabs — so a
    /// collection reachable from a category tab is reachable from search, and
    /// the two cannot drift.
    #[test]
    fn the_search_tab_is_exactly_the_union_of_the_category_tabs() {
        for b in BookType::ALL {
            let mut from_categories: Vec<&str> = b.tabs()[1..]
                .iter()
                .flat_map(|t| t.categories.iter().copied())
                .collect();
            from_categories.sort();
            from_categories.dedup();
            let mut search: Vec<&str> = b.tabs()[0].categories.to_vec();
            search.sort();
            assert_eq!(search, from_categories, "{b:?}");
        }
    }

    #[test]
    fn three_categories_belong_to_NO_book_at_all() {
        // 13 in the registry, 10 across the four books' search tabs.
        let mut covered: Vec<&str> = BookType::ALL
            .iter()
            .flat_map(|b| b.tabs()[0].categories.iter().copied())
            .collect();
        covered.sort();
        covered.dedup();
        assert_eq!(covered.len(), 10);
        assert_eq!(covered.len() + CATEGORIES_WITHOUT_A_TAB.len(), 13);
        for c in CATEGORIES_WITHOUT_A_TAB {
            assert!(!covered.contains(&c), "{c} must belong to no book");
            assert_eq!(category_tab_of(BookType::Crafting, c), None);
        }
    }

    /// `category_tab_of` skips the search tab, or it would answer 0 for every
    /// category the book has.
    #[test]
    fn a_category_resolves_to_its_CATEGORY_tab_not_to_search() {
        assert_eq!(
            category_tab_of(BookType::Crafting, "minecraft:crafting_equipment"),
            Some(1)
        );
        assert_eq!(
            category_tab_of(BookType::Crafting, "minecraft:crafting_redstone"),
            Some(4)
        );
        assert_eq!(category_tab_of(BookType::Crafting, "minecraft:furnace_food"), None);
        assert_eq!(category_tab_of(BookType::Crafting, "minecraft:not_a_category"), None);
        // The search tab DOES contain it, which is what makes skipping it the
        // load-bearing part.
        assert!(BookType::Crafting.tabs()[0]
            .categories
            .contains(&"minecraft:crafting_equipment"));
    }

    /// Two icons or one, and the pair is only ever on the tabs vanilla gives
    /// two — checked against the decompile's own lists rather than by a rule.
    #[test]
    fn the_paired_icon_tabs_are_the_four_vanilla_declares() {
        let paired: Vec<(&str, &str)> = BookType::ALL
            .iter()
            .flat_map(|b| b.tabs())
            .filter_map(|t| t.secondary.map(|sec| (t.primary, sec)))
            .collect();
        assert_eq!(
            paired,
            vec![
                ("iron_axe", "golden_sword"),
                ("lava_bucket", "apple"),
                ("lava_bucket", "emerald"),
                ("iron_shovel", "golden_leggings"),
            ]
        );
    }

    /// The filter art is "not crafting", not "named like a furnace" — there
    /// are only two `RecipeBookComponent` subclasses and the three furnaces
    /// share one.
    #[test]
    fn the_filter_art_splits_crafting_from_everything_else() {
        assert!(!BookType::Crafting.furnace_family());
        for b in [BookType::Furnace, BookType::BlastFurnace, BookType::Smoker] {
            assert!(b.furnace_family(), "{b:?}");
        }
    }

    #[test]
    fn the_tabs_stack_down_the_left_at_a_pitch_of_27() {
        assert_eq!(tab_position(0), (-30, 3));
        assert_eq!(tab_position(1), (-30, 30));
        assert_eq!(tab_position(3), (-30, 3 + 81));
    }

    #[test]
    fn an_ungrouped_recipe_is_its_own_collection() {
        let c = collections(&[(1, None, "a"), (2, None, "a"), (3, None, "b")]);
        assert_eq!(c.len(), 3);
        assert!(c.iter().all(|c| c.recipes.len() == 1));
    }

    #[test]
    fn a_group_is_one_collection_placed_at_its_FIRST_member() {
        // 7 arrives between the two members of group 0, so it must land
        // AFTER the group rather than inside or before it.
        let c = collections(&[(1, Some(0), "a"), (7, None, "a"), (2, Some(0), "a")]);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].recipes, vec![1, 2], "the group, at its first member");
        assert_eq!(c[1].recipes, vec![7]);
    }

    #[test]
    fn a_group_id_is_scoped_to_its_CATEGORY() {
        // The same group number in two categories is two collections —
        // vanilla's table is keyed by the pair.
        let c = collections(&[(1, Some(0), "a"), (2, Some(0), "b")]);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].category, "a");
        assert_eq!(c[1].category, "b");
    }

    #[test]
    fn the_unselected_are_hidden_whether_or_not_the_filter_is_on() {
        // The first stage is UNCONDITIONAL: the filter button toggles only the
        // third. "Show all recipes" still hides what the open menu cannot make.
        let s = |any_selected, has_craftable| CollectionState {
            any_selected,
            has_craftable,
            matches_search: true,
        };
        let states = [s(true, true), s(true, false), s(false, true), s(false, false)];
        assert_eq!(visible_collections(&states, false), vec![0, 1]);
        assert_eq!(visible_collections(&states, true), vec![0]);
        // Not-selected is hidden in BOTH, which is the point.
        assert!(!visible_collections(&states, false).contains(&2));
        assert!(!visible_collections(&states, true).contains(&2));
    }

    #[test]
    fn the_search_stage_is_independent_of_the_filter() {
        let s = |matches_search| CollectionState {
            any_selected: true,
            has_craftable: true,
            matches_search,
        };
        let states = [s(true), s(false)];
        assert_eq!(visible_collections(&states, false), vec![0]);
        assert_eq!(visible_collections(&states, true), vec![0]);
    }

    #[test]
    fn the_query_is_lowercased_by_the_ROOT_locale() {
        assert_eq!(search_query("IRON Ingot"), "iron ingot");
        assert_eq!(search_query(""), "");
    }

    // --- M104, the which-of-these overlay's chrome ---

    /// Its panel is the list's ONE nine-slice, and it comes before the buttons.
    ///
    /// The panel's **geometry** is pinned against `nine_slice::quads` for the
    /// rect and border this is meant to pass it, not merely counted. Two
    /// mutations survived an earlier cut that only asserted "several quads,
    /// each with a source rect": one sized the panel for a single button
    /// regardless of the count, the other dropped the border to zero. Both
    /// leave the quads several and sourced, and one of them even leaves the
    /// bounding box right — so nothing short of the set itself sees them.
    ///
    /// Not circular: `panel_size`'s literals are pinned in `recipe_overlay`
    /// and `nine_slice::quads`' behaviour in `nine_slice`, so this asserts only
    /// that the two are wired to each other correctly.
    #[test]
    fn the_overlay_panel_is_nine_sliced_and_sits_under_its_buttons() {
        use crate::recipe_overlay as ro;
        let origin = (40, 60);
        let qs = overlay_chrome(origin, &[true, false, true], false, None);
        let panel: Vec<_> = qs.iter().filter(|q| q.sprite == BookSprite::OverlayPanel).collect();
        assert!(panel.len() > 1, "nine-sliced, so several quads: {}", panel.len());
        // Three buttons on one row: 3 * 25 + 8 by 1 * 25 + 8.
        assert_eq!(ro::panel_size(3), (83, 33));
        let expect = crate::nine_slice::quads(
            (origin.0, origin.1, 83, 33),
            (ro::PANEL_SHEET, ro::PANEL_SHEET),
            crate::nine_slice::Border::all(ro::PANEL_BORDER),
        );
        assert_eq!(panel.len(), expect.len());
        for (got, want) in panel.iter().zip(&expect) {
            assert_eq!((got.x, got.y, got.w, got.h), (want.dx, want.dy, want.w, want.h));
            assert_eq!(got.src, Some((want.sx, want.sy, want.sw, want.sh)));
        }
        // Every panel quad precedes every button quad — the overlay is drawn
        // after `nextStratum()`, and inside it the backing goes down first.
        let first_button = qs
            .iter()
            .position(|q| !matches!(q.sprite, BookSprite::OverlayPanel))
            .unwrap();
        assert!(qs[..first_button].iter().all(|q| q.sprite == BookSprite::OverlayPanel));
        assert_eq!(qs.len() - first_button, 3, "one quad per recipe");
        // And only the panel carries a source rect: everything else takes the
        // whole of its own sprite.
        assert!(qs[..first_button].iter().all(|q| q.src.is_some()));
        assert!(qs[first_button..].iter().all(|q| q.src.is_none()));
    }

    /// Craftable and hover are independent, and the hover picks exactly one.
    #[test]
    fn a_button_reads_craftable_and_hover_separately() {
        let qs = overlay_chrome((0, 0), &[true, false], false, Some(1));
        let buttons: Vec<_> = qs
            .iter()
            .filter_map(|q| match q.sprite {
                BookSprite::OverlayButton { furnace, craftable, hovered } => {
                    Some((furnace, craftable, hovered))
                }
                _ => None,
            })
            .collect();
        assert_eq!(buttons, vec![(false, true, false), (false, false, true)]);
    }

    /// The family follows the MENU. A furnace book's buttons are furnace art
    /// whatever the recipes are, which is the same rule the ingredient grid
    /// obeys one module over.
    #[test]
    fn the_button_family_follows_the_menu() {
        let qs = overlay_chrome((0, 0), &[true], true, None);
        assert!(qs.iter().any(|q| q.sprite
            == BookSprite::OverlayButton { furnace: true, craftable: true, hovered: false }));
    }

    /// The buttons land where [`crate::recipe_overlay::button_origin`] says,
    /// inside the panel the same call sized.
    #[test]
    fn the_buttons_tile_inside_their_own_panel() {
        use crate::recipe_overlay as ro;
        let origin = (17, 23);
        let flags = [true; 6];
        let qs = overlay_chrome(origin, &flags, false, None);
        let (pw, ph) = ro::panel_size(flags.len());
        for (i, q) in qs
            .iter()
            .filter(|q| matches!(q.sprite, BookSprite::OverlayButton { .. }))
            .enumerate()
        {
            assert_eq!((q.x, q.y), ro::button_origin(origin, i, flags.len()));
            assert_eq!((q.w, q.h), (ro::BUTTON_SIZE, ro::BUTTON_SIZE));
            assert!(
                q.x >= origin.0 && q.y >= origin.1,
                "button {i} starts before the panel"
            );
            assert!(
                q.x + q.w <= origin.0 + pw && q.y + q.h <= origin.1 + ph,
                "button {i} spills out of the panel"
            );
        }
        // Six buttons at maxRow 4 is two rows, so the second row exists.
        assert_eq!(ro::rows(6), 2);
    }
}
