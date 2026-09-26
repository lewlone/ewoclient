use super::*;

/// The 46 slot rects the screen draws icons into, in screen pixels.
///
/// The same shape `hotbar_slot_rects` returns, because they feed the same
/// pass: an icon in an inventory slot is the same draw as an icon in a hotbar
/// slot, and only the rectangle differs.
/// The pass's panel description for a menu, or `None` for the player's own.
///
/// This is where `menu_screen`'s sheet-relative UVs are converted back to
/// PIXELS, which is the whole reason `PanelBlit` carries pixels: a screen's
/// UVs are normalised against the sheet size its blit declares — 256 for
/// twenty-one of them and **512 for the merchant** — while the atlas
/// normalises against its own dimensions. Handing the UVs straight across
/// would divide by the wrong number for exactly one screen.
///
/// `None` for a menu with no container screen (`lectern`, a `BookViewScreen`)
/// as well as for the player's, so an open lectern paints no panel rather than
/// some other menu's.
pub(super) fn container_panel(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    open: Option<&rewo_world::menu::OpenMenu>,
    player: EnchantPlayer<'_>,
    mouse_gui: Option<(f64, f64)>,
) -> Option<rewo_gpu::container::ContainerPanel> {
    if layout.protocol_id == rewo_world::menu_layout::NO_PROTOCOL_ID {
        return None;
    }
    let screen = rewo_world::menu_screen::screen_of(layout.protocol_id)?;
    let sheet = sheet_index(screen.texture)?;
    let blits = rewo_world::menu_screen::background_quads(screen)
        .into_iter()
        .map(|q| rewo_gpu::container::PanelBlit {
            dx: q.dx as f32,
            dy: q.dy as f32,
            w: q.w as f32,
            h: q.h as f32,
            sx: q.u0 * screen.sheet_w,
            sy: q.v0 * screen.sheet_h,
            // A background quad samples 1:1 — the panel sheet is drawn at its
            // own scale.
            sw: q.w as f32,
            sh: q.h as f32,
            tint: [1.0; 4],
        })
        .collect();
    Some(rewo_gpu::container::ContainerPanel {
        sheet,
        blits,
        gui_w: screen.image_w as f32,
        gui_h: screen.image_h as f32,
        overlays: {
            let mut o = menu_overlays(layout, open, player, mouse_gui);
            // M103 — the ghost's red wash goes UNDER the icons, so with the
            // screen's own overlays.
            o.extend_from_slice(player.ghost_under);
            o
        },
        front_overlays: player.ghost_over.to_vec(),
    })
}

/// A `menu_screen::ProgressBlit` in the render's own units.
pub(super) fn to_blit(b: rewo_world::menu_screen::ProgressBlit) -> rewo_gpu::container::PanelBlit {
    rewo_gpu::container::PanelBlit {
        dx: b.dx as f32,
        dy: b.dy as f32,
        w: b.w as f32,
        h: b.h as f32,
        sx: b.sx as f32,
        sy: b.sy as f32,
        // M93p — `None` resolves to the destination size, which every overlay
        // before the loom preview used.
        sw: b.source_size().0 as f32,
        sh: b.source_size().1 as f32,
        // M93q — a sprite blit is untinted; `to_fill` is the tinted form.
        tint: [1.0; 4],
    }
}

/// The scroller thumb's blit — 12x15 at `leftPos + 119`, whose y the caller
/// computes from `scrollOffs` (M93s).
pub(super) fn cut_scroller_blit(y: i32) -> rewo_world::menu_screen::ProgressBlit {
    use rewo_world::menu_screen as ms;
    rewo_world::menu_screen::ProgressBlit {
        dx: ms::CUT_SCROLLER_X,
        dy: y,
        w: ms::CUT_SCROLLER_W,
        h: ms::CUT_SCROLLER_H,
        sx: 0,
        sy: 0,
        src: None,
    }
}

/// A solid-colour overlay quad (M93q) — the loom's grey banner backing, and
/// anything else vanilla draws with `GuiGraphics.fill`.
///
/// `rgb` is an **sRGB** 0xRRGGBB, which is how vanilla's colour constants are
/// written; the shader linearises it, the same discipline every Rewo UI pass
/// follows.
pub(super) fn to_fill(
    b: rewo_world::menu_screen::ProgressBlit,
    rgb: u32,
) -> (usize, rewo_gpu::container::PanelBlit) {
    let c = |shift: u32| ((rgb >> shift) & 0xFF) as f32 / 255.0;
    (
        rewo_gpu::container::FILL_SPRITE,
        rewo_gpu::container::PanelBlit {
            tint: [c(16), c(8), c(0), 1.0],
            ..to_blit(b)
        },
    )
}

/// The open menu's enchanting rows, or `None` when it is not that menu.
///
/// **One derivation, two consumers** — the sprite overlays and the cost
/// labels. They must agree about which row is hovered and which is
/// unaffordable, because a row whose background says "available" while its
/// numeral says otherwise is not a state vanilla can produce; deriving it
/// twice gives two chances to disagree (M18's finding, in miniature).
pub(crate) fn enchant_rows_of(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    open: Option<&rewo_world::menu::OpenMenu>,
    player: EnchantPlayer<'_>,
    mouse_gui: Option<(f64, f64)>,
) -> Option<[rewo_world::menu_screen::EnchantRow; 3]> {
    if layout.protocol_id != 13 {
        return None;
    }
    let m = open?;
    Some(rewo_world::menu_screen::enchant_rows(
        m.enchant_costs(),
        m.enchant_lapis(),
        player.xp_level,
        player.creative,
        mouse_gui,
    ))
}

/// An enchanting-row press (M92f). Returns whether the press was taken.
///
/// Mirrors `EnchantmentScreen.mouseClicked`: the row loop runs first, and a
/// row is taken **only if `clickMenuButton` would return true**. A press on a
/// row that fails the gate is *not* consumed — vanilla falls through to
/// `super.mouseClicked`, so the slot logic still gets it.
///
/// The gate is deliberately not the render's: it additionally requires slot 0
/// to hold something, and tests the level against `row + 1` as well as the
/// cost. See `menu_screen::enchant_click_allowed`.
pub(super) fn enchant_press(
    session: &mut PlaySession,
    screen: &ScreenState,
    w: f32,
    h: f32,
) -> bool {
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != 13 {
        return false;
    }
    let (gx, gy) = rewo_gpu::container::screen_to_gui_for(
        screen.mouse,
        w,
        h,
        open.layout.image_w as f32,
        open.layout.image_h as f32,
    );
    let Some(row) = rewo_world::menu_screen::enchant_click_row(gx, gy) else {
        return false;
    };
    let allowed = rewo_world::menu_screen::enchant_click_allowed(
        row,
        open.enchant_costs(),
        open.enchant_lapis(),
        // `enchantSlots.getItem(0)` — the item being enchanted.
        open.menu.menu_slot(0).is_some(),
        session.hud.experience.level,
        session.abilities.instabuild,
    );
    if !allowed {
        return false;
    }
    if let Err(e) = session.container_button_click(row as i32) {
        log::warn!("enchant button {row}: {e}");
    }
    true
}

/// The beacon's live choice, from its data slots (M92).
///
/// **Vanilla's screen keeps its own `primary`/`secondary` fields**, seeded
/// from the menu by a `ContainerListener` on every `dataChanged` and then
/// moved by clicks *before* the server hears about them — a click updates the
/// screen and only `ServerboundSetBeaconPacket` on confirm tells the server.
/// Rewo has no click path here yet, so this reads the data slots directly:
/// the display is correct for whatever the server last said, and the
/// unconfirmed-choice half arrives with the button clicks.
/// `BeaconScreen`'s button press (M93m).
///
/// Placed beside `enchant_press` and called from the same seam, for the same
/// reason: `BeaconScreen.mouseClicked` is `AbstractContainerScreen`'s with
/// widgets in front of it, so a press on a live button consumes the click and
/// never reaches the slot logic. Returns whether it did.
///
/// The confirm sends and then closes; vanilla's order is the same
/// (`send(...)` then `closeContainer()`), and here the send failing must not
/// close the screen over a beacon the server never heard about.
pub(super) fn beacon_press(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    effect_ids: &BeaconEffectIds,
    w: f32,
    h: f32,
) -> bool {
    use rewo_world::menu_screen::BeaconPress;
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != BEACON_MENU_PROTOCOL_ID {
        return false;
    }
    let (gx, gy) = rewo_gpu::container::screen_to_gui_for(
        screen.mouse,
        w,
        h,
        open.layout.image_w as f32,
        open.layout.image_h as f32,
    );
    let choice = beacon_live(screen, open, effect_ids);
    let Some(button) = rewo_world::menu_screen::beacon_buttons()
        .into_iter()
        .find(|b| rewo_world::menu_screen::beacon_button_hovered(*b, gx, gy))
    else {
        return false;
    };
    match rewo_world::menu_screen::beacon_press(button, choice) {
        // An inactive or already-selected button is NOT a consumed click:
        // `AbstractWidget.mouseClicked` only returns true when it fires, so a
        // press on a dark button falls through to the slot logic exactly as a
        // disabled enchanting row does (M92f).
        BeaconPress::None => false,
        BeaconPress::Select(next) => {
            if let Some(b) = screen.beacon.as_mut() {
                b.choice = next;
            }
            true
        }
        BeaconPress::Confirm => {
            let id = |e: Option<rewo_world::menu_screen::BeaconEffect>| {
                e.and_then(|e| effect_ids.id_of(e))
            };
            match session.set_beacon(id(choice.primary), id(choice.secondary)) {
                Ok(()) => screen.close_beacon = true,
                Err(e) => log::warn!("set_beacon: {e}"),
            }
            true
        }
        BeaconPress::Cancel => {
            screen.close_beacon = true;
            true
        }
    }
}

/// `minecraft:menu`'s `beacon` id.
pub(super) const BEACON_MENU_PROTOCOL_ID: i32 = 9;

/// What the loom's pattern grid needs, resolved by the caller (M93q).
///
/// Carried rather than derived in the overlay builder because the pattern list
/// keys off the pattern slot's item NAME, and only the caller holds the
/// registry that turns an id into one — the same reason `ItemProps` is an
/// input to the click arithmetic.
#[derive(Debug, Clone, Copy)]
pub struct LoomView {
    /// `getSelectablePatterns()`.
    pub patterns: &'static [&'static str],
    /// `LoomScreen.startRow` — the first visible row.
    ///
    /// **Always 0 today**: the scrollbar's drag is not wired, so a loom with
    /// more than 16 patterns shows the first sixteen and no more. The field
    /// exists because the index arithmetic needs it and would otherwise have
    /// a 0 baked in where a variable belongs.
    pub start_row: i32,
    /// `getSelectedBannerPatternIndex()`, from data slot 0. `-1` for none.
    pub selected: i32,
    /// `displayPatterns` — the grid is hidden entirely when false.
    pub display: bool,
}

/// `AnvilScreen.keyPressed` and `EditBox.charTyped` (M93t).
///
/// ```java
/// public boolean keyPressed(final KeyEvent event) {
///    if (event.isEscape()) { this.minecraft.player.closeContainer(); return true; }
///    return !this.name.keyPressed(event) && !this.name.canConsumeInput()
///        ? super.keyPressed(event) : true;
/// }
/// ```
///
/// **Read that return carefully.** It falls through to `super` only when the
/// box did *not* handle the key **and** cannot consume input. With the field
/// focused and editable — which it is whenever slot 0 holds something — the
/// second half is false, so **every non-escape key is swallowed**: `E` does not
/// close the anvil, a number key does not swap a hotbar slot, `Q` does not
/// drop. That reads like a bug and is exactly what typing a name requires.
///
/// With slot 0 empty the field is uneditable, `canConsumeInput` is false, and
/// the screen behaves normally again.
///
/// Returns whether the key was consumed.
pub(super) fn anvil_key(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    items: &rewo_data::items::Items,
    // M101 — for `follow_cursor`, whose width function needs the font.
    baked: Option<&assets::BakedAssets>,
    input: rewo_world::edit_box::Input,
    clipboard: &mut String,
) -> bool {
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != ANVIL_MENU_PROTOCOL_ID {
        return false;
    }
    // Escape is handled by the screen's own close path, before the field.
    if input.key == 256 {
        return false;
    }
    let slot0 = open
        .menu
        .menu_slot(0)
        .and_then(|s| items.name(s.item_id).map(|n| (s, n)));
    let handled = {
        let local = anvil_local(screen, open, items);
        let handled = local.field.key_pressed(input, clipboard);
        rewo_world::anvil::key_consumed(handled, local.field.can_consume_input())
    };
    if let Some(a) = screen.anvil.as_mut() {
        follow_cursor(&mut a.field, baked, ANVIL_FIELD.2);
    }
    anvil_flush(session, screen, slot0);
    handled
}

/// A typed character — `EditBox.charTyped`, which the key path never sees
/// because winit reports text separately from the key (M93t).
pub(super) fn anvil_char(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    items: &rewo_data::items::Items,
    baked: Option<&assets::BakedAssets>,
    ch: char,
) -> bool {
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != ANVIL_MENU_PROTOCOL_ID {
        return false;
    }
    let slot0 = open
        .menu
        .menu_slot(0)
        .and_then(|s| items.name(s.item_id).map(|n| (s, n)));
    let handled = anvil_local(screen, open, items).field.char_typed(ch);
    if let Some(a) = screen.anvil.as_mut() {
        follow_cursor(&mut a.field, baked, ANVIL_FIELD.2);
    }
    anvil_flush(session, screen, slot0);
    handled
}

/// Drain the field's responder into `AnvilName::on_name_changed`, and send the
/// packet it asks for.
///
/// The two-stage gate is M93n's and both stages are real: the field fires on
/// every mutation, `on_name_changed` normalises "the item's own name" to the
/// empty string, and `setItemName` refuses to re-send a name the server already
/// has.
pub(super) fn anvil_flush(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    slot0: Option<(rewo_world::inventory::ItemSlot, &str)>,
) {
    let Some(local) = screen.anvil.as_mut() else {
        return;
    };
    let Some(typed) = local.field.take_value_changed() else {
        return;
    };
    let hover = slot0.map(|(_, n)| display_name_of(n));
    let input = slot0.zip(hover.as_deref()).map(|((s, _), hover_name)| {
        rewo_world::anvil::AnvilInput {
            // A stack whose patch carried anything is the closest Rewo gets to
            // `has(CUSTOM_NAME)` without decoding the component's text; the
            // approximation is one-directional, since a patched-but-unnamed
            // stack merely skips the clear-to-default normalisation.
            has_custom_name: s.has_components,
            hover_name,
        }
    });
    if let Some(send) = local.name.on_name_changed(&typed, input) {
        if let Err(e) = session.rename_item(&send) {
            log::warn!("anvil rename {send:?}: {e}");
        }
    }
}

/// `MerchantScreen.mouseClicked` — a trade button, or a scrollbar grab (M93u).
///
/// The button's press sets `shopItem = getIndex() + scrollOff` and then
/// `postButtonClick`, which does three things in order: `setSelectionHint`,
/// `tryMoveItems` and only then the packet. The first two are LOCAL — the
/// trade's items appear in the slots before the server answers — so a click
/// that the server later rejects still moved the screen first.
///
/// The scrollbar grab, like the stonecutter's, does **not** consume the press:
/// vanilla sets `isDragging` and falls through to `super.mouseClicked`.
/// The recipe book's press (M98).
///
/// **First of all**, and with a second rule under it:
///
/// ```java
/// if (this.recipeBookComponent.mouseClicked(...)) { … return true; }
/// else return this.widthTooNarrow && this.recipeBookComponent.isVisible()
///     ? true : super.mouseClicked(...);
/// ```
///
/// So a click the book does not want is still **swallowed** when the window is
/// too narrow and the book is open — the case where the book covers the menu.
/// Letting it fall through there would click a slot the player cannot see.
///
/// Returns whether the press was consumed.
pub(super) fn book_press(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    items: &rewo_data::items::Items,
    // M107 — `event.hasShiftDown()`, which vanilla passes straight through to
    // `useMaxItems`: shift-clicking a recipe places as many as the ingredients
    // allow rather than one.
    shift: bool,
    // M99 — the search's inputs, so the press resolves the SAME page the render
    // did: a search narrows the page, and hit-testing against an unfiltered one
    // would place a recipe the player is not looking at.
    display: &std::collections::HashMap<String, String>,
    right: bool,
    w: f32,
    h: f32,
) -> bool {
    use rewo_world::recipe_book_screen as rb;
    // `!minecraft.player.isSpectator()` — a spectator cannot click the book at
    // all, and the guard is on the whole method rather than on any one widget.
    if session.own_game_mode().is_some_and(|g| g.is_spectator()) {
        return false;
    }
    let (bl, bt, scale) = rewo_gpu::container::recipe_book_origin(w, h);
    let bx = ((screen.mouse.0 - bl as f64) / scale as f64).floor() as i32;
    let by = ((screen.mouse.1 - bt as f64) / scale as f64).floor() as i32;
    // Bound before the overlay branch, because both placement paths need it and
    // one `let` is how they cannot disagree.
    let narrow = rb::width_too_narrow((w / scale) as i32);
    // M104 — an OPEN which-of-these overlay eats every click, wherever it
    // lands. `RecipeBookPage.mouseClicked`'s overlay branch is an unconditional
    // `return true`, so while it is up the whole screen is modal: a click on
    // the page's arrows, on the search box, on the tabs, or on the menu's own
    // slots underneath all reach the overlay and nothing else.
    if let Some(open) = screen.book_overlay.as_ref() {
        match open.click_at(bx, by, right) {
            rewo_world::recipe_overlay::Click::Select(i) => {
                let picked = open.buttons.get(i).map(|b| (b.recipe, b.craftable));
                // **The overlay STAYS OPEN.** The selecting branch does not
                // call `setVisible(false)` — only the else does — so picking a
                // variant leaves the popup up and you can pick another. That
                // reads like an oversight and is what makes the feature usable.
                if let Some((id, craftable)) = picked {
                    place_from_book(session, screen, id, craftable, shift, narrow);
                }
            }
            rewo_world::recipe_overlay::Click::Close => screen.book_overlay = None,
        }
        return true;
    }
    let query = rewo_world::recipe_search::normalize(&screen.book_search.value());
    let Some(view) =
        live_recipe_book(session, items, screen.book, Some((bx, by)), &query, display)
            .and_then(|b| b.view)
    else {
        return false;
    };
    let hit = rb::book_hit(bx, by, view, view.tabs);
    let action = screen.book.press(hit, right);
    // The `EditBox` is the ONLY owner of "is the search focused" — see
    // `focus_change`'s docs for why the duplicate flag it replaced was a bug.
    if let Some(v) = rb::focus_change(hit) {
        screen.book_search.set_focused(v);
    }
    match action {
        Some(rb::BookAction::ToggleFilter) => {
            // `toggleFiltering()` then `sendUpdateSettings()` — the local flag
            // moves first and the packet reports it, which is why this is one
            // call rather than two.
            if let Err(e) = session.toggle_recipe_book_filter(shown_book_index(session))
            {
                log::warn!("rewo: recipe book filter: {e}");
            }
            true
        }
        Some(rb::BookAction::Recipe { index, right }) => {
            let render =
                live_recipe_book(session, items, screen.book, Some((bx, by)), &query, display);
            if right {
                // M104 — open the which-of-these overlay, but only on a cell
                // holding more than one recipe: `!button.isOnlyOption()`.
                //
                // `isOnlyOption` is `size() == 1`, not `size() > 1` negated —
                // an empty collection opens an (empty) overlay in vanilla too.
                let collection = render
                    .as_ref()
                    .and_then(|b| b.slot_collections.get(index))
                    .cloned()
                    .unwrap_or_default();
                if collection.len() != 1 {
                    screen.book_overlay = Some(open_overlay(collection, index, view));
                }
            } else {
                let picked = render.as_ref().and_then(|b| {
                    let id = (*b.slot_recipes.get(index)?)?;
                    // **`isCraftable(recipe)`, not `hasCraftable()`.** The
                    // cell's `(craftable, multiple)` pair carries the
                    // COLLECTION's answer, which is true when ANY of its
                    // recipes can be made — so on a group holding one craftable
                    // and one not, using it would let the uncraftable one be
                    // clicked forever. The per-recipe flag rides on the same
                    // `Button`s the which-of-these overlay is built from.
                    let craftable = b
                        .slot_collections
                        .get(index)?
                        .iter()
                        .find(|btn| btn.recipe == id)
                        .is_some_and(|btn| btn.craftable);
                    Some((id, craftable))
                });
                if let Some((id, craftable)) = picked {
                    place_from_book(session, screen, id, craftable, shift, narrow);
                }
            }
            true
        }
        Some(rb::BookAction::Navigated) => true,
        // Missed the book — swallowed anyway on a narrow window.
        None => narrow,
    }
}

/// `RecipeBookComponent.tryPlaceRecipe` and what its caller does with the
/// answer (M107).
///
/// Both placement paths — a page cell and a which-of-these button — funnel
/// through here, because vanilla has one `tryPlaceRecipe` and the guard is
/// stateful: two copies would each hold half the history and neither would
/// suppress correctly.
///
/// Three things happen on a successful placement, and only the packet is
/// obvious:
///
/// * **The ghost is cleared first.** `ghostSlots.clear()` runs before
///   `handlePlaceRecipe`, so a failed placement leaves the screen briefly with
///   no ghost until the server's reply refills it. Keeping the old one would
///   show the previous recipe's ingredients against the new request.
/// * **The book closes on a narrow window** — `if (!isOffsetNextToMainGUI())
///   setVisible(false)`, and `xOffset` is 0 exactly when the window is too
///   narrow. There the book covers the menu you are about to look at.
/// * `lastRecipe`/`lastRecipeCollection` are recorded for the Enter-key
///   re-place. **Rewo has no key path into the book yet**, so those are not
///   modelled; what matters here is that a SUPPRESSED click must not record
///   them either, and it does not, because it never gets past the guard.
///
/// Returns whether the click placed.
pub(super) fn place_from_book(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    recipe: i32,
    craftable: bool,
    use_max_items: bool,
    narrow: bool,
) -> bool {
    let effects = rewo_world::recipe_book_screen::place_effects(
        &mut screen.place_guard,
        recipe,
        craftable,
        use_max_items,
        narrow,
    );
    let Some(use_max_items) = effects.send else {
        return false;
    };
    if effects.clear_ghost {
        session.ghost_recipe = None;
    }
    if let Err(e) = session.place_recipe(recipe, use_max_items) {
        log::warn!("rewo: place_recipe: {e}");
    }
    if effects.close_book {
        // `setVisible(false)` — which also shuts the which-of-these overlay
        // (`recipeBookPage.setInvisible()`) and tells the server.
        screen.book_overlay = None;
        let book = shown_book_index(session);
        session.set_recipe_book_open(book, false);
        if let Err(e) = session.recipe_book_change_settings(book) {
            log::warn!("rewo: recipe book settings: {e}");
        }
    }
    true
}

/// Which of the four `RecipeBookSettings` slots the shown menu reads (M98).
pub(super) fn shown_book_index(session: &PlaySession) -> usize {
    book_type_of(session.shown_menu().layout()).map_or(0, |b| b.index())
}

pub(super) fn merchant_press(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    w: f32,
    h: f32,
) -> bool {
    use rewo_world::merchant_screen as ms;
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != ms::MERCHANT_MENU_PROTOCOL_ID {
        return false;
    }
    let n = session.merchant.as_ref().map_or(0, |m| m.offers.len());
    let (gx, gy) = rewo_gpu::container::screen_to_gui_for(
        screen.mouse,
        w,
        h,
        open.layout.image_w as f32,
        open.layout.image_h as f32,
    );
    let scroll_off = screen.merchant.map_or(0, |l| l.scroll_off);
    // The grab is tested FIRST in vanilla and does not return; the button is
    // an ordinary widget press, which happens inside `super`.
    if ms::can_scroll(n) && ms::scroller_grabbed(gx, gy) {
        if let Some(l) = screen.merchant.as_mut() {
            l.dragging = true;
        }
    }
    let Some(button) = ms::button_at(gx, gy) else {
        return false;
    };
    let offer = ms::offer_for_button(button, scroll_off);
    // A button past the end of the list is drawn but dead — vanilla hides the
    // widget (`visible = false`) rather than disabling it, so there is nothing
    // there to press.
    if offer as usize >= n {
        return false;
    }
    if let Some(l) = screen.merchant.as_mut() {
        // `postButtonClick` sets `shopItem` LOCALLY first — the trade's items
        // appear before the server answers.
        l.selected = offer;
    }
    if let Err(e) = session.select_trade(offer) {
        log::warn!("select_trade {offer}: {e}");
    }
    true
}

/// What the merchant's trade list needs, resolved by the caller (M93u).
#[derive(Debug, Clone)]
pub struct MerchantView {
    /// The offers as sent, in the order the click's index addresses.
    pub offers: Vec<rewo_net::merchant::MerchantOffer>,
    /// `MerchantScreen.scrollOff` — an **offer index**, not a fraction.
    pub scroll_off: i32,
    /// Each offer's modified cost-A count, resolved here because the clamp's
    /// ceiling is the item's own max stack size and only this side holds the
    /// item table.
    pub cost_a_counts: Vec<i32>,
    /// `MerchantScreen.shopItem` — the selected offer, whose out-of-stock X
    /// is drawn in the right-hand panel. Screen-local: the packet does not
    /// carry a selection.
    pub selected: i32,
    /// `getTraderLevel` / `getTraderXp`, straight off the packet.
    pub level: i32,
    pub xp: i32,
    /// `showProgressBar()` — false for a wandering trader, which has no level
    /// and no bar.
    pub show_progress: bool,
    /// `getFutureTraderXp` — the xp the currently-matched offer would grant.
    ///
    /// **Derived here rather than received**: no packet carries it.
    /// `MerchantContainer.updateSellItem` matches the payment slots against
    /// the offers and takes the matched offer's xp, and the client holds both
    /// halves. See `merchant_future_xp` for the one case it declines.
    pub future_xp: i32,
}

/// `MerchantContainer.updateSellItem`'s `futureXp` (M93v).
///
/// Vanilla derives it: the payment slots are matched against the offers by
/// `getRecipeFor`, and the matched offer's xp is what the bar's result segment
/// shows. Both halves are on the client, so this is a derivation and not a
/// gap — the same shape as M93u's four class-C corrections one level down.
///
/// **The one case it declines.** `ItemCost.test` is `stack.is(item) &&
/// components.test(stack)`, and the second half is a
/// `DataComponentExactPredicate` — per-component *values*, where M41 gives
/// Rewo a digest of the whole patch. So a **constrained** cost cannot be
/// evaluated, and an offer carrying one is treated as unmatched. The
/// consequence is narrow and one-directional: the result segment is missing
/// where vanilla would show it, never present where vanilla would not. Vanilla
/// villager trades are plain items, so in practice this is the enchanted-book
/// and dyed-armour tail.
///
/// The slot order is `MerchantContainer`'s own: **if slot 0 is empty, slot 1
/// becomes `buyA` and `buyB` is empty** — so paying with only the second slot
/// still matches a one-item trade.
pub(super) fn merchant_future_xp(
    m: &rewo_world::menu::OpenMenu,
    offers: &rewo_net::merchant::MerchantOffers,
    selected: i32,
    props: &dyn Fn(i32) -> i32,
) -> i32 {
    let slot = |i: usize| m.menu.menu_slot(i).map(|s| (s.item_id, s.count));
    let matches: Vec<rewo_world::merchant_screen::OfferMatch> = offers
        .offers
        .iter()
        .map(|o| rewo_world::merchant_screen::OfferMatch {
            cost_a_item: o.cost_a.item_id,
            need_a: o.modified_cost_a(props(o.cost_a.item_id)),
            cost_b: o.cost_b.as_ref().map(|c| (c.item_id, c.count)),
            constrained: o.cost_a.constrained
                || o.cost_b.as_ref().is_some_and(|c| c.constrained),
        })
        .collect();
    let satisfied = rewo_world::merchant_screen::satisfied_offers(&matches, slot(0), slot(1));
    rewo_world::merchant_screen::recipe_for(selected, &satisfied)
        .map_or(0, |i| offers.offers[i].xp)
}

/// The merchant screen's scroll, which no packet carries (M93u).
#[derive(Debug, Clone, Copy, Default)]
pub struct MerchantLocal {
    pub(super) container_id: i32,
    pub scroll_off: i32,
    pub selected: i32,
    pub dragging: bool,
}

/// Resolve the view, seeding the scroll on a new container.
pub(crate) fn merchant_view(
    screen: &mut ScreenState,
    m: &rewo_world::menu::OpenMenu,
    offers: &rewo_net::merchant::MerchantOffers,
    props: &dyn Fn(i32) -> i32,
) -> MerchantView {
    let stale = screen.merchant.is_none_or(|l| l.container_id != m.container_id);
    if stale {
        screen.merchant = Some(MerchantLocal {
            container_id: m.container_id,
            scroll_off: 0,
            selected: 0,
            dragging: false,
        });
    }
    let local = screen.merchant.expect("just seeded");
    MerchantView {
        level: offers.villager_level,
        xp: offers.villager_xp,
        show_progress: offers.show_progress,
        future_xp: merchant_future_xp(m, offers, local.selected, props),
        cost_a_counts: offers
            .offers
            .iter()
            .map(|o| o.modified_cost_a(props(o.cost_a.item_id)))
            .collect(),
        offers: offers.offers.clone(),
        // Clamped on read rather than on write: the list can SHRINK under a
        // held scroll when the villager restocks, and vanilla's own guard is
        // `offer_visible`'s `!canScroll` short-circuit rather than a stored
        // clamp.
        selected: local.selected,
        scroll_off: local
            .scroll_off
            .min(rewo_world::merchant_screen::max_scroll_off(offers.offers.len()).max(0)),
    }
}

/// `minecraft:menu`'s `anvil` id.
pub const ANVIL_MENU_PROTOCOL_ID: i32 = 8;

/// The anvil screen's name field and the name it has sent (M93t).
#[derive(Debug, Clone, Default)]
pub struct AnvilLocal {
    pub(super) container_id: i32,
    /// Slot 0 as last seen, so `slotChanged` fires exactly when vanilla's does.
    pub(super) slot0: Option<(i32, u64)>,
    pub field: rewo_world::edit_box::EditBox,
    pub name: rewo_world::anvil::AnvilName,
}

/// `AnvilScreen.subInit` + `slotChanged` — seed the field, and re-seed it
/// whenever slot 0 changes (M93t).
///
/// ```java
/// public void slotChanged(container, slotIndex, itemStack) {
///    if (slotIndex == 0) {
///       this.name.setValue(itemStack.isEmpty() ? "" : itemStack.getHoverName().getString());
///       this.name.setEditable(!itemStack.isEmpty());
///       this.setFocused(this.name);
///    }
/// }
/// ```
///
/// So an **empty** input slot leaves an empty, UNEDITABLE field — and that
/// matters for far more than the text, because `AnvilScreen.keyPressed` falls
/// through to `super` only when the box can consume nothing. See `anvil_key`.
pub(super) fn anvil_local<'a>(
    screen: &'a mut ScreenState,
    m: &rewo_world::menu::OpenMenu,
    items: &rewo_data::items::Items,
) -> &'a mut AnvilLocal {
    let slot0 = m.menu.menu_slot(0).map(|s| (s.item_id, s.components));
    let stale = match &screen.anvil {
        Some(a) => a.container_id != m.container_id || a.slot0 != slot0,
        None => true,
    };
    if stale {
        let mut field = anvil_field_new();
        let hover = slot0
            .and_then(|(id, _)| items.name(id))
            .map(display_name_of)
            .unwrap_or_default();
        field.set_value(&hover);
        field.set_editable(slot0.is_some());
        let mut local = AnvilLocal {
            container_id: m.container_id,
            slot0,
            field,
            name: screen
                .anvil
                .as_ref()
                .filter(|a| a.container_id == m.container_id)
                .map(|a| a.name.clone())
                .unwrap_or_default(),
        };
        // The seeding `setValue` fires the responder, as vanilla's does; drain
        // it so the re-seed does not send a rename of the name we were just
        // told.
        let _ = local.field.take_value_changed();
        screen.anvil = Some(local);
    }
    screen.anvil.as_mut().expect("just seeded")
}

/// A stack's `getHoverName().getString()` for the seed — the item's display
/// name, which for an un-renamed stack is its translated name.
///
/// **An approximation, and a recorded one**: Rewo resolves the display name
/// from the item registry, so a stack carrying a `custom_name` component seeds
/// the field with its *default* name instead. M41 decodes the component's
/// bytes but not its text, which is the same wall the tooltip's name override
/// hits. The consequence is narrow — the field starts on the wrong string for
/// an already-renamed item — and it does not reach the wire, because
/// `on_name_changed` compares against what the server was told.
pub(super) fn display_name_of(id: &str) -> String {
    id.rsplit(':')
        .next()
        .unwrap_or(id)
        .split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the stonecutter's recipe grid needs, resolved by the caller (M93s).
///
/// Carried rather than derived in the overlay builder for `LoomView`'s reason:
/// the list keys off the input slot's item NAME, and only this side holds the
/// registry that turns an id into one.
#[derive(Debug, Clone)]
pub struct CutView {
    /// `getVisibleRecipes()` — `selectByInput` of the input slot, **in master
    /// order**, because the index a click sends indexes this.
    pub recipes: Vec<&'static rewo_data::stonecutter_table::Cut>,
    /// `StonecutterScreen.startIndex` — the first visible recipe. A multiple
    /// of 4, since the grid scrolls by whole rows.
    pub start_index: i32,
    /// `getSelectedRecipeIndex()`, from data slot 0.
    pub selected: i32,
    /// `scrollOffs`, for the thumb's position.
    pub scroll_offs: f32,
    /// `displayRecipes` — the grid, the icons and the scrollbar are all hidden
    /// when false.
    pub display: bool,
}

/// The stonecutter screen's scroll, which is **screen-local**: no packet
/// carries it, and vanilla resets it in `containerChanged` (M93s).
#[derive(Debug, Clone, Copy)]
pub struct CutLocal {
    pub(super) container_id: i32,
    /// The input slot as last seen. `containerChanged` is registered as the
    /// menu's update listener and fires on **any** change to the input
    /// container — so taking one block off a stack resets the scroll even
    /// though `slotsChanged` rebuilds the recipe list only when the item TYPE
    /// changes. Two granularities on one event, and this is the screen's.
    pub(super) input: Option<(i32, i32)>,
    pub(super) scroll_offs: f32,
    /// Whether the thumb is being dragged. Cleared on any release.
    pub scrolling: bool,
}

/// `StonecutterScreen.mouseClicked` (M93s).
///
/// ```java
/// if (this.displayRecipes) {
///    for (int index = this.startIndex; index < endIndex; index++) {
///       … if (hit && this.menu.clickMenuButton(player, index)) {
///          play(UI_STONECUTTER_SELECT_RECIPE);
///          this.minecraft.gameMode.handleInventoryButtonClick(this.menu.containerId, index);
///          return true;
///       }
///    }
///    if (over the scrollbar) this.scrolling = true;   // and does NOT return
/// }
/// return super.mouseClicked(event, doubleClick);
/// ```
///
/// So a recipe consumes the press and a scrollbar grab does not — the grab
/// falls through to the slot logic, which finds nothing under the bar.
/// Returns whether the press was consumed.
pub(super) fn cut_press(
    session: &mut PlaySession,
    screen: &mut ScreenState,
    items: &rewo_data::items::Items,
    w: f32,
    h: f32,
) -> bool {
    use rewo_world::menu_screen as ms;
    let Some(open) = session.menus.open() else {
        return false;
    };
    if open.layout.protocol_id != ms::STONECUTTER_MENU_PROTOCOL_ID {
        return false;
    }
    let view = cut_view(screen, open, items);
    if !view.display {
        // The whole block is inside `if (this.displayRecipes)`, so with no
        // grid there is no recipe click AND no scrollbar grab.
        return false;
    }
    let (gx, gy) = rewo_gpu::container::screen_to_gui_for(
        screen.mouse,
        w,
        h,
        open.layout.image_w as f32,
        open.layout.image_h as f32,
    );
    if let Some(index) = ms::cut_cell_click_at(gx, gy, view.start_index) {
        // `isValidRecipeIndex` is the server's gate and the screen's: vanilla
        // calls `clickMenuButton` inside the hit test, so an out-of-range cell
        // does not consume the press either.
        if ms::cut_click_accepted(index, view.recipes.len()) {
            if let Err(e) = session.container_button_click(index) {
                log::warn!("stonecutter recipe {index}: {e}");
            }
            return true;
        }
    }
    if ms::cut_scroller_grabbed(gx, gy) {
        if let Some(c) = screen.cut.as_mut() {
            c.scrolling = true;
        }
    }
    false
}

/// The whole stonecutter view, resolved from the menu plus the screen's own
/// scroll — the caller's job, exactly as `beacon_live` is (M93s).
pub(crate) fn cut_view(
    screen: &mut ScreenState,
    m: &rewo_world::menu::OpenMenu,
    items: &rewo_data::items::Items,
) -> CutView {
    // Slots: input 0, result 1, then the player's 36.
    let name = m.menu.menu_slot(0).and_then(|s| items.name(s.item_id));
    let recipes = name.map_or_else(Vec::new, rewo_data::stonecutter_table::select_by_input);
    let local = cut_local(screen, m);
    let display = rewo_world::menu_screen::cut_display_recipes(name.is_some(), recipes.len());
    CutView {
        start_index: if display {
            rewo_world::menu_screen::cut_start_index(local.scroll_offs, recipes.len())
        } else {
            0
        },
        selected: m.data(0) as i32,
        scroll_offs: local.scroll_offs,
        display,
        recipes,
    }
}

/// `StonecutterScreen`'s scroll, seeded at 0 and owned by the screen until the
/// input changes — the beacon's shape (M93m), with a different reset trigger.
pub(super) fn cut_local(screen: &mut ScreenState, m: &rewo_world::menu::OpenMenu) -> CutLocal {
    let input = m.menu.menu_slot(0).map(|s| (s.item_id, s.count));
    let stale = match screen.cut {
        Some(c) => c.container_id != m.container_id || c.input != input,
        None => true,
    };
    if stale {
        screen.cut = Some(CutLocal {
            container_id: m.container_id,
            input,
            scroll_offs: 0.0,
            scrolling: false,
        });
    }
    screen.cut.expect("just seeded")
}

/// The beacon screen's local choice and the watermarks it was seeded at.
#[derive(Debug, Clone, Copy)]
pub struct BeaconLocal {
    pub(super) container_id: i32,
    pub(super) data_writes: u64,
    pub(super) choice: rewo_world::menu_screen::BeaconChoice,
}

/// `BeaconScreen`'s live choice — seeded from the menu, then owned by the
/// screen until the menu says otherwise (M93m).
pub(super) fn beacon_live(
    screen: &mut ScreenState,
    m: &rewo_world::menu::OpenMenu,
    effect_ids: &BeaconEffectIds,
) -> rewo_world::menu_screen::BeaconChoice {
    let stale = match screen.beacon {
        Some(b) => b.container_id != m.container_id || b.data_writes != m.data_writes,
        None => true,
    };
    if stale {
        screen.beacon = Some(BeaconLocal {
            container_id: m.container_id,
            data_writes: m.data_writes,
            choice: beacon_choice(m, effect_ids),
        });
    }
    // `levels` and `has_payment` are the MENU's on every frame, not the
    // screen's: `updateStatus(levels)` is passed the menu's value each time
    // and `hasPayment()` reads the slot directly. Only the two effects are
    // screen-owned, so a payment arriving mid-selection lights Confirm
    // without disturbing the pick.
    let mut c = screen.beacon.expect("seeded above").choice;
    c.levels = m.beacon_levels();
    c.has_payment = m.beacon_has_payment();
    c
}

pub(super) fn beacon_choice(
    m: &rewo_world::menu::OpenMenu,
    effect_ids: &BeaconEffectIds,
) -> rewo_world::menu_screen::BeaconChoice {
    rewo_world::menu_screen::BeaconChoice {
        levels: m.beacon_levels(),
        primary: m.beacon_primary().and_then(|id| effect_ids.of(id)),
        secondary: m.beacon_secondary().and_then(|id| effect_ids.of(id)),
        has_payment: m.beacon_has_payment(),
    }
}

/// The six beacon effects' `minecraft:mob_effect` registry ids.
///
/// From `rewo_data`'s report-backed table (M92c) — by NAME, never by position.
/// An unresolvable name leaves that slot `None` and the effect simply does not
/// match, which shows as a button with no icon rather than the *wrong* icon.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BeaconEffectIds(pub(super) [Option<i32>; 6]);

impl BeaconEffectIds {
    pub(crate) fn resolve(m: &rewo_data::mob_effects::MobEffects) -> Self {
        use rewo_world::menu_screen::BeaconEffect;
        Self(std::array::from_fn(|i| m.id_of(BeaconEffect::ALL[i].name())))
    }

    /// The registry id of one of the six, for `set_beacon` (M93m).
    pub(super) fn id_of(&self, e: rewo_world::menu_screen::BeaconEffect) -> Option<i32> {
        let i = rewo_world::menu_screen::BeaconEffect::ALL
            .iter()
            .position(|x| *x == e)?;
        self.0[i]
    }

    /// Which of the six a registry id is, or `None` for any other effect.
    pub(super) fn of(&self, id: i32) -> Option<rewo_world::menu_screen::BeaconEffect> {
        self.0
            .iter()
            .position(|e| *e == Some(id))
            .map(|i| rewo_world::menu_screen::BeaconEffect::ALL[i])
    }
}

/// What an enchanting-table row's state selects, as `(row sprite, numeral)`.
///
/// Kept beside the overlay builder rather than in `rewo-world` because the
/// indices are `rewo-data`'s and the state is `rewo-world`'s; this is the one
/// place both are in scope.
/// **The numeral goes through `EnchantRow::numeral()`, not a second match.**
/// This function had its own copy of that mapping, and a mutation found the
/// duplication: emptying `numeral()` changed nothing rendered, because nothing
/// rendered was reading it. That is M18's finding and M45's in one — a second
/// derivation of the same fact is a second chance to disagree, and here it was
/// also a witness grading a function the app did not call.
pub(super) fn enchant_row_sprites(
    i: usize,
    row: rewo_world::menu_screen::EnchantRow,
) -> (usize, Option<usize>) {
    use rewo_data::assets as a;
    use rewo_world::menu_screen::EnchantRow;
    let background = match row {
        // `cost == 0` blits the disabled background and RETURNS; an
        // unaffordable offer blits the SAME one and then its numeral.
        EnchantRow::Empty | EnchantRow::Unaffordable { .. } => a::ENCHANT_ROW_DISABLED,
        EnchantRow::Available { .. } => a::ENCHANT_ROW,
        EnchantRow::Hovered { .. } => a::ENCHANT_ROW_HIGHLIGHTED,
    };
    let numeral = row
        .numeral()
        .map(|greyed| if greyed { a::ENCHANT_LEVEL_DISABLED } else { a::ENCHANT_LEVEL } + i);
    (background, numeral)
}

/// Everything an open menu paints over its background sheet, in draw order
/// (M91 the furnaces, M92 the rest).
///
/// Empty for a menu with no overlays *and* for one that is not open — a
/// `containershot` panel built with no `OpenMenu` has no data slots to read,
/// and inventing a plausible-looking half-lit furnace would make the gate's
/// panel witnesses grade a state no server ever sent.
///
/// `player` carries the two inputs the enchanting table needs that are not the
/// menu's at all — the local XP level and the creative flag.
pub(super) fn menu_overlays(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    open: Option<&rewo_world::menu::OpenMenu>,
    player: EnchantPlayer<'_>,
    mouse_gui: Option<(f64, f64)>,
) -> Vec<(usize, rewo_gpu::container::PanelBlit)> {
    use rewo_data::assets as a;
    let mut out = Vec::new();
    let Some(m) = open else { return out };
    match layout.protocol_id {
        // The furnace family (M91): flame then arrow.
        id if a::progress_index(id).is_some() => {
            let base = a::progress_index(id).expect("guarded above");
            let (flame, arrow) = rewo_world::menu_screen::furnace_progress(
                m.furnace_is_lit(),
                m.furnace_lit_progress(),
                m.furnace_burn_progress(),
            );
            // The lit sprite is the pair's first, the burn its second.
            if let Some(f) = flame {
                out.push((base, to_blit(f)));
            }
            out.push((base + 1, to_blit(arrow)));
        }
        // loom (M93q): for each visible cell, the button chrome then the
        // banner preview — a grey fill under an untinted pattern.
        rewo_world::menu_screen::LOOM_MENU_PROTOCOL_ID => {
            let Some(l) = player.loom.filter(|l| l.display) else {
                return out;
            };
            for row in 0..rewo_world::menu_screen::LOOM_ROWS {
                for col in 0..rewo_world::menu_screen::LOOM_COLS {
                    let index = (row + l.start_row) * rewo_world::menu_screen::LOOM_COLS + col;
                    // `break label82` — the grid stops at the end of the list
                    // rather than drawing empty cells.
                    let Some(pattern) = usize::try_from(index)
                        .ok()
                        .and_then(|i| l.patterns.get(i))
                    else {
                        break;
                    };
                    let (cx, cy) = rewo_world::menu_screen::loom_cell_origin(row, col);
                    let cell = rewo_world::menu_screen::ProgressBlit {
                        dx: cx,
                        dy: cy,
                        w: rewo_world::menu_screen::LOOM_CELL,
                        h: rewo_world::menu_screen::LOOM_CELL,
                        sx: 0,
                        sy: 0,
                        src: None,
                    };
                    let hovered = mouse_gui.is_some_and(|(x, y)| {
                        rewo_world::menu_screen::loom_cell_at(x, y, l.start_row) == Some(index)
                    });
                    out.push((
                        a::LOOM_PATTERN_CHROME
                            + if index == l.selected {
                                0
                            } else if hovered {
                                1
                            } else {
                                2
                            },
                        to_blit(cell),
                    ));
                    // The grey backing FIRST, then the pattern over it. The
                    // order is the whole reason fills share the sprites' list.
                    out.push(to_fill(
                        rewo_world::menu_screen::loom_preview_backing(cx, cy),
                        rewo_world::menu_screen::LOOM_PREVIEW_BACKING,
                    ));
                    if let Some(sprite) = a::banner_pattern_overlay(pattern) {
                        out.push((
                            sprite,
                            to_blit(rewo_world::menu_screen::loom_pattern_preview(cx, cy)),
                        ));
                    }
                }
            }
        }
        // merchant (M93u): the scroller, then per visible offer a trade arrow.
        // The three ITEMS per row are icons and go through the GUI-item pass.
        rewo_world::merchant_screen::MERCHANT_MENU_PROTOCOL_ID => {
            use rewo_world::merchant_screen as ms;
            let Some(v) = player.merchant else { return out };
            let n = v.offers.len();
            if let Some(y) = ms::scroller_y(v.scroll_off, n) {
                out.push((
                    a::VILLAGER_SCROLLER + usize::from(!ms::can_scroll(n)),
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: ms::SCROLL_X,
                        dy: y,
                        w: ms::SCROLLER_W,
                        h: ms::SCROLLER_H,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
            // The trade buttons FIRST (M93x) — `addRenderableWidget` puts them
            // in the widget layer, which `extractBackground` has already run
            // under. Drawn after the arrow they would cover it.
            //
            // A row past the end of the list draws nothing: vanilla toggles
            // `visible`, not `active`, so there is no greyed button.
            for (i, _) in v.offers.iter().enumerate() {
                let i = i as i32;
                if !ms::offer_visible(i, v.scroll_off, n) {
                    continue;
                }
                let row = if ms::can_scroll(n) { i - v.scroll_off } else { i };
                let hovered = mouse_gui.is_some_and(|(x, y)| ms::button_hovered(row, x, y));
                for sl in ms::button_slices(ms::TRADE_BUTTON_W) {
                    if sl.w == 0 {
                        continue;
                    }
                    out.push((
                        a::WIDGET_BUTTON + usize::from(hovered),
                        to_blit(rewo_world::menu_screen::ProgressBlit {
                            dx: ms::TRADE_BUTTON_X + sl.dx,
                            dy: ms::button_y(row),
                            w: sl.w,
                            h: ms::TRADE_BUTTON_H,
                            sx: sl.sx,
                            sy: 0,
                            // 1:1 — the source size equals the destination, so
                            // this is a tile and not a scale.
                            src: None,
                        }),
                    ));
                }
            }
            for (i, offer) in v.offers.iter().enumerate() {
                let i = i as i32;
                if !ms::offer_visible(i, v.scroll_off, n) {
                    continue;
                }
                // The row is the offer's position in the WINDOW, which is the
                // offer index only when nothing is scrolled.
                let row = if ms::can_scroll(n) { i - v.scroll_off } else { i };
                let y = ms::row_item_y(row);
                // `xo + 5 + 35 + 20` — past cost B, not past cost A. The first
                // cut of this read `5 + 5 + 20` and put every arrow 30 px left,
                // on top of the cost-A icon.
                out.push((
                    a::VILLAGER_TRADE_ARROW + usize::from(offer.out_of_stock),
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: ms::COST_B_X + 20,
                        dy: y + 3,
                        w: 10,
                        h: 9,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
            // The discount strikethrough (M93w), through the FIRST number.
            for (i, offer) in v.offers.iter().enumerate() {
                let idx = i as i32;
                if !ms::offer_visible(idx, v.scroll_off, n) {
                    continue;
                }
                if !ms::cost_a_display(offer.cost_a.count, v.cost_a_counts[i]).strikethrough {
                    continue;
                }
                let row = if ms::can_scroll(n) { idx - v.scroll_off } else { idx };
                out.push((
                    a::VILLAGER_STRIKETHROUGH,
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: ms::COST_A_X + ms::STRIKETHROUGH_DX,
                        dy: ms::row_item_y(row) + ms::STRIKETHROUGH_DY,
                        w: ms::STRIKETHROUGH_W,
                        h: ms::STRIKETHROUGH_H,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
            // The XP bar (M93v): background, then the fill, then the result
            // segment — which samples the sprite from `w` rather than 0, so it
            // CONTINUES the gradient where the fill stopped.
            //
            // `showProgressBar()` gates it: a wandering trader has no level.
            if v.show_progress {
                if let Some((fill, future)) = ms::xp_bar(v.level, v.xp, v.future_xp) {
                    let bar = |dx: i32, sx: i32, w: i32| rewo_world::menu_screen::ProgressBlit {
                        dx,
                        dy: ms::XP_BAR_Y,
                        w,
                        h: ms::XP_BAR_H,
                        sx,
                        sy: 0,
                        // `None` — the source size EQUALS the destination, so
                        // this is a 1:1 slice at `sx` and not a scale.
                        //
                        // `blitSprite(sprite, 102, 5, u, v, x, y, w, h)` passes
                        // the SHEET's size, not the source rect's: the rect is
                        // `w x h` at `(u, v)`. Setting `src` to (102, 5) — the
                        // obvious reading of those two arguments — squeezes the
                        // whole bar into the segment, which a mutation caught
                        // by NOT dying: the `sx` offset had no visible effect
                        // because every segment was showing the entire sprite.
                        src: None,
                    };
                    out.push((
                        a::VILLAGER_XP_BAR,
                        to_blit(bar(ms::XP_BAR_X, 0, ms::XP_BAR_W)),
                    ));
                    if fill > 0 {
                        out.push((a::VILLAGER_XP_BAR + 1, to_blit(bar(ms::XP_BAR_X, 0, fill))));
                    }
                    if future > 0 {
                        out.push((
                            a::VILLAGER_XP_BAR + 2,
                            to_blit(bar(ms::XP_BAR_X + fill, fill, future)),
                        ));
                    }
                }
            }
            // The 28x21 red X is NOT per row — `extractButtonArrows` only
            // swaps the arrow. It belongs to the SELECTED offer, in the
            // right-hand trading panel at `leftPos + 83 + 99`.
            if let Some(sel) = usize::try_from(v.selected)
                .ok()
                .and_then(|i| v.offers.get(i))
                .filter(|o| o.out_of_stock)
            {
                let _ = sel;
                out.push((
                    a::VILLAGER_OUT_OF_STOCK,
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: 182,
                        dy: 35,
                        w: 28,
                        h: 21,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
        }
        // anvil (M93t): the name field's cursor and selection, measured by
        // `anvil_field_render` alongside the text so the two cannot disagree
        // about where the run ends.
        ANVIL_MENU_PROTOCOL_ID => {
            // `extractBackground` blits the field's own 110x16 background at
            // (59, 20) — over a RED placeholder baked into `anvil.png`, so
            // this is chrome the screen cannot omit. The pair is chosen by the
            // same slot-0 predicate that makes the field editable.
            let has_input = m.menu.menu_slot(0).is_some();
            out.push((
                a::ANVIL_TEXT_FIELD + usize::from(!has_input),
                to_blit(rewo_world::menu_screen::ProgressBlit {
                    dx: 59,
                    dy: 20,
                    w: 110,
                    h: 16,
                    sx: 0,
                    sy: 0,
                    src: None,
                }),
            ));
            // Then the field's own cursor and selection, over it.
            out.extend_from_slice(player.anvil_fills);
            // `extractErrorIcon` — an input present and NO result, which is
            // the combination the anvil refused.
            if (has_input || m.menu.menu_slot(1).is_some()) && m.menu.menu_slot(2).is_none() {
                out.push((
                    a::ANVIL_ERROR,
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: 99,
                        dy: 45,
                        w: 28,
                        h: 21,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
        }
        // stonecutter (M93s): the scroller, then one button chrome per visible
        // cell. The result ICONS are not here — they are items, and go through
        // the GUI-item pass with the slots (see `screen_icons`).
        rewo_world::menu_screen::STONECUTTER_MENU_PROTOCOL_ID => {
            use rewo_world::menu_screen as ms;
            let Some(c) = player.cut.filter(|c| c.display) else {
                // `displayRecipes` hides the grid AND the scrollbar: the
                // scroller is drawn inside `extractBackground` unconditionally,
                // but `isScrollBarActive` is false, so it is the *disabled*
                // sprite that shows — and vanilla draws it even with no input.
                out.push((
                    a::CUT_SCROLLER + 1,
                    to_blit(cut_scroller_blit(ms::cut_scroller_y(0.0))),
                ));
                return out;
            };
            let active = ms::cut_scroll_active(true, c.recipes.len());
            out.push((
                a::CUT_SCROLLER + usize::from(!active),
                to_blit(cut_scroller_blit(ms::cut_scroller_y(c.scroll_offs))),
            ));
            for pos_index in 0..ms::CUT_PAGE {
                let index = c.start_index + pos_index;
                if index as usize >= c.recipes.len() {
                    break;
                }
                // `extractButtons`' three-way test, in its own order: selected
                // wins over hovered, and the hover box is the ICON's, two
                // pixels below the box a click uses.
                let hovered = mouse_gui.is_some_and(|(x, y)| {
                    ms::cut_cell_highlight_at(x, y, c.start_index, c.recipes.len()) == Some(index)
                });
                let chrome = if index == c.selected {
                    0
                } else if hovered {
                    1
                } else {
                    2
                };
                let (sx, sy) = ms::cut_cell_sprite_origin(pos_index);
                out.push((
                    a::CUT_RECIPE_CHROME + chrome,
                    to_blit(rewo_world::menu_screen::ProgressBlit {
                        dx: sx,
                        dy: sy,
                        w: ms::CUT_CELL_W,
                        h: ms::CUT_CELL_H,
                        sx: 0,
                        sy: 0,
                        src: None,
                    }),
                ));
            }
        }
        // crafter_3x3 (M93j): the redstone arrow, then one cover per disabled
        // grid slot. Order matters only in that the covers must not be hidden
        // by the arrow, and they do not overlap it.
        rewo_world::menu::CRAFTER_MENU_PROTOCOL_ID => {
            out.push((
                a::CRAFTER_REDSTONE + usize::from(m.crafter_powered()),
                to_blit(rewo_world::menu_screen::crafter_redstone()),
            ));
            for slot in 0..rewo_world::menu::CRAFTER_GRID_SLOTS {
                if !m.crafter_slot_disabled(slot) {
                    continue;
                }
                // The slot's position comes from the LAYOUT, not re-derived
                // from `26 + x * 18`: a second copy of the grid arithmetic is
                // the drift M90's `slot_kind` bug was made of.
                let Some((sx, sy)) = layout.position(slot as usize) else {
                    continue;
                };
                out.push((
                    a::CRAFTER_DISABLED_SLOT,
                    to_blit(rewo_world::menu_screen::crafter_disabled_cover(
                        sx as i32, sy as i32,
                    )),
                ));
            }
        }
        // brewing_stand (M92): fuel, arrow, bubbles — vanilla's own order.
        11 => {
            let (fuel, brew, bubbles) =
                rewo_world::menu_screen::brewing_progress(m.brewing_fuel(), m.brewing_ticks());
            for (sprite, blit) in [
                (a::BREW_FUEL, fuel),
                (a::BREW_PROGRESS, brew),
                (a::BREW_BUBBLES, bubbles),
            ] {
                if let Some(b) = blit {
                    out.push((sprite, to_blit(b)));
                }
            }
        }
        // enchantment (M92): each row's background, then its numeral ON TOP of
        // it — which is why `overlays` is an ordered list and not a set.
        13 => {
            let rows = enchant_rows_of(layout, Some(m), player, mouse_gui)
                .expect("id 13 with an open menu");
            for (i, row) in rows.into_iter().enumerate() {
                let (bg, numeral) = enchant_row_sprites(i, row);
                out.push((bg, to_blit(rewo_world::menu_screen::enchant_row_rect(i))));
                if let Some(n) = numeral {
                    out.push((n, to_blit(rewo_world::menu_screen::enchant_level_rect(i))));
                }
            }
        }
        // beacon (M92): each button's 22x22 chrome, then its 18x18 icon.
        9 => {
            let choice = player
                .beacon_override
                .unwrap_or_else(|| beacon_choice(m, &player.beacon_effects));
            for b in rewo_world::menu_screen::beacon_buttons() {
                let hovered = mouse_gui
                    .is_some_and(|(x, y)| rewo_world::menu_screen::beacon_button_hovered(b, x, y));
                let state = rewo_world::menu_screen::beacon_button_state(b, choice, hovered);
                if state == rewo_world::menu_screen::BeaconButtonState::Hidden {
                    continue;
                }
                out.push((
                    a::BEACON_BUTTON_CHROME + beacon_chrome_index(state),
                    to_blit(rewo_world::menu_screen::beacon_button_rect(b)),
                ));
                if let Some(icon) = beacon_icon_sprite(b, choice) {
                    out.push((icon, to_blit(rewo_world::menu_screen::beacon_icon_rect(b))));
                }
            }
        }
        _ => {}
    }
    out
}

/// A button state's offset into the four chrome sprites, which are listed in
/// the order `extractContents` tests for them.
pub(super) fn beacon_chrome_index(s: rewo_world::menu_screen::BeaconButtonState) -> usize {
    use rewo_world::menu_screen::BeaconButtonState as S;
    match s {
        S::Disabled => 0,
        S::Selected => 1,
        S::Highlighted => 2,
        S::Normal => 3,
        // Never reached: a hidden button is skipped before it gets here.
        S::Hidden => 3,
    }
}

/// The 18x18 icon a beacon button draws inside its chrome.
///
/// The upgrade button **borrows the primary's effect**, so its icon changes as
/// you click elsewhere — it is the one button whose art is not a constant.
pub(super) fn beacon_icon_sprite(
    b: rewo_world::menu_screen::BeaconButton,
    choice: rewo_world::menu_screen::BeaconChoice,
) -> Option<usize> {
    use rewo_data::assets as a;
    use rewo_world::menu_screen::{beacon_upgrade_effect, BeaconButtonKind, BeaconEffect};
    let effect = match b.kind {
        BeaconButtonKind::Power { effect, .. } => effect,
        BeaconButtonKind::Upgrade => beacon_upgrade_effect(choice)?,
        BeaconButtonKind::Confirm => return Some(a::BEACON_CONFIRM),
        BeaconButtonKind::Cancel => return Some(a::BEACON_CANCEL),
    };
    let i = BeaconEffect::ALL.iter().position(|e| *e == effect)?;
    Some(a::BEACON_EFFECT_ICON + i)
}
