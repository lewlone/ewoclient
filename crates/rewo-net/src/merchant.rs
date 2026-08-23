//! `merchant_offers` — the villager trade list (M93u).
//!
//! `REWO_PACKET_COVERAGE.md` filed this as **class C**, "needs a subsystem Rewo
//! lacks". It does not, and this is the **fourth** such claim this arc not to
//! survive the decompile (M91's furnace recipes, M93's merchant quick-move,
//! M93s's stonecutter list). The reason differs from the others, and is worth
//! separating: the stonecutter's data turned out to be in the jar, whereas a
//! villager's trades genuinely are per-entity and server-rolled — they really
//! do have to come off the wire. What was wrong was that the packet needed
//! anything Rewo had not already built.
//!
//! It needs three things and Rewo has all three: `ItemStack` (M34/M41), a
//! `TypedDataComponent` walker (M52e built one for `can_place_on`, which
//! reaches the same `DataComponentExactPredicate` this does), and the item
//! registry.
//!
//! # Traps in the body
//!
//! **The wire order is `costA, result, costB`** — the result sits *between* the
//! two costs, while every constructor and accessor lists them
//! `costA, costB, result`. `createFromStream` even reorders them when it calls
//! the constructor. Reading them in the natural order decodes without erroring
//! for most offers, because a cost and a stack are both item-shaped, and
//! silently swaps the item being sold with the second thing you pay.
//!
//! **The numerics are `writeInt`** — fixed big-endian i32 in a protocol that is
//! var-int nearly everywhere — and `priceMultiplier` is a `writeFloat`. Same
//! shape as M34's `container_set_slot` i16 and M47's `DyedItemColor`.
//!
//! **`Item.STREAM_CODEC` is `ByteBufCodecs.holderRegistry`** — a **raw 0-based**
//! id, not `holder`'s `id + 1`. The fifth time this trap appears (M16, M21,
//! M55, M92d, M93l) and among the quietest: off by one, every trade's cost is
//! an adjacent item, with no error anywhere.

use crate::component_wire::Shape;
use rewo_data::components::DataComponentIds;
use rewo_proto::reader::PacketReader;

/// `ItemCost` — an item, a count, and an exact-component predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemCost {
    /// The item's **raw** registry id (`holderRegistry`).
    pub item_id: i32,
    pub count: i32,
    /// Whether the `DataComponentExactPredicate` named anything.
    ///
    /// Its *contents* are walked and discarded: a cost that demands a specific
    /// enchantment is drawn as the plain item either way, because the icon
    /// comes from the item and the predicate only narrows what the player may
    /// hand over. Recorded as a bit so a future tooltip can say "not any old
    /// one" rather than silently claiming the plain item will do.
    pub constrained: bool,
}

/// One `MerchantOffer`.
#[derive(Debug, Clone, PartialEq)]
pub struct MerchantOffer {
    pub cost_a: ItemCost,
    /// The thing being sold. **Read between the two costs**, not after them.
    pub result: crate::item_stack::WireSlot,
    pub cost_b: Option<ItemCost>,
    /// `isOutOfStock()` — the offer is spent until the villager restocks.
    pub out_of_stock: bool,
    pub uses: i32,
    pub max_uses: i32,
    pub xp: i32,
    /// The discount (negative) or surcharge the villager is applying right now.
    pub special_price_diff: i32,
    pub price_multiplier: f32,
    pub demand: i32,
}

impl MerchantOffer {
    /// `getModifiedCostCount` — what cost A actually costs.
    ///
    /// ```java
    /// int basePrice = cost.count();
    /// int demandDiff = Math.max(0, Mth.floor(basePrice * this.demand * this.priceMultiplier));
    /// return Mth.clamp(basePrice + demandDiff + this.specialPriceDiff, 1, cost.itemStack().getMaxStackSize());
    /// ```
    ///
    /// Two asymmetries. **`demandDiff` clamps at 0 from below** so negative
    /// demand cannot make a trade cheaper — but `specialPriceDiff` is added
    /// *after* that clamp and is not floored, which is exactly how a hero-of-
    /// the-village discount reduces a price. And the final clamp's ceiling is
    /// the **item's own max stack size**, so a price can never exceed one
    /// stack; that needs the item's prototype, not the wire.
    pub fn modified_cost_a(&self, max_stack_size: i32) -> i32 {
        let base = self.cost_a.count;
        let demand_diff = (base as f32 * self.demand as f32 * self.price_multiplier)
            .floor()
            .max(0.0) as i32;
        (base + demand_diff + self.special_price_diff).clamp(1, max_stack_size)
    }

    /// **Cost B is never modified** — `getCostB()` returns the stack as sent.
    /// Only A carries the demand and the special price.
    pub fn cost_b_count(&self) -> i32 {
        self.cost_b.as_ref().map_or(0, |c| c.count)
    }

    /// Whether the screen strikes through the base price — vanilla shows the
    /// original beside the adjusted one only when they differ.
    pub fn discounted(&self, max_stack_size: i32) -> bool {
        self.modified_cost_a(max_stack_size) != self.cost_a.count
    }
}

/// The whole packet.
#[derive(Debug, Clone, PartialEq)]
pub struct MerchantOffers {
    pub container_id: i32,
    pub offers: Vec<MerchantOffer>,
    pub villager_level: i32,
    pub villager_xp: i32,
    /// `showProgress` — false for a wandering trader, which has no level.
    pub show_progress: bool,
    pub can_restock: bool,
}

/// `DataComponentExactPredicate.STREAM_CODEC` — `TypedDataComponent.list()`.
const EXACT_PREDICATE: Shape = Shape::List(&Shape::TypedComponent);

fn read_cost(r: &mut PacketReader) -> Result<ItemCost, ()> {
    // `Item.STREAM_CODEC` is `holderRegistry` — RAW, no `+ 1`.
    let item_id = r.varint().map_err(|_| ())?;
    let count = r.varint().map_err(|_| ())?;
    let before = r.remaining();
    if !crate::component_wire::walk(r, &EXACT_PREDICATE, 0)? {
        return Err(());
    }
    // A predicate that consumed only its own length prefix named nothing.
    let constrained = before - r.remaining() > 1;
    Ok(ItemCost {
        item_id,
        count,
        constrained,
    })
}

/// Decode `ClientboundMerchantOffersPacket`.
pub fn parse(body: &[u8], ids: DataComponentIds) -> Result<MerchantOffers, String> {
    let mut r = PacketReader::new(body);
    let container_id = r.varint().map_err(|e| format!("merchant_offers: {e:?}"))?;
    let n = r.varint().map_err(|e| format!("merchant_offers: {e:?}"))?;
    if !(0..=1024).contains(&n) {
        return Err(format!("merchant_offers: {n} offers"));
    }
    let mut offers = Vec::with_capacity(n as usize);
    for i in 0..n {
        let e = |what: &str| format!("merchant_offers: offer {i}: {what}");
        let cost_a = read_cost(&mut r).map_err(|_| e("cost A"))?;
        // The RESULT, between the two costs.
        let result = crate::item_stack::read_optional(&mut r, ids).map_err(|_| e("result"))?;
        let cost_b = match r.u8().map_err(|_| e("cost B flag"))? {
            0 => None,
            _ => Some(read_cost(&mut r).map_err(|_| e("cost B"))?),
        };
        offers.push(MerchantOffer {
            cost_a,
            result,
            cost_b,
            out_of_stock: r.u8().map_err(|_| e("out of stock"))? != 0,
            // Fixed big-endian i32s, not var-ints.
            uses: r.i32().map_err(|_| e("uses"))?,
            max_uses: r.i32().map_err(|_| e("max uses"))?,
            xp: r.i32().map_err(|_| e("xp"))?,
            special_price_diff: r.i32().map_err(|_| e("special price"))?,
            price_multiplier: r.f32().map_err(|_| e("price multiplier"))?,
            demand: r.i32().map_err(|_| e("demand"))?,
        });
    }
    Ok(MerchantOffers {
        container_id,
        offers,
        villager_level: r.varint().map_err(|e| format!("merchant_offers: {e:?}"))?,
        villager_xp: r.varint().map_err(|e| format!("merchant_offers: {e:?}"))?,
        show_progress: r.u8().map_err(|e| format!("merchant_offers: {e:?}"))? != 0,
        can_restock: r.u8().map_err(|e| format!("merchant_offers: {e:?}"))? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dummy component ids. The fixtures' patches and predicates are all
    /// EMPTY, so no shape is ever looked up — what these have to be is
    /// distinct and out of the way, not correct.
    const IDS: DataComponentIds = DataComponentIds {
        swing_animation: 100,
        damage: 101,
        charged_projectiles: 102,
        max_damage: 103,
        rarity: 104,
        unbreakable: 105,
        custom_name: 106,
        item_name: 107,
        lore: 108,
        enchantments: 109,
        stored_enchantments: 110,
        enchantment_glint_override: 111,
        dyed_color: 112,
        trim: 113,
        map_id: 114,
        dye: 115,
        provides_banner_patterns: 116,
        bundle_contents: 117,
        container: 118,
        use_cooldown: 119,
        written_book_content: 122,
    };

    /// A hand-built body, to pin the wire ORDER.
    ///
    /// The arithmetic above is exact over an input nothing else verifies —
    /// M92's shape — so this drives `parse` on bytes. The headline is that the
    /// result sits BETWEEN the two costs, which no accessor ordering suggests.
    fn body() -> Vec<u8> {
        let mut b = Vec::new();
        b.push(7); // container id
        b.push(1); // one offer
        // cost A: item 264 (a raw holderRegistry id), count 3, empty predicate.
        b.extend_from_slice(&[0x88, 0x02]); // varint 264
        b.push(3);
        b.push(0); // predicate: an empty list
        // the RESULT, here — count 1, item 5, an empty patch (0 added, 0 removed).
        b.push(1);
        b.push(5);
        b.push(0);
        b.push(0);
        // cost B: present, item 9, count 2, empty predicate.
        b.push(1);
        b.push(9);
        b.push(2);
        b.push(0);
        b.push(0); // not out of stock
        for v in [4i32, 16, 2, -1] {
            b.extend_from_slice(&v.to_be_bytes()); // FIXED i32s
        }
        b.extend_from_slice(&0.05f32.to_be_bytes());
        b.extend_from_slice(&10i32.to_be_bytes()); // demand
        b.push(3); // villager level
        b.push(70); // villager xp
        b.push(1); // show progress
        b.push(0); // cannot restock
        b
    }

    #[test]
    fn the_result_is_read_BETWEEN_the_two_costs() {
        let ids = IDS;
        let m = parse(&body(), ids).expect("parse");
        assert_eq!(m.container_id, 7);
        assert_eq!(m.offers.len(), 1);
        let o = &m.offers[0];
        // 264 is a raw id — a `holder`-style `id + 1` reading would give 263.
        assert_eq!(o.cost_a.item_id, 264);
        assert_eq!(o.cost_a.count, 3);
        assert_eq!(o.cost_b.as_ref().map(|c| (c.item_id, c.count)), Some((9, 2)));
        match &o.result {
            crate::item_stack::WireSlot::Stack(s) => {
                assert_eq!((s.item_id, s.count), (5, 1), "the SOLD item, not cost B");
            }
            _ => panic!("empty result"),
        }
    }

    #[test]
    fn the_numerics_are_fixed_width_not_varints() {
        let ids = IDS;
        let o = &parse(&body(), ids).expect("parse").offers[0];
        assert_eq!((o.uses, o.max_uses, o.xp), (4, 16, 2));
        // A NEGATIVE special price, which is what a discount is — and which a
        // var-int reading of a fixed i32 would report as a huge positive.
        assert_eq!(o.special_price_diff, -1);
        assert_eq!(o.price_multiplier, 0.05);
        assert_eq!(o.demand, 10);
        assert!(!o.out_of_stock);
    }

    #[test]
    fn the_trailing_fields_land_after_the_offers() {
        let ids = IDS;
        let m = parse(&body(), ids).expect("parse");
        assert_eq!((m.villager_level, m.villager_xp), (3, 70));
        assert!(m.show_progress, "a villager");
        assert!(!m.can_restock);
    }

    #[test]
    fn a_truncated_body_is_an_error_rather_than_a_partial_list() {
        let ids = IDS;
        let full = body();
        for cut in [3, 8, 14, full.len() - 1] {
            assert!(parse(&full[..cut], ids).is_err(), "cut at {cut}");
        }
    }

    /// The price arithmetic, which is where a transcription goes wrong.
    #[test]
    fn demand_cannot_make_a_trade_cheaper_but_a_discount_can() {
        let offer = |demand, special, mult| MerchantOffer {
            cost_a: ItemCost {
                item_id: 1,
                count: 4,
                constrained: false,
            },
            result: crate::item_stack::WireSlot::Empty,
            cost_b: None,
            out_of_stock: false,
            uses: 0,
            max_uses: 16,
            xp: 2,
            special_price_diff: special,
            price_multiplier: mult,
            demand,
        };
        // No demand, no special: the base price.
        assert_eq!(offer(0, 0, 0.05).modified_cost_a(64), 4);
        // Demand raises it: floor(4 * 10 * 0.05) = 2.
        assert_eq!(offer(10, 0, 0.05).modified_cost_a(64), 6);
        // NEGATIVE demand is clamped at 0 — it does not discount.
        assert_eq!(offer(-10, 0, 0.05).modified_cost_a(64), 4);
        // But `specialPriceDiff` is added AFTER that clamp, so a hero-of-the-
        // village discount really does reduce the price.
        assert_eq!(offer(0, -2, 0.05).modified_cost_a(64), 2);
        // …and the two compose, the discount applying to the raised price.
        assert_eq!(offer(10, -2, 0.05).modified_cost_a(64), 4);
    }

    #[test]
    fn the_price_is_clamped_to_one_and_to_the_items_own_max_stack() {
        let mut o = MerchantOffer {
            cost_a: ItemCost {
                item_id: 1,
                count: 4,
                constrained: false,
            },
            result: crate::item_stack::WireSlot::Empty,
            cost_b: None,
            out_of_stock: false,
            uses: 0,
            max_uses: 16,
            xp: 2,
            special_price_diff: -100,
            price_multiplier: 0.05,
            demand: 0,
        };
        assert_eq!(o.modified_cost_a(64), 1, "never free");
        o.special_price_diff = 0;
        o.demand = 10_000;
        assert_eq!(o.modified_cost_a(64), 64, "never more than a stack");
        // The ceiling is the ITEM's, not 64 — so a stack-of-16 cost caps at 16.
        assert_eq!(o.modified_cost_a(16), 16);
    }

    #[test]
    fn only_cost_a_is_modified() {
        let o = MerchantOffer {
            cost_a: ItemCost {
                item_id: 1,
                count: 4,
                constrained: false,
            },
            result: crate::item_stack::WireSlot::Empty,
            cost_b: Some(ItemCost {
                item_id: 2,
                count: 3,
                constrained: false,
            }),
            out_of_stock: false,
            uses: 0,
            max_uses: 16,
            xp: 2,
            special_price_diff: 5,
            price_multiplier: 0.2,
            demand: 10,
        };
        assert_ne!(o.modified_cost_a(64), o.cost_a.count, "A moves");
        assert_eq!(o.cost_b_count(), 3, "B does not");
    }

    #[test]
    fn a_discount_is_reported_only_when_the_price_actually_moved() {
        let base = MerchantOffer {
            cost_a: ItemCost {
                item_id: 1,
                count: 4,
                constrained: false,
            },
            result: crate::item_stack::WireSlot::Empty,
            cost_b: None,
            out_of_stock: false,
            uses: 0,
            max_uses: 16,
            xp: 2,
            special_price_diff: 0,
            price_multiplier: 0.05,
            demand: 0,
        };
        assert!(!base.discounted(64));
        let mut d = base.clone();
        d.special_price_diff = -2;
        assert!(d.discounted(64));
        // A special price whose effect the clamp erases is NOT a discount on
        // screen, because the two counts end up equal.
        let mut e = base.clone();
        e.cost_a.count = 1;
        e.special_price_diff = -5;
        assert_eq!(e.modified_cost_a(64), 1);
        assert!(!e.discounted(64), "clamped back to its base");
    }
}
