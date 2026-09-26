use super::*;

pub(crate) fn screen_icons(
    inv: &rewo_world::inventory::Inventory,
    items: &rewo_data::items::Items,
    trim_materials: &[rewo_net::trim_parse::TrimMaterialDef],
    w: f32,
    h: f32,
    // M93j — the open menu, for the slots whose render it REPLACES. `None` is
    // the player's own inventory, which replaces none.
    open: Option<&rewo_world::menu::OpenMenu>,
    // M93s — the stonecutter's recipe grid, whose buttons draw item icons that
    // belong to no slot.
    cut: Option<&CutView>,
    // M93u — and the merchant's trade rows, likewise.
    merchant: Option<&MerchantView>,
    // M95 — the recipe book, whose tab icons and recipe results are items.
    // Its presence also MOVES the menu, so it moves every icon measured from
    // the menu's origin too.
    book: Option<&BookRender>,
    // M103 — the ghost recipe's slots, and the cycle index its items rotate on.
    ghosts: &[rewo_world::ghost_slots::Ghost],
    cycle: i32,
    // M104 — the open which-of-these overlay, whose ingredient grids are items
    // too. Supplied rather than reached for, so a gate drives the same call
    // the live client makes (M45).
    overlay: Option<&rewo_world::recipe_overlay::Open>,
) -> (Vec<rewo_gpu::gui_item::GuiItem>, Vec<rewo_gpu::world::OwnedTextLine>) {
    let rects = menu_slot_rects(inv, w, h, book.is_some());
    let (_, _, scale) = rewo_gpu::container::gui_origin(w, h);
    let mut icons = Vec::new();
    let mut labels = Vec::new();
    for (slot, rect) in rects.iter().enumerate() {
        // A slot the screen covers draws neither its icon nor its count —
        // `extractSlot` never reaches `super`, and the count is part of what
        // super draws.
        if open.is_some_and(|m| m.slot_hides_item(slot)) {
            continue;
        }
        if let Some(stack) = inv.menu_slot(slot) {
            if let Some(icon) = icon_for(items, trim_materials, stack, rect.0, rect.1, rect.2) {
                icons.push(icon);
            }
            labels.extend(count_label(stack, rect.0, rect.1, scale));
        }
    }
    // M103 — the ghost recipe's items, in the menu's own slots. Between the two
    // washes by construction: the red is in the panel's overlays (back half) and
    // the white in its front overlays, and the icon pass runs between them.
    //
    // **No count label** except on the result, which is `itemDecorations`'
    // own rule — an input ghost never shows a number even when the recipe wants
    // several of that ingredient.
    for g in ghosts {
        let Some(id) = g.item(cycle) else { continue };
        let Some((sx, sy)) = inv.layout().position(g.slot) else { continue };
        let (left, top, _) = rewo_gpu::container::gui_origin_placed(
            w,
            h,
            rewo_gpu::container::Placement::with_book(
                inv.layout().image_w as f32,
                inv.layout().image_h as f32,
                book.is_some(),
            ),
        );
        if let Some(icon) = icon_for(
            items,
            trim_materials,
            rewo_world::inventory::ItemSlot::plain(id, 1),
            left + sx as f32 * scale,
            top + sy as f32 * scale,
            16.0 * scale,
        ) {
            icons.push(icon);
        }
    }
    // M95 — the recipe book's items: the tab icons and the page's results.
    //
    // On the BOOK's origin, not the menu's — the book is window-anchored and
    // the gap between the two alternates with the window's parity (M94).
    //
    // **No count label**, for the stonecutter's reason one screen over: both
    // `fakeItem` and `graphics.item` draw the model alone, and
    // `itemDecorations` is a separate call neither makes. A recipe yielding 8
    // torches shows no "8".
    if let Some(b) = book.and_then(|b| b.view.map(|v| (b, v))) {
        let (bk, view) = b;
        let (bl, bt, _) = rewo_gpu::container::recipe_book_origin(w, h);
        let tabs = bk.book.tabs();
        for icon in rewo_world::recipe_book_screen::book_icons(view, tabs, &bk.slot_shadowed) {
            use rewo_world::recipe_book_screen::BookIconKind as K;
            let id = match icon.kind {
                K::TabPrimary(i) => tabs.get(i).and_then(|t| items.id(t.primary)),
                K::TabSecondary(i) => tabs.get(i).and_then(|t| t.secondary).and_then(|n| items.id(n)),
                // The SAME item for both copies: the shadow is the display
                // stack drawn twice, not a second recipe's result.
                K::Slot { index, .. } => bk.slot_items.get(index).copied().flatten(),
            };
            let Some(id) = id else { continue };
            if let Some(g) = icon_for(
                items,
                trim_materials,
                rewo_world::inventory::ItemSlot::plain(id, 1),
                bl + icon.x as f32 * scale,
                bt + icon.y as f32 * scale,
                16.0 * scale,
            ) {
                icons.push(g);
            }
        }
    }
    // M104 — the which-of-these overlay's ingredient grids. On the BOOK's
    // origin like the cells above, and drawn after them because the overlay is
    // a stratum over the whole book.
    //
    // **6 px, not 16**: `scale(0.375F)` sits between two translates, so the
    // trailing `-8, -8` is scaled with it and `Pos` is the ingredient's CENTRE.
    // `item_rect` is where that composition lives.
    //
    // No count label, for the same reason the cells have none: `graphics.item`
    // draws the model alone.
    if let Some(o) = overlay {
        let (bl, bt, _) = rewo_gpu::container::recipe_book_origin(w, h);
        for (i, b) in o.buttons.iter().enumerate() {
            let button = rewo_world::recipe_overlay::button_origin(o.origin, i, o.total());
            for (pos, choices) in &b.slots {
                // One-level cycle, on the same clock the cells use — so a cell
                // and the overlay it opened can be showing different items on
                // the same frame.
                let Some(n) = rewo_world::recipe_overlay::select_ingredient(choices.len(), cycle)
                else {
                    continue;
                };
                let (ix, iy, size) = rewo_world::recipe_overlay::item_rect(button, *pos);
                if let Some(g) = icon_for(
                    items,
                    trim_materials,
                    rewo_world::inventory::ItemSlot::plain(choices[n], 1),
                    bl + ix * scale,
                    bt + iy * scale,
                    size * scale,
                ) {
                    icons.push(g);
                }
            }
        }
    }
    // M93s — the stonecutter's recipe buttons, which are ITEMS and so belong
    // here rather than in the overlay atlas.
    //
    // **No count label.** `extractRecipes` calls `graphics.item`, which draws
    // the model alone; `itemDecorations` is a separate call it never makes. So
    // a slab recipe yielding 2 shows no "2", unlike every real slot above —
    // reusing `count_label` here would be the natural thing and would be wrong
    // on 124 of the 319 recipes.
    if let Some(c) = cut.filter(|c| c.display) {
        let (left, top, _) = rewo_gpu::container::gui_origin_for(
            w,
            h,
            open.map_or(176.0, |m| m.layout.image_w as f32),
            open.map_or(166.0, |m| m.layout.image_h as f32),
        );
        for pos_index in 0..rewo_world::menu_screen::CUT_PAGE {
            let index = c.start_index + pos_index;
            let Some(recipe) = usize::try_from(index).ok().and_then(|i| c.recipes.get(i)) else {
                break;
            };
            let Some(id) = items.id(recipe.result) else { continue };
            let (gx, gy) = rewo_world::menu_screen::cut_cell_origin(pos_index);
            if let Some(icon) = icon_for(
                items,
                trim_materials,
                rewo_world::inventory::ItemSlot::plain(id, recipe.count as i32),
                left + gx as f32 * scale,
                top + gy as f32 * scale,
                16.0 * scale,
            ) {
                icons.push(icon);
            }
        }
    }
    // M93u — the merchant's three items per visible row. Like the
    // stonecutter's grid these are items rather than sprites, and unlike it
    // they DO carry counts: `extractOffers` calls `itemDecorations` for cost B
    // and the result, and `extractAndDecorateCostA` for cost A.
    if let Some(v) = merchant {
        use rewo_world::merchant_screen as ms;
        let (left, top, scale) = rewo_gpu::container::gui_origin_for(
            w,
            h,
            open.map_or(176.0, |m| m.layout.image_w as f32),
            open.map_or(166.0, |m| m.layout.image_h as f32),
        );
        let n = v.offers.len();
        for (i, offer) in v.offers.iter().enumerate() {
            let idx = i as i32;
            if !ms::offer_visible(idx, v.scroll_off, n) {
                continue;
            }
            let row = if ms::can_scroll(n) { idx - v.scroll_off } else { idx };
            let y = ms::row_item_y(row);
            // Returns rather than pushes, so the cost-A branch below can use
            // the same geometry without a second mutable borrow.
            let at = |gx: i32, id: i32, count: i32| {
                let (px, py) = (left + gx as f32 * scale, top + y as f32 * scale);
                (
                    icon_for(
                        items,
                        trim_materials,
                        rewo_world::inventory::ItemSlot::plain(id, count),
                        px,
                        py,
                        16.0 * scale,
                    ),
                    count_label(
                        rewo_world::inventory::ItemSlot::plain(id, count),
                        px,
                        py,
                        scale,
                    ),
                )
            };
            // Cost A: ONE icon, at its MODIFIED count — `fakeItem` is called
            // once, outside `extractAndDecorateCostA`'s branch. The discounted
            // display is two NUMBERS over a single item, not two items.
            let modified = v.cost_a_counts[i];
            let disp = ms::cost_a_display(offer.cost_a.count, modified);
            //
            // Only `.0`, the icon: cost A's DIGITS take the branch below.
            // `icon_for` draws the item's model and ignores the count, so
            // which count is passed here is inert — mutating it to the base
            // cost is an equivalent mutant, and it is spelled `modified`
            // because that is the stack vanilla passes to `fakeItem`.
            icons.extend(at(ms::COST_A_X, offer.cost_a.item_id, modified).0);
            // Both digits are FORCED in the discounted branch, including a 1 —
            // `count == 1 ? "1" : null` exists to defeat `itemCount`'s own
            // "a single item shows no digit" rule.
            let forced = |n: i32| (n == 1).then(|| n.to_string());
            for (gx, count) in [
                (ms::COST_A_X, disp.at_icon),
                (ms::COST_A_X + ms::DISCOUNT_SECOND_X, disp.at_second),
            ] {
                let Some(n) = count else { continue };
                labels.extend(count_label_of(
                    n,
                    disp.strikethrough.then(|| forced(n)).flatten(),
                    left + gx as f32 * scale,
                    top + y as f32 * scale,
                    scale,
                ));
            }
            // Cost B and the result take the ordinary path: `itemDecorations`
            // with no `countText`, so a single item shows no digit there.
            for (gx, id, count) in [
                offer.cost_b.as_ref().map(|b| (ms::COST_B_X, b.item_id, b.count)),
                match &offer.result {
                    rewo_net::item_stack::WireSlot::Stack(st) => {
                        Some((ms::RESULT_X, st.item_id, st.count))
                    }
                    rewo_net::item_stack::WireSlot::Empty => None,
                },
            ]
            .into_iter()
            .flatten()
            {
                let (icon, label) = at(gx, id, count);
                icons.extend(icon);
                labels.extend(label);
            }
        }
    }
    (icons, labels)
}

pub(super) fn icon_for(
    items: &rewo_data::items::Items,
    trim_materials: &[rewo_net::trim_parse::TrimMaterialDef],
    stack: rewo_world::inventory::ItemSlot,
    x: f32,
    y: f32,
    size: f32,
) -> Option<rewo_gpu::gui_item::GuiItem> {
    let base = items.name(stack.item_id)?;
    // M49: a trimmed stack asks for its variant. `HeldItems::any` falls back to
    // the plain item when the bake has no such variant, so composing here is
    // always safe.
    let model = match stack
        .trim_material
        .and_then(|m| trim_materials.get(m as usize))
    {
        Some(m) => format!("{base}#{}", m.id),
        None => base.to_string(),
    };
    Some(rewo_gpu::gui_item::GuiItem {
        model,
        x,
        y,
        size,
        // `ItemStack.hasFoil()` (M43). Every slot icon goes through this one
        // constructor, so the hotbar, the screen and the cursor stack all get
        // the glint from the same place.
        glint: stack.enchanted,
    })
}

/// `GuiGraphicsExtractor.itemCount` — bottom-right of the slot, and **only
/// when the count is not one**, which is why a single sword shows no label.
pub(super) fn count_label(
    stack: rewo_world::inventory::ItemSlot,
    x: f32,
    y: f32,
    scale: f32,
) -> Option<rewo_gpu::world::OwnedTextLine> {
    count_label_of(stack.count, None, x, y, scale)
}

/// `itemCount` with vanilla's `countText` override (M93w).
///
/// ```java
/// if (itemStack.getCount() != 1 || countText != null) {
///    String amount = countText == null ? String.valueOf(itemStack.getCount()) : countText;
/// ```
///
/// **The override's only job is to defeat the `!= 1` rule.** A single item
/// normally shows no digit; the merchant's discounted price passes
/// `count == 1 ? "1" : null` so both halves of the comparison stay visible,
/// which matters exactly when a discount has reached 1.
pub(super) fn count_label_of(
    count: i32,
    force: Option<String>,
    x: f32,
    y: f32,
    scale: f32,
) -> Option<rewo_gpu::world::OwnedTextLine> {
    if count == 1 && force.is_none() {
        return None;
    }
    let text = force.unwrap_or_else(|| count.to_string());
    // Vanilla measures the string; the digits are a uniform 6 px including
    // their one-pixel gap, and the trailing gap is not part of the width.
    let width = text.chars().count() as f32 * 6.0 - 1.0;
    Some(rewo_gpu::world::OwnedTextLine {
        // `x + 19 - 2 - width`, `y + 6 + 3`.
        x: x + (17.0 - width) * scale,
        y: y + 9.0 * scale,
        px: scale,
        color_linear: [1.0, 1.0, 1.0],
        alpha: 1.0,
        shadow: true,
        style: rewo_gpu::text::TextStyle::PLAIN,
        text,
    })
}

/// The enchanting table's three cost numerals (M92).
///
/// The first text a container screen draws that is not a stack count, and the
/// alignment is why it needs the real advance table rather than a 6-px-per-
/// digit estimate: `leftPosText + 86 - font.width(costText)` is **right**-
/// aligned, so a wrong width moves a two-digit cost and leaves a one-digit one
/// looking correct.
///
/// An empty row draws nothing at all — `cost == 0` returns before the numeral,
/// the name and the cost, so a table with no item shows three blank rows.
pub(super) fn enchant_cost_labels(
    rows: [rewo_world::menu_screen::EnchantRow; 3],
    advance: &[u8; 256],
    w: f32,
    h: f32,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    let layout = &rewo_world::menu_layout::REGISTRY[13]; // enchantment
    let (left, top, scale) =
        rewo_gpu::container::gui_origin_for(w, h, layout.image_w as f32, layout.image_h as f32);
    let mut out = Vec::new();
    for (i, row) in rows.into_iter().enumerate() {
        let (Some(cost), Some(rgb)) = (row.cost(), row.cost_color()) else {
            continue;
        };
        let text = cost.to_string();
        let (x, y) =
            rewo_world::menu_screen::enchant_cost_pos(i, rewo_gpu::text::width(&text, advance));
        out.push(rewo_gpu::world::OwnedTextLine {
            x: left + x as f32 * scale,
            y: top + y as f32 * scale,
            px: scale,
            color_linear: srgb_bytes_to_linear(rgb),
            alpha: 1.0,
            shadow: true,
            style: rewo_gpu::text::TextStyle::PLAIN,
            text,
        });
    }
    out
}

/// The anvil name field's geometry, from `AnvilScreen.subInit` (M93t).
///
/// ```java
/// this.name = new EditBox(this.font, xo + 62, yo + 24, 103, 12, …);
/// this.name.setBordered(false);
/// ```
///
/// Unbordered, so `textX = getX() + 0` and `textY = getY()` — a bordered box
/// would inset by 4 and centre vertically by `(height - 8) / 2`, and reusing
/// those here would put the name four pixels right and two down.
/// `getInnerWidth()` is likewise the full 103 rather than `width - 8`.
pub(super) const ANVIL_FIELD: (i32, i32, i32) = (62, 24, 103);

/// The name field's text, cursor and selection (M93t).
///
/// The two-piece draw in `extractWidgetRenderState` exists to *place the
/// cursor*, not to change the text: the halves are separated by `+1` and then
/// pulled back by `-1` for an insert cursor, so the run is contiguous either
/// way and one label is exact.
pub(super) fn anvil_field_render(
    local: &rewo_world::edit_box::EditBox,
    advance: &[u8; 256],
    w: f32,
    h: f32,
    now_ms: u64,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
    Option<String>,
) {
    let layout = &rewo_world::menu_layout::REGISTRY[ANVIL_MENU_PROTOCOL_ID as usize];
    let (left, top, scale) =
        rewo_gpu::container::gui_origin_for(w, h, layout.image_w as f32, layout.image_h as f32);
    let (fx, fy, inner) = ANVIL_FIELD;
    edit_box_render(local, advance, (left, top, scale), (fx, fy, inner), None, now_ms)
}

/// One `EditBox`'s text, caret and selection — shared by the anvil's name field
/// (M93t) and the recipe book's search (M100).
///
/// Extracted rather than copied: the caret's x is the width of the run before
/// it, the `insert` rule decides whether the caret is a bar or an underscore,
/// and the selection's rect is clamped against the inner width. A second copy
/// of that is three chances to drift, and the drift would be a caret one pixel
/// out — invisible in review and obvious in use.
///
/// `origin` is the surface's `(left, top, scale)` in screen pixels; `field` is
/// `(x, y, inner_width)` in that surface's own coordinates. `hint` is the text
/// drawn when the field is **empty and unfocused**, with its own colour.
pub(super) fn edit_box_render(
    local: &rewo_world::edit_box::EditBox,
    advance: &[u8; 256],
    (left, top, scale): (f32, f32, f32),
    (fx, fy, inner): (i32, i32, i32),
    hint: Option<(&str, [f32; 3])>,
    // Wall-clock milliseconds, for the caret's blink phase (M101).
    now_ms: u64,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
    Option<String>,
) {
    let width = |u: &[u16]| rewo_gpu::text::width(&String::from_utf16_lossy(u), advance);
    let displayed = local.displayed(inner, &width).to_vec();
    let rel_cursor = local.cursor_position().saturating_sub(local.display_pos());
    let on_screen = rel_cursor <= displayed.len();

    let mut labels = Vec::new();
    let mut fills = Vec::new();
    let mut append = None;
    let text = String::from_utf16_lossy(&displayed);
    if !text.is_empty() {
        labels.push(rewo_gpu::world::OwnedTextLine {
            x: left + fx as f32 * scale,
            y: top + fy as f32 * scale,
            px: scale,
            // `setTextColor(-1)` AND `setTextColorUneditable(-1)` — the anvil
            // sets both to white, so an uneditable field is not greyed.
            color_linear: [1.0, 1.0, 1.0],
            alpha: 1.0,
            shadow: true,
            style: rewo_gpu::text::TextStyle::PLAIN,
            text,
        });
    }

    // `if (hint != null && displayed.isEmpty() && !isFocused())` — the hint
    // goes when the field takes FOCUS, not when the first character arrives.
    // Clicking an empty search box therefore blanks "Search..." before you
    // type, which reads as a bug until you check the guard.
    if let Some((text, color)) = hint {
        if displayed.is_empty() && !local.is_focused() {
            labels.push(rewo_gpu::world::OwnedTextLine {
                x: left + fx as f32 * scale,
                y: top + fy as f32 * scale,
                px: scale,
                // The hint is a styled COMPONENT, so its own colour wins over
                // the field's — `SEARCH_HINT_STYLE` is gray, where the book
                // sets the field itself to white. The constant is vanilla's
                // byte `/255`, so it converts here.
                color_linear: srgb_bytes_to_linear_f(color),
                alpha: 1.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text: text.to_string(),
            });
        }
    }

    // `insert = cursorPos < value.length() || value.length() >= maxLength` —
    // so a full field shows the BAR even with the cursor at the end, which is
    // how vanilla tells you there is no room left.
    let insert = local.cursor_position() < local.len() || local.len() >= local.max_length();
    let before = if on_screen { &displayed[..rel_cursor] } else { &displayed[..] };
    let mut cursor_x = fx + width(before) + if before.is_empty() { 0 } else { 1 };
    if on_screen && insert {
        cursor_x -= 1;
    }

    // The selection: `textHighlight(min(cursorX, x+width), textY-1,
    // min(highlightX-1, x+width), textY+1+9, invert)`. The anvil sets
    // `setInvertHighlightedTextColor(false)`, so only the blue fill runs.
    let rel_highlight = local
        .highlight_position()
        .saturating_sub(local.display_pos())
        .min(displayed.len());
    if rel_highlight != rel_cursor {
        let hx = fx + width(&displayed[..rel_highlight]);
        let (x0, x1) = (cursor_x.min(fx + inner), (hx - 1).min(fx + inner));
        let (x0, x1) = (x0.min(x1), x0.max(x1));
        fills.push((
            rewo_gpu::container::FILL_SPRITE,
            rewo_gpu::container::PanelBlit {
                dx: x0 as f32,
                dy: (fy - 1) as f32,
                w: (x1 - x0) as f32,
                h: 11.0,
                sx: 0.0,
                sy: 0.0,
                sw: 0.0,
                sh: 0.0,
                // `-16776961` = 0xFF0000FF. NOTE the pipeline is
                // `GUI_TEXT_HIGHLIGHT`, whose blend Rewo's single container
                // blend does not reproduce — the colour is exact, the
                // compositing is a plain alpha draw.
                tint: [0.0, 0.0, 1.0, 1.0],
            },
        ));
    }

    // `showCursor = isFocused() && isCursorVisible(millis - focusedTime) &&
    // cursorOnScreen` — THREE conditions. M93t had only the first, so the
    // anvil's caret was solid and drawn even when scrolled out of view.
    if local.is_focused() && local.cursor_visible(now_ms) && on_screen {
        if insert {
            // `fill(x, y - 1, x + 1, y + lineHeight)`, lineHeight 9 + 1.
            fills.push((
                rewo_gpu::container::FILL_SPRITE,
                rewo_gpu::container::PanelBlit {
                    dx: cursor_x as f32,
                    dy: (fy - 1) as f32,
                    w: 1.0,
                    h: 11.0,
                    sx: 0.0,
                    sy: 0.0,
                    sw: 0.0,
                    sh: 0.0,
                    tint: [1.0, 1.0, 1.0, 1.0],
                },
            ));
        } else {
            // The append cursor is the CHARACTER "_", not a rectangle.
            append = Some(String::from("_"));
            labels.push(rewo_gpu::world::OwnedTextLine {
                x: left + cursor_x as f32 * scale,
                y: top + fy as f32 * scale,
                px: scale,
                color_linear: [1.0, 1.0, 1.0],
                alpha: 1.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text: "_".into(),
            });
        }
    }
    (labels, fills, append)
}

/// [`anvil_field_render`] for `containershot`.
///
/// The clock is fixed at **0** (M101): a caret that blinks would make the same
/// scene render two ways depending on when the gate ran, and a witness cannot
/// hold that constant. At 0 the caret is visible, which is the state the
/// existing anvil witnesses were written against.
#[cfg(any(test, feature = "gates"))]
pub(crate) fn anvil_field_render_for_test(
    local: &rewo_world::edit_box::EditBox,
    advance: &[u8; 256],
    w: f32,
    h: f32,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
    Option<String>,
) {
    anvil_field_render(local, advance, w, h, 0)
}

/// `AbstractContainerScreen.extractCarriedItem` — the cursor stack, offset by
/// half a slot so it sits under the pointer rather than beside it.
pub(super) fn carried_icon(
    inv: &rewo_world::inventory::Inventory,
    items: &rewo_data::items::Items,
    trim_materials: &[rewo_net::trim_parse::TrimMaterialDef],
    mouse: (f64, f64),
    w: f32,
    h: f32,
) -> Option<(
    rewo_gpu::gui_item::GuiItem,
    Option<rewo_gpu::world::OwnedTextLine>,
)> {
    let stack = inv.carried()?;
    let (_, _, scale) = rewo_gpu::container::gui_origin(w, h);
    let (x, y) = (
        mouse.0 as f32 - 8.0 * scale,
        mouse.1 as f32 - 8.0 * scale,
    );
    let icon = icon_for(items, trim_materials, stack, x, y, 16.0 * scale)?;
    Some((icon, count_label(stack, x, y, scale)))
}

/// Resolve an item id into the facts the click arithmetic needs.
///
/// `None` for an id the registry does not contain, which makes the whole click
/// decline rather than predicting against a guessed stack cap.
///
/// `pub(crate)` so `containershot` can grade **this** function rather than its
/// own copy. M92's finding is the reason: a gate that constructs the input
/// production must derive leaves the derivation untested by construction —
/// there, five `mob_effect` ids were read from a `registry_data` branch that
/// cannot fire, and `lightmapshot`/`swingshot` could not see it because both
/// supplied the ids themselves. Every unit test of the quick-move hand-builds
/// an `ItemProps`, so without a witness on this function the table lookups
/// below could all return the wrong thing in the live client and stay green.
/// `recipes` is the last `update_recipes` (M152), or `None` before one has
/// arrived. It supplies the **only wire-derived predicates on this struct** —
/// the three smithing sets — and every other field is jar-derived, so a caller
/// that has no session (a gate, a unit test) passes `None` and gets a smithing
/// table that refuses everything. That is not a degraded mode: it is what
/// vanilla does before the packet lands, because
/// `ClientRecipeContainer.propertySet` is
/// `getOrDefault(id, RecipePropertySet.EMPTY)`.
pub(crate) fn item_props(
    items: &rewo_data::items::Items,
    id: i32,
    smithing: Option<&rewo_net::recipe_book::SmithingSets>,
) -> Option<rewo_world::inventory::ItemProps> {
    use rewo_data::item_props_table::{equip_slot, max_stack_size, EquipSlot};
    use rewo_world::inventory::ArmorPiece;
    let name = items.name(id)?;
    // The property sets carry RAW item ids (`holderRegistry`), which is the id
    // this function was handed — so this is a direct membership test with no
    // name round-trip, unlike every jar table above.
    let in_set = |pick: fn(&rewo_net::recipe_book::SmithingSets) -> &Vec<i32>| {
        smithing.is_some_and(|s| pick(s).contains(&id))
    };
    Some(rewo_world::inventory::ItemProps {
        max_stack: max_stack_size(name),
        // M152 — the smithing table's three input tests.
        smithing_template: in_set(|s| &s.template),
        smithing_base: in_set(|s| &s.base),
        smithing_addition: in_set(|s| &s.addition),
        // M91 — the furnace quick-move's two predicates, resolved here because
        // this is where the numeric id becomes a name.
        is_fuel: rewo_data::fuel_table::is_fuel(name),
        smeltable: [
            rewo_data::smelting_table::accepts(10, name) == Some(true),
            rewo_data::smelting_table::accepts(14, name) == Some(true),
            rewo_data::smelting_table::accepts(22, name) == Some(true),
        ],
        // M93 — the beacon quick-move's one item predicate.
        beacon_payment: rewo_data::beacon_payment_table::is_beacon_payment(name),
        // M93b — the stonecutter's.
        stonecuttable: rewo_data::stonecutter_table::accepts_input(name),
        // M93e — the PROTOTYPE half of `isDamageableItem`. The patch half
        // rides on the stack. `prototype_has_component` is M56's table, which
        // already exists for the tooltip's component count.
        proto_max_damage: rewo_data::item_components_table::prototype_has_component(
            name,
            "minecraft:max_damage",
        )
        .unwrap_or(false),
        proto_damage: rewo_data::item_components_table::prototype_has_component(
            name,
            "minecraft:damage",
        )
        .unwrap_or(false),
        // M93f — `is(PAPER) || is(MAP) || is(GLASS_PANE)`. Item identity, so
        // three names rather than a table. `minecraft:map` is the EMPTY map;
        // `filled_map` is a different item and routes by its MAP_ID component.
        cartography_additional: matches!(
            name,
            "minecraft:paper" | "minecraft:map" | "minecraft:glass_pane"
        ),
        // M93g — the loom. The banner half is item identity from the tag; the
        // other two are the CONJUNCTIONS, tag AND prototype component, and a
        // tag-only test would look correct on every vanilla item.
        loom_banner: rewo_data::loom_table::is_banner(name),
        loom_dye: rewo_data::loom_table::in_loom_dyes(name)
            && rewo_data::item_components_table::prototype_has_component(
                name,
                "minecraft:dye",
            )
            .unwrap_or(false),
        loom_pattern: rewo_data::loom_table::in_loom_patterns(name)
            && rewo_data::item_components_table::prototype_has_component(
                name,
                "minecraft:provides_banner_patterns",
            )
            .unwrap_or(false),
        equips: match equip_slot(name) {
            Some(EquipSlot::Head) => Some(ArmorPiece::Head),
            Some(EquipSlot::Chest) => Some(ArmorPiece::Chest),
            Some(EquipSlot::Legs) => Some(ArmorPiece::Legs),
            Some(EquipSlot::Feet) => Some(ArmorPiece::Feet),
            // `body` and `saddle` are animal equipment and `mainhand`/`offhand`
            // are not armour slots, so none of them satisfies an `ArmorSlot`.
            _ => None,
        },
    })
}
