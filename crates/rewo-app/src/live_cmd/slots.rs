use super::*;

/// The GUI-space top-left of the slot the cursor is over, for the highlight
/// (M106b).
///
/// **Through the same [`rewo_gpu::container::Placement`] the pass draws with.**
/// This was a bare `screen_to_gui_for`, which centres, while
/// `ContainerPass::set_state` resolves its own origin with
/// `Placement::with_book` — so with the recipe book open the cursor was
/// converted against a panel 77 GUI px left of the one the highlight was drawn
/// against, and the lit slot sat four columns right of the cursor. The
/// tooltip's conversion had the same bug; the panel, the slot icons
/// ([`menu_slot_rects`]) and the bespoke-widget hovers did not.
///
/// Extracted from `apply_screen` rather than fixed in place because that
/// function needs a `PlaySession` and no gate reaches it: every `set_container`
/// call in the whole app passes `hovered: None`, so the derivation had no
/// witness of any kind. M97's lesson again — move the logic to where a test can
/// see it.
pub(super) fn hovered_slot_position(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    mouse: (f64, f64),
    w: f32,
    h: f32,
    book_open: bool,
) -> Option<(i32, i32)> {
    let slot = hovered_menu_slot(layout, mouse, w, h, book_open)?;
    layout.position(slot).map(|(x, y)| (x as i32, y as i32))
}

/// `AbstractContainerScreen.getHoveredSlot` — which menu slot the cursor is
/// over, through the book-aware placement.
///
/// **Rewo does not model `AbstractRecipeBookScreen.isHovering`'s narrow-window
/// override**, which returns false for every slot while the book is visible and
/// the window is under 379 GUI px — the case where the book covers the menu. In
/// vanilla that makes `hoveredSlot` null there, suppressing the highlight, the
/// item tooltip, the ghost tooltip and the number/Q/F keyboard actions
/// together. It is one predicate with five consumers and belongs in its own
/// change, not smuggled in with a tooltip.
pub(crate) fn hovered_menu_slot(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    mouse: (f64, f64),
    w: f32,
    h: f32,
    book_open: bool,
) -> Option<usize> {
    // `AbstractRecipeBookScreen.isHovering` (M112):
    //
    //     return (!this.widthTooNarrow || !this.recipeBookComponent.isVisible())
    //            && super.isHovering(...);
    //
    // On a window under 379 GUI px the book does not sit BESIDE the menu, it
    // sits OVER it — `updateScreenPosition`'s 177 px shift collapses to 0 —
    // so vanilla answers "no slot" for every slot rather than letting a click
    // reach through the panel that is covering it. Without this, a narrow
    // window lets a click on the book land on whatever menu slot happens to be
    // underneath.
    //
    // Placed here rather than at each consumer because it is ONE predicate
    // with several: the highlight, both tooltips, the click, the double-click
    // detector, the drag and the number/Q/F actions all arrive through this
    // function.
    let scale = rewo_gpu::hud::gui_scale(w, h);
    let gui_w = (w / scale) as i32;
    if book_open && rewo_world::recipe_book_screen::width_too_narrow(gui_w) {
        return None;
    }
    let (gx, gy) = rewo_gpu::container::screen_to_gui_placed(
        mouse,
        w,
        h,
        rewo_gpu::container::Placement::with_book(
            layout.image_w as f32,
            layout.image_h as f32,
            book_open,
        ),
    );
    layout.slot_at(gx, gy)
}

/// [`hovered_menu_slot`] under its own name, so a gate can assert that a
/// witness's cursor really is over the slot it thinks it is.
///
/// b22 first passed for the wrong reason: it compared a book-open cursor
/// against a book-shut conversion, so the `None` it was reading came from the
/// 77 px placement shift and not from the guard it names. A mutation deleting
/// that guard survived. This exists so the witness can say which of the two it
/// is measuring.
#[cfg(any(test, feature = "gates"))]
pub(crate) fn hovered_menu_slot_for_gate(
    layout: &'static rewo_world::menu_layout::MenuLayout,
    mouse: (f64, f64),
    w: f32,
    h: f32,
    book_open: bool,
) -> Option<usize> {
    hovered_menu_slot(layout, mouse, w, h, book_open)
}

/// Which of a frame's three tooltip producers wins (M106c).
///
/// `setTooltipForNextFrameInternal` is
/// `if (this.deferredTooltip == null || replaceExisting)`, and `replaceExisting`
/// is false on every path any of these three takes — so the **first** tooltip
/// set in a frame wins, and the calls that follow it are discarded.
///
/// That inverts the reading of `AbstractRecipeBookScreen.extractRenderState`,
/// which calls the container's `extractTooltip` and *then* the book's, as
/// though the book overwrote it. Writing this as a named function rather than
/// an `or_else` chain at the call site is the point: the parameters say which
/// producer is which, so getting the order wrong is a named mistake instead of
/// an anonymous one, and the rule has somewhere to be tested.
///
/// The producers are closures because two of the three are only worth
/// evaluating when the earlier ones declined — the container's tooltip walks a
/// component patch and the book's measures a page.
///
/// `ctx` is threaded through them rather than captured because all three want
/// the same `&mut GlyphCache`, and three closures cannot hold it at once. The
/// reborrow per call is what makes the laziness expressible at all.
pub(super) fn frame_tooltip<T, C>(
    ctx: &mut C,
    book_widget: impl FnOnce(&mut C) -> Option<T>,
    menu: impl FnOnce(&mut C) -> Option<T>,
    book_page: impl FnOnce(&mut C) -> Option<T>,
    ghost: impl FnOnce(&mut C) -> Option<T>,
) -> Option<T> {
    // M133 — the book's WIDGET tooltips come first, and the order is read off
    // `AbstractRecipeBookScreen`: `recipeBookComponent.extractRenderState`
    // runs before `this.extractTooltip`, and every widget rendered inside it
    // calls `refreshTooltipForNextRenderPass` -> `setTooltipForNextFrame`. So
    // by the same first-wins rule M106a found, an arrow or the filter beats
    // both the container's item tooltip and the book's cell tooltip.
    if let Some(t) = book_widget(ctx) {
        return Some(t);
    }
    if let Some(t) = menu(ctx) {
        return Some(t);
    }
    if let Some(t) = book_page(ctx) {
        return Some(t);
    }
    ghost(ctx)
}

/// The tooltip of a GHOST ingredient under the cursor (M106c).
///
/// `GhostSlots.extractTooltip` — keyed on the hovered menu slot, showing the
/// item the ghost's own cycle is on.
///
/// **This is where first-wins becomes observable.** The container's tooltip is
/// set earlier in the frame and `setTooltipForNextFrameInternal` only assigns
/// when nothing has claimed the slot, so a ghost drawn over a slot that already
/// holds an item describes the REAL item, not the ghost. Ordering these
/// producers the way their calls read — the book's last, therefore winning —
/// would show the ghost's name over a filled slot, which is plausible and
/// wrong.
///
/// **Gated on the book being open**, unlike the ghost's own render:
/// `extractGhostRecipe` is called unconditionally from `extractSlots` while
/// `extractTooltip` sits inside `if (this.isVisible())`. So shutting the book
/// leaves the ghost painted and stops it describing itself.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ghost_tooltip(
    ghosts: &[rewo_world::ghost_slots::Ghost],
    cycle: i32,
    layout: &'static rewo_world::menu_layout::MenuLayout,
    book_open: bool,
    items: &rewo_data::items::Items,
    names: &std::collections::HashMap<String, String>,
    advance: &[u8; 256],
    glyphs: Option<&mut rewo_gpu::velvet_glyph::GlyphCache>,
    mouse: (f64, f64),
    (w, h): (f32, f32),
) -> Option<(
    rewo_gpu::container::TooltipDraw,
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<rewo_gpu::velvet_text::OwnedRun>,
)> {
    if !book_open {
        return None;
    }
    let slot = hovered_menu_slot(layout, mouse, w, h, book_open)?;
    // `this.ingredients.get(hoveredSlot)` — a map keyed by the slot, so a
    // hovered slot with no ghost is simply absent rather than a miss to
    // recover from.
    let ghost = ghosts.iter().find(|g| g.slot == slot)?;
    let item = ghost.item(cycle)?;
    let item_name = items.name(item)?;
    let lines = vec![vec![rewo_gpu::tooltip::Span::new(
        names.get(item_name)?.to_string(),
        rarity_color(stack_rarity(Some(item_name), None, false)),
    )]];
    tooltip_layout(lines, advance, glyphs, mouse, (w, h))
}

/// Every label and fill the open book contributes (M105).
///
/// The field's text and the page counter belong to different objects — an
/// `EditBox` on the component, and `RecipeBookPage`'s own `extractRenderState`
/// — but they share both preconditions (an open book, a font to measure with)
/// and they are drawn into one list. Composing them here rather than at the
/// call site is deliberate: `apply_screen` needs a `PlaySession` and so cannot
/// be reached from a test, and a composition step performed there would be
/// deletable with every unit test still green. That is M99's lesson — shrink
/// the untestable surface rather than pretend to cover it.
pub(super) fn book_labels(
    b: &BookRender,
    field: &rewo_world::edit_box::EditBox,
    lang: &rewo_data::lang::Language,
    advance: &[u8; 256],
    w: f32,
    h: f32,
    now_ms: u64,
) -> (
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<(usize, rewo_gpu::container::PanelBlit)>,
) {
    let (mut labels, fills) = book_field_render(field, advance, w, h, now_ms);
    labels.extend(b.view.and_then(|v| book_page_label(v, lang, advance, w, h)));
    (labels, fills)
}

/// The `x/y` page counter under the recipe grid (M105).
///
/// The one piece of the book vanilla draws as bare text rather than through a
/// widget: `RecipeBookPage.extractRenderState` opens with it, before the
/// buttons and the arrows. Its model constants have existed since M93z with
/// nothing reading them.
///
/// Three details that a call site alone does not carry:
///
/// * **The five-argument `graphics.text` delegates to the six-argument one with
///   `dropShadow = true`.** Transcribing the visible arguments and defaulting
///   the shadow to `false` loses it with nothing to notice.
/// * **Colour `-1` is `0xFFFFFFFF`** — opaque white. `text` also early-returns
///   on `ARGB.alpha(color) == 0`, which is why the alpha is worth reading
///   rather than assuming.
/// * The x is computed **by hand** as `xo - width / 2 + 73` rather than through
///   the `centeredText` helper sitting a few lines away in the same class. The
///   two are arithmetically identical here; the hand-written form is kept
///   because it is what the class does.
pub(super) fn book_page_label(
    view: rewo_world::recipe_book_screen::BookView,
    lang: &rewo_data::lang::Language,
    advance: &[u8; 256],
    w: f32,
    h: f32,
) -> Option<rewo_gpu::world::OwnedTextLine> {
    use rewo_world::recipe_book_screen as rb;
    // `Language.getOrDefault` returns the key when the map has no entry, and a
    // template with no specifiers survives `decomposeTemplate` unchanged — so
    // `or_key` is vanilla's behaviour, not a local fallback.
    let text = rb::page_label(view.page, view.total_pages, lang.or_key(rb::PAGE_LABEL_KEY))?;
    let (bl, bt, scale) = rewo_gpu::container::recipe_book_origin(w, h);
    // Measured with the SAME advances the text pass will draw with. Measuring
    // one font and drawing another is what M52b found in the tooltip when it
    // moved to Newsreader; here there is only the bitmap font, and passing the
    // table in keeps it that way by construction.
    let width = rewo_gpu::text::width(&text, advance);
    Some(rewo_gpu::world::OwnedTextLine {
        x: bl + rb::page_label_x(width) as f32 * scale,
        y: bt + rb::PAGE_LABEL_Y as f32 * scale,
        px: scale,
        color_linear: [1.0, 1.0, 1.0],
        alpha: 1.0,
        shadow: true,
        style: rewo_gpu::text::TextStyle::PLAIN,
        text,
    })
}

/// The book's chrome as blits, with its semantic sprites resolved to atlas
/// indices.
///
/// The mapping lives here rather than in the model for the reason the model's
/// doc gives: `MENU_OVERLAY_SPRITES` is append-only and its order is an atlas
/// contract, so keeping the geometry free of it lets the atlas grow without
/// touching a geometry file — and lets the model's tests name a *sprite* rather
/// than an index.
pub(crate) fn recipe_book_panel(
    b: &BookRender,
    // M100 — the search field's own quads, appended AFTER the chrome so the
    // field's background sits over the panel and its caret over that.
    field: &[(usize, rewo_gpu::container::PanelBlit)],
    // M104 — the open which-of-these overlay, if any, and the cursor in book
    // pixels for its hover. Drawn LAST, which is `graphics.nextStratum()`.
    open: Option<&rewo_world::recipe_overlay::Open>,
    book_mouse: Option<(i32, i32)>,
) -> Option<rewo_gpu::container::RecipeBookPanel> {
    use rewo_data::assets as a;
    use rewo_world::recipe_book_screen as rb;
    let view = b.view?;
    let mut blits = Vec::new();
    let mut overlays = Vec::new();
    let chrome = rb::book_chrome(view, &b.slots, b.hover)
        .into_iter()
        .chain(open.into_iter().flat_map(|o| {
            rb::overlay_chrome(
                o.origin,
                &o.craftable_flags(),
                o.furnace,
                book_mouse.and_then(|(bx, by)| o.hovered(bx, by)),
            )
        }));
    for q in chrome {
        let (sx, sy, sw, sh) = q.src.unwrap_or((0, 0, q.w, q.h));
        let blit = rewo_gpu::container::PanelBlit {
            dx: q.x as f32,
            dy: q.y as f32,
            w: q.w as f32,
            h: q.h as f32,
            sx: sx as f32,
            sy: sy as f32,
            sw: sw as f32,
            sh: sh as f32,
            tint: [1.0; 4],
        };
        match q.sprite {
            // The panel is the one quad that comes off a background SHEET
            // rather than a sprite, and the one whose source is not (0, 0).
            rb::BookSprite::Panel => blits.push(rewo_gpu::container::PanelBlit {
                sx: rb::PANEL_SOURCE.0 as f32,
                sy: rb::PANEL_SOURCE.1 as f32,
                ..blit
            }),
            rb::BookSprite::Tab { selected } => {
                overlays.push((a::BOOK_TAB + usize::from(selected), blit))
            }
            rb::BookSprite::Slot(s) => overlays.push((
                a::BOOK_SLOT
                    + match s {
                        rb::SlotSprite::Craftable => 0,
                        rb::SlotSprite::ManyCraftable => 1,
                        rb::SlotSprite::Uncraftable => 2,
                        rb::SlotSprite::ManyUncraftable => 3,
                    },
                blit,
            )),
            rb::BookSprite::PageForward { hovered } => {
                overlays.push((a::BOOK_PAGE_ARROW + usize::from(hovered), blit))
            }
            rb::BookSprite::PageBackward { hovered } => {
                overlays.push((a::BOOK_PAGE_ARROW + 2 + usize::from(hovered), blit))
            }
            rb::BookSprite::Filter { furnace, filtering, hovered } => {
                let base = if furnace { a::BOOK_FILTER_FURNACE } else { a::BOOK_FILTER };
                overlays.push((base + rb::filter_sprite_offset(filtering, hovered), blit))
            }
            // M104 — the only sprite in this list that is nine-sliced, so the
            // one whose `src` is not the whole of it.
            rb::BookSprite::OverlayPanel => overlays.push((a::BOOK_OVERLAY_PANEL, blit)),
            rb::BookSprite::OverlayButton { furnace, craftable, hovered } => overlays.push((
                a::BOOK_OVERLAY_BUTTON
                    + if furnace { 4 } else { 0 }
                    + 2 * usize::from(!craftable)
                    + usize::from(hovered),
                blit,
            )),
        }
    }
    overlays.extend_from_slice(field);
    Some(rewo_gpu::container::RecipeBookPanel { blits, overlays })
}

/// [`container_panel`] for `containershot`, which drives the production
/// builder rather than a copy of it — M45's finding: a gate that reimplements
/// a slice of the app's setup misses whatever the app adds to it.
#[cfg(any(test, feature = "gates"))]
pub(crate) fn container_panel_for_test(
    layout: &'static rewo_world::menu_layout::MenuLayout,
) -> Option<rewo_gpu::container::ContainerPanel> {
    container_panel(layout, None, EnchantPlayer::default(), None)
}

/// [`container_panel`] for an *open* menu, so `containershot` can grade the
/// M92 overlays — which only exist when there are data slots to read.
///
/// Drives the production builder for M45's reason: a gate that reimplements a
/// slice of the app's setup misses whatever the app adds to it.
#[cfg(any(test, feature = "gates"))]
pub(crate) fn container_panel_for_open_menu(
    open: &rewo_world::menu::OpenMenu,
    xp_level: i32,
    creative: bool,
    beacon_effects: BeaconEffectIds,
    mouse_gui: Option<(f64, f64)>,
    // M93m — the beacon screen's own choice. Carried on the SAME entry point
    // the gate already drives rather than a second one, so `containershot`
    // cannot exercise a path the live client does not take (M45).
    beacon_override: Option<rewo_world::menu_screen::BeaconChoice>,
    // M93q — the loom's grid. Carried for the same reason as the beacon's
    // choice, and needed for the same reason: with it hardcoded `None` the
    // gate could not reach `menu_overlays`' loom arm at all, so M93q's fill
    // was witnessed as a *primitive* (o19/o20) and never as a *use* — delete
    // the whole arm and both stayed green. That is M92's finding one level
    // over: a gate that cannot reach a call site does not test it.
    //
    // The view is SUPPLIED, not resolved, because resolving it needs an item
    // registry this gate has not got. What that leaves untested is the
    // resolution in `live_frame`, which `d7`–`d9` grade item-side and
    // `loom_pattern_table`'s own tests grade set-side.
    loom: Option<LoomView>,
    // M93s — the stonecutter's grid, supplied for the same reason.
    cut: Option<&CutView>,
    // M93u — and the merchant's trade list.
    merchant: Option<&MerchantView>,
) -> Option<rewo_gpu::container::ContainerPanel> {
    container_panel(
        open.layout,
        Some(open),
        EnchantPlayer {
            xp_level,
            creative,
            beacon_effects,
            beacon_override,
            loom,
            cut,
            // A gate drives no anvil field; `containershot` supplies its own
            // overlays when it wants a fill (M93q's o19/o20).
            anvil_fills: &[],
            merchant,
            // A gate drives no ghost recipe either.
            ghost_under: &[],
            ghost_over: &[],
        },
        mouse_gui,
    )
}

/// [`BeaconEffectIds::resolve`] for `containershot`.
#[cfg(any(test, feature = "gates"))]
pub(crate) fn beacon_effect_ids_for_test(
    m: &rewo_data::mob_effects::MobEffects,
) -> BeaconEffectIds {
    BeaconEffectIds::resolve(m)
}

/// [`sheet_index`] for `containershot`.
#[cfg(any(test, feature = "gates"))]
pub(crate) fn sheet_index_for_test(texture: &str) -> Option<usize> {
    sheet_index(texture)
}

/// A texture's index in the atlas, by the path the bake loaded it under.
///
/// `menu_screen` spells paths in vanilla's `Identifier` form and the bake in
/// the jar-relative one, so the prefix comes off here. The two lists are
/// cross-checked by a test in `rewo-world`; this returning `None` would mean
/// that check had been removed.
pub(super) fn sheet_index(texture: &str) -> Option<usize> {
    let want = texture.trim_start_matches("textures/");
    rewo_data::assets::MENU_BACKGROUND_TEXTURES
        .iter()
        .position(|t| *t == want)
}

/// Every slot's on-screen rect for a menu, in its own layout and at its own
/// panel size (M87).
///
/// Takes the menu rather than assuming the player's: a container is a
/// different slot count *and* a different panel, and the panel is what centres
/// it — a six-row chest is 176x222, so measuring its slots from a 176x166
/// origin puts every one of them 28 px low.
/// Where each of a menu's slots sits on screen.
///
/// `book_open` is not optional decoration: an open recipe book MOVES the menu
/// (M94), and this origin has to move with the panel's or every icon lands 77 px
/// left of the slot it belongs to. M94 threaded the book through the panel draw
/// and the hover and **missed this path**, which is the one-accessor rule half
/// applied — the same shape as M90's `slot_kind`, and missed for the same
/// reason: a function taking bare numbers does not look like it belongs to the
/// menu.
pub(super) fn menu_slot_rects(
    menu: &rewo_world::inventory::Inventory,
    w: f32,
    h: f32,
    book_open: bool,
) -> Vec<(f32, f32, f32)> {
    let layout = menu.layout();
    let (left, top, scale) = rewo_gpu::container::gui_origin_placed(
        w,
        h,
        rewo_gpu::container::Placement::with_book(
            layout.image_w as f32,
            layout.image_h as f32,
            book_open,
        ),
    );
    (0..menu.slot_count())
        .map(|i| {
            let (x, y) = layout.position(i).unwrap_or((0, 0));
            (
                left + x as f32 * scale,
                top + y as f32 * scale,
                16.0 * scale,
            )
        })
        .collect()
}

/// The player inventory's slot rects — [`menu_slot_rects`] for the menu that
/// was the only one before M87.
pub(super) fn screen_slot_rects(w: f32, h: f32) -> Vec<(f32, f32, f32)> {
    menu_slot_rects(&rewo_world::inventory::Inventory::default(), w, h, false)
}
