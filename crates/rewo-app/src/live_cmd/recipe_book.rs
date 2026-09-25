use super::*;

/// The two enchanting-table inputs that are the *player's* rather than the
/// menu's (M92).
///
/// A named pair rather than two loose parameters because they are only ever
/// read together and swapping a level for a flag would compile.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EnchantPlayer<'a> {
    /// `player.experienceLevel`, from `set_experience` (M79).
    pub xp_level: i32,
    /// `player.hasInfiniteMaterials()` — `abilities.instabuild` (M75).
    pub creative: bool,
    /// The beacon's six effect ids (M92c). Carried here rather than passed
    /// separately because both screens' extra inputs travel the same seam.
    pub beacon_effects: BeaconEffectIds,
    /// The loom's pattern grid (M93q), when a loom is open.
    pub loom: Option<LoomView>,
    /// The stonecutter's recipe grid (M93s), when a stonecutter is open.
    ///
    /// A reference, not a value: the visible list is a `Vec` built per frame
    /// from `selectByInput`, and this struct is `Copy` because the enchanting
    /// rows and the panel each take it by value.
    pub cut: Option<&'a CutView>,
    /// The anvil field's cursor and selection quads (M93t), already measured.
    pub anvil_fills: &'a [(usize, rewo_gpu::container::PanelBlit)],
    /// The merchant's trade list (M93u), when a merchant is open.
    pub merchant: Option<&'a MerchantView>,
    /// The beacon SCREEN's own choice (M93m), when a screen is driving.
    ///
    /// `None` re-reads the menu's data slots, which is right for a gate with
    /// no screen state — and wrong for the live client, where a click moves
    /// the choice and only `set_beacon` on confirm tells the server. Without
    /// this the render would keep painting the server's last word and a click
    /// would light nothing, which is M93i's "correct on the wire, invisible on
    /// screen" one screen over.
    pub beacon_override: Option<rewo_world::menu_screen::BeaconChoice>,
    /// M103 — the ghost recipe's two washes, already measured. Resolved by the
    /// caller for the reason the beacon's choice and the anvil's fills are: the
    /// slot positions need the layout, and `apply_screen` holds no ScreenState.
    pub ghost_under: &'a [(usize, rewo_gpu::container::PanelBlit)],
    pub ghost_over: &'a [(usize, rewo_gpu::container::PanelBlit)],
}

/// Which of `RecipeBookSettings`' four `TypeSettings` a menu reads (M94).
///
/// **Only four menus have a book at all** — `RecipeBookMenu` is abstract and
/// exactly three concrete classes implement `getRecipeBookType`: the player's
/// own `InventoryMenu` and `CraftingMenu` (both CRAFTING) and
/// `AbstractFurnaceMenu`, which returns the type its subclass was built with.
/// Every other screen returns `None` here and draws no book, which is why a
/// chest is unaffected by any of this.
pub(super) fn book_type_of(
    layout: &rewo_world::menu_layout::MenuLayout,
) -> Option<rewo_world::recipe_book_screen::BookType> {
    use rewo_world::recipe_book_screen::BookType;
    // Keyed on the registry NAME, not the protocol id: the id is the server's
    // and a version bump renumbers it, while these four names are what the
    // decompile's class names correspond to. (M94's gate learned the hard way
    // that 13 is `enchantment`, not `crafting`.)
    if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID {
        // The player's own inventory IS a `RecipeBookMenu`: `InventoryMenu`
        // returns CRAFTING. It has no menu registry id because it is never
        // opened by `open_screen`.
        return Some(BookType::Crafting);
    }
    match layout.name {
        "crafting" => Some(BookType::Crafting),
        "furnace" => Some(BookType::Furnace),
        "blast_furnace" => Some(BookType::BlastFurnace),
        "smoker" => Some(BookType::Smoker),
        _ => None,
    }
}

/// An item's `getMaxStackSize`, for `accountStack`'s `min` (M96).
///
/// Falls back to 64, which is the default `Item.Properties` value and so the
/// right answer for the 1,242 of 1,537 items that do not override it.
pub(super) fn max_stack_of(items: &rewo_data::items::Items, id: i32) -> i32 {
    items
        .name(id)
        .map_or(rewo_data::item_props_table::DEFAULT_MAX_STACK, |n| {
            rewo_data::item_props_table::max_stack_size(n)
        })
}

/// Everything the recipe book's chrome needs for one frame (M94).
#[derive(Debug, Clone, Default)]
pub(crate) struct BookRender {
    pub view: Option<rewo_world::recipe_book_screen::BookView>,
    /// `(hasCraftable, hasMultipleRecipes)` for each collection on the page.
    pub slots: Vec<(bool, bool)>,
    pub hover: rewo_world::recipe_book_screen::BookHover,
    /// Which book this is, which decides its tab list and its filter art
    /// (M95). M94 assumed four tabs for every book; a crafting book has FIVE.
    pub book: rewo_world::recipe_book_screen::BookType,
    /// The item each visible slot shows this frame, already cycled — `None`
    /// for a collection whose result needs a context Rewo has not got.
    pub slot_items: Vec<Option<i32>>,
    /// Per visible slot: several recipes AND one shared result display, the
    /// pair of conditions that draws the shadow copy.
    pub slot_shadowed: Vec<bool>,
    /// The recipe id each visible slot would place if clicked (M98) — the one
    /// the display cycle is on, not the collection's first.
    pub slot_recipes: Vec<Option<i32>>,
    /// Per visible slot, its whole collection resolved into overlay buttons —
    /// **in the collection's own order**, not the overlay's (M104).
    ///
    /// The promotion to craftable-first happens when the overlay is opened,
    /// because that is when vanilla does it and because the overlay is a
    /// snapshot: re-promoting each frame would re-sort an open overlay under
    /// the cursor. See `rewo_world::recipe_overlay::Open`.
    pub slot_collections: Vec<Vec<rewo_world::recipe_overlay::Button>>,
}

/// The recipe book for the menu currently on screen (M94), or `None` when it
/// is shut — which is also what keeps the menu centred.
///
/// # What this does not do yet
///
/// The selected tab and the current page are **client** state that only a
/// click can change, and nothing can click the book yet, so both are pinned to
/// 0 and the tab column renders with the first tab selected. `hasCraftable` is
/// answered as of M96 — see `held` below for the one input it still misses.
/// `RecipeBookComponent.isVisible()` — whether the book is showing.
///
/// **One predicate, consulted by the render's placement and by every hover.**
/// `ScreenState::hovered` had its own conversion through `Placement::centred`
/// and so ignored the book entirely; M89 and M106b each fixed one consumer of
/// this same question and each recorded that a per-call-site choice is how
/// they come to disagree.
///
/// It is the **settings flag for the open menu's book type**, which is what
/// `initVisuals` sets `this.visible` from. [`live_recipe_book`] additionally
/// requires the display registries, so `book.is_some()` is narrower in
/// principle — and identical in practice, because those come from the bake at
/// session setup rather than off the wire, so they are present before any
/// menu can open. A menu with no book at all answers `false`.
pub(crate) fn book_visible(session: &rewo_net::play::PlaySession) -> bool {
    let layout = session
        .menus
        .open()
        .map(|m| m.menu.layout())
        .unwrap_or_else(|| session.inventory.layout());
    book_visible_for(book_type_of(layout), &session.recipe_book_settings)
}

/// [`book_visible`]'s rule, over plain values.
///
/// Lifted out because `PlaySession` owns a socket and cannot be constructed in
/// a test — M71's finding, and M97's fix. A mutation making a bookless menu
/// answer `true` survived the whole suite while this lived inside the
/// session-taking function, and a bookless menu answering `true` would
/// displace a chest's panel by 77 px and suppress its hover on a narrow
/// window.
pub(crate) fn book_visible_for(
    book: Option<rewo_world::recipe_book_screen::BookType>,
    settings: &rewo_net::recipe_book::BookSettings,
) -> bool {
    use rewo_world::recipe_book_screen as rb;
    match book {
        Some(rb::BookType::Crafting) => settings.crafting.open,
        Some(rb::BookType::Furnace) => settings.furnace.open,
        Some(rb::BookType::BlastFurnace) => settings.blast_furnace.open,
        Some(rb::BookType::Smoker) => settings.smoker.open,
        // A menu with no book — a chest, a beacon — is never displaced.
        None => false,
    }
}

pub(super) fn live_recipe_book(
    session: &rewo_net::play::PlaySession,
    items: &rewo_data::items::Items,
    state: rewo_world::recipe_book_screen::BookState,
    // M98 — the cursor in BOOK coordinates, or `None` when it is not being
    // tracked. Book space rather than screen: the conversion needs the window
    // size, and doing it once at the caller keeps the hover and the press
    // reading the same number.
    book_mouse: Option<(i32, i32)>,
    // M99 — the search field's contents, already lowercased, and the item
    // display-name map the search indexes.
    query: &str,
    display: &std::collections::HashMap<String, String>,
) -> Option<BookRender> {
    use rewo_world::recipe_book_screen as rb;
    let layout = session
        .menus
        .open()
        .map(|m| m.menu.layout())
        .unwrap_or_else(|| session.inventory.layout());
    let book = book_type_of(layout)?;
    let st = match book {
        rb::BookType::Crafting => session.recipe_book_settings.crafting,
        rb::BookType::Furnace => session.recipe_book_settings.furnace,
        rb::BookType::BlastFurnace => session.recipe_book_settings.blast_furnace,
        rb::BookType::Smoker => session.recipe_book_settings.smoker,
    };
    if !st.open {
        return None;
    }
    let names = session.recipe_display_ids.as_ref()?;
    let entries: Vec<BookEntry<'_>> = session
        .recipe_book
        .values()
        .filter_map(|e| {
            Some(BookEntry {
                id: e.id,
                group: e.group,
                category: names.category.name(e.category)?,
                results: e.display.result().items(),
                // M96 — the ingredient slots, tags resolved against the
                // server's own `update_tags` payload.
                ingredients: e.ingredients(&session.tags),
                // M99 — what the search indexes.
                search: search_entry_of(&e.display.result().items(), items, display),
                // M104 — the which-of-these overlay's ingredient grid.
                shape: e.display.overlay_shape(),
                grid_items: e.display.overlay_ingredients(),
            })
        })
        .collect();
    // What the player is holding, for `hasCraftable` — see
    // [`crafting_contents`], which is where the rules live so a test can reach
    // them (M97's lesson, fourth application).
    let mut held = crafting_contents(
        &session.inventory,
        session.menus.open().map(|m| &m.menu),
        book,
        &|id| max_stack_of(items, id),
    );
    // `Mth.floor(time / 30)` — vanilla's `time` advances by the partial tick
    // each render, so this is the session's tick clock divided by the swap
    // period. Shared by every slot, which is why a page of several-recipe
    // collections flips together rather than each on its own phase.
    let cycle = (session.ticks as f32 / rewo_net::recipe_book::TICKS_TO_SWAP_SLOT).floor() as i32;
    Some(book_render_from(
        book,
        st.filtering,
        &entries,
        &mut held,
        cycle,
        state,
        book_mouse,
        query,
    ))
}

/// The ghost recipe's two washes, split into the halves they belong in (M103).
///
/// Returns `(under, over)`. Vanilla's order per slot is fill, item, fill — so
/// the red goes with the panel's overlays and the white goes after the icons,
/// which is why they cannot be one list.
pub(crate) fn ghost_washes(
    ghosts: &[rewo_world::ghost_slots::Ghost],
    layout: &rewo_world::menu_layout::MenuLayout,
    player_inventory: bool,
) -> (
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
) {
    use rewo_world::ghost_slots as gs;
    let big = gs::big_result_slot(player_inventory);
    let argb = |v: u32| {
        [
            ((v >> 16) & 255) as f32 / 255.0,
            ((v >> 8) & 255) as f32 / 255.0,
            (v & 255) as f32 / 255.0,
            ((v >> 24) & 255) as f32 / 255.0,
        ]
    };
    let mut under = Vec::new();
    let mut over = Vec::new();
    for g in ghosts {
        let Some((sx, sy)) = layout.position(g.slot) else { continue };
        let (dx, dy, size) = gs::wash_rect(g.is_result, big);
        let fill = |tint: [f32; 4], dx: i32, dy: i32, size: i32| {
            (
                rewo_gpu::container::FILL_SPRITE,
                rewo_gpu::container::PanelBlit {
                    dx: (sx as i32 + dx) as f32,
                    dy: (sy as i32 + dy) as f32,
                    w: size as f32,
                    h: size as f32,
                    sx: 0.0,
                    sy: 0.0,
                    sw: 0.0,
                    sh: 0.0,
                    tint,
                },
            )
        };
        under.push(fill(argb(gs::WASH_UNDER), dx, dy, size));
        // The veil is always the plain 16x16 at the slot: only the wash BELOW
        // widens for a big result slot. Widening both would ring the icon in
        // white.
        over.push(fill(argb(gs::WASH_OVER), 0, 0, 16));
    }
    (under, over)
}

/// The ghost recipe for the shown menu, from `place_ghost_recipe` (M103).
///
/// M93y decoded that packet into `PlaySession::ghost_recipe` and nothing
/// consumed it — it is the only decoded-but-unrendered packet left in this
/// area. `handlePlaceRecipe` is gated on the container id matching the open
/// menu, so a ghost for a screen you have since closed is dropped rather than
/// drawn over whatever replaced it.
pub(crate) fn live_ghosts(
    session: &rewo_net::play::PlaySession,
) -> Vec<rewo_world::ghost_slots::Ghost> {
    use rewo_net::recipe_book::RecipeDisplay as D;
    use rewo_world::ghost_slots as gs;
    let Some((container, display)) = session.ghost_recipe.as_ref() else {
        return Vec::new();
    };
    // The ghost belongs to a container id; the shown menu has one too, and a
    // mismatch means the ghost is stale.
    if *container != session.shown_container_id() {
        return Vec::new();
    }
    let layout = session.shown_menu().layout();
    let book = match book_type_of(layout) {
        Some(b) => b,
        None => return Vec::new(),
    };
    let items = |d: &rewo_net::recipe_book::SlotDisplay| d.items();
    let (menu, inputs, shape) = match display {
        D::CraftingShaped { width, height, ingredients, .. } => (
            // `getGridWidth/Height` — 2x2 for the player's own inventory, 3x3
            // for a crafting table.
            gs::crafting_menu(
                if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID { 2 } else { 3 },
                if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID { 2 } else { 3 },
            ),
            ingredients.iter().map(items).collect::<Vec<_>>(),
            Some((*width as usize, *height as usize)),
        ),
        D::CraftingShapeless { ingredients, .. } => (
            gs::crafting_menu(
                if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID { 2 } else { 3 },
                if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID { 2 } else { 3 },
            ),
            ingredients.iter().map(items).collect(),
            None,
        ),
        D::Furnace { ingredient, fuel, .. } => (
            gs::FURNACE_MENU,
            vec![items(ingredient), items(fuel)],
            None,
        ),
        // A stonecutter or smithing display: `fillGhostRecipe`'s switch has no
        // case, so the result alone is ghosted — and neither screen has a book,
        // so this is unreachable in practice and transcribed anyway.
        _ => (
            gs::GhostMenu { result: 0, grid: None, furnace: None },
            Vec::new(),
            None,
        ),
    };
    let fuel_empty = menu
        .furnace
        .is_some_and(|(_, f)| session.shown_menu().menu_slot(f).is_none());
    let _ = book;
    gs::layout(menu, items(display.result()), &inputs, shape, fuel_empty)
}

/// `fillStackedContents` + `fillCraftSlotsStackedContents` (M102).
///
/// Vanilla calls **two** fills and they are disjoint:
///
/// ```java
/// player.getInventory().fillStackedContents(contents);   // the ITEMS
/// menu.fillCraftSlotsStackedContents(contents);          // the GRID
/// ```
///
/// `Inventory.items` is armour + storage + hotbar + offhand —
/// `PLAYER_ITEM_SLOTS`, **5..46**. It contains neither the 2x2 crafting grid
/// (1..5, which belongs to `InventoryMenu`) nor the craft **result** (slot 0,
/// which belongs to nothing). Walking the whole 46-slot menu is the obvious
/// reading and it double-counts the grid *and* adds the result — so a recipe
/// would read as craftable off its own output.
///
/// The inventory fill is `accountSimpleStack`, hence gated on
/// `isUsableForCrafting`; the craft-slot fill's gating depends on the family
/// (`craft_slots`). M96 named the predicate in a comment and applied nothing.
///
/// Extracted from `live_recipe_book` because that function needs a
/// `PlaySession` and so cannot be reached from a test — a mutation deleting the
/// craft-slot half survived until this split.
pub(crate) fn crafting_contents(
    player: &rewo_world::inventory::Inventory,
    open: Option<&rewo_world::inventory::Inventory>,
    book: rewo_world::recipe_book_screen::BookType,
    // `getMaxStackSize` per item id. A closure rather than the whole registry,
    // so this stays a function of plain values and its tests do not need the
    // user's decompile on disk.
    max_stack: &dyn Fn(i32) -> i32,
) -> rewo_world::stacked_contents::StackedContents {
    use rewo_world::recipe_book_screen as rb;
    let mut held = rewo_world::stacked_contents::StackedContents::new();
    let mut account = |inv: &rewo_world::inventory::Inventory, slot: usize, gated: bool| {
        if let Some(st) = inv.menu_slot(slot) {
            if !gated || inv.is_usable_for_crafting(st) {
                held.account_stack(st.item_id, st.count, max_stack(st.item_id));
            }
        }
    };
    for slot in rb::PLAYER_ITEM_SLOTS {
        account(player, slot, true);
    }
    let shown = open.unwrap_or(player);
    if let Some(cs) = rb::craft_slots(book, open.is_none()) {
        for slot in cs.range.clone() {
            account(shown, slot, cs.gated);
        }
    }
    held
}

/// One unlocked recipe, with everything already resolved (M97).
///
/// The seam between the session and the derivation: the session half is
/// lookups, the derivation half is the grouping, paging, cycling and craftable
/// arithmetic — and only the second half is worth grading, which is why it is
/// on this side of the line.
pub(crate) struct BookEntry<'a> {
    pub id: i32,
    pub group: Option<i32>,
    pub category: &'a str,
    /// The result display's item ids, in order.
    pub results: Vec<i32>,
    /// `None` when `craftingRequirements` was absent — never craftable.
    pub ingredients: Option<Vec<rewo_world::stacked_contents::Ingredient>>,
    /// The result items' names and ids, lowercased, for the search (M99).
    pub search: rewo_world::recipe_search::SearchEntry,
    /// How the which-of-these overlay lays this recipe's ingredients out, and
    /// what each of them resolves to (M104). Read together: the shape decides
    /// where ingredient `n` goes, the list decides what it shows.
    pub shape: rewo_world::recipe_overlay::Shape,
    pub grid_items: Vec<Vec<i32>>,
}

/// What the search indexes for one recipe's result items (M99).
///
/// `getTooltipLines` over the results, plus their registry keys. For Rewo the
/// tooltip of a bare item id is its **display name** and nothing else, since
/// every other line comes from a component and a recipe's result carries none —
/// so this is exact rather than an approximation, and stops being exact only
/// if results ever arrive with components.
pub(crate) fn search_entry_of(
    results: &[i32],
    items: &rewo_data::items::Items,
    display: &std::collections::HashMap<String, String>,
) -> rewo_world::recipe_search::SearchEntry {
    let mut out = rewo_world::recipe_search::SearchEntry::default();
    for id in results {
        let Some(name) = items.name(*id) else { continue };
        // A missing translation falls back to the id's own path, prettified —
        // the same fallback the tooltip takes, so a search finds what the
        // tooltip shows.
        let shown = display
            .get(name)
            .cloned()
            .unwrap_or_else(|| display_name_of(name));
        out.names.push(shown.to_lowercase());
        let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
        out.ids.push((ns.to_lowercase(), path.to_lowercase()));
    }
    out
}

/// The book's per-frame state, from resolved inputs (M97).
///
/// Split out of [`live_recipe_book`] because a `PlaySession` owns a socket and
/// cannot be built in a test — M71's lesson: logic in a place with no test
/// module is untestable, so move it. M96 shipped this arithmetic graded only at
/// its two ends (the solver's own tests, and the chrome witness), with the
/// derivation between them untested — the M92/M93b shape.
pub(crate) fn book_render_from(
    book: rewo_world::recipe_book_screen::BookType,
    filtering: bool,
    entries: &[BookEntry<'_>],
    held: &mut rewo_world::stacked_contents::StackedContents,
    cycle: i32,
    // M98 — the tab and page a click chose. Clamped here rather than trusted:
    // a tab index survives a book-type change (a furnace book has fewer tabs
    // than a crafting one) and a page survives the list shrinking under it.
    state: rewo_world::recipe_book_screen::BookState,
    // The cursor in book coordinates, for the arrows' and filter's hover art.
    book_mouse: Option<(i32, i32)>,
    // The search field's contents, already lowercased (M99).
    query: &str,
) -> BookRender {
    use rewo_world::recipe_book_screen as rb;
    let flat: Vec<(i32, Option<i32>, &str)> =
        entries.iter().map(|e| (e.id, e.group, e.category)).collect();
    let all = rb::collections(&flat);
    let tabs = book.tabs();
    let selected_tab = state.selected_tab.min(tabs.len() - 1);
    // Stage one of `updateCollections` is the tab's own membership. Tab 0 is
    // the SEARCH tab, whose categories are the book's whole set — so with the
    // selection pinned to 0 this shows everything the book has.
    let wanted = tabs[selected_tab].categories;
    // `updateCollections`' stages, in order (M93z): the tab's membership, then
    // the SEARCH, then the filter. The search stage is skipped entirely on an
    // empty query rather than run with one — see `recipe_search::matches`.
    let mine: Vec<_> = all
        .iter()
        .filter(|c| wanted.contains(&c.category.as_str()))
        .filter(|c| {
            // A collection's searchable text is the union of its recipes',
            // which is what `flatMap` over `getRecipes()` gives.
            let mut e = rewo_world::recipe_search::SearchEntry::default();
            for id in &c.recipes {
                if let Some(src) = entries.iter().find(|x| x.id == *id) {
                    e.names.extend(src.search.names.iter().cloned());
                    e.ids.extend(src.search.ids.iter().cloned());
                }
            }
            rewo_world::recipe_search::matches(&e, query)
        })
        .collect();
    let total_pages = rb::total_pages(mine.len());
    let page = rb::clamp_page(state.page, mine.len(), false);
    let range = rb::page_range(page, mine.len());
    let find = |id: &i32| entries.iter().find(|e| e.id == *id);
    let mut slots = Vec::new();
    let mut slot_items = Vec::new();
    let mut slot_shadowed = Vec::new();
    let mut slot_recipes = Vec::new();
    let mut slot_collections = Vec::new();
    for c in &mine[range] {
        let per_entry: Vec<Vec<i32>> = c
            .recipes
            .iter()
            .map(|id| find(id).map(|e| e.results.clone()).unwrap_or_default())
            .collect();
        let multiple = per_entry.len() > 1;
        // `allRecipesHaveSameResultDisplay` — every display item across every
        // entry the same. Rewo compares item IDS, where vanilla compares whole
        // stacks with `isSameItemSameComponents`; two recipes yielding the same
        // item with different components would shadow here and not in vanilla.
        let mut seen = per_entry.iter().flatten();
        let first = seen.next().copied();
        let same_result = seen.all(|i| Some(*i) == first);
        // Per-recipe affordability, which the which-of-these overlay needs one
        // by one (M104) where the cell needs only whether ANY of them is.
        // A recipe whose `craftingRequirements` were absent is never craftable,
        // which is `canCraft`'s own opening line rather than a default.
        let per_recipe: Vec<bool> = c
            .recipes
            .iter()
            .map(|id| {
                find(id)
                    .and_then(|e| e.ingredients.as_ref())
                    .is_some_and(|ing| held.try_pick(ing, 1))
            })
            .collect();
        // `hasCraftable()` — ANY of them.
        let craftable = per_recipe.iter().any(|&c| c);
        slot_collections.push(
            c.recipes
                .iter()
                .zip(&per_recipe)
                .map(|(&id, &can)| {
                    let (shape, grid) = find(&id)
                        .map(|e| (e.shape, e.grid_items.clone()))
                        .unwrap_or((rewo_world::recipe_overlay::Shape::Other, Vec::new()));
                    rewo_world::recipe_overlay::Button {
                        recipe: id,
                        craftable: can,
                        // `if (!items.isEmpty())` — an ingredient that resolves
                        // to nothing contributes no position, and because
                        // neither placement arm derives its position from a
                        // running counter, dropping one does not shift the rest.
                        slots: rewo_world::recipe_overlay::grid_positions(
                            book.furnace_family(),
                            shape,
                        )
                        .into_iter()
                        .filter_map(|p| {
                            let items = grid.get(p.ingredient).cloned().unwrap_or_default();
                            (!items.is_empty()).then_some((p, items))
                        })
                        .collect(),
                    }
                })
                .collect(),
        );
        slots.push((craftable, multiple));
        slot_items.push(rewo_net::recipe_book::display_item(&per_entry, cycle));
        slot_shadowed.push(multiple && same_result);
        // `getCurrentRecipe` — the entry the cycle is showing, which is what a
        // left-click places. Not the collection's first: clicking while the
        // cycle is on the second of two recipes places the SECOND.
        let n = c.recipes.len() as i32;
        slot_recipes.push(if n == 0 {
            None
        } else {
            c.recipes
                .get((cycle - n * cycle.div_euclid(n)) as usize)
                .copied()
        });
    }
    let view = rb::BookView {
        tabs: tabs.len(),
        selected_tab,
        page,
        total_pages,
        shown: slots.len(),
        filtering,
        furnace_family: book.furnace_family(),
    };
    let hover = match book_mouse.and_then(|(bx, by)| rb::book_hit(bx, by, view, tabs.len())) {
        Some(rb::BookHit::PageForward) => rb::BookHover { page_forward: true, ..Default::default() },
        Some(rb::BookHit::PageBackward) => {
            rb::BookHover { page_backward: true, ..Default::default() }
        }
        Some(rb::BookHit::Filter) => rb::BookHover { filter: true, ..Default::default() },
        _ => rb::BookHover::default(),
    };
    BookRender {
        view: Some(rb::BookView {
            tabs: tabs.len(),
            selected_tab,
            page,
            total_pages,
            shown: slots.len(),
            filtering,
            furnace_family: book.furnace_family(),
        }),
        slots,
        // M98 — from the SAME `book_hit` the press uses, so what lights up is
        // what a click would take. Deriving the hover from its own rects would
        // be two chances to disagree.
        hover,
        book,
        slot_items,
        slot_shadowed,
        slot_recipes,
        slot_collections,
    }
}

/// Build the which-of-these overlay a right-click on page cell `index` opens
/// (M104).
///
/// A free function of plain values rather than a method reaching into the
/// session, so a test can drive it — M97's lesson, and M92's finding one layer
/// on: the arithmetic between "a collection" and "a placed popup" is exactly
/// what neither the model's tests nor a pixel gate can see on its own.
///
/// The `centre` it passes is the one `RecipeBookPage.mouseClicked` passes, and
/// its `+ 13` on the vertical half is *inert* — see
/// `recipe_overlay::origin`'s tests for the proof, and note the constant is
/// still written out rather than dropped.
pub(crate) fn open_overlay(
    collection: Vec<rewo_world::recipe_overlay::Button>,
    index: usize,
    view: rewo_world::recipe_book_screen::BookView,
) -> rewo_world::recipe_overlay::Open {
    use rewo_world::recipe_book_screen as rb;
    use rewo_world::recipe_overlay as ro;
    // Craftable first, and the flag rides along rather than being recomputed:
    // the overlay is a snapshot, so its ordering and its greying are fixed at
    // this moment together.
    let pairs: Vec<(ro::Button, bool)> = collection
        .into_iter()
        .map(|b| {
            let c = b.craftable;
            (b, c)
        })
        .collect();
    let buttons: Vec<ro::Button> =
        ro::promote(&pairs, view.filtering).into_iter().map(|(b, _)| b).collect();
    ro::Open {
        origin: ro::origin(
            rb::grid_slot(index),
            buttons.len(),
            (rb::IMAGE_W / 2, 13 + rb::IMAGE_H / 2),
            ro::BUTTON_PITCH as f32,
        ),
        furnace: view.furnace_family,
        buttons,
    }
}

/// The anvil's name field, as `AnvilScreen.init` builds it (M101).
///
/// `setCanLoseFocus(false)` and `setInitialFocus(this.name)` — focused from the
/// moment the screen opens and unable to lose it, so its caret blinks for as
/// long as the screen is up.
///
/// A function rather than three lines inline, because those three lines sit
/// inside a path that needs a `PlaySession` and so cannot be reached from a
/// test — M93t wrote the comment and did only the first half, and nothing
/// caught it for eight milestones. Here the claim is one call to something
/// graded below.
pub(super) fn anvil_field_new() -> rewo_world::edit_box::EditBox {
    let mut field = rewo_world::edit_box::EditBox::new(rewo_world::anvil::MAX_NAME_LENGTH);
    field.set_can_lose_focus(false);
    field.set_focused(true);
    field
}

/// `scrollTo(cursorPos)` for the book's field (M101).
///
/// Vanilla folds this into `setCursorPosition`; Rewo cannot, because the width
/// function is the caller's — so every input path that moves the cursor calls
/// it. Missing it leaves a field typed past its visible width showing the head
/// of the string with no caret at all.
pub(super) fn follow_cursor(
    field: &mut rewo_world::edit_box::EditBox,
    baked: Option<&assets::BakedAssets>,
    inner_width: i32,
) {
    let Some(font) = baked.and_then(|b| b.font.as_ref()) else {
        return;
    };
    let advance = font.advance;
    let width = move |u: &[u16]| rewo_gpu::text::width(&String::from_utf16_lossy(u), &advance);
    field.follow_cursor(inner_width, &width);
}

/// The search field's background, text, caret and selection (M100).
///
/// # The nine-slice degenerates to two blits
///
/// `widget/text_field` is 200x20 with `border: 1` — but it is a **1-bit
/// paletted** image of exactly two colours: the border 160-grey (white when
/// focused) and the interior black, both fully opaque, measured out of the PNG.
/// Every one of the nine regions is therefore uniform, and a stretched 1x1
/// source is **pixel-identical** to a tiled one.
///
/// So: one blit of the whole rect sampling a border texel, then one of the
/// interior sampling a centre texel. The 1 px the first blit still shows around
/// the second *is* the border. Nine blits would draw the same pixels; two is
/// what the measurement licenses.
pub(super) fn book_field_render(
    field: &rewo_world::edit_box::EditBox,
    advance: &[u8; 256],
    w: f32,
    h: f32,
    now_ms: u64,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
) {
    use rewo_data::assets as a;
    use rewo_world::recipe_book_screen as rb;
    let origin = rewo_gpu::container::recipe_book_origin(w, h);
    // `SPRITES.get(isActive(), isFocused())` — the one use of that record on
    // this screen that means what its argument names say.
    let sprite = a::BOOK_SEARCH_FIELD + usize::from(rb::search_sprite_focused(field.is_focused()));
    let quad = |dx: i32, dy: i32, w: i32, h: i32, sx: f32, sy: f32| {
        (
            sprite,
            rewo_gpu::container::PanelBlit {
                dx: dx as f32,
                dy: dy as f32,
                w: w as f32,
                h: h as f32,
                sx,
                sy,
                // A 1x1 source, stretched — exact because the region is
                // uniform. See the note above.
                sw: 1.0,
                sh: 1.0,
                tint: [1.0; 4],
            },
        )
    };
    let mut fills = vec![
        // The border colour over the whole rect…
        quad(rb::SEARCH_X, rb::SEARCH_Y, rb::SEARCH_W, rb::SEARCH_H, 0.0, 0.0),
        // …then the interior, one pixel in on every side.
        quad(
            rb::SEARCH_X + 1,
            rb::SEARCH_Y + 1,
            rb::SEARCH_W - 2,
            rb::SEARCH_H - 2,
            100.0,
            10.0,
        ),
    ];
    let (labels, text_fills, _) = edit_box_render(
        field,
        advance,
        origin,
        (rb::SEARCH_TEXT_X, rb::SEARCH_TEXT_Y, rb::SEARCH_INNER_W),
        Some((rb::SEARCH_HINT, rb::SEARCH_HINT_COLOR)),
        now_ms,
    );
    fills.extend(text_fills);
    (labels, fills)
}
