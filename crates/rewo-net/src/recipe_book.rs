//! The recipe book's four packets (M93y).
//!
//! `REWO_PACKET_COVERAGE.md` files these as **class C**, "needs a subsystem
//! Rewo lacks", and here that is **true** — unlike the four claims M91–M93u
//! overturned. The recipe book is a tabbed, searchable, filterable list with
//! ghost-recipe placement, and none of that exists.
//!
//! What is here is the half that has to come first, and M63's split is the
//! precedent: *decoding a packet needs no listening; making a noise does*.
//! Nothing consumes this yet, and the module says so rather than pretending
//! otherwise.
//!
//! # The recursion, and why it needs a bound
//!
//! `SlotDisplay` is **recursive** — `composite` holds a list of them,
//! `with_remainder` holds two, `dyed` holds two — so a hostile or corrupt
//! packet can nest arbitrarily. M41's `DataComponentPatch` rule applies with
//! the same teeth: the variants have **different body lengths**, so a reader
//! that loses its place does not merely mislabel, it desyncs and the rest of
//! the packet is garbage. The depth bound is charged only by the recursive
//! variants, which is M52e's correction to M41 — charging every combinator
//! made a legitimate `can_place_on` report `Stuck`.
//!
//! # The ids are resolved by NAME
//!
//! All three registries are `BuiltInRegistries` entries, so the server never
//! sends them; see [`rewo_data::recipe_display`], which also records why an
//! `enumerate()`-based table would desync every packet rather than mislabel.

use rewo_data::recipe_display::{RecipeDisplayIds, RecipeKind, SlotKind};
use rewo_proto::reader::PacketReader;

/// How deep a `SlotDisplay` may nest before the read is abandoned.
///
/// Only the recursive variants charge it (M52e): `composite`, `dyed`,
/// `with_remainder`, `with_any_potion`, `only_with_component` and
/// `smithing_trim`. A flat `item` at depth 0 costs nothing, so a wide recipe
/// is never mistaken for a deep one.
pub const MAX_DEPTH: u32 = 16;

/// One `SlotDisplay`.
///
/// The payloads Rewo cannot use yet are kept as ids rather than resolved:
/// nothing renders a recipe book, and an id is what a future renderer would
/// look up anyway.
#[derive(Debug, Clone, PartialEq)]
pub enum SlotDisplay {
    Empty,
    AnyFuel,
    /// The item's **raw** registry id — `Item.STREAM_CODEC` is
    /// `holderRegistry`, not `holder`'s `id + 1` (M93u's fifth sighting).
    Item(i32),
    Stack(crate::component_wire::ItemTemplate),
    /// A `TagKey<Item>`, which is a plain identifier string on the wire.
    Tag(String),
    WithAnyPotion(Box<SlotDisplay>),
    /// The source, and the component type id it is gated on.
    OnlyWithComponent(Box<SlotDisplay>, i32),
    Dyed {
        dye: Box<SlotDisplay>,
        target: Box<SlotDisplay>,
    },
    SmithingTrim {
        base: Box<SlotDisplay>,
        material: Box<SlotDisplay>,
        /// `Holder<TrimPattern>` — a **datapack** registry, so `holder`'s
        /// `id + 1` with 0 meaning an inline definition follows. Rewo does not
        /// decode the inline form and refuses the packet rather than guessing.
        pattern: i32,
    },
    WithRemainder {
        input: Box<SlotDisplay>,
        remainder: Box<SlotDisplay>,
    },
    Composite(Vec<SlotDisplay>),
}

/// One `RecipeDisplay`.
#[derive(Debug, Clone, PartialEq)]
pub enum RecipeDisplay {
    CraftingShapeless {
        ingredients: Vec<SlotDisplay>,
        result: SlotDisplay,
        station: SlotDisplay,
    },
    CraftingShaped {
        width: i32,
        height: i32,
        ingredients: Vec<SlotDisplay>,
        result: SlotDisplay,
        station: SlotDisplay,
    },
    Furnace {
        ingredient: SlotDisplay,
        fuel: SlotDisplay,
        result: SlotDisplay,
        station: SlotDisplay,
        duration: i32,
        experience: f32,
    },
    Stonecutter {
        input: SlotDisplay,
        result: SlotDisplay,
        station: SlotDisplay,
    },
    Smithing {
        template: SlotDisplay,
        base: SlotDisplay,
        addition: SlotDisplay,
        result: SlotDisplay,
        station: SlotDisplay,
    },
}

/// One `RecipeDisplayEntry`, plus `ClientboundRecipeBookAddPacket.Entry`'s flags.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// `RecipeDisplayId` — a bare var-int index, and the handle every other
    /// recipe-book packet uses.
    pub id: i32,
    pub display: RecipeDisplay,
    /// `OptionalInt group`, over `ByteBufCodecs.OPTIONAL_VAR_INT`:
    ///
    /// ```java
    /// VAR_INT.map(i -> i == 0 ? OptionalInt.empty() : OptionalInt.of(i - 1), …)
    /// ```
    ///
    /// The `+ 1` family again (M16/M21/M55/M92d/M93l/M93u), in its optional
    /// form: **0 means absent and group 0 is encoded as 1**. Reading it raw
    /// makes every group one too high and turns "no group" into group 0.
    pub group: Option<i32>,
    /// `RecipeBookCategory`'s raw registry id.
    pub category: i32,
    /// `craftingRequirements` — the ingredient slots, each a set of item ids
    /// or a tag name (M96).
    ///
    /// `None` means the field was absent, and vanilla's `canCraft` opens with
    /// `craftingRequirements.isEmpty() ? false` — so **a recipe with no
    /// requirements is never craftable**, rather than trivially craftable,
    /// which is the reading the shape invites.
    ///
    /// M93y walked these and discarded them; the solver they feed is M96.
    pub requirements: Option<Vec<IngredientSet>>,
    /// `FLAG_NOTIFICATION` (1) — the recipe pops a toast.
    pub notification: bool,
    /// `FLAG_HIGHLIGHT` (2) — the book's tab glows.
    pub highlight: bool,
}

/// `ClientboundRecipeBookAddPacket`.
/// One ingredient slot as the wire carries it (M96).
///
/// A `HolderSet<Item>`, which is **either** an inline id list **or** a tag
/// name — the same two-form encoding M41 records, and the reason resolving an
/// ingredient needs the tag data from `update_tags` rather than the packet
/// alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngredientSet {
    Ids(Vec<i32>),
    Tag(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BookAdd {
    pub entries: Vec<Entry>,
    /// **`replace` clears the book first.** A server sends it true on join and
    /// false for each recipe unlocked afterwards, so treating it as "always
    /// append" leaves a stale book across a respawn or a dimension change.
    pub replace: bool,
}

/// `RecipeBookSettings.TypeSettings`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TypeSettings {
    pub open: bool,
    pub filtering: bool,
}

/// `RecipeBookSettings` — four `TypeSettings` in a fixed order.
///
/// **Positional, not keyed**: the codec is a plain composite of four, so the
/// order `crafting, furnace, blastFurnace, smoker` is the wire contract and a
/// map keyed by book type would have to preserve it exactly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BookSettings {
    pub crafting: TypeSettings,
    pub furnace: TypeSettings,
    pub blast_furnace: TypeSettings,
    pub smoker: TypeSettings,
}

fn boolean(r: &mut PacketReader) -> Result<bool, ()> {
    Ok(r.u8().map_err(|_| ())? != 0)
}

fn slot(r: &mut PacketReader, ids: &RecipeDisplayIds, depth: u32) -> Result<SlotDisplay, ()> {
    if depth > MAX_DEPTH {
        return Err(());
    }
    let kind = ids.slot(r.varint().map_err(|_| ())?).ok_or(())?;
    // Only the recursive arms charge depth (M52e).
    let d = depth + 1;
    Ok(match kind {
        SlotKind::Empty => SlotDisplay::Empty,
        SlotKind::AnyFuel => SlotDisplay::AnyFuel,
        SlotKind::Item => SlotDisplay::Item(r.varint().map_err(|_| ())?),
        SlotKind::ItemStack => SlotDisplay::Stack(
            crate::component_wire::read_item_template(r, depth)?.ok_or(())?,
        ),
        SlotKind::Tag => SlotDisplay::Tag(r.string(32767).map_err(|_| ())?),
        SlotKind::WithAnyPotion => SlotDisplay::WithAnyPotion(Box::new(slot(r, ids, d)?)),
        SlotKind::OnlyWithComponent => {
            let source = Box::new(slot(r, ids, d)?);
            SlotDisplay::OnlyWithComponent(source, r.varint().map_err(|_| ())?)
        }
        SlotKind::Dyed => SlotDisplay::Dyed {
            dye: Box::new(slot(r, ids, d)?),
            target: Box::new(slot(r, ids, d)?),
        },
        SlotKind::SmithingTrim => SlotDisplay::SmithingTrim {
            base: Box::new(slot(r, ids, d)?),
            material: Box::new(slot(r, ids, d)?),
            pattern: {
                // `Holder<TrimPattern>` — `id + 1`, 0 meaning an inline
                // definition. Rewo has no trim-pattern codec, and an inline one
                // has no length, so this refuses rather than desyncing.
                let h = r.varint().map_err(|_| ())?;
                if h == 0 {
                    return Err(());
                }
                h - 1
            },
        },
        SlotKind::WithRemainder => SlotDisplay::WithRemainder {
            input: Box::new(slot(r, ids, d)?),
            remainder: Box::new(slot(r, ids, d)?),
        },
        SlotKind::Composite => {
            let n = r.varint().map_err(|_| ())?;
            if !(0..=1024).contains(&n) {
                return Err(());
            }
            let mut v = Vec::with_capacity(n as usize);
            for _ in 0..n {
                v.push(slot(r, ids, d)?);
            }
            SlotDisplay::Composite(v)
        }
    })
}

fn slots(r: &mut PacketReader, ids: &RecipeDisplayIds, depth: u32) -> Result<Vec<SlotDisplay>, ()> {
    let n = r.varint().map_err(|_| ())?;
    if !(0..=1024).contains(&n) {
        return Err(());
    }
    let mut v = Vec::with_capacity(n as usize);
    for _ in 0..n {
        v.push(slot(r, ids, depth)?);
    }
    Ok(v)
}

fn display(r: &mut PacketReader, ids: &RecipeDisplayIds) -> Result<RecipeDisplay, ()> {
    let kind = ids.recipe(r.varint().map_err(|_| ())?).ok_or(())?;
    Ok(match kind {
        RecipeKind::CraftingShapeless => RecipeDisplay::CraftingShapeless {
            ingredients: slots(r, ids, 0)?,
            result: slot(r, ids, 0)?,
            station: slot(r, ids, 0)?,
        },
        RecipeKind::CraftingShaped => RecipeDisplay::CraftingShaped {
            // Width and height come FIRST, before the ingredients they
            // describe — so a reader that takes the list first is off by two
            // var-ints for the whole rest of the packet.
            width: r.varint().map_err(|_| ())?,
            height: r.varint().map_err(|_| ())?,
            ingredients: slots(r, ids, 0)?,
            result: slot(r, ids, 0)?,
            station: slot(r, ids, 0)?,
        },
        RecipeKind::Furnace => RecipeDisplay::Furnace {
            ingredient: slot(r, ids, 0)?,
            fuel: slot(r, ids, 0)?,
            result: slot(r, ids, 0)?,
            station: slot(r, ids, 0)?,
            // A var-int and then a FLOAT, the only non-integer in the tree.
            duration: r.varint().map_err(|_| ())?,
            experience: r.f32().map_err(|_| ())?,
        },
        RecipeKind::Stonecutter => RecipeDisplay::Stonecutter {
            input: slot(r, ids, 0)?,
            result: slot(r, ids, 0)?,
            station: slot(r, ids, 0)?,
        },
        RecipeKind::Smithing => RecipeDisplay::Smithing {
            template: slot(r, ids, 0)?,
            base: slot(r, ids, 0)?,
            addition: slot(r, ids, 0)?,
            result: slot(r, ids, 0)?,
            station: slot(r, ids, 0)?,
        },
    })
}

/// `RecipeDisplayEntry`, then `ClientboundRecipeBookAddPacket.Entry`'s flags.
fn entry(r: &mut PacketReader, ids: &RecipeDisplayIds) -> Result<Entry, ()> {
    let id = r.varint().map_err(|_| ())?;
    let display = display(r, ids)?;
    let group = match r.varint().map_err(|_| ())? {
        0 => None,
        n => Some(n - 1),
    };
    let category = r.varint().map_err(|_| ())?;
    let requirements = if boolean(r)? {
        // `Ingredient.CONTENTS_STREAM_CODEC.list()` — a list of HolderSets.
        let n = r.varint().map_err(|_| ())?;
        if !(0..=1024).contains(&n) {
            return Err(());
        }
        let mut out = Vec::with_capacity(n.min(64) as usize);
        for _ in 0..n {
            // `holderSet`: the var-int is `count + 1`, and a literal **0**
            // means a TAG NAME follows rather than an empty set (M41).
            out.push(match r.varint().map_err(|_| ())? {
                0 => IngredientSet::Tag(r.string(32767).map_err(|_| ())?),
                c => {
                    let mut ids = Vec::with_capacity((c - 1).min(64) as usize);
                    for _ in 0..(c - 1) {
                        ids.push(r.varint().map_err(|_| ())?);
                    }
                    IngredientSet::Ids(ids)
                }
            });
        }
        Some(out)
    } else {
        None
    };
    let flags = r.u8().map_err(|_| ())?;
    Ok(Entry {
        id,
        display,
        group,
        category,
        requirements,
        notification: flags & 1 != 0,
        highlight: flags & 2 != 0,
    })
}

/// `ClientboundRecipeBookAddPacket`.
pub fn parse_add(body: &[u8], ids: &RecipeDisplayIds) -> Result<BookAdd, String> {
    let mut r = PacketReader::new(body);
    let n = r
        .varint()
        .map_err(|e| format!("recipe_book_add: {e:?}"))?;
    if !(0..=4096).contains(&n) {
        return Err(format!("recipe_book_add: {n} entries"));
    }
    let mut entries = Vec::with_capacity(n as usize);
    for i in 0..n {
        entries.push(entry(&mut r, ids).map_err(|_| format!("recipe_book_add: entry {i}"))?);
    }
    Ok(BookAdd {
        entries,
        replace: boolean(&mut r).map_err(|_| "recipe_book_add: replace".to_string())?,
    })
}

/// `ClientboundRecipeBookRemovePacket` — a bare list of ids.
pub fn parse_remove(body: &[u8]) -> Result<Vec<i32>, String> {
    let mut r = PacketReader::new(body);
    let n = r
        .varint()
        .map_err(|e| format!("recipe_book_remove: {e:?}"))?;
    if !(0..=4096).contains(&n) {
        return Err(format!("recipe_book_remove: {n} ids"));
    }
    (0..n)
        .map(|i| {
            r.varint()
                .map_err(|_| format!("recipe_book_remove: id {i}"))
        })
        .collect()
}

/// `ClientboundRecipeBookSettingsPacket` — eight booleans, in pairs.
pub fn parse_settings(body: &[u8]) -> Result<BookSettings, String> {
    let mut r = PacketReader::new(body);
    let mut pair = || -> Result<TypeSettings, ()> {
        Ok(TypeSettings {
            open: boolean(&mut r)?,
            filtering: boolean(&mut r)?,
        })
    };
    let e = || "recipe_book_settings: short body".to_string();
    Ok(BookSettings {
        crafting: pair().map_err(|_| e())?,
        furnace: pair().map_err(|_| e())?,
        blast_furnace: pair().map_err(|_| e())?,
        smoker: pair().map_err(|_| e())?,
    })
}

impl SlotDisplay {
    /// `resolveForStacks` — the item ids this display stands for, in order
    /// (M95).
    ///
    /// Vanilla resolves through a `ContextMap`, which carries the fuel table,
    /// the item tags and the enchantment/potion registries. Rewo resolves the
    /// arms that need **no context** and yields nothing for the rest, so an
    /// unresolvable display draws an empty slot rather than a wrong item:
    ///
    /// * `Item` and `Stack` are the two a recipe RESULT is in practice.
    /// * `Composite` flat-maps, which is how "any of these" is expressed.
    /// * `WithRemainder` resolves its **input**; the remainder only decorates
    ///   the stack for the ingredient display, and a result never has one.
    /// * `Empty` is nothing, which is not a failure.
    ///
    /// `AnyFuel`, `Tag`, `Dyed`, `SmithingTrim`, `WithAnyPotion` and
    /// `OnlyWithComponent` all need the context and yield nothing. That is
    /// visible — an ingredient slot that would show a rotating set of tag
    /// members shows none — and it is the honest state, because the alternative
    /// is picking an arbitrary member and calling it the recipe.
    pub fn resolve_items(&self, out: &mut Vec<i32>) {
        match self {
            SlotDisplay::Item(id) => out.push(*id),
            SlotDisplay::Stack(t) => out.push(t.item_id),
            SlotDisplay::Composite(v) => {
                for d in v {
                    d.resolve_items(out);
                }
            }
            SlotDisplay::WithRemainder { input, .. } => input.resolve_items(out),
            SlotDisplay::Empty
            | SlotDisplay::AnyFuel
            | SlotDisplay::Tag(_)
            | SlotDisplay::Dyed { .. }
            | SlotDisplay::SmithingTrim { .. }
            | SlotDisplay::WithAnyPotion(_)
            | SlotDisplay::OnlyWithComponent(..) => {}
        }
    }

    pub fn items(&self) -> Vec<i32> {
        let mut v = Vec::new();
        self.resolve_items(&mut v);
        v
    }
}

impl IngredientSet {
    /// The item ids this slot accepts (M96).
    ///
    /// A tag is looked up in the server's own `update_tags` payload
    /// (`minecraft:item`), which M69 decodes and nothing consumed until now.
    /// **An unknown tag yields nothing**, which makes the ingredient
    /// unsatisfiable and the recipe uncraftable — the safe direction: greying a
    /// recipe you could make is a smaller lie than lighting one you cannot.
    pub fn resolve(&self, tags: &crate::tags::TagOverrides) -> Vec<i32> {
        match self {
            IngredientSet::Ids(v) => v.clone(),
            IngredientSet::Tag(name) => tags
                .tag("minecraft:item", name)
                .map(|v| v.to_vec())
                .unwrap_or_default(),
        }
    }
}

impl Entry {
    /// `RecipeDisplayEntry.canCraft`'s ingredient list, resolved.
    ///
    /// `None` when `craftingRequirements` was absent — which vanilla turns
    /// into **not craftable**, not "no requirements to meet".
    pub fn ingredients(
        &self,
        tags: &crate::tags::TagOverrides,
    ) -> Option<Vec<rewo_world::stacked_contents::Ingredient>> {
        Some(
            self.requirements
                .as_ref()?
                .iter()
                .map(|i| rewo_world::stacked_contents::Ingredient {
                    accepts: i.resolve(tags),
                })
                .collect(),
        )
    }
}

impl RecipeDisplay {
    /// The display's **result** — the item a recipe button shows.
    pub fn result(&self) -> &SlotDisplay {
        match self {
            RecipeDisplay::CraftingShapeless { result, .. }
            | RecipeDisplay::CraftingShaped { result, .. }
            | RecipeDisplay::Furnace { result, .. }
            | RecipeDisplay::Stonecutter { result, .. }
            | RecipeDisplay::Smithing { result, .. } => result,
        }
    }
}

/// `ContainerSelectTime` — `Mth.floor(time / 30.0F)`, with `time` advanced by
/// the partial tick each render (M95).
pub const TICKS_TO_SWAP_SLOT: f32 = 30.0;

/// `RecipeButton.getDisplayStack` — which of a collection's items is on show.
///
/// A **two-level** cycle, and the second level is easy to miss:
/// `offsetIndex = currentIndex / entryCount` selects *within* an entry's own
/// display items while `entryIndex = currentIndex % entryCount` selects the
/// entry. So a collection of three recipes whose results each have two forms
/// cycles through six over 3 minutes, not three.
///
/// Returns `None` for an entry with no resolvable items, which is
/// `ItemStack.EMPTY` — vanilla draws nothing rather than skipping to the next.
pub fn display_item(entries: &[Vec<i32>], current_index: i32) -> Option<i32> {
    if entries.is_empty() {
        return None;
    }
    let n = entries.len() as i32;
    let offset_index = current_index.div_euclid(n);
    let entry_index = current_index - n * offset_index;
    let items = entries.get(entry_index as usize)?;
    if items.is_empty() {
        return None;
    }
    items
        .get((offset_index % items.len() as i32) as usize)
        .copied()
}

/// `ClientboundPlaceGhostRecipePacket` — a container id and one display.
pub fn parse_place_ghost(
    body: &[u8],
    ids: &RecipeDisplayIds,
) -> Result<(i32, RecipeDisplay), String> {
    let mut r = PacketReader::new(body);
    let container = r
        .varint()
        .map_err(|e| format!("place_ghost_recipe: {e:?}"))?;
    let d = display(&mut r, ids).map_err(|_| "place_ghost_recipe: display".to_string())?;
    Ok((container, d))
}

/// `ServerboundRecipeBookSeenRecipePacket` — one var-int.
///
/// Written by the caller through `PacketWriter`; there is nothing to compose
/// here beyond the id, so this exists to name the packet rather than to build
/// it. Nothing sends it yet — see the module docs.
pub const SEEN_RECIPE_IS_ONE_VARINT: () = ();

#[cfg(test)]
mod tests {

    /// An ingredient is either an inline id list or a TAG NAME, and a tag
    /// resolves against the server's own `update_tags` payload.
    #[test]
    fn an_ingredient_resolves_ids_directly_and_a_tag_through_update_tags() {
        let mut tags = crate::tags::TagOverrides::default();
        tags.apply(&crate::tags::TagUpdate {
            registries: vec![crate::tags::RegistryTags {
                registry: "minecraft:item".into(),
                tags: vec![("minecraft:planks".into(), vec![10, 11, 12])],
            }],
        });
        assert_eq!(IngredientSet::Ids(vec![1, 2]).resolve(&tags), vec![1, 2]);
        assert_eq!(
            IngredientSet::Tag("minecraft:planks".into()).resolve(&tags),
            vec![10, 11, 12]
        );
        // An UNKNOWN tag yields nothing, which makes its ingredient
        // unsatisfiable — greying a recipe you could make is a smaller lie
        // than lighting one you cannot.
        assert!(IngredientSet::Tag("minecraft:nope".into())
            .resolve(&tags)
            .is_empty());
        // ...and so is a tag looked up before the server has said anything.
        assert!(IngredientSet::Tag("minecraft:planks".into())
            .resolve(&crate::tags::TagOverrides::default())
            .is_empty());
    }

    /// `canCraft` opens with `craftingRequirements.isEmpty() ? false`, so an
    /// entry that carried none is **never** craftable — not trivially
    /// craftable, which is what an empty ingredient list would otherwise mean
    /// to the solver.
    #[test]
    fn an_entry_with_no_requirements_yields_no_ingredients_at_all() {
        let tags = crate::tags::TagOverrides::default();
        let mut e = Entry {
            id: 1,
            display: RecipeDisplay::Stonecutter {
                input: SlotDisplay::Empty,
                result: SlotDisplay::Item(1),
                station: SlotDisplay::Empty,
            },
            group: None,
            category: 0,
            requirements: None,
            notification: false,
            highlight: false,
        };
        assert!(e.ingredients(&tags).is_none(), "absent, not empty");
        // An explicitly EMPTY list is a different thing and does resolve —
        // the solver then finds nothing to satisfy and answers true, which is
        // vanilla's behaviour for a zero-ingredient recipe that declared one.
        e.requirements = Some(Vec::new());
        assert_eq!(e.ingredients(&tags).map(|v| v.len()), Some(0));
    }

    /// The arms that need no context resolve; the six that do yield NOTHING,
    /// so an unresolvable display draws an empty slot rather than a wrong item.
    #[test]
    fn a_display_resolves_only_what_it_can_without_a_context() {
        assert_eq!(SlotDisplay::Item(7).items(), vec![7]);
        assert_eq!(SlotDisplay::Empty.items(), Vec::<i32>::new());
        assert_eq!(
            SlotDisplay::Composite(vec![
                SlotDisplay::Item(1),
                SlotDisplay::Item(2),
                SlotDisplay::Empty,
            ])
            .items(),
            vec![1, 2],
            "flat-mapped, in order"
        );
        // `WithRemainder` resolves its INPUT — the remainder decorates an
        // ingredient and is never part of a result.
        assert_eq!(
            SlotDisplay::WithRemainder {
                input: Box::new(SlotDisplay::Item(3)),
                remainder: Box::new(SlotDisplay::Item(99)),
            }
            .items(),
            vec![3],
        );
        // Each context-needing arm yields nothing rather than guessing.
        assert_eq!(SlotDisplay::AnyFuel.items(), Vec::<i32>::new());
        assert_eq!(SlotDisplay::Tag("c:ingots".into()).items(), Vec::<i32>::new());
        assert_eq!(
            SlotDisplay::WithAnyPotion(Box::new(SlotDisplay::Item(4))).items(),
            Vec::<i32>::new(),
            "the base is NOT yielded — a potion display is not its bottle"
        );
    }

    /// The cycle is TWO levels: the modulo picks the entry and the DIVISION
    /// picks which of that entry's own items. Reading only the modulo makes a
    /// collection of three recipes with two forms each cycle through three.
    #[test]
    fn the_display_cycle_walks_entries_and_their_items_independently() {
        let entries = vec![vec![10, 11], vec![20], vec![30, 31]];
        // Index 0..2 walks the entries at offset 0.
        assert_eq!(display_item(&entries, 0), Some(10));
        assert_eq!(display_item(&entries, 1), Some(20));
        assert_eq!(display_item(&entries, 2), Some(30));
        // Index 3..5 walks them again at offset 1 — and entry 1, which has one
        // item, repeats it rather than running off the end.
        assert_eq!(display_item(&entries, 3), Some(11));
        assert_eq!(display_item(&entries, 4), Some(20));
        assert_eq!(display_item(&entries, 5), Some(31));
        // Six distinct steps before it repeats, not three.
        assert_eq!(display_item(&entries, 6), Some(10));
    }

    #[test]
    fn an_entry_with_no_resolvable_items_shows_nothing() {
        // `ItemStack.EMPTY` — vanilla draws nothing rather than skipping to
        // the next entry, so the slot really is blank for its turn.
        let entries = vec![vec![1], vec![], vec![3]];
        assert_eq!(display_item(&entries, 1), None);
        assert_eq!(display_item(&entries, 0), Some(1));
        assert_eq!(display_item(&Vec::new(), 0), None);
    }

    #[test]
    fn the_swap_period_is_thirty_ticks() {
        assert_eq!(TICKS_TO_SWAP_SLOT, 30.0);
    }
    use super::*;

    fn ids() -> Option<RecipeDisplayIds> {
        let paths = rewo_data::DataPaths::for_version("26.2")?;
        RecipeDisplayIds::load(&paths.registries_json()).ok()
    }

    /// A stonecutter display: input, result, station — three slot displays.
    fn stonecutter_body() -> Vec<u8> {
        let mut b = Vec::new();
        b.push(3); // recipe_display: stonecutter
        b.push(4); // slot_display: item
        b.extend_from_slice(&[0x88, 0x02]); // item 264
        b.push(4);
        b.push(9); // item 9
        b.push(0); // slot_display: empty (the station)
        b
    }

    #[test]
    fn a_display_reads_its_variant_by_registry_id() {
        let Some(i) = ids() else { return };
        let body = stonecutter_body();
        let mut r = PacketReader::new(&body);
        let d = display(&mut r, &i).expect("display");
        assert_eq!(
            d,
            RecipeDisplay::Stonecutter {
                input: SlotDisplay::Item(264),
                result: SlotDisplay::Item(9),
                station: SlotDisplay::Empty,
            }
        );
        assert_eq!(r.remaining(), 0, "the body is consumed exactly");
    }

    #[test]
    fn a_shaped_recipes_dimensions_come_BEFORE_its_ingredients() {
        let Some(i) = ids() else { return };
        let mut b = vec![1u8]; // crafting_shaped
        b.push(2); // width
        b.push(3); // height
        b.push(1); // one ingredient
        b.push(0); // empty
        b.push(0); // result: empty
        b.push(0); // station: empty
        let mut r = PacketReader::new(&b);
        let d = display(&mut r, &i).expect("display");
        match d {
            RecipeDisplay::CraftingShaped { width, height, .. } => {
                assert_eq!((width, height), (2, 3));
            }
            other => panic!("{other:?}"),
        }
        // Taking the list first would consume the width as a count and leave
        // the reader two var-ints out for the rest of the packet.
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn the_group_is_optional_var_int_so_zero_means_ABSENT() {
        let Some(i) = ids() else { return };
        let mk = |g: u8| {
            let mut b = stonecutter_body();
            b.insert(0, 7); // the entry's own id
            b.push(g); // group
            b.push(0); // category
            b.push(0); // no crafting requirements
            b.push(3); // both flags
            b
        };
        let e = |g: u8| entry(&mut PacketReader::new(&mk(g)), &i).expect("entry");
        // 0 is absent, and group 0 rides the wire as 1.
        assert_eq!(e(0).group, None);
        assert_eq!(e(1).group, Some(0));
        assert_eq!(e(5).group, Some(4));
        // Reading it raw would make "no group" into group 0 and shift the rest.
        assert_ne!(e(0).group, Some(0));
    }

    #[test]
    fn the_two_flag_bits_are_independent() {
        let Some(i) = ids() else { return };
        let mk = |f: u8| {
            let mut b = stonecutter_body();
            b.insert(0, 7);
            b.extend_from_slice(&[0, 0, 0, f]);
            b
        };
        let e = |f: u8| entry(&mut PacketReader::new(&mk(f)), &i).expect("entry");
        assert_eq!((e(0).notification, e(0).highlight), (false, false));
        assert_eq!((e(1).notification, e(1).highlight), (true, false));
        assert_eq!((e(2).notification, e(2).highlight), (false, true));
        assert_eq!((e(3).notification, e(3).highlight), (true, true));
    }

    #[test]
    fn a_composite_nests_and_a_runaway_one_is_refused() {
        let Some(i) = ids() else { return };
        // composite[ item(1), composite[ empty ] ]
        let b = vec![10, 2, 4, 1, 10, 1, 0];
        let d = slot(&mut PacketReader::new(&b), &i, 0).expect("slot");
        assert_eq!(
            d,
            SlotDisplay::Composite(vec![
                SlotDisplay::Item(1),
                SlotDisplay::Composite(vec![SlotDisplay::Empty]),
            ])
        );
        // A chain of `with_any_potion` deeper than the bound is abandoned
        // rather than recursed. The variants have DIFFERENT body lengths, so a
        // reader that loses its place desyncs the rest of the packet.
        let deep: Vec<u8> = std::iter::repeat(2u8)
            .take(MAX_DEPTH as usize + 2)
            .chain([0])
            .collect();
        assert!(slot(&mut PacketReader::new(&deep), &i, 0).is_err());
        // …and one just inside it is fine, so the bound is not merely "any
        // nesting fails".
        let ok: Vec<u8> = std::iter::repeat(2u8)
            .take(MAX_DEPTH as usize - 1)
            .chain([0])
            .collect();
        assert!(slot(&mut PacketReader::new(&ok), &i, 0).is_ok());
    }

    #[test]
    fn an_unknown_variant_is_refused_rather_than_skipped() {
        let Some(i) = ids() else { return };
        // Variant 99 does not exist, and its body has no length, so there is
        // nothing to skip — M41's rule.
        assert!(slot(&mut PacketReader::new(&[99]), &i, 0).is_err());
        assert!(display(&mut PacketReader::new(&[99]), &i).is_err());
    }

    #[test]
    fn settings_are_four_pairs_in_a_fixed_order() {
        let s = parse_settings(&[1, 0, 0, 1, 1, 1, 0, 0]).expect("settings");
        assert_eq!(
            s,
            BookSettings {
                crafting: TypeSettings { open: true, filtering: false },
                furnace: TypeSettings { open: false, filtering: true },
                blast_furnace: TypeSettings { open: true, filtering: true },
                smoker: TypeSettings { open: false, filtering: false },
            }
        );
        // A short body is an error rather than a partial read.
        assert!(parse_settings(&[1, 0, 0]).is_err());
    }

    #[test]
    fn replace_is_read_AFTER_the_entries() {
        let Some(i) = ids() else { return };
        let mut b = vec![1u8]; // one entry
        let mut e = stonecutter_body();
        e.insert(0, 7);
        e.extend_from_slice(&[0, 0, 0, 0]);
        b.extend_from_slice(&e);
        b.push(1); // replace
        let a = parse_add(&b, &i).expect("add");
        assert_eq!(a.entries.len(), 1);
        assert!(a.replace, "a join sends true; an unlock sends false");
    }

    #[test]
    fn remove_is_a_bare_list_of_ids() {
        assert_eq!(parse_remove(&[3, 1, 2, 3]).expect("remove"), vec![1, 2, 3]);
        assert_eq!(parse_remove(&[0]).expect("empty"), Vec::<i32>::new());
        assert!(parse_remove(&[3, 1]).is_err(), "short");
    }
}
