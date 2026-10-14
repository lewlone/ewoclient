---
paths:
  - "crates/rewo-ui/**"
  - "crates/rewo-world/src/menu*.rs"
  - "crates/rewo-world/src/inventory.rs"
  - "crates/rewo-world/src/stacked_contents.rs"
  - "crates/rewo-world/src/chat*.rs"
  - "crates/rewo-world/src/suggestions.rs"
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

- **Screen models live in `crates/rewo-ui`** (screens, layouts, widgets, chat
  log, edit boxes, recipe book). It depends on `rewo-world`, never the
  reverse: protocol state the world needs (`menu_layout`, `menu`, `inventory`,
  `stacked_contents`, the `chat_style` Component model, `string_splitter`,
  Brigadier's `suggestions`) stays in `rewo-world`. `rewo-net` depends on both.
- **Vanilla has one screen slot, not a stack.** A nested screen is a replacement
  carrying a callback, and anything opening over a screen must drop the stale
  one's state.
- **One accessor for "the shown menu"** (`PlaySession::shown_menu{,_mut}`,
  `shown_container_id`). Clicks, prediction, hover, tooltip and the packet's
  ids all go through it. `state_id` is **per menu**. The open container
  behind it is `PlaySession::shown_open_menu` / `Menus::shown_open`, which is
  `None` while the player's inventory is shown over a screenless menu
  (`inventory_over`, the lectern case). Raw `menus.open()` is only for "is a
  server container open" (the close watermark and the lectern reader's sync in
  `frame.rs`); a draw, hover, click or slot read of "the menu on screen" that
  uses it mixes the hidden container's slots in.
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

## Link prompt

- **A clicked chat `open_url` goes through `Screen.clickUrlAction`**, never
  straight to the browser: `chatLinks` off ignores the click, `chatLinksPrompt`
  off opens it, and the default is `rewo_ui::confirm_link_screen`'s prompt
  ("Yes" / "Copy to Clipboard" / "No" plus the `chat.link.warning` line,
  because `trusted` is false on this path). Both options are read from
  `options.txt` (`rewo_net::options::Options::{chat_links, chat_links_prompt}`,
  default true). `LiveApp::chat_click_url` is the one entry.
- **The prompt replaces the chat screen** (one slot) and the callback hands the
  *same* `ChatScreen` back (`LiveApp::link_return_chat`, `ChatScreen::reinit`):
  typed text survives, and `removed()` runs on the way out (draft, scroll
  reset). Esc on the prompt is "No" (`confirm_link_screen::key_pressed`), not
  `close_on_esc`. Only chat links use it; the book reader and the server-links
  dialog still do not open URLs.
- **The opener child is scrubbed** (`uri_open::scrub_env`): no `REWO_*`, and
  nothing named `*TOKEN*`, `*SECRET*` or `*PASSWORD*`.
- Graded by `confirmlinkshot --check` (model + pixels) and render-check r67.
