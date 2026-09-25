use super::*;

/// This frame's icons and stack counts for the open screen.
///
/// Returns the draw list plus the count labels, which go through the text pass
/// — vanilla's `itemCount` draws them at `x + 19 - 2 - width` and `y + 6 + 3`,
/// right-aligned inside the slot, and **only when the count is not one**.
/// Load the Velvet families into a glyph cache (M52b).
///
/// Returns `None` if any face is missing, and the caller falls back to the
/// bitmap pass. Partial loading is deliberately not a state: a tooltip that
/// renders its upright spans and drops its italic ones would look like a
/// styling bug rather than a missing file.
pub(super) fn load_velvet_fonts() -> Option<rewo_gpu::velvet_glyph::GlyphCache> {
    use rewo_gpu::velvet_glyph::{Family, GlyphCache};
    // Next to the executable first (a packaged build), then the workspace
    // path (cargo run) -- the same order the launcher's font resolution uses.
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("assets/fonts")));
    let dir = match beside_exe {
        Some(d) if d.is_dir() => d,
        _ => std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts"),
    };
    let mut c = GlyphCache::new();
    for (fam, italic) in [
        (Family::Newsreader, false),
        (Family::Newsreader, true),
        (Family::Fraunces, false),
        (Family::JetBrainsMono, false),
    ] {
        let p = dir.join(format!("{}.ttf", fam.file_stem(italic)));
        let data = std::fs::read(&p).ok()?;
        if !c.load(fam, italic, data) {
            log::warn!("velvet: {} failed to parse", p.display());
            return None;
        }
    }
    log::info!("velvet: fonts loaded from {}", dir.display());
    Some(c)
}

/// The tooltip's Velvet body size, in **GUI pixels**.
///
/// Vanilla's tooltip text is the 8px bitmap font on a 10px line. Newsreader at
/// the same nominal size reads smaller, because a bitmap glyph fills its cell
/// and a proportional one does not -- so this is calibrated to the *cap
/// height* rather than the em, which is what makes the two look like the same
/// size next to each other.
pub const TOOLTIP_TEXT_GUI_PX: f32 = 9.0;

/// A tooltip line's Velvet key.
pub(super) fn tooltip_key(italic: bool, scale: f32) -> rewo_gpu::velvet_glyph::ScalerKey {
    rewo_gpu::velvet_glyph::ScalerKey::new(
        rewo_gpu::velvet_glyph::Family::Newsreader,
        italic,
        TOOLTIP_TEXT_GUI_PX * scale,
        rewo_gpu::velvet_glyph::Axes::DEFAULT,
    )
}

/// Width of a styled tooltip line in **GUI pixels**, measured with the same
/// font that will draw it.
///
/// This is the half of the flip that is easy to skip and would have shown up
/// as text spilling out of its box: once the tooltip renders in Newsreader,
/// sizing it with the bitmap advances measures a font it no longer uses.
pub(super) fn velvet_line_width(
    cache: &mut rewo_gpu::velvet_glyph::GlyphCache,
    line: &rewo_gpu::tooltip::Line,
    scale: f32,
) -> f32 {
    line.iter()
        .map(|sp| cache.measure_tracked(tooltip_key(sp.italic, scale), &sp.text, 0.0))
        .sum::<f32>()
        / scale.max(0.001)
}
/// The hovered slot's tooltip: the box to draw, and the text line inside it
/// (M40).
///
/// Vanilla builds the lines with `Screen.getTooltipFromItem`, which starts
/// with the stack's hover name and then appends everything its **components**
/// say — enchantments, lore, durability, attribute modifiers, the "When on
/// body" block. Rewo can see none of those: `rewo_net::item_stack` reports
/// only *whether* a patch was present. So a Rewo tooltip is the first line and
/// nothing else, which is exactly what vanilla shows for a plain stack, and
/// short of what it shows for an enchanted one. Drawing a wrong second line
/// would be worse than drawing none.
///
/// The name is also always in the common rarity's white: rarity rides on
/// `DataComponents.RARITY`, which is another component.
/// The durability bars for a set of slots (M41).
///
/// `ItemStack.isBarVisible()` is `isDamaged()`, so a pristine tool has **no
/// bar**, not a full one — which is why this returns nothing for an undamaged
/// stack rather than a 13-wide green one.
///
/// The two halves of the fraction come from different places, and that is the
/// point of the milestone: the numerator `minecraft:damage` rides on the wire
/// as a component patch, the denominator does not — every diamond pickaxe has
/// the same 1561, so it lives in the generated item table. A patch that
/// overrides `max_damage` still wins, because a plugin may.
pub(super) fn item_bars(
    slots: &[(Option<rewo_world::inventory::ItemSlot>, (f32, f32, f32))],
    items: &rewo_data::items::Items,
    unbreakable: impl Fn(rewo_world::inventory::ItemSlot) -> bool,
) -> Vec<rewo_gpu::container::ItemBar> {
    let mut out = Vec::new();
    for (stack, (x, y, size)) in slots {
        let Some(stack) = stack else { continue };
        let max = stack
            .max_damage
            .or_else(|| items.name(stack.item_id).and_then(rewo_data::item_props_table::max_damage));
        // An item whose maximum this build cannot resolve gets no bar. A bar
        // needs a denominator, and inventing one would draw a confident and
        // wrong amount of remaining life.
        let Some(max) = max.filter(|m| *m > 0) else {
            continue;
        };
        // `isBarVisible()` is `isDamaged()`, and that is
        // `has(MAX_DAMAGE) && !has(UNBREAKABLE) && has(DAMAGE) && damage > 0`
        // — so an **Unbreakable** tool never shows one however much damage it
        // carries (M66 corrected this; M41 read the damage alone). The damage
        // is also clamped to the maximum, which is what stops a server sending
        // more than the item can take from producing a negative width.
        let damage = stack.damage.unwrap_or(0).clamp(0, max);
        if damage <= 0 || unbreakable(*stack) {
            continue;
        }
        out.push(rewo_gpu::container::ItemBar {
            x: *x,
            y: *y,
            // The slot rects are in screen pixels at `size` per 16 GUI pixels.
            scale: size / 16.0,
            width: rewo_gpu::container::bar_width(damage, max),
            color: rewo_gpu::container::bar_color(damage, max),
        });
    }
    out
}

/// `ItemStack.getRarity()` — the id whose colour the hover name takes (M50).
///
/// ```java
/// Rarity baseRarity = this.getOrDefault(DataComponents.RARITY, Rarity.COMMON);
/// if (!this.isEnchanted()) return baseRarity;
/// return switch (baseRarity) {
///    case COMMON, UNCOMMON -> Rarity.RARE;
///    case RARE             -> Rarity.EPIC;
///    default               -> baseRarity;
/// };
/// ```
///
/// Two halves, and Rewo could previously see only one of them. `getOrDefault`
/// answers from the item's **prototype** when the patch says nothing, and the
/// patch is all the wire carries — so `rarity.unwrap_or(COMMON)` painted all
/// **115** of 26.2's non-common items white. The prototype half is
/// [`rewo_data::item_props_table::rarity`], generated from the datagen
/// component report.
///
/// `is_enchanted` is `minecraft:enchantments` alone — see
/// [`rewo_world::inventory::SlotText::is_enchanted`] on why an enchanted book
/// is not enchanted.
///
/// An id outside the enum passes through the `default` arm unchanged, exactly
/// as a future `Rarity` constant would.
pub(crate) fn stack_rarity(item_name: Option<&str>, patch: Option<i32>, is_enchanted: bool) -> i32 {
    let base = patch.unwrap_or_else(|| {
        item_name
            .map(rewo_data::item_props_table::rarity)
            .unwrap_or(rewo_data::item_props_table::DEFAULT_RARITY)
    });
    if !is_enchanted {
        return base;
    }
    match base {
        0 | 1 => 2,
        2 => 3,
        other => other,
    }
}

/// `Rarity.color()` — the hover name's colour, by rarity id.
///
/// An unknown id is treated as common rather than wrapping into another
/// colour: the enum is a small one today and a version that grows it should
/// not repaint every item.
pub(super) fn rarity_color(rarity: i32) -> [f32; 3] {
    let rgb = match rarity {
        1 => 0xFFFF55u32, // UNCOMMON — yellow
        2 => 0x55FFFF,    // RARE — aqua
        3 => 0xFF55FF,    // EPIC — light purple
        _ => 0xFFFFFF,    // COMMON
    };
    [
        ((rgb >> 16) & 0xFF) as f32 / 255.0,
        ((rgb >> 8) & 0xFF) as f32 / 255.0,
        (rgb & 0xFF) as f32 / 255.0,
    ]
}

/// `ItemLore`'s style — dark purple and italic. Rewo has no italic face, so
/// only the colour carries.
pub(super) const LORE_COLOR: [f32; 3] = [170.0 / 255.0, 0.0, 170.0 / 255.0];
/// `ItemStack.UNBREAKABLE_TOOLTIP`, which is blue.
pub(super) const UNBREAKABLE_COLOR: [f32; 3] = [85.0 / 255.0, 85.0 / 255.0, 1.0];

/// `ChatFormatting.GRAY` — an ordinary enchantment line.
pub(super) const ENCHANT_COLOR: [f32; 3] = [170.0 / 255.0, 170.0 / 255.0, 170.0 / 255.0];
/// `ChatFormatting.RED` — a curse's.
pub(super) const CURSE_COLOR: [f32; 3] = [1.0, 85.0 / 255.0, 85.0 / 255.0];

/// `Enchantment.getFullname` for each of a stack's enchantments, in
/// `ItemEnchantments.addToTooltip`'s order (M42).
///
/// Three rules, each of which is a way to be visibly wrong:
///
/// - **The level numeral is suppressed only when `level == 1 && maxLevel == 1`.**
///   So a level-1 Mending (max 1) reads "Mending" and a level-1 Sharpness
///   (max 5) reads "Sharpness I". Suppressing on `level == 1` alone loses the
///   numeral from every single-level enchant a player actually applies.
/// - **A curse is red**, everything else grey.
/// - **The order is the `minecraft:tooltip_order` tag first**, then whatever
///   the stack carries that the tag does not mention — appended after, in the
///   stack's own order, whatever their ids.
///
/// An id the registry does not contain yields **no line**. That case means the
/// server sent an enchantment this session never synced, and inventing a name
/// for it would be worse than the omission.
pub(crate) fn enchantment_lines(
    enchantments: &[(i32, i32)],
    registry: &[rewo_net::enchantment_parse::EnchantmentDef],
    text: &rewo_data::enchantments::EnchantmentText,
) -> Vec<rewo_gpu::tooltip::Line> {
    let mut rows: Vec<(Option<usize>, usize, String, [f32; 3])> = Vec::new();
    for (order, &(id, level)) in enchantments.iter().enumerate() {
        let Some(def) = usize::try_from(id).ok().and_then(|i| registry.get(i)) else {
            continue;
        };
        // A datapack may name an enchantment with literal text rather than a
        // translation key; translating that would find nothing.
        let name = if def.literal {
            Some(def.description_key.clone())
        } else {
            text.translate(&def.description_key).map(str::to_string)
        };
        let Some(name) = name else { continue };
        let line = if level == 1 && def.max_level == 1 {
            name
        } else {
            format!("{name} {}", text.level(level))
        };
        let color = if text.is_curse(&def.id) {
            CURSE_COLOR
        } else {
            ENCHANT_COLOR
        };
        rows.push((text.tooltip_rank(&def.id), order, line, color));
    }
    // `None` sorts after `Some` for an `Option` key, which is exactly the
    // behaviour wanted here: the tag's members first, in tag order, then the
    // rest in the order the stack listed them.
    rows.sort_by_key(|(rank, order, _, _)| (rank.is_none(), *rank, *order));
    // One span per line here -- an enchantment name is uniformly coloured. The
    // span model earns its keep on lore (italic) and leaves room for the
    // mid-line colour changes vanilla does elsewhere.
    rows.into_iter()
        .map(|(_, _, l, c)| vec![rewo_gpu::tooltip::Span::new(l, c)])
        .collect()
}

/// `ItemContainerContents.addToTooltip` (M66) — the shulker-box preview.
///
/// The line count comes from [`rewo_gpu::tooltip::container_plan`], which is
/// the loop verbatim; what happens here is the translation, and the two keys
/// it needs (`item.container.item_count`, `item.container.more_items`) exist
/// **only after M54's deprecation pass** — `en_us.json` still carries them
/// under their pre-rename `container.shulkerBox.*` names, so a raw read of the
/// language file produces no container lines at all.
///
/// A present stack whose item id this session cannot name drops the **whole**
/// block rather than one line: the remainder is computed from the stack count,
/// so omitting a line silently makes "and 2 more…" wrong as well.
pub(crate) fn container_lines(
    slots: &[Option<rewo_net::item_stack::ContainerSlot>],
    items: &rewo_data::items::Items,
    names: &std::collections::HashMap<String, String>,
    lang: &rewo_data::lang::Language,
) -> Vec<rewo_gpu::tooltip::Line> {
    use rewo_gpu::tooltip::Span;
    // `nonEmptyItemsStream` — a gap is a real slot position, and the tooltip
    // is the one consumer that does not care. The filter belongs here rather
    // than in the decode, which is exactly why M63 keeps the gaps.
    let entries: Vec<&rewo_net::item_stack::ContainerSlot> = slots.iter().flatten().collect();
    if entries.is_empty() {
        return Vec::new();
    }
    let (Some(count_key), Some(more_key)) = (
        lang.get("item.container.item_count"),
        lang.get("item.container.more_items"),
    ) else {
        return Vec::new();
    };
    let plan = rewo_gpu::tooltip::container_plan(entries.len());
    let mut out = Vec::new();
    for e in entries.iter().take(plan.shown) {
        let Some(translated) = items.name(e.item_id).and_then(|n| names.get(n)) else {
            return Vec::new();
        };
        out.push(vec![Span::new(
            rewo_data::lang::format(
                count_key,
                &[
                    &e.hover_name(translated, Some(lang)),
                    &e.count.to_string(),
                ],
            ),
            GRAY_TEXT,
        )]);
    }
    if plan.more > 0 {
        // `.withStyle(ChatFormatting.ITALIC)` — expressible since M52b's span
        // model, and rendered as the italic face by the Velvet pass.
        out.push(vec![Span::new(
            rewo_data::lang::format(more_key, &[&plan.more.to_string()]),
            GRAY_TEXT,
        )
        .italic()]);
    }
    out
}

/// `ItemStack.getStyledHoverName()` as one tooltip line (M163).
///
/// ```java
/// MutableComponent n = Component.empty().append(getHoverName())
///                               .withStyle(getRarity().color());
/// if (this.has(DataComponents.CUSTOM_NAME)) n.withStyle(ChatFormatting.ITALIC);
/// ```
/// (`ItemStack.java:829-836`.)
///
/// **Two things here are only expressible because `SlotText` keeps
/// `custom_name` and `item_name` apart.** `getHoverName()` is
/// `getCustomName() ?? getItemName()`, so the value falls back — but the ITALIC
/// keys off `has(CUSTOM_NAME)` alone, so an item whose patch sets only
/// `minecraft:item_name` is renamed and **not** slanted. A merged
/// `custom_name.or(item_name)` field cannot say which one answered.
///
/// And the name is a *component*, resolved here against `lang` rather than at
/// the wire — see [`rewo_world::chat_style::flatten`] on why the decode cannot.
///
/// Extracted from the tooltip closure so a gate can reach it: that closure
/// needs a `PlaySession`, an `Inventory` and four registries, which is M97's
/// rule (a rule with no reachable seam is a rule no witness can grade).
pub(crate) fn styled_hover_name(
    text: Option<&rewo_world::inventory::SlotText>,
    translated: &str,
    color: [f32; 3],
    lang: &rewo_data::lang::Language,
) -> rewo_gpu::tooltip::Line {
    let custom = text.and_then(|t| t.custom_name.as_ref());
    let named = custom.or_else(|| text.and_then(|t| t.item_name.as_ref()));
    let span = rewo_gpu::tooltip::Span::new(
        match named {
            Some(tag) => rewo_world::chat_style::flatten(tag, Some(lang)),
            None => translated.to_string(),
        },
        color,
    );
    vec![if custom.is_some() { span.italic() } else { span }]
}

/// `minecraft:lore`'s lines, in `ItemLore.LORE_STYLE` (M163).
///
/// `Style.EMPTY.withColor(DARK_PURPLE).withItalic(true)`. The colour was always
/// right; the italic had nowhere to live until M52b's span model, and the TEXT
/// was the raw component's literal until M163 — so a lore line carrying a
/// `translate` showed its key and one carrying a legacy colour code showed the
/// code.
pub(crate) fn lore_lines(
    text: &rewo_world::inventory::SlotText,
    lang: &rewo_data::lang::Language,
) -> Vec<rewo_gpu::tooltip::Line> {
    text.lore
        .iter()
        .map(|line| {
            vec![rewo_gpu::tooltip::Span::new(
                rewo_world::chat_style::flatten(line, Some(lang)),
                LORE_COLOR,
            )
            .italic()]
        })
        .collect()
}

/// `ItemStack.addDetailsToTooltip`'s advanced block, translated (M66).
///
/// The order and the arguments are [`rewo_gpu::tooltip::advanced_lines`]'s;
/// this resolves the two keys and the registry id, which is a **literal** and
/// therefore never goes through the language file.
pub(crate) fn advanced_tooltip_lines(
    lines: &[rewo_gpu::tooltip::AdvancedLine],
    registry_key: &str,
    lang: &rewo_data::lang::Language,
) -> Vec<rewo_gpu::tooltip::Line> {
    use rewo_gpu::tooltip::{AdvancedLine, Span, DARK_GRAY};
    lines
        .iter()
        .filter_map(|l| match l {
            AdvancedLine::Durability { remaining, max } => {
                let key = lang.get("item.durability")?;
                Some(vec![Span::new(
                    rewo_data::lang::format(key, &[&remaining.to_string(), &max.to_string()]),
                    WHITE_TEXT,
                )])
            }
            AdvancedLine::RegistryId => {
                Some(vec![Span::new(registry_key.to_string(), DARK_GRAY)])
            }
            AdvancedLine::Components { count } => {
                let key = lang.get("item.components")?;
                Some(vec![Span::new(
                    rewo_data::lang::format(key, &[&count.to_string()]),
                    DARK_GRAY,
                )])
            }
        })
        .collect()
}

/// `PatchedDataComponentMap.has(type)` for a decoded stack (M66).
///
/// The prototype answers first and the patch overrides it, which is what
/// separates an addition from an override for
/// [`rewo_net::item_stack::StackComponents::component_count`] — and is also
/// how `isDamageableItem`'s three `has(...)` calls are resolved.
///
/// `None` means unanswerable: either the item is outside this build's
/// prototype table or the component id has no name in the runtime registry.
pub(crate) fn stack_has_component(
    item: &str,
    component: &str,
    detail: Option<&rewo_net::item_stack::StackDetail>,
    registry: Option<&rewo_data::components::DataComponentRegistry>,
) -> Option<bool> {
    let mut has = rewo_data::item_components_table::prototype_has_component(item, component)?;
    if let (Some(d), Some(reg)) = (detail, registry) {
        for &id in &d.added {
            if reg.name_of(id)? == component {
                has = true;
            }
        }
        for &id in &d.removed {
            if reg.name_of(id)? == component {
                has = false;
            }
        }
    }
    Some(has)
}

/// `ChatFormatting.GRAY` — the container's own preview lines.
pub(super) const GRAY_TEXT: [f32; 3] = [170.0 / 255.0, 170.0 / 255.0, 170.0 / 255.0];
/// An unstyled tooltip line, which `GuiGraphics.tooltip` draws in white.
pub(super) const WHITE_TEXT: [f32; 3] = [1.0, 1.0, 1.0];

#[allow(clippy::too_many_arguments)]
pub(crate) fn screen_tooltip(
    inv: &rewo_world::inventory::Inventory,
    items: &rewo_data::items::Items,
    names: &std::collections::HashMap<String, String>,
    lang: &rewo_data::lang::Language,
    enchant_registry: &[rewo_net::enchantment_parse::EnchantmentDef],
    enchant_text: &rewo_data::enchantments::EnchantmentText,
    details: &rewo_net::item_stack::StackDetails,
    component_registry: Option<&rewo_data::components::DataComponentRegistry>,
    flag: rewo_gpu::tooltip::TooltipFlag,
    advance: &[u8; 256],
    glyphs: Option<&mut rewo_gpu::velvet_glyph::GlyphCache>,
    mouse: (f64, f64),
    (w, h): (f32, f32),
    // M93k — the SHOWN menu's layout. Without it this hover centres a
    // 176x166 panel and scans the player's 46 slots, which is the M89 bug in
    // a fourth consumer: the highlight and the icons were made container-aware
    // and this one was not, so with a chest open the tooltip named whatever
    // the PLAYER happened to have at the same index.
    layout: &'static rewo_world::menu_layout::MenuLayout,
    open: Option<&rewo_world::menu::OpenMenu>,
    // `player.isSpectator()` — the fifth of the hint's conditions.
    spectator: bool,
    // M106b — whether the recipe book is open, which MOVES the menu. The panel,
    // its slot icons and its hover highlight all resolve their origin through
    // `Placement::with_book`; this one resolved through the centred form, so
    // with the book up it named a slot 77 GUI px right of the cursor.
    book_open: bool,
) -> Option<(
    rewo_gpu::container::TooltipDraw,
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<rewo_gpu::velvet_text::OwnedRun>,
)> {
    // A stack on the cursor suppresses the tooltip: vanilla's guard is
    // `hoveredSlot.hasItem() && getCarried().isEmpty()`, so picking something
    // up hides the label of whatever you drag it over.
    if inv.carried().is_some() {
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
    let slot = layout.slot_at(gx, gy)?;
    // M93k — the crafter's `gui.togglable_slot` hint, which is shown on an
    // EMPTY slot and so must be resolved before the item tooltip's
    // `menu_slot(slot)?` bails out.
    //
    // Vanilla's five conditions here are exactly the preconditions of a PICKUP
    // that would DISABLE the slot, so the hint is DERIVED from the same
    // decision the click uses rather than transcribed a second time — the two
    // then cannot disagree about whether a click would do anything. The
    // string says as much: the constant is named DISABLED_SLOT_TOOLTIP and
    // reads "Click to disable slot", and it appears on an ENABLED slot.
    let crafter_hint: Option<Vec<rewo_gpu::tooltip::Line>> = open.and_then(|m| {
        if !rewo_world::menu::is_crafter_grid_slot(layout.protocol_id, slot as i32) {
            return None;
        }
        let would_disable = rewo_world::menu::crafter_toggle(
            rewo_world::inventory::CONTAINER_INPUT_PICKUP,
            m.crafter_slot_disabled(slot as i32),
            inv.menu_slot(slot).is_some(),
            spectator,
            // Already known empty — the guard at the top of this function
            // returns early otherwise — but passed rather than hard-coded, so
            // the two guards cannot drift apart.
            inv.carried().is_none(),
            false,
        ) == rewo_world::menu::CrafterToggle::Disable;
        if !would_disable {
            return None;
        }
        lang.get("gui.togglable_slot")
            .map(|t| vec![vec![rewo_gpu::tooltip::Span::new(t.to_string(), [1.0, 1.0, 1.0])]])
    });
    // The item tooltip's content. A closure so the crafter's hint can
    // select between the two and share the assembly below — the measure,
    // position and glyph-run code is the same for any tooltip, and a second
    // copy of it is how two tooltips come to sit in different places.
    let item_lines = || -> Option<Vec<rewo_gpu::tooltip::Line>> {
        let stack = inv.menu_slot(slot)?;
        let item_name = items.name(stack.item_id)?;
        let translated = names.get(item_name)?;

        // `getTooltipLines` in vanilla's order: the styled hover name, then the
        // details each component contributes. Rewo produces the three it can read
        // exactly — the name, the lore, and the `Unbreakable` marker.
        //
        let text = inv.text_of(stack);
        // A line is a sequence of styled spans (M52b), not a string and a colour.
        // Vanilla styles per run, and the old model was strictly less: it could
        // not say "italic", so `ItemLore.LORE_STYLE`'s italic was silently
        // dropped. Geometry is unchanged -- the box is still measured from the
        // plain text with the vanilla advances.
        let mut lines: Vec<rewo_gpu::tooltip::Line> = Vec::new();
        lines.push(styled_hover_name(
            text,
            translated,
            rarity_color(stack_rarity(
                Some(item_name),
                text.and_then(|t| t.rarity),
                text.is_some_and(|t| t.is_enchanted),
            )),
            lang,
        ));
        if let Some(t) = text {
            // Vanilla's order: the enchantments come before the lore.
            lines.extend(enchantment_lines(
                &t.enchantments,
                enchant_registry,
                enchant_text,
            ));
            lines.extend(lore_lines(t, lang));
            if t.unbreakable {
                lines.push(vec![rewo_gpu::tooltip::Span::new(
                    "Unbreakable".to_string(),
                    UNBREAKABLE_COLOR,
                )]);
            }
        }
        // M66 stage 4: `minecraft:container`'s preview. Vanilla's component
        // tooltips run in `DataComponents` registration order and `container`
        // comes after the lore block, which is where this sits.
        let detail = details.get(stack.components);
        if let Some(d) = detail {
            lines.extend(container_lines(&d.container, items, names, lang));
        }
        // M66 stage 3: the advanced block, last of everything a component adds.
        {
            let has = |c: &str| stack_has_component(item_name, c, detail, component_registry);
            let durability = rewo_gpu::tooltip::DurabilityState {
                damage: stack.damage.unwrap_or(0),
                max: stack
                    .max_damage
                    .or_else(|| rewo_data::item_props_table::max_damage(item_name))
                    .unwrap_or(0),
                has_max_damage: has("minecraft:max_damage").unwrap_or(false),
                has_damage: has("minecraft:damage").unwrap_or(false),
                unbreakable: has("minecraft:unbreakable").unwrap_or(false),
            };
            // `PatchedDataComponentMap.size()`. Both halves can decline — an item
            // outside the prototype table, or a component id with no name — and
            // either way the line is dropped rather than guessed.
            let count = match detail {
                Some(d) => rewo_net::item_stack::StackComponents {
                    added: d.added.clone(),
                    removed: d.removed.clone(),
                    ..Default::default()
                }
                .component_count(
                    || rewo_data::item_components_table::prototype_component_count(item_name),
                    |id| {
                        let name = component_registry?.name_of(id)?;
                        rewo_data::item_components_table::prototype_has_component(item_name, name)
                    },
                ),
                // No patch at all: the merged size is the prototype's.
                None => rewo_data::item_components_table::prototype_component_count(item_name),
            };
            // `display.shows(DataComponents.DAMAGE)` — Rewo does not interpret
            // `minecraft:tooltip_display`, so this is `TooltipDisplay.DEFAULT`,
            // which shows everything. A server that hid the damage line would
            // still see it here.
            let advanced = rewo_gpu::tooltip::advanced_lines(flag, durability, true, count);
            lines.extend(advanced_tooltip_lines(&advanced, item_name, lang));
        }

        Some(lines)
    };
    let lines = match crafter_hint {
        Some(l) => l,
        None => item_lines()?,
    };
    tooltip_layout(lines, advance, glyphs, mouse, (w, h))
}

/// The tooltip of the recipe cell under the cursor (M106).
///
/// `RecipeBookPage.extractTooltip` — `Screen.getTooltipFromItem(displayStack)`
/// plus a "Right Click for More" line when the collection holds more than one
/// recipe.
///
/// **This loses to the menu's own tooltip, and that is not the order it is
/// called in.** `AbstractRecipeBookScreen` runs `this.extractTooltip` (the
/// container's) and *then* `recipeBookComponent.extractTooltip`, which reads as
/// "the book overwrites it" — and `setTooltipForNextFrameInternal`'s body is
/// `if (this.deferredTooltip == null || replaceExisting)`, with
/// `replaceExisting` false on every path either of them takes. So the FIRST
/// tooltip of a frame wins and the book's is discarded. Hence the caller's
/// `screen_tooltip(..).or_else(..)` rather than the reverse.
///
/// For a page cell the two can never both fire — the book sits beside the menu
/// on a wide window, and on a narrow one `AbstractRecipeBookScreen.isHovering`
/// returns false for every slot, so there is no hovered slot to describe. It is
/// the GHOST tooltip that makes first-wins observable; see [`ghost_tooltip`].
///
/// The stack is Rewo's `slot_items` id, which carries no components (M95): a
/// recipe result that shipped with a custom name or lore would show its plain
/// name here. The advanced block is still produced, because F3+H adds the
/// registry id to a book cell in vanilla too.
#[allow(clippy::too_many_arguments)]
pub(crate) fn book_tooltip(
    b: &BookRender,
    overlay_open: bool,
    book_mouse: Option<(i32, i32)>,
    items: &rewo_data::items::Items,
    names: &std::collections::HashMap<String, String>,
    lang: &rewo_data::lang::Language,
    flag: rewo_gpu::tooltip::TooltipFlag,
    advance: &[u8; 256],
    glyphs: Option<&mut rewo_gpu::velvet_glyph::GlyphCache>,
    mouse: (f64, f64),
    (w, h): (f32, f32),
) -> Option<(
    rewo_gpu::container::TooltipDraw,
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<rewo_gpu::velvet_text::OwnedRun>,
)> {
    use rewo_world::recipe_book_screen as rb;
    let view = b.view?;
    let (bx, by) = book_mouse?;
    let slot = rb::page_tooltip_slot(rb::book_hit(bx, by, view, view.tabs), overlay_open)?;
    // `getDisplayStack()` — the item the cycle is currently on, which is
    // already what `slot_items` holds. A collection whose result needs a
    // context Rewo cannot build has `None` and shows nothing, rather than a
    // tooltip for some other member of the group.
    let item = (*b.slot_items.get(slot)?)?;
    let item_name = items.name(item)?;
    let mut lines: Vec<rewo_gpu::tooltip::Line> = vec![vec![rewo_gpu::tooltip::Span::new(
        names.get(item_name)?.to_string(),
        rarity_color(stack_rarity(Some(item_name), None, false)),
    )]];
    {
        let has = |c: &str| stack_has_component(item_name, c, None, None);
        let durability = rewo_gpu::tooltip::DurabilityState {
            // A recipe result is undamaged by construction: the display stack
            // comes from the recipe, not from an inventory.
            damage: 0,
            max: rewo_data::item_props_table::max_damage(item_name).unwrap_or(0),
            has_max_damage: has("minecraft:max_damage").unwrap_or(false),
            has_damage: has("minecraft:damage").unwrap_or(false),
            unbreakable: has("minecraft:unbreakable").unwrap_or(false),
        };
        let advanced = rewo_gpu::tooltip::advanced_lines(
            flag,
            durability,
            true,
            rewo_data::item_components_table::prototype_component_count(item_name),
        );
        lines.extend(advanced_tooltip_lines(&advanced, item_name, lang));
    }
    // `if (this.hasMultipleRecipes()) texts.add(MORE_RECIPES_TOOLTIP)` — LAST,
    // after everything the item contributed, including the advanced block.
    if b.slots.get(slot).is_some_and(|&(_, multiple)| multiple) {
        lines.push(vec![rewo_gpu::tooltip::Span::new(
            lang.or_key(rb::MORE_RECIPES_KEY).to_string(),
            [1.0, 1.0, 1.0],
        )]);
    }
    tooltip_layout(lines, advance, glyphs, mouse, (w, h))
}

/// Turn a resolved set of tooltip lines into its box, position and glyph runs
/// (M106).
///
/// Extracted from [`screen_tooltip`] unchanged so the recipe book's own
/// tooltips can share it. The comment it was extracted from already said why —
/// "the measure, position and glyph-run code is the same for any tooltip, and a
/// second copy of it is how two tooltips come to sit in different places" — and
/// M106 is the second producer that makes it true rather than prospective.
pub(super) fn tooltip_layout(
    lines: Vec<rewo_gpu::tooltip::Line>,
    advance: &[u8; 256],
    glyphs: Option<&mut rewo_gpu::velvet_glyph::GlyphCache>,
    mouse: (f64, f64),
    (w, h): (f32, f32),
) -> Option<(
    rewo_gpu::container::TooltipDraw,
    Vec<rewo_gpu::world::OwnedTextLine>,
    Vec<rewo_gpu::velvet_text::OwnedRun>,
)> {
    // `if (!lines.isEmpty())` in `setTooltipForNextFrameInternal` — an empty
    // list sets no tooltip at all, which is not the same as an empty box.
    if lines.is_empty() {
        return None;
    }
    // Measure with the font that will DRAW. Once the tooltip renders in
    // Newsreader, sizing it with the bitmap advances measures a font it no
    // longer uses -- which shows up as text spilling out of its box, and is
    // the half of this flip that is easiest to skip.
    let scale = rewo_gpu::hud::gui_scale(w, h);
    let mut glyphs = glyphs;
    let widths: Vec<i32> = match glyphs.as_deref_mut() {
        Some(cache) => lines
            .iter()
            .map(|l| velvet_line_width(cache, l, scale).ceil() as i32)
            .collect(),
        None => lines
            .iter()
            .map(|l| {
                rewo_data::sign_text::width(&rewo_gpu::tooltip::line_text(l), advance).round()
                    as i32
            })
            .collect(),
    };
    let (tw, th) = rewo_gpu::container::tooltip_size(&widths);
    // The positioner works in GUI pixels, and so does the screen size it
    // clamps against — `guiWidth()`/`guiHeight()` are the *scaled* dimensions,
    // not the framebuffer's. Passing raw pixels here would let a tooltip run
    // off the right of a small window before the flip ever triggered.
    let scale = rewo_gpu::hud::gui_scale(w, h);
    let (sw, sh) = ((w / scale) as i32, (h / scale) as i32);
    let (mx, my) = ((mouse.0 / scale as f64) as i32, (mouse.1 / scale as f64) as i32);
    let (tx, ty) = rewo_gpu::container::tooltip_position(sw, sh, mx, my, tw, th);
    // `localY += line.getHeight(font) + (i == 0 ? 2 : 0)` — the gap goes after
    // the **first** line only, which is what separates the name from the
    // details without spacing the details apart.
    let mut y = ty;
    // With the cache present the text goes through the Velvet pass, which is
    // what makes italic lore actually slant. The bitmap path stays as the
    // fallback for a build with no fonts on disk.
    if let Some(cache) = glyphs.as_deref_mut() {
        let mut runs: Vec<rewo_gpu::velvet_text::OwnedRun> = Vec::new();
        let mut ly = ty;
        for (i, spans) in lines.iter().enumerate() {
            // Vanilla's `y` is the line's TOP; Velvet lays out from the
            // BASELINE. Dropping the ascent would raise every tooltip line by
            // most of its own height.
            let ascent = cache
                .metrics(tooltip_key(false, scale))
                .map(|m| m.ascent)
                .unwrap_or(TOOLTIP_TEXT_GUI_PX * scale * 0.75);
            let baseline = ly as f32 * scale + ascent;
            let mut pen = tx as f32 * scale;
            for sp in spans {
                let key = tooltip_key(sp.italic, scale);
                let mut g = Vec::new();
                let adv = cache.layout_run(key, &sp.text, 0.0, (pen, baseline), &mut g);
                pen += adv;
                if !g.is_empty() {
                    runs.push(rewo_gpu::velvet_text::OwnedRun {
                        glyphs: g,
                        color: sp.color,
                        alpha: 1.0,
                    });
                }
            }
            ly += rewo_gpu::container::TOOLTIP_LINE_HEIGHT + if i == 0 { 2 } else { 0 };
        }
        return Some((
            rewo_gpu::container::TooltipDraw {
                pos: (tx, ty),
                size: (tw, th),
                bundle: None,
            },
            Vec::new(),
            runs,
        ));
    }
    let out = lines
        .into_iter()
        .enumerate()
        .map(|(i, spans)| {
            // The bitmap pass takes one colour per line, so the vanilla-font
            // path still draws the first span's. That is a property of THAT
            // pass, not of the model: the styled line already carries every
            // span, and `tooltip::to_velvet_spans` renders it in full through
            // the Velvet type stack. Switching which pass a tooltip uses is a
            // rendering decision, deliberately left open while the HUD's
            // visual direction settles.
            let color = spans.first().map(|s| s.color).unwrap_or([1.0; 3]);
            let line = rewo_gpu::world::OwnedTextLine {
                x: tx as f32 * scale,
                y: y as f32 * scale,
                px: scale,
                color_linear: srgb_bytes_to_linear_f(color),
                alpha: 1.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text: rewo_gpu::tooltip::line_text(&spans),
            };
            y += rewo_gpu::container::TOOLTIP_LINE_HEIGHT + if i == 0 { 2 } else { 0 };
            line
        })
        .collect();
    Some((
        rewo_gpu::container::TooltipDraw {
            pos: (tx, ty),
            size: (tw, th),
            // **The decode exists; the carrier does not.** M61 made the patch
            // reader *keep* `minecraft:bundle_contents` — see
            // `rewo_net::item_stack::StackComponents::bundle_contents`, which
            // returns the stacks as `ItemTemplate`s. What is still missing is
            // a way to get them from there to here.
            //
            // The tooltip reads its stack out of `rewo_world`'s `Inventory`,
            // and neither of that crate's two carriers takes a `Vec` today:
            // `ItemSlot` is `Copy` on purpose (the click arithmetic moves it
            // through a dozen struct-update expressions, and growing it would
            // make every one of those a clone), and `SlotText` — the non-`Copy`
            // side-channel keyed by the component fingerprint — is the natural
            // home but would need its `is_empty` taught the new field, or a
            // bundle carrying *only* `bundle_contents` would be recorded as
            // having no text and dropped. That is the exact bug M42's
            // enchantments hit.
            //
            // Left `None` rather than guessed: `container::bundle_chrome` and
            // `tooltip::bundle_image` are both graded by `inventoryshot`
            // against synthetic bundles, so an empty grid drawn from a full
            // bundle would be a confident wrong answer with a green gate
            // behind it.
            bundle: None,
        },
        out,
        Vec::new(),
    ))
}
