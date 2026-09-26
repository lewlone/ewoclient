# Rewo — the CLAUDE.md milestone record, M87–M142 (containers, recipe book, docs audit, chat arc, tickable sounds)

<!-- Moved verbatim from CLAUDE.md lines 4919–6936 on 2026-09-26, when CLAUDE.md
     was cut from 7,192 lines to an always-loaded index plus path-scoped
     rules in .claude/rules/. Everything below the rule is the original text,
     unedited. It is history: read it for the reasoning behind a rule. -->

> **Superseded statements in this file** (kept verbatim below; the current truth is in `CLAUDE.md` or `.claude/rules/`):
>
> - Test counts and gate totals inside each entry are snapshots. Current numbers live only in `REWO_PLAN.md` §0.0.

---


### M87 — the container/menu screens (2026-08-02)

Twelve commits (`f99ad5c..bd39954`, merged `2cd5635`). **The first bite out of
class C**, and a worked example of what that class costs: `open_screen` and
`container_set_data` are eleven lines of decode between them, and the other
eleven commits are what makes those lines mean anything. Before it,
`apply_container_set_content` opened with `if container != 0 { return false }`
and its own comment called that *"the whole truth about what this client can
show"* — on a real server you could not open a chest.

**Findings that invert, in the order they bite:**

- **`crafter_3x3` puts its result slot AFTER the player inventory** — grid
  0..8, `addStandardInventorySlots` 9..44, result at **45**. Every other menu
  appends the player's 36 last, so "container slots, then the player's" puts
  the crafter's output *inside* the player's inventory and shifts nothing
  else. **`crafting` inverts the other way**: its result is slot 0, before the
  grid.
- **`lectern` has one slot, no player inventory, and no container screen.**
  `LecternMenu` never calls `addStandardInventorySlots`, and `LecternScreen
  extends BookViewScreen` — the same fact from both sides. Any
  `slots.len() - 36` is a panic there. So it is **24 container screens and one
  book viewer**, not 25.
- **`open_screen`'s menu type is `registry(...)` — raw 0-based, not `holder`'s
  `id + 1`.** Fourth time (M16, M21, M55) and the quietest: id 2
  (`generic_9x3`) reads as 1 (`generic_9x2`), a real menu with a real screen
  and nine fewer slots, so a chest opens with its bottom row missing.
- **`container_set_data` is a VarInt then two *signed* `readShort`s** in a
  mostly-var-int protocol. Negatives are real (the anvil's cost, the beacon's
  "no effect").
- **Six screens override the title's x, in two different ways.** `dispenser`,
  `crafter_3x3`, `brewing_stand` compute `(imageWidth - font.width(title)) / 2`
  — a server-chosen name, so not storable as a constant; `anvil` (60),
  `crafting` (29), `smithing` (44 + `titleLabelY` 15) are literals.
- **The blit's sheet size is a per-call argument.** Twenty-one backgrounds pass
  `256, 256`; **`MerchantScreen` passes `512, 256`**, because a 276 px panel
  cannot come off a 256-wide texture. A global 256 gives `u1 = 1.078` and the
  sampler repeats its left edge across the right-hand third of the trade
  screen — which reads as a texture bug, not an arithmetic one.
- **A chest's background stops one pixel short of its declared height**
  (`114 + rows*18` vs blits covering `rows*18 + 113`). Vanilla's arithmetic;
  closing the gap samples a row of `generic_54.png` vanilla never samples.

**Three process results, each of which changed the work:**

1. **A checker, not a generator.** Every other bulk fact in Rewo comes from a
   `tools/gen_*.py`; here that is measurably the wrong tool. The 25 menus use
   **four idioms** (direct `addSlot`, a nested loop, a field assigned earlier,
   a fluent builder consumed by a base class) and five declare no slots and
   inherit them — one extractor reaches **17 of 25**, and chasing the rest is a
   small Java interpreter whose failure mode is a silently *short* slot list.
   `tools/check_menu_layouts.py` re-derives independently and diffs; it earned
   that on run one by **refusing to proceed**, seeing four slots in
   `BrewingStandMenu` where the table has five (`IngredientsSlot` takes a
   leading argument a four-arg pattern misses). The table was right.
2. **One dispatcher per packet id.** First a hazard — `container_close` is
   already owned by `route_client_state`, and the play loop is a chain of
   `else if`s, so a second claimant either steals M74's counter or never fires,
   with no error either way. Then a design constraint:
   `container_set_content` is one id addressing two menus, so `route_inventory`
   grew a `&mut Menus` rather than the container path getting its own router.
3. **A half-landed feature is a bug, not half a feature.** M87j shipped the
   panel setter *uncalled* on purpose: the icons and hover still keyed off the
   player's 176x166 origin, so setting only the panel would paint a chest sheet
   with the player's icons 28 px off — broken, not unfinished. M87k landed all
   four consumers together, choosing the menu **once** and threading it.

**The gate, and the witness that caught its own vacuity.** `rewo containershot
--check` — serverless, validation-required, fail-closed, **13 witnesses**,
grading against oracles the tables cannot influence (the slot geometry
re-derived from `ChestMenu`'s constructor; the panel against `generic_54.png`
itself). **Its first run failed on the witness written to detect exactly
that**: `p3` asks whether `p2`'s probes can distinguish the two readings, and
on a six-row chest they cannot — `split` is 125, the lower band maps
`y -> y + 1`, and the two candidate source rows are *adjacent* and identical
wherever the art is flat. The band probes now use a **one**-row chest (offset
91) and centring the **six**-row one (28 px vs 2). One fixture cannot serve
both claims.

**Measured:** 1683 tests, 0 failures; `containershot` 13/13, `inventoryshot`
152/152 **unchanged across all twelve commits**, `itemshot` 75/75, `handshot`
34/34, `menucheck` 25/25, demo PNG `2cc56b4acbfb92cb` byte-identical
throughout. `REWO_PACKET_COVERAGE.md` 107/0/34 → **109/0/32**, class C 23 → 21.
**`live --render-check` 18/18, validation ON, 0 validation errors** — note
validation is `cfg!(debug_assertions)`-gated for `live`, so a *release* binary
reports `r17` false and makes `r18` vacuous.

**What that check did NOT prove, and it mattered:** `--render-check` opens the
*inventory*, not a chest, so it graded the windowed client's health with M87 in
it and **not** that a container rendered there — the same shape of blind spot
M86 was. **M88 closed this**; see below.

### M88 + M89 — proving the container renders, then making it work (2026-08-02)

**M87's merge commit said "Rewo can open a chest" and that was an over-claim.**
It built the *render*. These two make it true. Detail in `REWO_PLAN.md` §15.

**M88 (`9666045`)** closed the render-check gap with `r19` (a container screen
was drawn — 1513 of 3551 frames) and `r20` (its panel was its own, 168, not the
player's 166). The container is opened by injecting a raw `open_screen` body
through the **production router**, per M17: injection is the deterministic
proof where a live encounter depends on the server's timing and the client
aiming at the right block.

**`r20` was wrong in its first cut**, and that is the transferable part: it read
`image_h` off the open menu's **layout**, which answers 168 for a chest whether
or not the panel builder returned one — so it could not tell a working
container from a silent fallback to the player's panel, the failure actually
worth naming. It now reads the height back **out of the renderer** after the
draw set it. *A value witness is only a value witness if it reads the value the
draw used* — reading one that merely **implies** the draw is a proxy that looks
more rigorous than it is. Mutation-tested: a silent `None` fallback drops `r19`
to 0 frames and `r20` to `None`; the first cut stayed green through it.

**M89 (`6123058`)** made a container *usable*. Three things were still
player-keyed, all reachable today (open a container, press E, click):

1. **Nothing opened the screen** on `open_screen` — the menu was recorded and
   nothing shown unless the player independently pressed E. In vanilla
   `handleOpenScreen` **is** `MenuScreens.create`; decode and screen are one
   action.
2. **Every click operated on the player's menu** — all sixteen sites used
   `session.inventory`, so clicking a chest's slot 5 picked up the player's
   crafting grid.
3. **The click packet hard-coded container 0 and the player's `state_id`** —
   and `stateId` is per-menu (`incrementStateId` is an instance counter; the
   resync test is against the menu the click *names*), so the server would
   apply a chest click to the inventory or reject it on a stale id.

The fix is **one accessor** (`PlaySession::shown_menu{,_mut}`,
`shown_container_id`) that every consumer goes through — the five click
actions, the prediction apply, the hover, and the packet's two ids. A
per-call-site choice is *how* they came to disagree. The hover needed **both**
halves: `screen_to_gui` centres the panel to find the origin and `slot_at`
scans that layout's slots, so asking the player's 176x166 while a 176x222 chest
is up shifts the cursor 28 px **and** looks it up in the wrong slot list — the
two errors do not cancel.

**`r21` isolates the new behaviour by ordering** — the container is injected at
0.4, *before* the gate force-opens the inventory at 0.5, so frames in that
window can only exist if the packet opened the screen. **And that reordering
silently broke M86's own coverage**: the forced-open branch guarded on
`!inventory_open()`, which the injected container now satisfies, so the branch
was skipped — including the **cursor park**, the only thing that lays out a
tooltip and therefore the only door to `VelvetTextPass::sync_atlas`. `r16`
stayed green while proving nothing it was written for. **The tell was a number
that was too good** (`r21` counting all 2244 frames rather than a ~290-frame
window) — the shape of a guard that has stopped firing. *A test can be disabled
by a change to an unrelated part of its harness, and it reports success while
it happens.*

**Measured:** 1683 tests; `containershot` 13 → **17**, `live --render-check`
18 → **21** validation ON 0 VUIDs, `inventoryshot` 152/152, demo PNG
byte-identical.

### M90 — shift-click routes by the menu's own quickMoveStack (2026-08-02)

The last silently-wrong path in the arc, and a second one under it.
`quickMoveStack` is a **per-menu-class override** and Rewo's routing was
hard-coded to `InventoryMenu`'s ranges, so shift-clicking a chest's slot 0
routed as though it were the crafting result.

**Nine of the 25 menus share one shape** (the six chests, dispenser, hopper,
shulker box): `slot < containerSize` → the player range **backwards** (the
hotbar's right-hand end, since `addStandardInventorySlots` appends it last),
else → the container range forwards. The furnace and crafting families have
their own and are **not** transcribed — they answer `QuickMove::Unimplemented`
and the caller **declines**, because moving nothing is inert where a
shift-click under another menu's rules moves the wrong stack and the server
applies it.

**The second bug was found by a witness, not by reading.** The first cut failed
on the container→player direction only: `move_stack_to` calls `slot_kind(i)` —
the *player's* — which returns `None` past 45, so the `?` aborted the whole
move. Nine call sites shared it, and the consequence is wider than shift-click:
**plain clicks past a chest's slot 45 also silently did nothing**, and below 45
read the wrong kind. M89 routed *which menu* a click applies to and not the
slot-kind lookup, so it fixed only the visible half. **When a type is
generalized, the functions it calls generalize with it — and the ones taking a
bare index rather than `&self` are the ones that get missed, because they do
not look like they belong to anything.** `slot_kind` is now
`MenuLayout::slot_kind`, with `SlotKind::Plain` for a container's slots and
`None` (decline) for an untranscribed menu.

### M92 — the rest of `container_set_data`, the crafting quick-move, and the first bespoke widget (2026-08-03)

Eight commits, and all three of M91's recorded open items. Detail in
`REWO_PLAN.md` §15.

**Three data consumers, each inverting against the last.** The brewing stand's
slots are the **reverse** of the furnace's — `getBrewingTicks()` is `get(0)`
and `getFuel()` is `get(1)`, where the furnace puts fuel at 0; both menus are
five bytes on the wire and naming them by analogy swaps a 0..20 fuel level with
a 0..400 tick counter. Its timer counts **down**, its arrow grows downward
while its bubbles grow upward (one function apart), its fuel bar grows rightward
— three directions on one screen — and its arrow **truncates where the furnace
ceils**, so at 399 ticks vanilla shows no arrow where a ceil shows a pixel.
`BUBBLELENGTHS` ends in **0**, so one frame in seven is blank.

The enchanting table's costs are **only a third of what its rows need**: the
lapis is the *count of the stack in menu slot 1* (a different packet) and the XP
level and creative flag are the player's. **The lapis requirement is the row
INDEX plus one, not the cost.** There are **three row states, not two** — an
empty row draws its background and nothing else; an unaffordable one draws the
same background *plus* its numeral. `col` does double duty and is reassigned
before the cost text, so a row's name and cost are different colours and the
cost's does not track the hover. The highlight and the tooltip use **different
rectangles**, and neither is a slip.

The beacon says "absent" with **0 and shifts real ids up by one**, where the
enchanting table one menu earlier uses **-1** — two conventions in the same
signed short on the same packet. And **an invisible button moves a visible
one**: the upgrade slot is counted into the column's `totalWidth` while
`visible = false`, so dropping it slides regeneration 12 px.

**The crafting quick-move needed a structural change**: `quickMoveStack` there
is a **fallback chain** (`if (!moveItemStackTo(grid)) { cross-move }`), which a
single-range return cannot express — it must either always try the grid or
never. This is the branch that makes a crafting table *fill its grid* on a
shift-click, which `InventoryMenu` does not do. The two crafting menus put
their result at **opposite ends** (CraftingMenu slot 0, CrafterMenu slot 45).

**`container_button_click`** is two var-ints and the **whole** input surface for
four screens; it carries **no state id**, unlike its sibling. The enchanting
table's click gate is **not** its render gate — it additionally requires slot 0
to hold something and tests the level against both `row + 1` and `costs[row]`.

**The bug it uncovered is bigger than the milestone.** Five `mob_effect` ids
(night vision, darkness, haste, conduit power, mining fatigue) were read from a
`registry_data` branch **that cannot fire** — `MOB_EFFECT` is a
`BuiltInRegistries` entry and appears **zero times** in
`RegistryDataLoader.java`'s synchronised list. So M13's night vision/darkness
and M19's dig-speed adjustment had **never worked live**, and no gate could see
it because `lightmapshot` and `swingshot` are serverless and *construct* the
effect state, supplying the very ids the live path fails to obtain. **When a
gate supplies an input production must derive, the derivation is untested by
construction** — worth a sweep for other instances. Fixed by the rule
`attributes.rs` already states: a built-in registry resolves **by name from the
report**.

**Three detector errors and a harness bug**, all mine: a control frame that
differed in its *background* rather than its subject; a probe on a glyph's
transparent row; a probe on a button that its icon repainted independently of
its chrome; and a mutation harness whose `mv` restore preserved the **older**
mtime, so cargo skipped the rebuild and the next run silently graded the
mutated binary (it presented as a green witness regressing with no code
change). A fourth finding came out of the same battery: `enchant_row_sprites`
had its own copy of the numeral mapping, so emptying `EnchantRow::numeral()`
changed nothing rendered — a model accessor graded by tests the app did not
call.

**Measured:** 1712 → **1773 tests**, all seven crates confirmed reporting;
`containershot` 17 → **27**; `live --render-check` 21 → **22/22** with
validation ON and 0 errors (it must be run from a **debug** build — validation
is `cfg!(debug_assertions)`-gated for `live`); demo PNG `2cc56b4acbfb92cb`
byte-identical; 41 mutations, 40 killed and the survivor shown to be doubly
guarded in vanilla too. `REWO_PACKET_COVERAGE.md` 109 / 0 / 32.

**Open:** `container_set_data` is now consumed by every menu that sends it.
`quickMoveStack` still declines for the brewing stand (three item predicates)
and the enchantment table (its last branch is not a range move — it places
exactly one item). Of the bespoke widgets the enchanting rows are done; the
loom and crafter now only need their button lists, the beacon needs
`set_beacon`, the anvil a text field, and the merchant and stonecutter are
blocked on class-C packets. **M93 took the beacon's and three of the eight
item-combiner menus' quick-moves** — see below.

### M93 — the single-input quick-moves, and the derivation nobody was grading (2026-08-03)

Two commits. The plan called the eight item-combiner / single-input menus "a
few lines each"; **the decompile does not support that**, and the correction is
worth more than the code. They are **four shapes**, and two of the plan's own
claims invert.

- **`MerchantMenu.quickMoveStack` consults nothing at all.** The merchant is
  listed as blocked on class-C `merchant_offers` — true of the trade-list
  *widget*, false of the quick-move, which never routes a player stack into
  slots 0 or 1. **Vanilla will not load a trade for you.**
- **`ItemCombinerMenu`'s player branch is a guard that CONSUMES, not a fallback
  chain** (M92e's `CraftingMenu` shape). `canMoveIntoInputSlots` defaults to
  `true`, so for the **anvil** the two main/hotbar arms below it are
  structurally unreachable: an anvil does not cross-move your inventory, and a
  full anvil moves *nothing*. Behaviour, not an omission.
- **The beacon's count test is in the branch, not in `mayPlace`** — so the same
  item routes two ways by count (one diamond claimed, two cross-moved). Its
  guard failing *does* fall through, unlike the combiner's. Vanilla's fifth
  beacon arm is **dead** and deliberately not transcribed. Its tag comes from
  the jar via `tools/gen_beacon_payment.py` (the `gen_fuel_values.py`
  precedent), and its slot gets `SlotKind::BeaconPayment` so a **plain** click
  respects the tag too — `Plain` would keep the quick-move exact and let an
  ordinary click predict a placement the server rejects.

**M93b–d then took the stonecutter**, and found that
`stonecutterRecipes().acceptsInput` is **not** a `RecipePropertySet` — that
registry's seven keys are smithing×3, the three furnaces and campfire.
`RecipeAccess` exposes it separately as a `SelectableRecipe.SingleInputSet`.
The difference does not reach the accepted-input table (`Ingredient.test` is
item identity) but *is* what the recipe **list widget** needs, so that stays
blocked on `update_recipes`. It is also the **third guard behaviour in three
menus**: the anvil's is always true, the beacon's falls through when it fails,
and the stonecutter's falls through when the *guard* fails but **moves nothing
when the MOVE fails** — two exits from one branch, only the first cross-moves.
Its predicate is branch-only (slot 0 is a bare `Slot`), and vanilla's
`player.drop` of an unfitting result remainder is **recorded, not modelled**.

**M93e then took the grindstone**, which **inverts the arrangement of every
other menu here**: the beacon and stonecutter ask about the *item* in the
branch and let the slot accept anything, while the grindstone asks about the
*slots* (`!input.isEmpty() && !additional.isEmpty()`) and puts the item
predicate in `mayPlace`. So `SlotKind::GrindstoneInput` is load-bearing for the
**shift-click**, not merely an ordinary click — and when it refuses, the move
returns false and vanilla `return`s, so a stick shift-clicked into an empty
grindstone moves **nothing**.

> **⚠ A blocker recorded above was wrong.** M93a said the loom and cartography
> table needed a prototype-component model Rewo lacks. **Rewo has one** —
> `rewo_data::item_components_table::prototype_has_component`, generated by
> **M56** for the tooltip's component count, covering all 1537 items. Check it
> before calling any component question unanswerable. The reusable shape:
> `has(X)` = *removed → false, patch-set → true, else prototype*.

**M93f then took the cartography table** — the only menu here whose branch
predicates and `mayPlace` predicates are the **same two tests, written twice**
(the branch picks which slot to try, `mayPlace` confirms it will take it;
neither is droppable). **`has(MAP_ID)` is tested first, and that ordering IS
map cloning**: `filled_map` carries the component and takes slot 0, while
`minecraft:map` is a *different item* with no component and falls through to
the paper slot. Vanilla writes the middle arm as a triple negation, so the
paper slot is the branch reached when the stack *is* one of the three —
transcribed forwards with the arms swapped. No prototype carries MAP_ID, so
this is the cleanest three-step `has()` case: one bit. The value is read and
discarded but **must** be read, or the length-prefix-less patch desynchronises.

**M93g then took the loom, closing the arc bar one.** Its banner test is
`instanceof BannerItem` — a **class** — and the obvious data stand-in is wrong
by exactly one item: every banner's prototype carries
`minecraft:banner_patterns`, set by the very lambda that constructs the
`BannerItem`, **and so does `Items.SHIELD`**. `BannerItem` has exactly one
construction site (the 16-colour `ColorCollection`) and `#minecraft:banners`
holds exactly those 16, which the generator asserts rather than assumes. Its
other two predicates are genuine **conjunctions** (`is(#LOOM_DYES) &&
has(DYE)`), each with its own removal bit — two rather than one shared, since
an item is in at most one tag and a shared bit would falsify the wrong
predicate.

**Seven of the eight single-input quick-moves are done**; nine of the 25 menus
route a shift-click. **Only `smithing` remains**, nominally blocked on
`RecipePropertySet` off `update_recipes` (class C).

> **⚠ CORRECTED 2026-08-13 — this paragraph used to say these sets are "*not*
> jar-derivable the way M91's smelting ones were, because a smithing recipe's
> three ingredient slots are per-recipe rather than one flat `ingredient`
> field." That is wrong, and it is a **field-name** difference rather than a
> structural one.** Measured against
> `<D>/data/minecraft/recipe/*.json`: **30 smithing recipes — 12
> `smithing_transform` and 18 `smithing_trim` — and all 30 carry all three of
> `base`, `template` and `addition`.** The shape is perfectly uniform, and
> `tools/recipe_ingredients.py` already does the recursive tag expansion the
> values need (`#trimmable_armor` expands to 29, `#trim_materials` to 11,
> `#netherite_tool_materials` to 1). So the last hard decline in the container
> arc is roughly thirty generator lines, not a subsystem. This is the FIFTH
> time a "class-C blocker" in this project turned out not to be one — after
> M91's furnace recipes, M93's merchant quick-move, M93s's stonecutter list and
> M93u's merchant offers — and the pattern is the same every time: **the data
> was in the client jar rather than on the wire.** Check for it before
> believing any such claim, including this file's.

> **⚠ Do NOT use `ItemSlot::enchanted` for the grindstone.** Its doc comment
> says `ItemStack.isEnchanted`; the assignment is `c.has_foil()`, and M43
> proved those differ (`ENCHANTMENT_GLINT_OVERRIDE` wins both ways). It
> compiles, reads correctly, and is wrong for exactly the cases that component
> exists to create. `hasAnyEnchantments` is also `ENCHANTMENTS` **or**
> `STORED_ENCHANTMENTS` — an enchanted *book* is the canonical grindstone input.

**A fixture rotted, loudly this time.** M90's "an untranscribed menu declines"
test named the **anvil**, which M93 transcribes — the rot M41 found in
`swingshot` and M43 in two `item_stack` fixtures, where it was *silent*. It now
asks the registry which menus are undone, proves the property on all of them,
and fails if that set is ever empty. **Two witnesses were wrong before the code
was:** a full anvil answers `None` (not `Some`-with-no-changes), so it is
paired with a one-free-input anvil that must answer `Some`; and `click_pickup`
*does* return `Some` with an empty change set, an asymmetry that is vanilla's.

**M93b is the part that generalises — and it is the M92 sweep applied to my own
code the same session.** M93a shipped with exactly the hole M92 names: all
eight witnesses hand-build an `ItemProps`, so `beacon_payment` could have been
wired to nothing and every one would stay green while a real beacon cross-moved
every diamond. `containershot` now calls the production `live_cmd::item_props`
and grades what it returns for real registry ids (`d1`), with a second witness
(`d2`) pinning something the same call must get right in the *other* direction,
because the negative alone would pass against a function that resolved nothing.
**The general sweep is still open** — grep for a `*shot` gate that builds a
struct production resolves from a table or the wire.

**A generated file was lying about itself, and the extraction caught it.** The
stonecutter needed M91's recursive tag expander, so it moved to
`tools/recipe_ingredients.py`; an extraction is only safe if provably inert, so
the check was to re-run `gen_smelting_inputs.py` and diff. **54 deletions.** The
data was byte-identical — what the diff showed is that `smelting_table.rs` says
*"Do not edit. Re-run the generator"* and carried five hand-added tests the
generator never emitted, **including the one pinning M91's own headline
finding**. The generator emits them now. The fifth was dropped deliberately: a
`.len()` assertion is fine hand-written and **vacuous once generated**, so the
guard moved to the generator's recipe-count floor, where a re-run cannot
recalibrate it.

**A surviving mutation found a hole spanning M93a**: nothing witnessed
`backwards` for *any* of the four menus, which is the difference between a
taken result landing in the hotbar's right-hand end and in the first free main
slot. And a fourth witness of the session was wrong before the code was —
**stone is both stonecuttable and smeltable** (slabs, and smooth stone), so the
disjoint pair is andesite/beef, with cobblestone pinned as M91's log one menu
over.

**The grindstone's second disjunct is why the `enchanted` warning above
matters.** `hasAnyEnchantments` admits an **enchanted book**, which is not
damageable at all — and `ItemSlot::enchanted` misses it twice over: it is
`has_foil()` (M43) *and* `isEnchanted()`, which reads `minecraft:enchantments`
alone while a book's live in `stored_enchantments`. `c.enchantments` was
already the union of both, so the correct bit was one line.

**A mutation caught M92's finding in my own new code**: the decoder's
damage-removal flag was untested by construction, because the only witness
constructed an `ItemSlot` with it already set. Now asserted from **bytes**.

**M93f's decode witnesses were written WITH the feature**, not after a
surviving mutation as M93e's were — and its battery came back **13/13 clean
first time**, which is what that bought. Their first run failed on the
**harness**: the shape table decides *walkability* and the interpretation
decides *meaning*, kept separate on purpose, so a fabricated test id absent
from `install_test_shapes` reads as unwalkable however well the interpreter
handles it. Production was never affected.

**Two mutation lessons from M93g.** One **survived and was shown equivalent
rather than fixed**: dropping a conjunction's second term changes no answer the
jar can produce, because every item in `#loom_dyes` also carries the component
— so `d9` pins the *coincidence* instead, and fires if a version ever breaks
it. Two others were real and shared a shape: **a witness on one of two mirrored
terms leaves the other free to be deleted** (the dye removal was witnessed and
the pattern one was not; both on the quick-move path and neither on the
plain-click path).

**And a timeout kill left a mutation on disk** — the battery restores in a
`finally`, which a killed process skips. **Grep for the mutation markers before
anything else after an interrupted battery**, and split batteries so each stays
inside the 10-minute tool cap.

**Measured:** 1773 → **1942 tests**, 0 failures, seven crates reporting (world
717, net 609, gpu 255, data 212, app 97, mesh 45, proto 11); `containershot`
27 → **76**; `inventoryshot` 152, `itemshot` 75, `handshot` 34, `swingshot` 97,
`mobshot` 246/246; `live --render-check` **22/22** validation ON, 0 errors
(re-run at M93q, the first of the arc to touch a render path); demo PNG
`2cc56b4acbfb92cb` byte-identical; **205 mutations across M93a–y, 199 killed,
2 shown equivalent, 1 alive by construction (named)**.

### M103 — the ghost recipe, and two vanilla quirks no Minecraft grid can show

M93y decoded `place_ghost_recipe` and nothing consumed it — the last
decoded-but-unrendered packet in this area.

**The item is sandwiched between two washes of DIFFERENT colours** — `0x30FF0000`
red *under*, `0x30FFFFFF` white *over*, both alpha 48. They land in different
halves of the container pass (the icons are a separate pass that runs between
them), hence a new `front_overlays` list. **And only the wash beneath widens**
for a big result slot; widening the veil too rings the icon in white.
`isBiggerResultSlot()` is **true by default**, false only for `InventoryScreen`.

**The families place differently:** shaped crafting **centres** a small recipe in
a big grid via `PlaceRecipeHelper`; shapeless fills the first
`min(ingredients, slots)` in order. A furnace ghosts its **fuel only if the fuel
slot is empty**. A stonecutter or smithing display ghosts the result alone.

**Two `placeRecipe` quirks no Minecraft grid can show**, each found by a mutation
that survived until a non-Minecraft fixture existed: the centring test is
**strict** and `<=` is indistinguishable on 2x2/3x3 (a 4x4 shows it); and the row
skip advances the row a **second** time, which needs `gridHeight >= 5` to matter
(a 6-tall grid shows it). My doc had claimed the strictness mattered generally —
corrected.

A witness was wrong twice more: a 3-wide 1-tall recipe centres **vertically** in
a 3x3, and the row skip advances **one** row rather than jumping to `startPos`.

**And the mutation harness gave a false SURVIVED** — second wrong verdict today
after M95's em-dash decode. A shapeless off-by-one reported SURVIVED in batch and
died immediately when run alone; the rest were run directly. *A harness wrong
twice is a detector to check, not to trust.*

**2101 tests**; containershot 89, inventoryshot 152, itemshot 75, handshot 34,
mobshot 246/246; **`live --render-check` 23/23** validation ON 0 errors (run
because `set_state` changed); demo PNG `2cc56b4acbfb92cb` byte-identical; **12
mutations, 12 killed**.

### M104 — the which-of-these overlay, and three clamps that round three different ways

M98 wrote the gap into `BookAction::Recipe`'s own doc — *"Rewo has no overlay,
so a right-click on a multi-recipe cell is reported and does nothing."* This
reads that note.

**`OverlayRecipeComponent.init` nudges the panel back on screen in whole 25-px
steps, three times, and the three roundings are not stylistic.** The horizontal
clamp truncates with a C-style `(int)` cast, so a positive quotient floors — an
overlay overhanging by 1..24 px is not moved at all, and one overhanging by 38
moves 25 and still overhangs by 13. The bottom clamp takes `Mth.ceil` of a
**positive** quotient and is the only one guaranteed to clear its bound. The top
clamp takes `Mth.ceil` of a **negative** one, and since `Mth.ceil` is a true
ceiling, `ceil(-0.6) == 0` makes it a complete no-op below one step. Same
function, opposite effect, decided by the sign alone. Reaching for a symmetric
"clamp into the box" diverges on the whole right-hand column.

**`centerY`'s `+ 13` is inert, and no fixture can catch it.** Every cell's `y`
is `31 + 25r`, so `y ≡ 6 (mod 25)`, and the two candidate bounds are 13 apart —
inside one quantisation step, so both overflows land in the same `ceil` bucket
for every cell and every count. The witness was written asserting the opposite,
failed, and became an exhaustive proof of inertness instead. It joins
`extractRenderState`'s unread `int border = 4;`.

**It opens on a right-click and accepts only left-clicks**, so a second
right-click closes it. **An open overlay is modal** — the overlay branch is an
unconditional `return true`, so a click on the arrows, the search box, the tabs
or the menu's own slots underneath all reach it and nothing else. **Selecting
does not close it** (only the else-branch calls `setVisible(false)`), which
reads like an oversight and is what makes the feature usable. **And it is a
snapshot, not a view**: `init` resolves everything once and `updateCollections`
leaves it alone, so crafting while it is open does not re-sort or re-grey it —
which is why `Open` is stored rather than recomputed.

Smaller inversions: the 4-or-5 row width keys off the **total** (16 recipes are
four rows of four, 17 are four rows of five); the padding is asymmetric (4/5 and
5/4); the button is 24 on a 25 pitch so there **is** a gutter a click falls
into; `Pos` is the ingredient's **centre**, because `scale(0.375F)` sits between
two translates; the ingredient cycle is **one** level where the cell's is two,
on the same clock; the button class follows the **menu**, not the display; and
shaped centres through `PlaceRecipeHelper` while shapeless is a bare
`i % 3, i / 3` with the 3 a literal.

**`blitNineSlicedSprite` became tested geometry** in `rewo_world::nine_slice`.
`rewo_gpu::screen::nine_slice` already exists and is left alone — different
pass, different vertex format, and **no unit tests at all** — so this is the
arithmetic on its own, with tests, rather than a silent second copy. On
`overlay_recipe` the tile-vs-stretch choice is **unobservable** (flat centre,
edge bands constant along their repeat axis), recorded so a green pixel gate is
not read as having graded it.

**The witnesses were wrong nine times and the code twice**, and the shapes
repeat: a binding shadowed 200 lines away made three gate witnesses probe the
recipe cell's corner (and the rename then missed the one using `c0y` alone); an
`any` over two right-hand corners could only see total failure; two fixtures
could not express their claim (a mutant equivalent *by construction*, and two
**symmetric** shaped fixtures blind to a transposition — a 2x1 recipe, centring
on one axis only, is what pins it); a control depended on what happened to be
underneath (a button's centre and the cell it covers are both 139); and one
witness counted quads instead of placing them.

**67 mutations, 65 killed, 2 proven equivalent.** A killed battery **left a
mutation on disk** when it hit the 10-minute cap and its `finally` never ran —
caught by grepping the markers first. And `cargo run -q` swallowed a compile
error, so a debug print that never appeared read as "branch not reached" rather
than "did not build" — third detector error of that shape in the log.

**2139 tests** (world 860, net 613, gpu 255, data 212, app 143, mesh 45, proto
11); `containershot` 89 → **96**; **`live --render-check` 24/24** validation ON
0 errors, with a new r24 that required splitting `book_quads_max` in two,
because the claim is a *difference* and one max cannot see it; demo PNG
`2cc56b4acbfb92cb` byte-identical. **Open:** overlay and recipe tooltips,
`tryPlaceRecipe`'s `lastPlacedRecipe` guard (unmodelled since M98), `useMaxItems`,
and the page counter text.

### M102 — the two crafting fills, and a fourth comment that described what the code did not do

M96 recorded one approximation and left another unrecorded, both in the same
eight lines. `hasCraftable`'s contents come from **two disjoint fills** —
`Inventory.fillStackedContents` (the ITEMS) and
`menu.fillCraftSlotsStackedContents` (the GRID).

**The range:** `Inventory.items` is menu slots **5..46** — not the 2x2 grid
(1..5, which arrives through the second fill) and not the craft **result**
(slot 0, which arrives through neither). M96 walked all 46, which double-counts
the grid *and* adds the result, so a recipe could read as craftable off its own
output.

**The predicate:** `accountSimpleStack` gates on `isUsableForCrafting` =
`!isDamaged() && !isEnchanted() && !has(CUSTOM_NAME)`. M96's comment named it and
applied nothing. **Fourth comment this session describing behaviour its code did
not have** (M93t's `setCanLoseFocus`, M96's note, `any_enchantments`' doc, this).

**`isEnchanted()` is the middle of three near-identical flags:**
`ItemSlot::enchanted` is `has_foil()`; `ItemSlot::any_enchantments` is
ENCHANTMENTS **or** STORED (the grindstone's `hasAnyEnchantments`);
`SlotText::is_enchanted` is ENCHANTMENTS alone and is the right one.
`any_enchantments`' doc claimed to be `isEnchanted()` too — corrected. An
enchanted **book** separates them, and M93 recorded this trap one field over.

**The fills differ in gating, not just range:** the crafting container is gated,
the furnace **block entity** calls bare `accountStack` and contributes its whole
container **including the result**. A damaged pickaxe counts in a furnace and not
on a grid.

A mutation deleting the craft-slot half **survived** — the fill sat in a
`PlaySession` path — so it moved to `crafting_contents`, taking the max-stack
lookup as a closure. M97's lesson, fourth application.

**2077 tests**; containershot 89, inventoryshot 152, itemshot 75, handshot 34,
mobshot 246/246; demo PNG `2cc56b4acbfb92cb` byte-identical; **12 mutations, 12
killed**; no render path changed.

### M101 — the caret blinks, and the field it blinks in never scrolled

M100 recorded the blink as a shared gap between the book's field and the anvil's.
Fixing it in the extracted renderer fixed both — and exposed two older bugs.

**`showCursor` is THREE conditions** — `isFocused() && isCursorVisible(millis -
focusedTime) && cursorOnScreen` — where M93t had only the first. The blink is
`/300 % 2 == 0` measured from `focusedTime`, which `setFocused(true)` resets and
`setFocused(false)` does not, so a freshly focused field shows its caret at once.
And `setFocused` is gated on `canLoseFocus || focused`: **the anvil sets that
false**, so its caret blinks as long as the screen is open where the book's stops
on losing focus.

**First older bug:** M93t's comment claimed `setCanLoseFocus(false)` and the code
did only `setInitialFocus`. Nothing pinned the focus for eight milestones,
because those lines sat in a path needing a `PlaySession` and so were unreachable
from a test — they are `anvil_field_new()` now.

**Second older bug, surfaced by the caret's own gate: the field never scrolled.**
Vanilla's `insertText → setCursorPosition → scrollTo` keeps the cursor visible;
Rewo's `set_cursor_position` cannot, because `scroll_to` needs a font width the
`EditBox` does not own. So `display_pos` never moved and a field typed past its
width kept showing the head of the string. **Before this the caret was drawn
anyway at a bogus x; with `cursorOnScreen` correct it vanished — which is what
made the gap visible.** `follow_cursor` is the missing half, called from every
input path, for both fields.

The headless renderer takes a **fixed clock of 0**: a blinking caret would render
the same scene two ways depending on when the gate ran.

**2068 tests**; containershot 89, inventoryshot 152, itemshot 75, handshot 34,
mobshot 246/246; demo PNG `2cc56b4acbfb92cb` byte-identical; **11 mutations, 11
killed** — two only after witnesses that could reach them were written.

### M100 — the search field's text, and a nine-slice that degenerates to two blits

The field typed and filtered since M99 and drew nothing.

**The nine-slice is two blits, measured not assumed.** `widget/text_field` is
200x20 border 1 — but the PNG is **1-bit paletted**, exactly two colours (border
160-grey, white when focused; interior black). Every one of the nine regions is
uniform, so a stretched 1x1 source is **pixel-identical** to a tiled one: one
blit of the whole rect from a border texel, one of the interior from a centre
texel, and the 1 px the first still shows *is* the border.

**The hint goes on FOCUS, not on the first character** —
`displayed.isEmpty() && !isFocused()` — so clicking an empty box blanks
"Search..." before you type. It is a styled component (GRAY + ITALIC), so its
own colour beats the field's white; the italic is not reproduced (no slant in
the bitmap pass).

**The bordered case decides all three text numbers and none is obvious:**
`textX = getX() + 4`, `textY = getY() + (height - 8) / 2` (**3**, not `getY()`),
`getInnerWidth() = width - 8` (**73**, not 81 — the inset comes off both ends).

**Third meaning of `WidgetSprites::get` on one screen:** the field passes
`(isActive(), isFocused())`, exactly what the names say, where a tab passes
`selected` as *focused* and the filter passes `filtering` as *enabled*. One
convention across this screen is wrong two times out of three.

The renderer is **extracted** from the anvil's, not copied — a second copy of
the caret-x/insert/selection arithmetic is three chances to drift by a pixel.

**A staging trap:** `io.open(p,'w')` truncates when the file object is created,
*before* its argument is evaluated — so `open(p,'w').write(sub(open(p).read()))`
wrote an empty `server.properties`, Minecraft regenerated a default, and the run
died with a bare "Failed to initialize server". Read first, then write.

**2059 tests**; containershot 89, inventoryshot 152, itemshot 75, handshot 34,
mobshot 246/246; **`live --render-check` 23/23** validation ON 0 errors, r23
rising 8 → 10 quads as the field's two blits reach the windowed client; demo PNG
`2cc56b4acbfb92cb` byte-identical; **9 mutations, 9 killed**. **Open:** the caret
does not blink (`isCursorVisible`, 300 ms) — a shared gap with the anvil's field,
not a new one.

### M99 — the search box, and a suffix array the consumer does not need

`updateCollections`' second stage (M93z's unfed `matches_search`) plus typing.

**The suffix array is unnecessary here, measured rather than assumed.** Vanilla
indexes every *suffix*, so a search is a substring match; the array exists for
speed and for a defined result order, and **neither is used** — the result goes
into a set read only via `contains`, and survivors keep their existing order
(`removeIf`, not a re-sort). `contains` is exactly equivalent.

**Two indexes, a colon picks between them:** no colon → **names only** (the ids
are *not* searched, though the tree holds them); a colon →
`namespace ∩ (path ∪ name)` with both halves **trimmed**. For Rewo a result's
"tooltip lines" are its display name alone — exact, since a recipe's result is a
bare id with no components. **An empty query skips the stage** rather than
matching everything: a collection with no searchable text is kept by the skip
and dropped by a match-everything reading.

**A duplicate flag was the bug and removing it was the fix.** The first cut kept
`search_focused` on `BookState` *and* mirrored it into the `EditBox`, whose
`can_consume_input` gates keystrokes on its **own** flag. A test caught it
(typing produced nothing); then a mutation deleting the mirror **survived**,
because `book_press` needs a `PlaySession`. So the flag is gone — `focus_change`
is a pure function of the hit and the `EditBox` is the only owner. **Shrink the
untestable surface rather than pretend to cover it.**

Two more caught by tests: the field takes the **book's** max length (50), not
`EditBox::default`'s 32, so `ScreenState`'s `Default` is written out rather than
derived; and a witness could not isolate the colon query's name half because
`plank` matches both "Wooden Plank" and `oak_planks` — third time this session a
fixture could not express its own claim.

**2053 tests**; containershot 89, inventoryshot 152, itemshot 75, handshot 34,
mobshot 246/246; demo PNG `2cc56b4acbfb92cb` byte-identical; **17 mutations, 17
killed**. **Open:** the field's *text* is not drawn — it types and filters, and
nothing renders the characters or caret (M93t's anvil seam, one screen over).

### M98 — the book takes clicks, and one it does not want is still swallowed

Tabs, pages, the filter, hover, and the two serverbound packets. The book had
been drawable and inert since M94.

**The order is a contract and it is not the draw order:** the **page** first
(arrows, then cells), then the search box, then the filter, then the tabs — and
the whole book before `super.mouseClicked`, so it dispatches ahead of every
other screen widget. **And a second rule in the else-branch:** a click the book
does *not* want is still **swallowed** when the window is too narrow and the
book is open, because there the book covers the menu.

**Four inversions.** A **selected tab's hit rect does not move with its sprite**
— the 2 px shift is draw-time only, so its leftmost two columns are painted and
unclickable. The **magnifier counts as the search box** and its rect *overlaps*
the box rather than abutting it. A click anywhere but the **page** unfocuses the
search field, because `setFocused(false)` sits unconditionally in the
else-branch and the page path returns before reaching it. And **switching tabs
resets the page**, while re-selecting the tab you are on does nothing
(`selectedTab != button`).

**The packets:** `recipe_book_change_settings` carries an ordinal and **both**
flags, read out of the local settings rather than passed — so toggling the
filter re-reports open, and vice versa, because the server persists both.
`place_recipe` places **the recipe the cycle is showing**, not the collection's
first. A right-click on a multi-recipe cell is consumed and does nothing (no
overlay; placing an unchosen recipe is worse than nothing).

**The hover comes from the same `book_hit` the press uses**, with the cursor
converted to book space once in `apply_screen` — M95's note that this needed the
renderer was wrong.

**A surviving mutation was a weak fixture, not an equivalent mutant** — M93z's
lesson again: swapping `clamp_page` for a clamp-to-last-page survived a
**one-page** fixture, where reset-to-front and clamp-to-last are both 0.

**2039 tests**; containershot 89, inventoryshot 152, itemshot 75, handshot 34,
mobshot 246/246; **`live --render-check` 23/23**, validation ON, 0 errors (run
because `apply_screen`'s signature changed and a new dispatch sits at the front
of the click chain); demo PNG `2cc56b4acbfb92cb` byte-identical; **15 mutations,
15 killed**. **Open:** the search box focuses and does nothing (needs the recipe
search tree); no page counter, overlay, ghost slots or tooltips; `useMaxItems`
is always false.

### M97 — closing M96's own recorded gap: the book's derivation, graded

M96 shipped `hasCraftable` graded at its two **ends** — the solver's tests
below, the gate's chrome witness above — and nothing in between. The arithmetic
turning an inventory into a per-slot flag, which is what M96 added, was
untested: M92's shape, M93b's close.

**The obstacle was structural.** `PlaySession` owns a socket and cannot be built
in a test, so the fix is M71's lesson rather than a fixture — *logic in a place
with no test module is untestable, so move it.* `live_recipe_book` is the
session half (lookups) and `book_render_from` the derivation (grouping, tab
membership, paging, cycle, craftable), taking plain values.

Nine tests name rules neither end could see — notably that an entry with **no**
requirements is never craftable while one declaring an **empty** list is (the
distinction `canCraft`'s opening line makes, which the solver alone cannot
express because it never sees the entry), and that asking about one collection
does not spend another's items (a consuming solver would light the first slot
and grey the second).

**10 mutations, 10 killed** — including *"nothing is ever craftable"*, which is
exactly M96's pre-state and would otherwise have been indistinguishable from it.

**2019 tests**; containershot 89, inventoryshot 152, mobshot 246/246; demo PNG
`2cc56b4acbfb92cb` byte-identical; no render path changed.

### M96 — the craftable solver, and two of vanilla's guards that do not matter

`StackedContents` ported, fed and wired — the blocker M35, M94 and M95 all
named. Every recipe slot wore the *uncraftable* chrome because nothing could
answer the question.

**It is a bipartite matching, not a subtraction.** Walking the ingredients
decrementing a count is wrong whenever accept-sets overlap: one `#planks` slot
and one `oak_planks` slot, against a stack of oak and a stack of birch, is
craftable only if `#planks` takes the birch. Vanilla finds it with augmenting
paths (`RecipePicker`).

**Two of my own witnesses were wrong before the code was** — both claimed one
item *type* satisfies one slot only, so a stack of 64 dirt could not fill a
nine-slot recipe. `try_pick` loops, `take`s per satisfied ingredient, and
`hasAtLeast` re-reads the decremented amount: the matching is over **(item,
ingredient) pairs** and what runs out is the count, not the type.

**Two mutations survived and both are genuinely equivalent**, which is the
opposite of the natural assumption. Transposing either bit-matrix index is a
**relabelling** — each region is read and written only through its own index
function and both formulas are bijections onto the same range (the module doc
claimed the reverse; corrected in place). And dropping the `count > 0` filter is
an **optimisation**: a zero-count item enters the matrix and `hasAtLeast`
refuses it anyway. Settled by a **brute-force oracle** sharing no code, order or
bit layout — 27,648 problems (3 item types × counts 0..=2 × 3 slots × all 8
accept-subsets × capacities 1..=2), all agreeing — so "equivalent" is a
measurement, not a claim.

**The wire half:** M93y walked `craftingRequirements` and discarded it, so the
ingredients are captured now — each a `HolderSet`, an inline id list **or a tag
name**, and a tag resolves against `update_tags`, **which M69 decoded and
nothing had consumed**. An unknown tag yields nothing, so its ingredient is
unsatisfiable: greying a recipe you could make is a smaller lie than lighting
one you cannot. `canCraft` opens `craftingRequirements.isEmpty() ? false`, so a
recipe carrying none is **never** craftable — the two states stay distinct.

**Recorded approximation:** vanilla fills from the inventory **and** the open
menu's craft slots; Rewo counts the inventory alone, so a recipe whose last
ingredient sits on the grid reads uncraftable. The craft-slot range differs per
menu class and guessing it would be a confident wrong answer.

**Process:** a hung mutant left its test binary holding the link output, so the
*next* mutation reported BUILD-FAIL rather than the previous one's hang — and a
botched harness left a mutation **on disk**, caught only by a grep. The harness
reaps strays now and counts a hang as a kill.

**2010 tests**; `containershot` 89, `inventoryshot` 152, `mobshot` 246/246; demo
PNG `2cc56b4acbfb92cb` byte-identical; **13 mutations — 11 killed, 2 proven
equivalent**. **Open:** the inventory→solver→flag derivation is graded at both
ends but not end to end (driving it needs a `PlaySession` — the M92/M93b sweep
shape); nothing can click the book.

### M95 — the recipe book's items, and the tab structure M93z got wrong

Tab icons and recipe results, on the book's origin — plus a correction to each
of the two milestones before it.

**M93z modelled the tabs wrong.** Its `Tab` enum was the four
`SearchRecipeBookCategory` values, and those are *the search tab of each of the
four books*, not the tabs within one. Each book has its own hand-written list —
**crafting five, furnace four, blast furnace three, smoker two** — the first of
each a search tab with a **compass** icon. M94 therefore drew four tabs on every
book. And the search flag must be **explicit**: a smoker's search tab holds
exactly one category, the same one its single category tab does, so a
"several categories" heuristic is right for three books out of four.

**M94 left the menu's icons behind** — it threaded the displacement through the
panel and the hover and missed `menu_slot_rects`, so with the book open every
slot icon sat 77 px left of its slot. M90's reason: *a function taking bare
numbers does not look like it belongs to the menu.*

**The items:** `getDisplayStack`'s cycle is **two levels** (`% entryCount` picks
the recipe, `/ entryCount` picks which of *that* recipe's display items), so
three recipes with two forms each cycle through six, not three;
`resolveForStacks` resolves only the context-free arms (`Item`, `Stack`,
`Composite`, and `WithRemainder`'s **input**) and yields **nothing** for the six
that need a `ContextMap`, because an arbitrary tag member would be a confident
wrong answer; the shadow copy is the **same stack drawn twice**.

**Three gate findings.** b8 measured 26 icons against the 27 it named — the
missing one was M93z's error surviving in the gate's own **fixture**. **Two
mutations survived b8–b11**, one putting the book's icons on the menu's origin
and one leaving the menu's icons centred: **counting icons cannot see a wrong
origin**, so b12/b13 measure positions. And b13's first draft told the two
apart **by position**, which is circular when position is what it measures.

**A harness bug of M93v's family**: the mutation runner's `'PASS —' in out` used
`text=True`, which decodes with the Windows locale codec, so the em dash became
mojibake and **every gate verdict read KILLED whether or not anything failed** —
which is what hid the two survivors. Uses the exit code now. Third detector bug
of this arc, all the same shape: *cannot tell "passed" from "could not tell".*

**1992 tests**; `containershot` 83 → **89**; `live --render-check` 23/23,
validation ON, 0 errors; demo PNG `2cc56b4acbfb92cb` byte-identical; **14
mutations, 14 killed**. **Open:** `hasCraftable` is still false everywhere (it
needs `StackedItemContents`); nothing can click the book; no search box, page
counter, overlay popup, ghost slots or recipe tooltips.

### M94 — the recipe book renders, and two errors only the windowed client could show

M93z built the model; this draws it. Panel, tabs, recipe slots, arrows, filter.

**Opening the book MOVES the menu**, so this is not a pure addition:
`updateScreenPosition` swaps `(width - imageWidth) / 2` for
`177 + (width - imageWidth - 200) / 2` — 77 px for a 176-wide panel. Rewo
measures slot hit-testing, slot icons and the hover box from that origin, so
drawing the book without the shift leaves the menu centred under a book that
overlaps it and every click lands on the wrong slot — **silently**, because the
render still looks plausible. `topPos` does not move. The draw and the hit test
now resolve through one `Placement`, M89's one-accessor rule applied to
geometry.

**Two design errors, both found by `live --render-check`, neither visible
headlessly:**

* The book was hung off `ContainerPanel` — which is `None` for the player's own
  inventory (the path `inventoryshot` pins), and **the player's inventory is one
  of exactly four screens that HAS a book**. So it was undrawable in its
  commonest case, and `containershot` structurally cannot see that: it only ever
  drives an open container.
* The gate's "crafting table" fixture used menu type **13, which is
  `enchantment`** — same 176x166 size, so every headless witness measured what
  it expected and passed. `crafting` is 12.

Both are the M86 shape. **Run `--render-check` on any milestone that adds a
render path.**

**Four chrome inversions:** a tab's sprite tracks **selection, not hover**
(`get(true, this.selected)` hard-codes `enabled` and passes `selected` as
*focused*), while the filter toggle inverts the same record the other way
(`get(filtering, hovered)` — and the sprite names make reading it as the
widget's own state easy, giving a button that never changes when clicked); a
selected tab shifts 2 px left **and takes its icon with it**; the stacked-recipe
look is **two items at (5, 3)**, because vanilla draws a copy at `offset + 1`
then *decrements* offset — reading it as applying to the back copy gives (4, 4),
which renders as one item; and the panel is sampled from **(1, 1)**.

**The gate's rewrite was the instructive half. b1 passed while measuring nothing
it claimed** — it probed the book's centre and compared open against shut,
reading `[0,0,0]` against `[255,255,255]`, and neither was the book: the probe
sat on a recipe slot's black border, and the *control* frame had the menu over
that position because an open book moves the menu. **A frame diff may not let
its control change with its subject.** Every witness now names its sprite's
value read out of the PNG (`tab.png` 139 vs `tab_selected.png` 198,
`slot_craftable` 139 vs `slot_uncraftable` 106) — "different from the backdrop"
would pass with every tab drawing the same art. And a mutation deleting
`take(view.shown)` **survived the gate** and was killed by the model's test,
because the gate's fixture sized its slot vec to `shown` and made the guard a
no-op.

**1979 tests**; `containershot` 76 → **83**; **`live --render-check` 22 →
23/23**, validation ON, 0 errors; demo PNG `2cc56b4acbfb92cb` byte-identical;
**18 mutations, 18 killed**. **Open:** no items in the book (grid results, tab
icons); `hasCraftable` is `false` for every collection so every slot wears the
uncraftable chrome (it needs `StackedItemContents`, and guessing `true` is
worse); nothing can click it, so tab/page are pinned to 0 and hover never
reaches the arrows or filter; no search box, page counter, overlay popup or
ghost slots.

### M93z — the recipe book's UI model, and a filter button that toggles one stage of three

M93y decoded the packets and named the book as the subsystem that had to
follow. This is its **model** — geometry, tabs, collections, filtering,
pagination. **The render is separate and not in it**, on M63's split.

**It is positioned against the WINDOW, and nothing else Rewo draws is.** Every
other screen is panel-relative. `getXOrigin` is `(width - 147) / 2 - xOffset`,
centred on the *window* then pushed left 86 to flank the menu — and **`xOffset`
collapses to 0 on a narrow window**, which is what makes the book cover the menu
rather than hang off the edge. Deriving its origin from the open menu's panel is
right at one window size and wrong at every other, invisibly until a resize.

**`updateCollections` is three `removeIf` stages and the FIRST IS
UNCONDITIONAL** — the filter button toggles only the third (`hasCraftable`).
Read as gating the filter, it takes the first (`hasAnySelected`) with it, and
"show all recipes" then lists furnace recipes in a crafting table.

**The crafting tab lists `equipment` first**, not in the registry's id order
(building_blocks, redstone, equipment, misc) — `includedCategories()` is a
separate hand-written order, so deriving a tab's contents from ids reorders
every crafting collection. **Three of the 13 categories belong to NO tab**
(stonecutter, smithing, campfire — those screens have their own UI), so the
lookup must be allowed to answer nothing. **Zero collections give ZERO pages**,
and `clamp_page`'s `totalPages <= currentPage` resets an index *equal* to the
count, to the **front** rather than the new last page.

**Ordering is deliberately not a contract**: a group takes its first-seen
member's position, and insertion order is preserved because a stable book beats
an arbitrary one — **not** because vanilla guarantees it (its input is a
`HashMap`'s `values()`). The opposite of M93s's stonecutter, where the index a
click sends made order load-bearing.

**The witness was wrong before the code, again**: it shrank to 20 collections —
**one** page, whose last index *is* 0 — and asserted the reset was not to "the
new last page", so `assert_ne!(0, 0)` failed and the fixture could not have
expressed its claim either way. Now it shrinks a five-page list.

**1956 tests**; `containershot` 76, `mobshot` 246/246; demo PNG
`2cc56b4acbfb92cb` byte-identical; **10 mutations, 10 killed**. It also found
**§0.0's own drift running in reverse** — the prose was current through M93t
while the coverage number was four milestones stale at 109/0/32, because a
milestone that ships a finding writes the paragraph and forgets the table.
**Open:** the book's render (panel, tabs, 20 grid buttons, arrows, the search
field on M93t's `EditBox`, the filter toggle), ghost placement into container
slots, and the two serverbound packets — reachable now, still unsent.

### M93y — the recipe book's decode, and a class-C claim that IS one

Four packets decoded into session state, plus the `SlotDisplay` (11 variants)
and `RecipeDisplay` (5) trees. **The class-C label here is correct** — unlike
the four M91–M93u overturned — and saying so matters: the book is a tabbed,
searchable, filterable list with ghost placement, none of which exists. This is
the half that comes first, on **M63's split**: decoding needs no listening.

**Dispatched rather than left resolved-but-ignored, and M74's check is why** —
it caught the ids the moment they resolved and named the class the coverage doc
keeps at **zero**: a packet whose id resolves and whose body is dropped reads as
*handled* to every grep, which is worse than absent.

The registries are **built-in**, so they come from the report (M92's rule) — and
the alphabetisation trap bites harder here than in M64, because the variants
have **different body lengths**, so a wrong table **desyncs the reader
mid-packet** rather than mislabelling. `group` is `OPTIONAL_VAR_INT`, the `+1`
family in optional form (**0 is absent; group 0 rides as 1**). A shaped recipe's
**width and height precede** the ingredients they describe. **`replace` clears
the book** — true on join, false per unlock.

**Verified against a real server, not only its fixtures.** The nine decode tests
drive bytes *I* wrote; a temporary counter against a live 26.2 server showed the
book reaching one entry on join, through the production path. Worth doing
because **"no warning" is also what a packet that never arrived looks like** —
the render check was green either way, and only a *positive* assertion about
what was decoded tells them apart.

1942 tests; coverage **110/0/31 → 114/0/27**, class C 20 → **16**; `live
--render-check` 22/22 validation ON 0 errors. **Open:** the book's UI (its
search field now has M93t's `EditBox` to build on) and the two serverbound
packets, unsent because nothing can yet click what would send them.

### M93x — the trade button's chrome, and reading WHICH witness fires

`Button.Plain`'s `extractDefaultSprite` — `widget/button` nine-sliced from a
200×20 sheet with border 3, empty label. **Only two of `WidgetSprites`' four
cases are reachable**: vanilla toggles the button's `visible` and never its
`active`, so a row past the end of the list draws *nothing* rather than a greyed
one.

**The slicing is the find.** At 88×20 on a 200×20 sheet the height matches, so
the nine-slice degenerates to horizontal-only — and vanilla's `NineSlice`
**tiles** rather than stretches, so a narrower button draws **one partial
tile**: the middle is a 1:1 slice of the sheet's first `w - 6` face pixels.
"Scale the middle" would resample every pixel of the face and blur it. The
witness that pins it: the button's **last column is `(0,0,0)`**, source x 199 —
the sheet's black border — where a naive 1:1 blit from x 0 would give
`(112,112,112)`.

**The transferable part is which witness fired.** All three mutations died, but
inverting the hover pair was killed by **z3, not z2** — because z2 asserted only
that the two frames *differ*, which is symmetric. Exactly M93t's x5 flaw, and it
surfaced *only* because the kill came from the wrong witness. **Reading which
witness a mutation kills is worth as much as reading whether one did.**

1929 tests; `containershot` 73 → **76**; `live --render-check` 22/22 validation
ON 0 errors. The merchant is complete; the one remaining limit is not a widget
but the component-predicate decline.

### M93w — the discounted price pair, and an override that defeats a rule

`extractAndDecorateCostA`. **One icon, not two** — `fakeItem` is called once,
outside the branch, with the **modified** cost, so the discounted display is two
*numbers* over a single item. The strikethrough at `+7` crosses the **first**
number rather than the gap, because the labels are right-aligned into the icon's
16 px box. And **a count of 1 normally draws nothing**, so the
`count == 1 ? "1" : null` override exists *solely* to defeat that rule — passing
`null` throughout drops a digit exactly when a discount has reached 1.

**A witness had to be narrowed rather than fixed.** It first claimed the two
digits as well as the strikethrough and measured **0 changed pixels** —
correctly, because the gate's frame builds the **panel** and the count labels
come from `screen_icons`, which it never calls. M45's shape again: a gate
reimplementing a slice of the app's setup misses what lives outside it. The
strikethrough is a panel overlay and is witnessed; the digits are graded at the
model level, and the gate now says so in a comment so the next reader does not
read it as an omission.

One equivalent mutant, labelled in the code: `icon_for` ignores the count, so
which count the cost-A icon call passes cannot matter.

1927 tests; `containershot` 71 → **73**; `live --render-check` 22/22 validation
ON 0 errors. **Open on the merchant:** only the trade button's own
`Button.Plain` chrome, and the component-predicate decline.

### M93v — the XP bar, and a blit argument I read as a size

M93u recorded this as blocked on `VillagerData`'s thresholds and
`getFutureTraderXp`. **Neither was.** The thresholds are five ints, and the
future xp is *derived by vanilla itself* — `updateSellItem` matches the payment
slots against the offers and takes the matched offer's xp.

**`traderLevel < 5` gates the background too**, so a maxed villager shows
nothing rather than a full bar; `getMinXpPerLevel` and `getMaxXpPerLevel` both
return **0** outside the levelling range, so their difference is the bar's
divisor and only the two guards keep it safe; the fill is the fraction of the
**level**, not the career; and `getRecipeFor`'s `selectionHint > 0` is
**strictly** greater, so selecting the *first* trade falls through to the scan.

**Two mutations survived and both were real.** One was M71/M93t's shape — logic
in `live_cmd`, which has no test module — now `satisfied_offers`. **The other
is a lesson about reading a signature**: mutating the result segment's source
offset changed *nothing*, because I had read
`blitSprite(sprite, 102, 5, u, v, x, y, w, h)`'s first two arguments as the
source **rect's** size when they are the **sheet's**. Every segment was drawing
the whole bar squeezed into its width, so the offset could not matter. **A
surviving mutation is a question, not just a verdict** — what it asks about is
sometimes not what you mutated.

**And an instrument failure of my own**: the shell totalling test counts used
`grep -v "0 passed; 0 failed"`, which matches `71`**`0 passed`**`; 0 failed`, so
it silently dropped `rewo-world` the moment its count hit a multiple of ten.
M91's finding in the measuring tool rather than the build — the only signal was
a total moving the wrong way.

1924 tests; `containershot` 67 → **71**; `live --render-check` 22/22 validation
ON 0 errors. **Open:** the discounted-price pair with its strikethrough, and a
cost carrying a component predicate declines rather than guesses (M41 has a
digest, not per-component values).

### M93u — the merchant, and the fourth class-C claim to fall

The coverage doc filed `merchant_offers` as class C. It needed nothing Rewo had
not built — `ItemStack` (M34/M41) and the `TypedDataComponent` walker M52e
wrote for `can_place_on`. **That is four this arc, and the reasons differ**,
which is the part worth keeping: M91's furnace recipes and M93s's stonecutter
list were **jar data**; M93's merchant quick-move **never consulted** the
packet; and here the data really **is** server-rolled — only the decode was
mislabelled. So "blocked on a packet we don't decode" deserves a check against
what the packet carries *and* what decoding it would cost.

**Traps:** the order is **costA, result, costB** (the sold item sits *between*
the costs, while every constructor lists them costA/costB/result); the numerics
are `writeInt`, **fixed big-endian** in a var-int protocol, so a var-int reading
turns a discount into a surcharge; and `Item.STREAM_CODEC` is `holderRegistry`,
**raw 0-based** — the fifth appearance. In the price, `demandDiff` clamps at 0
from below while `specialPriceDiff` is added *after* and is not floored (that is
the discount), and **only cost A is modified**.

**The scroll is not the stonecutter's with new numbers**: `scrollOff` is an
**offer index**, one notch is one offer, and the drag rounds the index rather
than a fraction. The thumb's bottom override is load-bearing for **short**
scrollable lists (8/9/10 offers land at 91/106/111) and redundant for long ones,
where `min(113, …)` caps an overshoot — the opposite of what I first asserted,
and only visible by computing both regimes.

**Reading the render loop caught two offsets no witness covered**:
`offerY = yo + 16 + 1` against the buttons' `+ 2`, so a row's items sit one
pixel **above** its button; and `sellItem1X = xo + 5 + 5`, so cost A adds the
button's 5 **twice**. And M93s's lesson landed twice — the arrow's x was wrong
(`5 + 5 + 20` for `xo + 5 + 35 + 20`), and when the witness failed again I
explained it *wrongly* rather than re-reading the sprites.

A surviving mutation is **equivalent here and not in vanilla**: dropping the
visibility guard changes nothing because this computes `row = i - scroll_off`
directly, where vanilla's `offerY` advances only inside the drawn branch.

1916 tests; `containershot` 63 → **67**; `live --render-check` 22/22 validation
ON 0 errors; coverage **110/0/31**, class C 21 → 20. **Open:** the XP bar's fill
(needs `VillagerData` thresholds and `getFutureTraderXp`) and the
discounted-price pair with its strikethrough.

### M93t — the EditBox, a subsystem Rewo never had, and a red band

M93n shipped the anvil's semantics and recorded that **nothing could type** —
Rewo read `PhysicalKey` and never a character. This is `EditBox`'s editing core
plus the `KeyEvent.text` seam, wiring the anvil end to end.

**The buffer is `Vec<u16>` and that is not fussiness**: every index in vanilla's
EditBox is a Java String index, and one rule — `isHighSurrogate(charAt(max - 1))`,
which stops a truncation splitting a pair — is only *expressible* in UTF-16.
M93n had already counted the anvil's 50 in code units for the same reason.

Findings that read backwards: `insertText`'s room is a **double negative**
(`maxLength - length - (start - end)`, so the selection's width is added *back*);
`setValue` truncates with **no** surrogate check where `insertText` has one; an
**uneditable box still swallows** backspace, because `return true` sits outside
the `if (isEditable)`; Insert and the vertical arrows share the **`default`**
label and so are treated as unrecognised; **word motion is not symmetric**, so
Ctrl+Left then Ctrl+Right does not return you; and the four shortcuts need
control down **and shift up and alt up**.

**`AnvilScreen.keyPressed` reaches `super` only when the box neither handled
the key nor could have** — so with an item in slot 0 **every non-escape key is
swallowed**: E does not close the anvil, a number key does not swap, Q does not
drop. That reads like a bug and is exactly what typing requires.

**A red band said the chrome was missing.** `anvil.png` carries a pure
`255,0,0` band exactly where the name field goes, and `extractBackground`
covers it with a sprite chosen by slot 0. Rewo drew the panel and not the
sprite, so the first run of these witnesses read `[255,0,0]` for "the bare
panel". **A placeholder in a vanilla texture is a deliberate signal.**

**Three mutations survived with three different verdicts**, which is the
transferable part: the swallow rule was a **real gap** (it lived in `live_cmd`,
which has no test module — M71's finding — and is now `anvil::key_consumed`);
the field-background inversion was a **real gap behind a symmetric witness**
(x5 asserted the two frames *differ*, so swapping them passed — it now names
the values, read out of the PNGs); and `deleteWords`' selection guard is
**equivalent**, because `deleteCharsToPos` carries the same check and vanilla
is doubly guarded too.

1899 tests; `containershot` 58 → **63**; `live --render-check` 22/22 validation
ON 0 errors; demo PNG byte-identical. **Open:** the clipboard is **in-process**,
not the OS's (no crate pulls one in, `winit` exposes none); no IME pre-edit; no
click-to-position or drag-select inside the field.

### M93s — the stonecutter, and an order that is a wire contract

The plan called this widget "genuinely class-C (`update_recipes`)". **It is
not** — the third such claim this arc to not survive the decompile, after M91's
furnace recipes and M93's merchant quick-move. The pattern is worth carrying:
*"blocked on a packet we don't decode" deserves a check against what the packet
actually carries*, because for vanilla content the answer is usually in the jar.

**The contents were never the hard part; the ORDER is, and it is part of the
wire contract.** A click sends an *index*, and the server resolves it against
`selectByInput` — a **filter**, which preserves the master list's order. Get the
order wrong and every click cuts a different block than the one drawn, **with no
error anywhere**: M64's alphabetisation trap somewhere nastier, because there
the ids merely came out wrong while here the server acts on it. It reproduces
because `RecipeManager.prepare` loads into a `SortedMap<Identifier, _>` — and
**`Identifier.compareTo` is path first, then namespace**, not the combined
`namespace:path`. The generator sorts by the file stem explicitly rather than
the filename: those agree only because `.` (0x2E) is below every character
`[a-z0-9_]` uses.

**One cell has three y-origins** and vanilla means all three — `+2` for the icon
/ highlight / tooltip, `+1` for the chrome and the cursor, **`+0` for the
click**. The first witness called the top two pixel rows "clickable but not
highlighted"; they are not. Both boxes are 18 tall on an 18 pitch, so they
**tile** — the offset is a *shear*, not a gap — and those rows highlight the row
**above**. A click lands one row *below* the lit cell at every boundary, and
away from a boundary they agree, which is why it is easy to miss. The scrollbar
likewise has three origins (grab `+9`, drag track `+14`, draw `+15`) and the
drag divides by 39 while the draw multiplies by 41, so **vanilla's thumb
overshoots its own track by two pixels**.

**The fourth detector error of the arc, same shape as the other three.** `w2`
proved "cell 6 draws no chrome" by comparing against the bare panel — and
`recipe_selected.png`'s centre is `(81, 73, 58)`, *exactly* what
`stonecutter.png` reads at that probe, so a cell 6 wrongly drawing a **selected**
chrome would have passed. The control is now a twelve-recipe view, differing in
one thing only. Reading the sprite PNGs' pixels *before* writing the witnesses
is what made the rest sound. Also: `mouse_gui` is GUI pixels and the first cut
converted the other way, so the hover never landed.

**A surviving mutation was a real gap, not an equivalent mutant** — swapping
`selected` and `hovered` changed nothing, because the orderings differ *only* on
a cell that is both and no witness hovered the selected cell.

1879 tests; `containershot` 52 → **58**; 6 mutations, 6 killed; demo PNG
byte-identical. **Open:** no tooltip on a recipe button, and the datapack caveat
now has teeth — a pack that *reorders* stonecutting recipes makes a click cut
the wrong block.

### M93r — the self-calibrating-witness sweep, and what it did NOT find

M93q's closing line asked for a sweep of anywhere a `*shot` witness computes an
expectation from a `pub const` the renderer also reads. Run over ~1,400
witnesses in 34 gates: **96 of 480 SCREAMING consts are read by both a gate and
production**, narrowed to value-shaped ones, each checked for **value** (against
the decompile) and for **pinning** (by mutation, the only real evidence).

**Three real holes, all with correct values** — process debt, not a rendering
bug. `GLINT_STRENGTH`: `handshot`'s `n3` asserts every glint vertex carries it
while *reading* it; mutating `0.75 → 0.55`, a 27% change in the foil's alpha,
left `handshot` (34), `inventoryshot` (152) and all of `rewo-gpu`'s tests green.
`DARK_GRAY`: same shape in the advanced tooltip, and the mutation used was
`0x555555 → 0x3F3F3F` — **the exact wrong grey M93p shipped for the loom**,
which is the point: a plausible flat grey is what a guess produces, and no
amount of pixel-reading catches one when the reader shares the value.
`DYE_DIFFUSE_COLORS` is a different failure — **duplicated** across `rewo-data`
(banners/signs) and `rewo-gpu` (fish, the sheep derivation), and neither crate
depends on the other, so **no test *could* have compared them**; the agreement
test has to live in `rewo-app`.

**What it did not find matters as much, and both shapes look like the bug.**
An **enum comparison** (`== PotWobble::Positive`) names which outcome is
expected — an identity, not a transcribed value. And a constant used as a
**search key** self-guards: `blockentityshot` finds a sign by
`line_height == HANGING_LINE_HEIGHT` then asserts `line_y` against literals, so
a wrong constant makes `find` return `None` and fails. Most value-shaped consts
are also pinned already, **often inside the gate rather than a unit test**
(`BAR_W == 182`, `scale_armor == 0.16`, `(ANCHOR_ACCEL - 0.0139…).abs() <
1e-17`) — a first detector looking only in `#[cfg(test)]` called all of those
unpinned, so **the mechanical search has a high false-positive rate and its
shortlist must be read**.

**The best pin in the codebase is a derivation, not a literal.**
`SHEEP_WOOL_COLORS` is self-calibrating in `mobshot` and needs no fix, because
`entities.rs` pins it *by rule* — `floor(diffuse * 0.75)`, white overridden to
`0xE6E6E6` — and **12 of its 16 rows would differ under `round`**, so the test
proves the rule rather than the numbers. Prefer that form for any derived table.
Also: there are two different `TITLE_SCALE`s (hud 4, death screen 2), both
right, which a name-keyed search reports as one value.

**1865 tests / 0 failures**; 5 mutations, all alive before the pins and all
killed after; demo PNG `2cc56b4acbfb92cb` byte-identical.

### M93q — the overlay colour quad, and two ways a pixel gate goes blind

The loom's preview is `fill` then `blit`, and the overlay path could not draw
the first half: `overlays` is `(sprite, PanelBlit)` and every sprite index
samples the atlas. M93q adds an untextured mode (a negative-`u` sentinel in the
fragment shader, a per-quad `tint`, a `FILL_SPRITE` index), the 43
banner-pattern textures, and the loom arm — closing the loom **end to end**.

**The milestone is the two blind spots, not the quad.**

**A gate that cannot reach a call site does not test it.** `o19`/`o20` grade the
fill primitive from a hand-made overlay list and pass whether or not any menu
emits one, because `container_panel_for_open_menu` hardcoded `loom: None` —
**delete the whole loom arm and both stay green**. This is M92's finding one
level over: M92's case was a gate *supplying* an input production derives; this
is a gate *unable to enter* the branch. The wrapper now carries the view (the
M93m precedent), and `o21` drives the real arm with a control frame and a
two-sided test that catches the fill/pattern order.

**And a witness can be sound on one property of the same draw and vacuous on
another.** `o21` reads `LOOM_PREVIEW_BACKING` to compute its expectation, so a
wrong constant moves render and expectation together — sound for the **order**,
self-calibrating for the **value**. The value was wrong.
`DyeColor.GRAY.getTextureDiffuseColor()` is the **third** constructor argument,
`4673362` = `0x474F52`, faintly blue; M93p shipped `0x3F3F3F`, which is none of
GRAY's three colours — and the trap is that both neighbouring arguments
(`fireworkColor` `0x434343`, `textColor` `0x808080`) are **more neutral than the
right answer**, so a plausible flat grey is exactly what a guess produces.
Mutating it back demonstrates the asymmetry: `containershot` — 52 witnesses,
validation on, real pixels — **survives**, and three lines of unit test stating
the decompile's literal **kill it**. **Pin a number against its source, not
against itself.** Worth a sweep: any `*shot` witness computing an expectation
from a `pub const` the renderer also reads covers everything about that draw
except the constant.

**Recorded, not fixed:** `--render-check` never opens a loom, so the fill's
windowed call site is unexercised. The blocker is not injection —
`loom_display_patterns` is false without a banner **and** a dye in the slots, so
an injected empty loom would witness nothing; staging it is a harness of its
own. The loom's **scrollbar drag** is also unwired, so only the first 16
patterns are reachable.

### M93l — the beacon's press state machine and `set_beacon`

M92d shipped the chrome, the geometry **and** `beacon_button_hovered`, so
unlike M93h the call site already existed and the model is not built against a
guessed one.

**The guard that reads backwards:** choosing a new primary **discards** the
secondary — *unless* the secondary is already the same effect. A secondary is
only meaningful alongside the primary it was chosen with, and the exception is
the "primary at level II" double. Inverting it keeps exactly what should be
discarded and discards what should be kept.

**The upgrade button is not a fourth kind of press** —
`BeaconUpgradePowerButton extends BeaconPowerButton` with `isPrimary = false`,
and `updateStatus` re-points its effect at the primary, so it presses as an
ordinary *secondary* holding the primary's effect.

**A press only happens on an active, visible button**, so the gate is
`beacon_button_state(..)` rather than a re-derivation — `updateStatus`'s rules
stay the single source for both what is drawn and what responds.

**`MobEffect.STREAM_CODEC` is `holderRegistry` — a RAW 0-based id**, not
`holder`'s `id + 1`. That has now bitten in M16, M21, M55 and M92d, and it is
quiet every time: an off-by-one names a **real** effect, so the beacon grants
the wrong one. The witness pins effect id **0**, which is where the two
conventions disagree most visibly.

**M93m wired the press — and the choice had to stop being derived.** M92's own
comment admitted it: *"Rewo has no click path here yet, so this reads the data
slots directly"*. Vanilla's `BeaconScreen` owns `primary`/`secondary`, seeded
from the menu and then **moved by clicks** before the server hears anything, so
a click-driven beacon cannot re-derive them each frame.

**The seeding rule is odder than it looks**: `dataChanged` re-reads *both*
effects on **any** slot id — including the pyramid levels — so a beacon growing
under you discards an unconfirmed pick. Hence the watermark is a per-menu
**data-write counter**, not the menu identity (misses it) and not the effect
slots (also misses it, because the clobbering write is to a different slot).
Only the two effects are screen-owned; levels and payment are re-read every
frame, so a payment arriving mid-selection lights Confirm without disturbing
the pick.

**A dark button does not consume the click** — `AbstractWidget.mouseClicked`
returns true only when it fires, so a disabled beacon button falls through to
the slot logic exactly as a disabled enchanting row does.

**One mutation survived, and it was the render**: every witness drove the
menu's data slots, so reverting the render to the derived choice changed
nothing they could see — a click would have moved a choice nothing drew, M93i's
"correct but invisible" one screen over.

**Recorded, not fixed:** the confirm closes the **client's** screen only. Rewo
resolves no *serverbound* `container_close` — `ids.rs` has the clientbound one
alone — so the server still believes the menu is open. That predates this and
affects **every** screen close.

### M93n — the anvil's rename

Listed as "needs a text field"; it needs *two* things and only one is the field.

**`validateName`'s `length() <= 50` counts UTF-16 code units, not characters.**
An emoji is 2 there and 1 to `chars().count()`, so a char-count check accepts
names the server rejects — silently, since `setItemName` just returns false
while the client has drawn the text. 25 emoji are legal, 26 are not.

**Typing an item's own name means *clear* the name.** No `CUSTOM_NAME` plus a
typed string equal to the hover name sends `""` — there is nothing to set.
Without the `!has(CUSTOM_NAME)` half, renaming a named item back to its
displayed name *clears* it; without the equality, every rename becomes a clear.

Also: `None` (too long) is **not** `Some("")` (a legal clear); a rejected name
does **not** advance the stored name; and the empty string is meaningful on the
wire, so a sender that suppressed it could never un-name anything. Sent on
every accepted keystroke, not on a confirm — the anvil has none.

**Recorded, not built:** `EditBox`. Rewo's key handler reads
`PhysicalKey`/`KeyCode` and never `KeyEvent.text`, so **nothing can type** — a
subsystem it has never had, shared with the class-C chat/command-input cluster.

`containershot` `d12` pins that the container arc now needs **four distinct
serverbound screen packets** — `container_button_click` 17,
`container_slot_state_changed` 20, `rename_item` 48, `set_beacon` 52. Four
screens, four packets, none a mode of another.

### M93o — the loom, and two of three recorded blockers that were wrong

M93h listed three. **One was real.** The occupied-pattern-slot case does *not*
need the `PROVIDES_BANNER_PATTERNS` HolderSet value off the wire — the
component is on the item's **prototype**, which never crosses the wire — and it
does *not* need the `banner_pattern` registry, because the value is a **named**
HolderSet, i.e. a tag id whose contents are jar data. The real blocker was that
`expand_tag` hardcoded `tags/item`.

**Same shape as M93e's correction**, and the lesson repeats: *a blocker
recorded from the wire's point of view can be wrong because the answer was
never on the wire.*

The item→tag mapping is **extracted from both sides** (`Items.java` names a
constant per item, `BannerPatternTags` maps it to a tag id) rather than
inferred — `flower_banner_pattern → pattern_item/flower` looks like a rule and
is a naming coincidence.

Four screen details that invert: an item with **no** patterns offers
`ImmutableList.of()`, **not** the default set (falling back would let junk
unlock everything); the grid needs a **dye**, not just a banner; `canScroll` is
strictly `> 16`; and the bounds test and the **range** test are separate, so a
cell past the end is *hit* then *rejected* and must not consume the click.

**One mutation shown equivalent — in Rust only.** Deleting `index >= 0` changes
nothing because `(-1i32) as usize` wraps past any representable bound; it is
load-bearing in Java, where `<` does not wrap. Rewritten as `try_from` so the
intent does not lean on the wrap.

**M93p landed the preview's geometry, not its render** — and says so, with a
surviving mutation as the evidence: reverting the pass to ignore the new source
size changes nothing observable, because no overlay uses it yet.

Transcribed: a **5x10** destination at `(cell + 4, cell + 2)` sampling the
**21x40** region of the 64x64 banner texture starting **one pixel down**. The
ratio is **not uniform** (21/5 vs 40/10), so it cannot be a scale factor —
hence `ProgressBlit.src` and `PanelBlit.{sw,sh}`, inert for every 1:1 blit
before it. And the pattern is drawn **untinted over flat grey**
(`DyeColor.GRAY.getTextureDiffuseColor()`) — not the banner's base colour, not
the dye's; tinting it with the dye would look plausible and be wrong for every
button.

**Left, precisely:** the 43 banner textures into the **overlay atlas**
(mechanical, but M48's lesson is that atlas growth is where addresses move),
and a way to draw the **solid grey backing** — `overlays` is
`(sprite, PanelBlit)` with no colour and no untextured mode, a third structural
change.

### M93h — the crafter's slot toggles, and a scoping claim that was wrong twice

The first bespoke-widget work, and it opens by **correcting the plan**: the
claim that "the loom and crafter need only their button lists" was wrong about
both.

**The crafter does not use `container_button_click`.** `CrafterMenu` has no
`clickMenuButton` override; `CrafterScreen` sends
`container_slot_state_changed` — **id 20** against the button click's 17. Only
the loom, the enchanting table and the two class-C screens are button-click
screens.

**The loom needs far more than a button list** (and is not shipped):
`getSelectablePatterns` is `BannerPatternTags.NO_ITEM_REQUIRED` when the
pattern slot is empty — a **`banner_pattern`** tag, where `expand_tag`
hardcodes `tags/item` — and otherwise the stack's `PROVIDES_BANNER_PATTERNS`
**HolderSet value**, which Rewo walks and discards, resolved through the
`minecraft:banner_pattern` registry that `parse_registry_data` does not
capture.

Four crafter facts that invert:

1. **`containerData[i] == 1` is DISABLED** — `setSlotState` takes an
   `isEnabled` and stores its inverse, so reading the value as "enabled"
   disables exactly the slots the player left on.
2. **`isSlotDisabled`'s `< 9` is load-bearing**: index 9 is the **power flag in
   the same array**, and 9 is a legal index, so a powered crafter would read as
   having a ninth disabled slot with nothing faulting.
3. **PICKUP is asymmetric** — re-enabling is unconditional, disabling needs an
   **empty cursor**, because clicking an empty enabled slot while holding
   something is a placement.
4. **The toggle is ADDITIVE** — `slotClicked` ends in an unconditional
   `super.slotClicked(...)`.

**And the packet body inverts against its sibling**: `container_button_click`
is `(containerId, button)`, this is `(slotId, containerId, newState)` — slot
first. The transposition yields a *well-formed* packet that toggles the wrong
slot of the wrong menu.

**M93i wired it into both click paths, and the wiring exposed a defect in
M93h.** `crafter_toggle` took an `is_swap: bool` and so treated **every**
non-swap input as PICKUP — but vanilla's `switch` has `case PICKUP` and
`case SWAP` and **no default**, so a shift-click would have silently re-enabled
a disabled slot. **No witness could see it because the function had no
caller.** That is the argument against leaving a model unwired: the shape of
the call site is an input to the design.

One funnel (`PlaySession::crafter_slot_click`) called from both paths —
including `finish_drag`'s one-slot-drag re-dispatch, which vanilla routes back
through PICKUP and which would otherwise have quietly not toggled. And because
**`PlaySession` has no test module anywhere in the repo** (M71's hazard — it
owns a socket), everything the adapter does except the send is extracted into a
tested function.

**M93j drew it, pixel-graded.** The cover is a **third slot geometry** — the
icon is 16x16 at the slot, M35's highlight 24x24 at `slot - 4` *bracketing* it,
and the cover 18x18 at `slot - 1`. And it **replaces** the slot's render rather
than layering over it (`extractSlot` never reaches `super`), the opposite
composition from the toggle's additive one. Vanilla writes the redstone arrow
in **screen coordinates**, alone in the class; the two forms agree only for the
standard 176x166 panel, so the witness re-derives it at three window sizes.

**Two render mutations survived first, and both taught something.** The arrow
*swap* survived a bbox witness — same box, different sprite — and the witness
that fixed it was itself wrong: it asserted the powered arrow is *brighter*,
where measured it is luma 68 against 124, because lit redstone is saturated
**red** (`0.299·255 ≈ 76`) against pale grey. **Luma is the wrong statistic for
"lit"**; redness separates them 0.0 vs 227.5. The witness derived its
expectation from the art, so the art corrected the premise rather than the
premise inverting the witness. The item-suppression mutation survived because
`containershot` never calls `init_gui_items`, so no pixel witness there reaches
the icon pass — graded instead by calling the production `screen_icons`.

**M93k added the `gui.togglable_slot` hint — and found a fourth hover that
was never made container-aware.** `screen_tooltip` was handed
`session.inventory` and the free `slot_at` (which *is* `PLAYER.slot_at`), so
with a chest open it named whatever the player had at the same index, at a
differently-centred origin. The highlight and the icons were both fixed; this
one was missed — **M89's "a per-call-site choice is how they come to disagree",
surviving in a fourth site.**

The hint is **derived, not transcribed**: vanilla's five conditions are exactly
the preconditions of a PICKUP that would *disable* the slot, so it asks
`crafter_toggle(PICKUP, ..) == Disable` and cannot promise an action the click
will not take. (It shows on an **enabled** slot — the constant is named
`DISABLED_SLOT_TOOLTIP` and reads "Click to disable slot".)

**Two mutations survived and each needed a different fixture.** Dropping the
grid gate survived because every witness hovered a crafter's grid — so every
empty slot in every menu would have offered to disable itself. And dropping the
panel-size half survived because **the crafter's panel IS 176x166**, making the
two forms identical for it; only a six-row chest (176x222, origin 28 px off
against an 18 px pitch) can see it.

**The crafter is complete end to end**: decode, model, packet, click routing,
render, hint. Only `requestCursor(POINTING_HAND)` remains, and Rewo has **no
cursor-shape concept at all** — winit plumbing, not a transcription.

 `live --render-check` not re-run —
M93 adds no render path.

*(An earlier draft of this entry said M87–M92 were all unmerged. They were not: `main` was already at M91, and only M92 was outstanding. The claim came from trusting REWO_PLAN §0.0's stale 2026-08-02 audit line instead of reading `git log` — the exact failure that section warns about. M92 is merged now.)*

### M91 — the furnace family (2026-08-03)

Five commits. A furnace takes a shift-clicked stack to the right slot and
shows its flame and progress arrow — `container_set_data`'s first consumers.
Detail in `REWO_PLAN.md` §15.

**The premise this was scoped on was wrong, and checking it before building
saved the work.** A fuel table alone unblocks nothing: vanilla checks
`canSmelt` **before** `isFuel`, and a log is **both** — fuel, and smeltable to
charcoal — so without `canSmelt` the *first* branch is unevaluable for every
item. **What unblocked it: the recipes are in the jar.** `canSmelt` reads a
`RecipePropertySet` the client normally gets from `update_recipes` (class C),
but for vanilla its contents are the ingredient sets of
`data/minecraft/recipe/*.json` — already the source for `ItemTags.SPEARS`
(M19) and the enchantment tags (M42). A class-C blocker that turned out not to
be one. **The caveat is the same one M19/M42 carry** and is stated in the
generated file: a datapack that adds or removes a smelting recipe makes the
table wrong with no error anywhere.

**Generators here, where M87a's layouts are a hand table**, and the difference
is measurable rather than stylistic: `FuelValues` is one regular builder idiom
whose only cross-file work is expanding tags (data), where the layouts were
four idioms plus cross-class builders that defeated extraction at 17 of 25.
280 fuels; FURNACE 156 / BLAST 62 / SMOKER 9 accepted inputs.

**The generator's arithmetic was wrong first, and the near-miss is the
lesson.** I wrote the evaluator left-to-right *and said so in a comment*; Java
respects precedence, so `1 + baseUnit * 20` is 4001, not 4020. The *other*
`1 + …` term gives **67 under either reading** — spot-checking that one (the
distinctive number, the natural choice) would have confirmed a broken
evaluator. Only `dried_kelp_block` separates them, out of 280. **Pin a set of
known-good values, not a representative one: the space is not uniform.**

**Three accepted-input sets, not one** — a smoker takes food and not ore, a
blast furnace the reverse, and a log is smeltable in a furnace *only*, so in a
smoker it is merely fuel.

**The flame grows upward**: its source and destination `y` move together, so
the bottom edge is fixed and the top rises. Anchoring at a fixed top makes it
shrink downward — an animation rather than an error.

**Two instrument failures, both found here:** M87f's screen survey did not
follow `extends`, so it recorded six centred titles where there are **nine**
(the furnaces inherit theirs); the checker now walks the chain. And **my own
test-totalling loop could not tell "0 tests passed" from "no tests ran"** —
`rewo-app`'s tests stopped compiling while its library built, every gate
passed, and the total silently fell 1712 → 1620. Ninety-two tests were not
running, and the only tell was a number moving the wrong way.

**Open on the container arc:** the ~11 bespoke-widget screens (anvil text
field, enchantment buttons, beacon, merchant trade list, loom/stonecutter
scroll grids, crafter toggles); `container_set_data` is consumed by the
furnace family and by **nothing else** (brewing bubbles, enchantment levels,
the beacon); and the crafting `quickMoveStack` shape, which declines rather
than guess.

- **Verification policy (user mandate): headless-first.** `rewo --headless N
  --chart-demo --out x.png` renders offscreen (no window) to a PNG;
  `rewo --run-seconds N` soaks windowed and prints percentile stats. Every
  Rewo milestone must ship a self-check path like these — the user does not
  manually test what a machine can check.
- Render disciplines already load-bearing: shader color constants are
  authored sRGB and **must convert to linear in-shader** (SRGB attachments
  encode on store); UI passes **mask alpha writes** so readback PNGs stay
  opaque. Shaders are GLSL compiled by glslc from the installed Vulkan SDK
  (`VULKAN_SDK` env; validation layers come with it).
- The user's network (Phase H's "chickenedin") is now named **Frogsy** and
  is Rewo's staging target (D1 in the plan); public servers are out of
  scope for Rewo until the user says otherwise (anti-cheat ban risk).

---

## Docs audit, 2026-08-07 (after M107)

A pass over **every** `.md` in the repo root, not just the ones the milestone
touched. Five documents were stale and two carried statements that were
actively false. What it found is more useful than the diffs:

- **This file has a GENERATED mirror** (the other of the two agent-instruction
  files in the repo root — its own header says which it is and gives the
  regeneration command), and the mirror had drifted **3,061 lines** (2,599
  against 5,660), about thirty-five milestones. It was still calling the Rewo
  work branch "72 commits ahead of `origin/main` and unmerged, the largest
  non-code risk in the project", which has been false since 2026-07-27. Its
  header already warns about exactly this, having caught a 634-line drift in
  July. **Regenerate it whenever you edit this file**; it is not a document to
  hand-edit. Note the generator is a blind whole-file rename, so a sentence
  naming *both* files reads as nonsense on one side — refer to "the mirror".
- **`AGENT_LOOP_BRIEF.md` duplicated §0.0's gate list and status**, and both
  had rotted — 435 tests against a real 2161, `mobshot` 243/243 against
  246/246, and a "Current state" section still describing the M10–M18 arc as
  unpushed local work. Rather than reset the numbers, those two sections now
  **point at §0.0**, which is the only place they belong. Its test-server
  recipe was also wrong in a way that costs an hour: it said
  `nohup java … &`, and the server **stops on stdin EOF**, so a backgrounded
  shell kills it instantly.
- **`REWO_HEALTH_BAR_SPEC.md` said `crosshairPickEntity` "needs an entity
  raycast Rewo does not have"** and that the pick clause is fed a hard `false`.
  **M73 built that raycast** and the clause resolves from it. Corrected in
  place rather than rewritten, because the reasoning for suppressing was sound.
- **The mixed-CRLF file list had drifted in BOTH directions**, was measured to
  exactly four on 2026-08-07 — and the "second measurement the same day" this
  bullet used to report, which found *zero* mixed files and *every* `.rs`
  all-CRLF, **was produced by a broken detector and is the exact opposite of
  the truth.** `core.autocrlf` is false and there is no `.gitattributes`, so
  the working tree is what is stored — and reading every `.rs` under `crates/`
  as **bytes** on 2026-08-13 gives **378 all-LF, one all-CRLF
  (`rewo-gpu/src/cem.rs`), four mixed** (`rewo-app/src/mobshot_cmd.rs`,
  `rewo-gpu/src/vanilla_hier.rs`, `rewo-world/src/chunk.rs`,
  `rewo-world/src/light.rs`). The tree is overwhelmingly LF and the hazard has
  **not** inverted: it is still a scripted edit normalising one of those five.
  `REWO_PLAN.md` §0.0 gotcha 9 has carried the corrected form since M126 —
  including the `grep -c $'\r$'` failure that produced the wrong version — and
  is the one to read. **This bullet is left here rather than deleted because
  M149b was misled by it**: the corrected fact and the broken one lived in two
  files, a session read the nearer one, normalised three LF files, and turned a
  50-line change into a 3,256-line diff. **Re-measure with a byte count; never
  with a shell pattern containing a raw CR.**
- `README.md`'s "What's next" offered two items that had both shipped (merging
  the Rewo branch; an inventory model), and its gate count said fourteen where
  there are **33**.

**The pattern, for the third documented time:** a number with a test behind it
stays true (`REWO_PACKET_COVERAGE.md`'s table is machine-checked by a unit test
in `ids.rs` and was exact again), and the sentence next to it does not. The
generalisable fix is not to re-check prose more often — it is to **stop keeping
the same number in two places**, which is what the `AGENT_LOOP_BRIEF` sections
now do by pointing rather than restating.

---

## The chat arc — M108–M113 (2026-08-07)

Six milestones in one session, each merged `--no-ff`. Chat went from
`chat_log: Vec<String>` and eight truncated lines to a complete subsystem, and
the packet coverage went **114 / 0 / 27 → 116 / 0 / 25** with class C at 14.
`REWO_PLAN.md` §15 has the per-milestone detail; this is what a future session
should carry.

**M108 — the chat HUD.** `ChatComponent`, the wrap under it, the signature
cache, `delete_chat`, and the text render. **`ComponentRenderUtils.wrapComponents`
calls a DIFFERENT `splitLines` overload** from the one M85 transcribed: same
breaks, but a per-line `isWrapped` flag that is `!isNewLine` (a width wrap
indents its continuation, an explicit `\n` does not), and `"a\n"` yields TWO
lines. `forEachLine` emits **top-row-first**, and that order is load-bearing for
the tag-icon accumulator. **`delete_chat` is unreadable without a
`MessageSignatureCache`** — `Packed.read` is `readVarInt() - 1`, so the
signature is usually a cache index, and the cache is a move-to-front LRU that
dedupes rather than a ring. `system_chat`'s `overlay` bool was being read and
discarded; it routes to the **action bar**.

**M109 — the backdrop.** A colour channel on the HUD vertex, which first
exposed the `v.len() * 16` hardcode beside `VERTEX_STRIDE` — M21's shape,
latent until the vertex grew. **A witness caught a wrong draw order I had
justified with invented reasoning**: chat is a later stratum than the hotbar and
draws over it.

**M110 — `ChatScreen`.** `normalizeChatMessage` collapses *internal* whitespace;
`historyPos` starts one past the list and that slot is a **buffer, not an
entry**; `isDraftRestorable` is asymmetric; `shouldDiscardDraft` **keeps** the
draft on Esc; the wheel is clamped **before** it is multiplied.

**M111 — the scrollbar.** 1 px of colour plus 1 px of light grey, because the
second fill's x arguments are backwards and `fill` normalises. `HudFill` grew an
`rgb`, and it must be handed over in **linear** space — black is 0 in both and
hid that for two milestones.

**M112 — `isHovering`, and the bug under it.** Three handoffs had named the
narrow-window override as "one predicate with five consumers". **Four of those
consumers were not using the predicate at all**: `ScreenState::hovered`
converted through `Placement::centred`, so the click, the double-click, the drag
and the item-hover highlight all ignored the recipe book's 77 px displacement.
Third occurrence of M89's finding, first to reach an input path. There is now
one conversion and one visibility predicate, and a consumer has to ask.

**M113 — the Brigadier tree.** 2,017 nodes off a real server, **consumed
exactly**. An argument node's properties have no length prefix and only its own
type knows their size; 44 of 57 types are singletons and the other 13 are
transcribed. `time` has **no flags byte**; the numeric ranges are fixed
big-endian; the suggestion id is read **after** the properties. The registry
names are **namespaced**, and matching the bare name compiles and reads zero
bytes.

### Process, which generalises past this arc

* **Read a gate's EXIT CODE, never a substring.** M109 grepped for witness names,
  saw `ok` on every line, and missed that the gate was red on a declared-count
  assert. Then the consequence: **a mutation battery run against an
  already-failing command reads KILLED for every entry** — eight mutations across
  two batteries were vacuous and looked like 8/8. **Every battery now carries a
  no-op control** that must SURVIVE.
* **`cargo build` passing says nothing about whether the tests compile.** M110's
  signature change broke `rewo-app`'s test module while the build stayed green;
  the totalling loop counts `test result` lines, so that crate contributed 0 and
  read as silence. Read each crate's exit code.
* **Probe the port you are about to use.** M111's first run reported 27/28
  against a server that had crashed on `FAILED TO BIND`. **Most witnesses passed
  because they are injected** — only r25, which needs a real server, could tell.
  A gate whose witnesses are mostly self-driven can look healthy against nothing.
* **A mutation must be run against the check that covers it.** M111's sRGB
  mutation survived the pixel gate (which builds its input by hand) and died
  against the unit tests. A survivor is a question about the instrument as much
  as about the code.
* **Witnesses were wrong more often than the code.** Across the arc: roughly a
  dozen witness errors against three code errors, and the recurring shapes were
  a fixture sitting exactly where two candidate readings agree (an empty cache,
  a truncated body, the default line spacing) and a control that changes with
  its subject.
* **M97's lesson applied twice more** (`apply_chat_events`, `book_visible_for`),
  both found by a mutation surviving because the rule lived somewhere no test
  could reach.

---

## M141 — the ten tickable ramps, their velocity, and every ordinary trigger (2026-08-11)

`SoundEngine.tickInGameSound` drove **one `tick()` body out of ten** since
M131, because `EntityBoundSoundInstance` was the only subclass Rewo modelled.
`crates/rewo-net/src/tickable.rs` is the other nine, and the engine now drives
all of them. Detail in `REWO_PLAN.md` §15; three things belong here.

**The headline is a vanilla bug that punishes a careful reader.**
`MinecartSoundInstance.java:16` declares `private float pitch = 0.0F;` over
`AbstractSoundInstance`'s `protected float pitch = 1.0F;`, and **Java field
access is statically bound** — so `getPitch()`, declared in the superclass, is
the only reader and never sees the subclass field. `PITCH_MIN`, `PITCH_MAX` and
`PITCH_DELTA = 0.0025F` name a ramp that reaches nothing. Transcribing the class
in isolation gives every minecart ride a twenty-second pitch glissando vanilla
does not have. It is the only field shadow in the whole
`client/resources/sounds` package.

**Two more "the named constant is not the ceiling" cases, both from `Mth.lerp`
taking its factor first.** The bee's volume is
`lerp(clamp(speed, 0, 0.5), 0, 1.2)`, so the factor saturates at 0.5 and the
ceiling is **0.6** against the declared 1.2; the minecart's is 0.35 against 0.7.
And the bee's *pitch* clamps the factor to the **pitch range**, pinning an adult
bee at a constant 0.98 and a baby at 1.54 — never the bands the getters
describe. Meanwhile `RidingEntitySoundInstance` uses `Mth.clampedLerp`, which
clamps the factor to `0..1` and so is a different function. Two adjacent classes
mapping speed to volume, incompatibly.

**Two live fixes rode along.** The per-tick entity position was not narrowed
through f32 (the constructor was, and had a test) — with a comment that did not
merely omit the cast but *justified* omitting it, which is worse, because a
reader checking that line comes away reassured. Its witness could never have
caught it: the fixture moved the entity to three coordinates exactly
representable in f32. And `SoundWorld::entity_position` is gone in favour of
`RampWorld::position` — one name for one query, M89's finding, which has now
recurred at M90, M106b and M112.

**The batteries found more about instruments than about code**, which is the
pattern worth carrying: reading a mutation battery's **exit code cannot
distinguish a failing test from a failing build**, and this one's no-op control
came back KILLED because the previous run's binary still held the link output
(M138d's linker-1104 hazard). `tools/m141_mutate.py` reads the `test result:`
line and retries once; every earlier battery in `tools/` still has the hazard.
Two of my own witnesses were also wrong before any code was — one measuring a
bee's switch against an instance that had simply not been reclaimed yet, and one
asserting a direction vector from a remembered note rather than from the
expression, which on being worked out revealed that
**`Vec3.directionFromRotation` IS `Entity.calculateViewVector`** by another
route.

**M141e built the first trigger** — the elytra, which is now the one tickable
sound this client constructs. Its input, the local player's `fall_flying`, gates
the ramp at *both* ends (the survival guard `time <= 20 || isFallFlying()` and
the `onSyncedDataUpdated` rising edge), so one decode closed both. The decode
itself is **M73's asymmetry for the third time**: vanilla's local player is in
the level and its metadata is processed like anyone else's, but `EntityTable`
has no row for you, so the router dropped it.

Its finding is the sort that only a mutation surfaces: **`canPlaySound()` is a
per-class override that six of the ten declare and four decline**, so the
elytra — which does not declare it — must *not* be silence-gated on its player,
and Rewo's `Binding::Entity` had been meaning "follow" and "gate" at once.
`Ramp::silence_gated_entity()` is deliberately not `Ramp::entity()`.

The rising edge is also not "the flag changed": `assignValues` fires
`onSyncedDataUpdated` once per *entry in the packet* with **no** change guard,
and what makes the edge terminate is `wasFallFlying` being sampled once per
tick — which means two flag-carrying packets inside one tick each start a
sound, and vanilla has no dedup.

**M141f then took the bee and the minecart**, which are one vanilla method
(`postAddEntitySoundInstance`) with two arms — so implementing half an
`if/else if` would have been half a transcription. **Three of the ten ramps are
constructed now.**

Its finding is an index that needed counting twice. `Bee.DATA_ANGER_END_TIME` is
**19**, and the count that gets it wrong is `AgeableMob`'s: it declares **two**
accessors (`DATA_BABY_ID` *and* an `AGE_LOCKED` no earlier milestone noted), so
reading M20's "index 16 BOOLEAN is baby" as the whole of it puts this on
`Bee.DATA_FLAGS_ID`. The serializer catches that only by luck — one slot is a
BYTE and the other a LONG — and would not on a neighbour of the same type.

And anger is a **deadline, not a flag**: `endTime > 0 && endTime - gameTime > 0`,
whose second half changes every tick with no packet arriving. Storing a boolean
would freeze a bee's anger at whatever it was when the last metadata came in,
which is why the sound world grew a clock rather than the table growing a flag.

**M141g then took the guardian and the sniffer** (`handleEntityEvent` 21 and
63), so **five of the ten ramps are constructed**. The guardian's input is the
one among the ten that is **not a decode at all** — `clientSideAttackTime` is a
counter vanilla runs in `aiStep`'s client branch, and its rules read backwards
twice: it increments only while there *is* a target and never counts down, and
what zeroes it is **the metadata arriving, not the target going away** (there is
no change guard in `assignValues` — M141e's finding again).

**And it found a live decode bug on the way.** `(17, 35..=37)` claimed the
sniffer's, armadillo's and copper golem's state enums share an index. They do
not: `AgeableMob` declares **two** accessors, so the sniffer's and armadillo's
are at **18** while the copper golem's really is at 17 — which is presumably how
"their shared index" got written. On a sniffer, 17 is `AgeableMob.AGE_LOCKED`, a
BOOLEAN, so the state silently never arrived from a real server. **No gate could
see it**: the gesture rigs are driven by `REWO_FORCE_GESTURE` and
`mobshot --gesture`, which inject the state rather than decode it — and the unit
test encoded the bug rather than catching it.

The transferable half is the method, not the fix. An hour earlier I had
"found" that Rewo's serializer ids were off by one and the fault was my
counting, so the `extends` walk was run mechanically over several classes and
checked against two known-good readings (`SpellcasterIllager -> 17`, which is
live-verified, and `Bee -> 18`, which M141f had just shipped) **before** the
sniffer's answer was believed. **A counting method is an instrument; calibrate
it against a known reading before reporting what it finds.**

**M141h then closed the ordinary triggers with the riding pair**, so **seven of
the ten ramps are constructed**. Its finding is that the trigger does *not*
choose a sound: `LocalPlayer.startRiding`'s minecart arm plays **both**
instances at once — dry and underwater — and each mutes itself from the same
submersion input, so the crossfade belongs to the ramp. Picking one at mount
time is the natural implementation and is silent for half of every ride,
because diving does not re-fire `startRiding`. Two more, both invertible: the
loop is `Attenuation.NONE` (you are sitting on the thing), and it is
silence-gated on the **vehicle**, so a silenced cart silences its rider — the
same `canPlaySound()` trap M141e found, in the one place where binding it to
the obvious entity reads perfectly correctly.

It also has the clearest example of a limit on mutation testing: the "there are
two minecart instances" claim is pinned by a **destructure**
(`let [wet, dry] = RIDING_MINECART`), so shortening the array is a compile error
and the mutant never runs. **A battery can only grade claims that survive
compilation** — anything the types make unrepresentable is pinned harder and
shows up as BUILD-FAIL noise, so mutate the runtime claim underneath it instead.

**M142 then took the ambient handlers**, the subsystem that was left — and its
first finding is that **the class name is not the feature**:
`UnderwaterAmbientSoundHandler` plays the three rare sub-sounds and *nothing
else*, while the loop is minted by `LocalPlayer.updateIsUnderwater()`'s rising
edge alongside two positioned one-shots it never sees. Nor does any of that
handler's state do anything — `tickDelay` starts at 0, is decremented
unconditionally, and is only ever assigned 0, so it never gates; its four named
chance constants are declared and never read. **The three chances partition one
draw**, so the real rates are 0.0001 / 0.0009 / 0.009, not what the constants
are called, and **a spectator hears the additions and nothing else**, because
the early return that suppresses the loop lives in `updateIsUnderwater` while
the handler has no spectator gate at all.

The load-bearing one for anyone adding another: **a tickable ambient instance
must be constructed at volume 1.0**, because `SoundEngine.play` returns
`NOT_STARTED` for a zero-volume instance unless `canStartSilent()` — which none
of these classes overrides. Building a fading-in loop at 0.0 *because it is
about to fade in* makes it never play, with a debug log as the only trace. And
`relative` does **not** imply `Attenuation.NONE`; these three classes are
exactly what falsifies that pairing.

**The Overworld's cave sounds come from its DIMENSION TYPE**, not from any
biome: `DimensionTypes.java:43` sets `LEGACY_CAVE_SETTINGS` there, and no
vanilla Overworld or End biome sets the attribute at all. Drop that layer and
those dimensions go silent; hard-code it as a universal default and
`ambient.cave` plays in the Nether, which declares nothing. A biome
**replaces** the record rather than merging it, samples at the **raw quart**
with no fiddle (unlike M14's colour path), and does not interpolate.

Its battery came back **23/32 first time, and all eight survivors were real
gaps in my own witnesses** — including a partition test that *re-implemented
its subject* and so could not see a widened band, and a placement test whose
threshold the wrong answer already satisfied. Strictness needed an exact tie:
`<` versus `<=` differs only when a draw equals the chance (2⁻⁵³, and 4M seeds
produced none), so the witness reads the draw off a cloned RNG and uses it *as*
the chance.

**M142c then wired the bubble column**, whose scan has its own inversion:
"X varies fastest" means the **priority is the reverse** — `betweenClosed`
visits a whole Z slice before the next, so with `findFirst()` the winner is the
lowest Z, then Y, then X. My witness asserted it backwards, and **its Y case
passed for the wrong reason** because the block it expected to win on X was
also the lower one in Y. Two more: the property is serialised `drag` (not
`drag_down`) and the block's **default state is `drag=true`**, so a missed
lookup makes every column a whirlpool rather than silence; and a missing chunk
**empties the whole scan**, which the handler reads as "no column" and so
*re-arms* on, firing when the chunk arrives. Its table is graded from the real
bake by four `blockentityshot` witnesses, because every unit test supplies its
own — and the battery needed a **per-file runner**, since `assets::bake` is
unreachable from `cargo test` and a test-only harness would call both its
mutations SURVIVED. Its one survivor (39/40) is the milestone's only genuinely
**equivalent** mutant rather than a hole in a witness: every `bubble_column`
state declares `drag`, so the branch it defaults cannot be reached — pinned as
a coincidence, so a version that breaks it makes the branch live again.

**M142d then wired the biome loop, and all three handlers now reach a running
client.** It came last because it is the one that could not be expressed as
"play a sound": vanilla's handler **holds** its `LoopSoundInstance`s and calls
`fadeOut()`/`fadeIn()` on them, so Rewo's handler names the outcome and the
engine applies it to the live set — which *is* vanilla's map, filtered to
biome-loop ramps, so the reuse falls out rather than being arranged. Two rules
there read like bugs and are not: **every** loop fades out on a transition
including the incoming one (that `min(fade, 40)` is the only place a runaway
fade is capped, and the tick never bounds it upward), and **a live instance is
reused** rather than replaced, so crossing back inside ~41 ticks resumes the
same voice instead of restarting the sample. The transition keys on the
**sound**, not the biome, so two biomes sharing a loop cross silently. And one
snapshot feeds all three features while only the loop is change-gated —
otherwise standing still would stop every addition.

Its battery's two misses were both about the harness rather than the
transcription: an anchor that matched **twice** (so the mutation was skipped,
which is not the same as surviving — the count is reported for exactly this
reason), and a genuine gap whose consequence is worse than the mutation looks.
Dropping the **ramp-kind** guard from the reuse lookup lets an ordinary sound
that happens to share the bed's identifier stand in for it, at which point the
fade fails silently and **the bed never starts at all**. The **directional sound is a different feature** — the End flash from
`ClientLevel.tick`, needing `EndFlashState` — and one reader in M142's survey
claimed that class is dead in vanilla and advised deleting Rewo's ramp. It is
not.

**M141d fed them the velocity**, which was the input gating four of the ten,
and its finding is the sort that punishes a sensible implementation: a remote
entity's `getDeltaMovement()` is **a decaying echo of the last
`set_entity_motion` packet**, not a velocity. A client never integrates it into
a position, so a bee gliding steadily past with no motion packets has it falling
to zero while it is visibly moving — and its buzz fades with it. A finite
difference over the interpolated positions is more truthful about the bee and is
not what vanilla sounds like.

Nor is it one rule. The 0.98 decay is the **`else` of the interpolation
branch**, so an entity still catching up does not decay at all; it is skipped
for a vehicle the local player rides (authority is inherited from the
controlling passenger); the deadband that follows has **two forms** (a player's
joint `< 9.0E-6`, everything else's per-axis `< 0.003`, disagreeing at
`(0.0025, 0.0025)`); and **none of it runs for a minecart**, because `aiStep` is
`LivingEntity`'s and both minecart behaviours' client branches touch position
only. `EntityTableWorld`'s doc carries the full table of what it can and cannot
answer — and now says how to re-derive that count, having been wrong in both
directions.
