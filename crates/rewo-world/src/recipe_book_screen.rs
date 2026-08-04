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

/// `SearchRecipeBookCategory` — the four tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Crafting,
    Furnace,
    BlastFurnace,
    Smoker,
}

impl Tab {
    /// In declaration order, which is the order the tabs stack down the left.
    pub const ALL: [Tab; 4] = [Tab::Crafting, Tab::Furnace, Tab::BlastFurnace, Tab::Smoker];

    /// `includedCategories()`, by registry name.
    ///
    /// **Crafting lists `equipment` first**, not `building_blocks` — the
    /// registry's own id order is building_blocks, redstone, equipment, misc,
    /// and the tab's is a different, hand-written order. Deriving a tab's
    /// contents from registry ids would reorder every crafting collection.
    pub fn included(self) -> &'static [&'static str] {
        match self {
            Tab::Crafting => &[
                "minecraft:crafting_equipment",
                "minecraft:crafting_building_blocks",
                "minecraft:crafting_misc",
                "minecraft:crafting_redstone",
            ],
            Tab::Furnace => &[
                "minecraft:furnace_food",
                "minecraft:furnace_blocks",
                "minecraft:furnace_misc",
            ],
            Tab::BlastFurnace => &[
                "minecraft:blast_furnace_blocks",
                "minecraft:blast_furnace_misc",
            ],
            Tab::Smoker => &["minecraft:smoker_food"],
        }
    }

    /// Where this tab's button sits, relative to the book's origin.
    ///
    /// `index` is its position among the **visible** tabs, which for the four
    /// search categories is always their declaration order: `updateTabs` sets
    /// `visible = true` unconditionally for a `SearchRecipeBookCategory` and
    /// only asks `updateVisibility` for the others.
    pub fn position(index: i32) -> (i32, i32) {
        (TAB_DX, TAB_DY + TAB_PITCH * index)
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

/// Which tab a category belongs to, or `None` for the three above.
pub fn tab_of(category: &str) -> Option<Tab> {
    Tab::ALL
        .into_iter()
        .find(|t| t.included().contains(&category))
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn the_crafting_tab_does_not_follow_the_registrys_id_order() {
        // The registry is building_blocks, redstone, equipment, misc; the tab
        // is equipment FIRST. Deriving from ids would reorder every crafting
        // collection on screen.
        assert_eq!(Tab::Crafting.included()[0], "minecraft:crafting_equipment");
        assert_eq!(Tab::Crafting.included().len(), 4);
        assert_eq!(Tab::Smoker.included(), ["minecraft:smoker_food"]);
    }

    #[test]
    fn three_categories_belong_to_NO_tab() {
        // 13 in the registry, 10 across the four tabs.
        let covered: usize = Tab::ALL.iter().map(|t| t.included().len()).sum();
        assert_eq!(covered, 10);
        assert_eq!(covered + CATEGORIES_WITHOUT_A_TAB.len(), 13);
        for c in CATEGORIES_WITHOUT_A_TAB {
            assert_eq!(tab_of(c), None, "{c} must not resolve to a tab");
        }
        assert_eq!(tab_of("minecraft:furnace_food"), Some(Tab::Furnace));
        assert_eq!(tab_of("minecraft:crafting_misc"), Some(Tab::Crafting));
        assert_eq!(tab_of("minecraft:not_a_category"), None);
    }

    #[test]
    fn the_tabs_stack_down_the_left_at_a_pitch_of_27() {
        assert_eq!(Tab::position(0), (-30, 3));
        assert_eq!(Tab::position(1), (-30, 30));
        assert_eq!(Tab::position(3), (-30, 3 + 81));
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
}
