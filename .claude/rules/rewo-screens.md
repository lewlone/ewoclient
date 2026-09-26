---
paths:
  - "crates/rewo-world/src/*screen*.rs"
  - "crates/rewo-world/src/menu*.rs"
  - "crates/rewo-world/src/inventory.rs"
  - "crates/rewo-world/src/recipe_*.rs"
  - "crates/rewo-world/src/stacked_contents.rs"
  - "crates/rewo-world/src/ghost_slots.rs"
  - "crates/rewo-world/src/edit_box.rs"
  - "crates/rewo-world/src/anvil.rs"
  - "crates/rewo-world/src/chat*.rs"
  - "crates/rewo-world/src/command_suggestions.rs"
  - "crates/rewo-world/src/suggestions.rs"
  - "crates/rewo-world/src/nine_slice.rs"
  - "crates/rewo-net/src/menu.rs"
  - "crates/rewo-net/src/merchant.rs"
  - "crates/rewo-net/src/recipe_book.rs"
  - "crates/rewo-net/src/item_stack.rs"
  - "crates/rewo-net/src/commands.rs"
  - "crates/rewo-app/src/containershot_cmd.rs"
  - "crates/rewo-app/src/inventoryshot_cmd.rs"
  - "tools/check_menu_layouts.py"
---

# Rewo screens — containers, recipe book, chat, command line, sign/book/options

Reasoning per rule: `docs/rewo/milestones-M87-M142.md` (containers, recipe
book, chat) and `docs/history/claude-md-session-log.md` (M143–M180).

## Structure

- **Vanilla has one screen slot, not a stack.** A nested screen is a replacement
  carrying a callback, and anything opening over a screen must drop the stale
  one's state.
- **One accessor for "the shown menu"** (`PlaySession::shown_menu{,_mut}`,
  `shown_container_id`). Clicks, prediction, hover, tooltip and the packet's
  ids all go through it. `state_id` is **per menu**.
- **One conversion for screen → panel space** (`Placement`), including the
  recipe book's 77 px displacement. Every consumer (click, double-click, drag,
  highlight, tooltip, icons) must ask it.
- `slot_kind` is `MenuLayout::slot_kind`, **per layout**. Functions that take a
  bare index are the ones missed when a type generalises.
- Menu slot layouts are a **hand table** checked by
  `tools/check_menu_layouts.py`, because four Java idioms defeat a generator.
  Oddities: `crafter_3x3` puts its result **after** the player inventory,
  `crafting` puts it first, and `lectern` has one slot and no player inventory.
- `quickMoveStack` is **per menu class**. Untranscribed menus return
  `Unimplemented` and the caller **declines**, because moving nothing is inert
  and moving the wrong stack isn't.

## Item predicates

- `has(X)` = removed → false, patch-set → true, else
  `rewo_data::item_components_table::prototype_has_component`.
- **`ItemSlot::enchanted` is `has_foil()`, not `isEnchanted()`.**
  `ItemSlot::any_enchantments` is enchantments **or** stored enchantments (the
  grindstone's `hasAnyEnchantments`). `SlotText::is_enchanted` is
  `isEnchanted()` proper, the one crafting uses.
- Jar-derived tables (smelting, stonecutter, beacon payment, loom, fuel) come
  from `tools/gen_*.py`. The stonecutter **order is a wire contract** (a click
  sends an index): sort by recipe file stem, path first, then namespace.

## Text input and tooltips

- `EditBox` stores `Vec<u16>` because every index is a Java UTF-16 index (the
  surrogate rule is only expressible there). Name limits count code units.
- **The first tooltip of a frame wins** (`setTooltipForNextFrameInternal`
  doesn't replace), which is why a recipe cell's tooltip loses to the menu's.
- The anvil swallows every non-Esc key while its field can type. The sign
  editor **commits on every exit**, including being replaced by another screen.
- Chat: `wrapComponents` uses the `splitLines` overload with per-line
  `isWrapped`. `delete_chat` needs the `MessageSignatureCache` (move-to-front
  LRU). The width provider takes a style (bold adds 1.0 per character).
