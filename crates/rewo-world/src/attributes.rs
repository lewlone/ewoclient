//! Entity attributes: the value model behind `ClientboundUpdateAttributesPacket`
//! (M55).
//!
//! The packet carries a *base* and a modifier list; the number a health bar
//! wants is `AttributeInstance.getValue()`, which is
//! `AttributeInstance.calculateValue` followed by the attribute's own
//! `sanitizeValue`. Both are transcribed here verbatim — see [`Instance::value`]
//! for the operation order, which is the one thing about this file that cannot
//! be guessed from the enum.
//!
//! Metadata index 9 (`DATA_HEALTH_ID`, M24) is the *current* health and arrives
//! independently; nothing here touches it. This module is only the other half —
//! the maximum, and every other attribute that rides the same packet.

use std::collections::HashMap;

use rewo_data::entity_attributes::AttrDef;

/// `AttributeModifier.Operation`.
///
/// The ids are the enum's explicit `id` field, not its ordinal — they happen to
/// coincide in 26.2, and the wire codec (`ByteBufCodecs.idMapper`) writes the
/// explicit one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// `add_value` — a flat addend, applied first.
    AddValue,
    /// `add_multiplied_base` — a fraction of the *post-`AddValue`* base.
    AddMultipliedBase,
    /// `add_multiplied_total` — a compounding factor on the running total.
    AddMultipliedTotal,
}

impl Operation {
    /// `AttributeModifier.Operation.BY_ID`, which is
    /// `ByIdMap.continuous(..., OutOfBoundsStrategy.ZERO)`.
    ///
    /// **An out-of-range id is not an error: it yields `ADD_VALUE`.** The
    /// `ZERO` strategy is `id >= 0 && id < length ? sortedValues[id] :
    /// sortedValues[0]`, so a server (or a corrupted stream) sending operation
    /// 7 gets a flat addend, not a rejected packet. Rejecting it here would be
    /// a silent behavioural difference from every vanilla client.
    pub fn from_id(id: i32) -> Operation {
        match id {
            1 => Operation::AddMultipliedBase,
            2 => Operation::AddMultipliedTotal,
            _ => Operation::AddValue,
        }
    }

    /// The wire id.
    pub fn id(self) -> i32 {
        match self {
            Operation::AddValue => 0,
            Operation::AddMultipliedBase => 1,
            Operation::AddMultipliedTotal => 2,
        }
    }
}

/// One `AttributeModifier`.
#[derive(Clone, Debug, PartialEq)]
pub struct Modifier {
    /// `AttributeModifier.id()` — a namespaced identifier, the key vanilla
    /// deduplicates on.
    pub id: String,
    pub amount: f64,
    pub operation: Operation,
}

/// One `AttributeInstance`: a base value plus the modifiers currently on it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Instance {
    pub base: f64,
    pub modifiers: Vec<Modifier>,
}

impl Instance {
    /// `AttributeInstance.getValue()` — `calculateValue` then `sanitizeValue`.
    ///
    /// The order is load-bearing and is **not** what "apply the modifiers"
    /// suggests:
    ///
    /// ```text
    /// base = baseValue
    /// for m in ADD_VALUE:            base   += m.amount
    /// result = base
    /// for m in ADD_MULTIPLIED_BASE:  result += base * m.amount
    /// for m in ADD_MULTIPLIED_TOTAL: result *= 1.0 + m.amount
    /// return sanitizeValue(result)
    /// ```
    ///
    /// Two consequences worth stating, because both are invisible with a single
    /// modifier and only separate when several are present:
    ///
    /// * `ADD_MULTIPLIED_BASE` multiplies the **post-`ADD_VALUE`** base, and
    ///   every such modifier sees that same base — they do not compound with
    ///   each other. `ADD_MULTIPLIED_TOTAL` compounds with everything before it.
    ///   So base 20 with `+10 ADD_VALUE` and `+0.5 ADD_MULTIPLIED_BASE` is
    ///   `30 + 30*0.5 = 45`, not `30 + 20*0.5 = 40`.
    /// * The two multiplying operations are therefore not interchangeable even
    ///   though a lone `+0.5` of either turns 20 into 30.
    ///
    /// **Deviation (documented, not accidental):** vanilla iterates
    /// `Object2ObjectOpenHashMap.values()` within each operation group, whose
    /// order is unspecified, so vanilla's own `ADD_MULTIPLIED_TOTAL` product is
    /// not bit-reproducible across two JVMs when several are present. This
    /// applies them in packet order, which is deterministic and agrees with
    /// vanilla to within float-multiplication reassociation.
    pub fn value(&self, def: &AttrDef) -> f64 {
        let mut base = self.base;
        for m in &self.modifiers {
            if m.operation == Operation::AddValue {
                base += m.amount;
            }
        }
        let mut result = base;
        for m in &self.modifiers {
            if m.operation == Operation::AddMultipliedBase {
                result += base * m.amount;
            }
        }
        for m in &self.modifiers {
            if m.operation == Operation::AddMultipliedTotal {
                result *= 1.0 + m.amount;
            }
        }
        sanitize(result, def)
    }
}

/// `RangedAttribute.sanitizeValue`: `NaN ? min : Mth.clamp(v, min, max)`.
///
/// Every registered attribute in 26.2 is a `RangedAttribute`, so this is not
/// conditional on the attribute kind. Note that **NaN becomes the minimum, not
/// the maximum and not NaN** — `max_health` has minimum 1.0, so an entity whose
/// modifiers multiply out to NaN has one half-heart, not an empty bar.
///
/// `Mth.clamp` is `value < min ? min : Math.min(value, max)`, which is written
/// out rather than using Rust's `f64::clamp` because that panics when
/// `min > max` and returns NaN for a NaN input; the NaN branch above already
/// took NaN out of play, and the ordering here matches the Java exactly.
pub fn sanitize(value: f64, def: &AttrDef) -> f64 {
    if value.is_nan() {
        return def.min;
    }
    if value < def.min {
        def.min
    } else if value < def.max {
        value
    } else {
        def.max
    }
}

/// Every attribute one entity currently holds, keyed by wire id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntityAttributes {
    by_id: HashMap<i32, Instance>,
}

impl EntityAttributes {
    /// Apply one `AttributeSnapshot`.
    ///
    /// `handleUpdateAttributes` does `setBaseValue` → `removeModifiers()` → add
    /// each, so a snapshot **replaces** that attribute's whole state. It does
    /// not merge with a previous packet's modifiers, and it leaves every
    /// attribute the packet did not name alone.
    ///
    /// **Deviation:** vanilla's `addTransientModifier` → `addModifier` throws
    /// `IllegalArgumentException` when two modifiers in one snapshot share an
    /// id, which kills the client's packet thread. Here the first wins — the
    /// state `modifierById.putIfAbsent` would have been left in — and the
    /// packet is otherwise honoured.
    pub fn apply(&mut self, attr: i32, base: f64, modifiers: Vec<Modifier>) {
        let mut kept: Vec<Modifier> = Vec::with_capacity(modifiers.len());
        for m in modifiers {
            if !kept.iter().any(|k| k.id == m.id) {
                kept.push(m);
            }
        }
        self.by_id.insert(attr, Instance { base, modifiers: kept });
    }

    /// The stored instance for a wire id, if the server has sent one.
    pub fn get(&self, attr: i32) -> Option<&Instance> {
        self.by_id.get(&attr)
    }

    /// How many attributes the server has sent for this entity.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether nothing has been received yet.
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// Where a resolved attribute value came from.
///
/// The distinction exists so a caller can tell "the server told me this entity
/// has 40 max health" from "nobody has said anything, and this entity type's
/// supplier starts at 20" from "there is no such thing as this entity's max
/// health". A health bar that cannot tell the last case from the middle one
/// draws a full bar over a boat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// An `update_attributes` snapshot established it.
    Synced,
    /// No packet has covered it; the value is the entity type's
    /// `AttributeSupplier` base with no modifiers.
    Default,
}

/// The final value of one attribute for one entity, and where it came from.
///
/// Returns `None` — deliberately, rather than any number — when nothing
/// establishes a value:
///
/// * the entity type is unknown (`entity_name` is `None`), or
/// * `DefaultAttributes.SUPPLIERS` has no entry for it (it is not a
///   `LivingEntity`), or
/// * its supplier does not declare this attribute.
///
/// The last two are exactly the cases `AttributeMap.getInstance` answers with
/// null and `handleUpdateAttributes` skips. A `Some(20.0)` from this function
/// always means a real supplier declared `max_health`; it is never a fallback.
pub fn resolve(
    stored: Option<&EntityAttributes>,
    entity_name: Option<&str>,
    attr: &str,
    reg: &rewo_data::attributes::AttributeRegistry,
) -> Option<(f64, Source)> {
    let id = reg.id_of(attr)?;
    let def = reg.def(id)?;
    // The supplier is consulted first and unconditionally: it is the filter on
    // what the entity may hold at all, not merely a fallback. An attribute it
    // does not declare has no value even if a (malformed) packet carried one.
    let default_base = reg.default_base(entity_name?, attr)?;
    match stored.and_then(|s| s.get(id)) {
        Some(inst) => Some((inst.value(def), Source::Synced)),
        None => Some((sanitize(default_base, def), Source::Default)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_HEALTH: AttrDef = AttrDef {
        name: "max_health",
        default: 20.0,
        min: 1.0,
        max: 1024.0,
        syncable: true,
    };

    fn m(amount: f64, operation: Operation) -> Modifier {
        Modifier {
            id: format!("test:{}", operation.id()),
            amount,
            operation,
        }
    }

    #[test]
    fn a_bare_base_is_the_value() {
        let i = Instance { base: 20.0, modifiers: vec![] };
        assert_eq!(i.value(&MAX_HEALTH), 20.0);
    }

    #[test]
    fn add_multiplied_total_uses_the_running_total() {
        let i = Instance {
            base: 20.0,
            modifiers: vec![m(0.5, Operation::AddMultipliedTotal)],
        };
        assert_eq!(i.value(&MAX_HEALTH), 30.0);
    }

    #[test]
    fn the_two_multiplying_operations_differ_only_alongside_add_value() {
        // Alone they are indistinguishable — this is why a single-modifier
        // witness cannot tell them apart.
        let base_only = Instance {
            base: 20.0,
            modifiers: vec![m(0.5, Operation::AddMultipliedBase)],
        };
        let total_only = Instance {
            base: 20.0,
            modifiers: vec![m(0.5, Operation::AddMultipliedTotal)],
        };
        assert_eq!(base_only.value(&MAX_HEALTH), total_only.value(&MAX_HEALTH));

        // With an ADD_VALUE present they separate: ADD_MULTIPLIED_BASE reads
        // the post-ADD_VALUE base (30), so both give 45 here...
        let with_add_base = Instance {
            base: 20.0,
            modifiers: vec![m(10.0, Operation::AddValue), m(0.5, Operation::AddMultipliedBase)],
        };
        assert_eq!(with_add_base.value(&MAX_HEALTH), 45.0);

        // ...and it takes two multipliers to make the orders disagree.
        let both = Instance {
            base: 20.0,
            modifiers: vec![
                m(0.5, Operation::AddMultipliedBase),
                m(1.0, Operation::AddMultipliedTotal),
            ],
        };
        // (20 + 20*0.5) * 2 = 60, not 20*1.5*2 evaluated the other way round.
        assert_eq!(both.value(&MAX_HEALTH), 60.0);
    }

    #[test]
    fn the_clamp_bites_at_both_ends() {
        let low = Instance { base: 0.0, modifiers: vec![] };
        assert_eq!(low.value(&MAX_HEALTH), 1.0, "min is 1.0, not 0.0");
        let high = Instance { base: 5000.0, modifiers: vec![] };
        assert_eq!(high.value(&MAX_HEALTH), 1024.0);
    }

    #[test]
    fn nan_becomes_the_minimum_not_the_maximum() {
        let i = Instance {
            base: f64::INFINITY,
            modifiers: vec![m(-1.0, Operation::AddMultipliedTotal)],
        };
        // inf * 0.0 = NaN, and sanitizeValue sends NaN to min.
        assert_eq!(i.value(&MAX_HEALTH), 1.0);
    }

    #[test]
    fn a_snapshot_replaces_rather_than_merging() {
        let mut a = EntityAttributes::default();
        a.apply(23, 20.0, vec![m(10.0, Operation::AddValue)]);
        a.apply(23, 20.0, vec![]);
        assert_eq!(a.get(23).unwrap().value(&MAX_HEALTH), 20.0);
    }

    #[test]
    fn a_duplicate_modifier_id_keeps_the_first() {
        let mut a = EntityAttributes::default();
        let dup = Modifier {
            id: "test:same".into(),
            amount: 5.0,
            operation: Operation::AddValue,
        };
        let other = Modifier {
            id: "test:same".into(),
            amount: 100.0,
            operation: Operation::AddValue,
        };
        a.apply(23, 20.0, vec![dup, other]);
        assert_eq!(a.get(23).unwrap().value(&MAX_HEALTH), 25.0);
    }

    #[test]
    fn an_out_of_range_operation_id_is_add_value() {
        assert_eq!(Operation::from_id(0), Operation::AddValue);
        assert_eq!(Operation::from_id(1), Operation::AddMultipliedBase);
        assert_eq!(Operation::from_id(2), Operation::AddMultipliedTotal);
        assert_eq!(Operation::from_id(3), Operation::AddValue);
        assert_eq!(Operation::from_id(-1), Operation::AddValue);
    }
}
