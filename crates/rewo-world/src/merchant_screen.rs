//! `MerchantScreen` — the villager trade list's geometry and scroll (M93u).
//!
//! Seven trade buttons down the left, a scrollbar beside them, and three item
//! positions per row: cost A, cost B, the result.
//!
//! # It is not the stonecutter's grid with different numbers
//!
//! Both scroll a list of buttons, and the two models disagree on almost every
//! rule. The stonecutter scrolls by whole **rows** through a fractional
//! `scrollOffs` in `0..=1`; the merchant's `scrollOff` is an **offer index**,
//! an integer, and one wheel notch moves exactly one offer. The stonecutter's
//! thumb travels a straight `41 * scrollOffs`; the merchant's is quantised to a
//! computed step and then **special-cased at the bottom**, because the step
//! arithmetic cannot reach the end on its own.

/// `minecraft:menu`'s `merchant` id.
pub const MERCHANT_MENU_PROTOCOL_ID: i32 = 19;

/// `NUMBER_OF_OFFER_BUTTONS` — the visible window, in offers.
pub const TRADE_BUTTONS: i32 = 7;
/// `TRADE_BUTTON_X` / `_WIDTH` / `_HEIGHT`, relative to the panel.
pub const TRADE_BUTTON_X: i32 = 5;
pub const TRADE_BUTTON_W: i32 = 88;
pub const TRADE_BUTTON_H: i32 = 20;
/// `init`'s `buttonY = yo + 16 + 2`, then `+= 20` per button.
pub const TRADE_BUTTON_Y: i32 = 18;

/// `SELL_ITEM_1_X` / `SELL_ITEM_2_X` / `BUY_ITEM_X`, each offset from
/// `xo + 5` — the button's own x.
///
/// **Cost A adds the 5 twice.** Vanilla computes `sellItem1X = xo + 5 + 5`
/// once, outside the loop, and then passes it; the other two are written
/// inline as `xo + 5 + 35` and `xo + 5 + 68`. So cost A sits at **10** and not
/// at the button's edge, which reading `SELL_ITEM_1_X = 5` alone suggests.
pub const COST_A_X: i32 = TRADE_BUTTON_X + 5;
pub const COST_B_X: i32 = TRADE_BUTTON_X + 35;
pub const RESULT_X: i32 = TRADE_BUTTON_X + 68;

/// Where a drawn row's ITEMS sit — `offerY = yo + 16 + 1`, then
/// `decorHeight = offerY + 2`, advancing 20 per **drawn** offer.
///
/// **One pixel above the buttons.** `init` starts them at `yo + 16 + 2` while
/// the offer cursor starts at `+ 1`, so the row's items are at `19 + row * 20`
/// and its button at `18 + row * 20`. Deriving the items from
/// [`button_y`] — the obvious thing — puts every icon a pixel low.
///
/// Vanilla's cursor advances only inside the drawn branch, so it counts drawn
/// offers; this computes the row directly instead, which is equivalent while
/// the visibility guard holds and is why removing that guard here changes
/// nothing (it would stack vanilla's offers down the panel).
pub fn row_item_y(row: i32) -> i32 {
    17 + row * TRADE_BUTTON_H + 2
}

/// `SCROLL_BAR_START_X` / `_TOP_POS_Y` / `_HEIGHT`, and the thumb's size.
pub const SCROLL_X: i32 = 94;
pub const SCROLL_TOP: i32 = 18;
pub const SCROLL_HEIGHT: i32 = 139;
pub const SCROLLER_W: i32 = 6;
pub const SCROLLER_H: i32 = 27;

// ── The discounted price pair (M93w) ──────────────────────────────────────

/// How far right the **modified** count is drawn when a price is discounted.
pub const DISCOUNT_SECOND_X: i32 = 14;
/// The strikethrough's offset from the cost-A icon, and its size.
pub const STRIKETHROUGH_DX: i32 = 7;
pub const STRIKETHROUGH_DY: i32 = 12;
pub const STRIKETHROUGH_W: i32 = 9;
pub const STRIKETHROUGH_H: i32 = 2;

/// What `extractAndDecorateCostA` draws for one row's cost A (M93w).
///
/// ```java
/// graphics.fakeItem(costA, sellItem1X, decorHeight);
/// if (baseCostA.getCount() == costA.getCount()) {
///    graphics.itemDecorations(font, costA, sellItem1X, decorHeight);
/// } else {
///    graphics.itemDecorations(font, baseCostA, sellItem1X, decorHeight, baseCostA.getCount() == 1 ? "1" : null);
///    graphics.itemDecorations(font, costA, sellItem1X + 14, decorHeight, costA.getCount() == 1 ? "1" : null);
///    graphics.blitSprite(DISCOUNT_STRIKETHRUOGH_SPRITE, sellItem1X + 7, decorHeight + 12, 9, 2);
/// }
/// ```
///
/// Three things a reader would get wrong:
///
/// * there is **one icon, not two** — `fakeItem` is called once, outside the
///   branch, and with the **modified** cost. The discounted display is two
///   *numbers* over a single item.
/// * the second number sits 14 px right, over **empty panel**, and the
///   strikethrough at `+7` crosses the *first* one — the count labels are
///   right-aligned into the icon's 16 px box, so the base number lands around
///   `+9..+16` and a 9-px line from `+7` strikes it.
/// * **a count of 1 normally draws nothing**, and the `countText` override is
///   there solely to defeat that: `count == 1 ? "1" : null` forces the digit so
///   both halves of the comparison are visible. Passing `null` throughout —
///   the obvious simplification — silently drops a number whenever a discount
///   reaches 1, which is exactly when the discount is most worth seeing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostADisplay {
    /// The count drawn at the icon. `None` when vanilla draws no digit.
    pub at_icon: Option<i32>,
    /// The count drawn 14 px right, present only when discounted.
    pub at_second: Option<i32>,
    /// Whether the strikethrough is drawn.
    pub strikethrough: bool,
}

/// `extractAndDecorateCostA`'s two branches.
///
/// `base` is `getBaseCostA().getCount()` and `modified` is `getCostA()`'s.
pub fn cost_a_display(base: i32, modified: i32) -> CostADisplay {
    if base == modified {
        CostADisplay {
            // The plain path passes no `countText`, so `itemCount`'s own rule
            // applies and a single item shows no digit.
            at_icon: (modified != 1).then_some(modified),
            at_second: None,
            strikethrough: false,
        }
    } else {
        CostADisplay {
            // Both forced, including a 1.
            at_icon: Some(base),
            at_second: Some(modified),
            strikethrough: true,
        }
    }
}

// ── The XP bar (M93v) ─────────────────────────────────────────────────────
//
// `MerchantScreen.extractProgressBar`, and `VillagerData`'s thresholds.

/// `PROGRESS_BAR_X` / `_Y`, and the bar's size.
pub const XP_BAR_X: i32 = 136;
pub const XP_BAR_Y: i32 = 16;
pub const XP_BAR_W: i32 = 102;
pub const XP_BAR_H: i32 = 5;

/// `VillagerData.NEXT_LEVEL_XP_THRESHOLDS`.
pub const XP_THRESHOLDS: [i32; 5] = [0, 10, 70, 150, 250];
/// `MIN_VILLAGER_LEVEL` / `MAX_VILLAGER_LEVEL`.
pub const MIN_VILLAGER_LEVEL: i32 = 1;
pub const MAX_VILLAGER_LEVEL: i32 = 5;

/// `VillagerData.canLevelUp` — `level >= 1 && level < 5`.
pub fn can_level_up(level: i32) -> bool {
    (MIN_VILLAGER_LEVEL..MAX_VILLAGER_LEVEL).contains(&level)
}

/// `getMinXpPerLevel` / `getMaxXpPerLevel`.
///
/// **Both return 0 when the villager cannot level up**, which makes their
/// difference 0 — and the bar's `multiplier` divides by exactly that. The
/// division is safe only because `extractProgressBar` guards on `canLevelUp`
/// *and* on `traderLevel < 5` before computing it; reproducing the arithmetic
/// without the guard is a divide by zero on a master villager.
pub fn min_xp_per_level(level: i32) -> i32 {
    if can_level_up(level) {
        XP_THRESHOLDS[(level - 1) as usize]
    } else {
        0
    }
}

pub fn max_xp_per_level(level: i32) -> i32 {
    if can_level_up(level) {
        XP_THRESHOLDS[level as usize]
    } else {
        0
    }
}

/// The XP bar's three segment widths, or `None` for no bar at all.
///
/// ```java
/// if (traderLevel < 5) {
///    blit(BACKGROUND, xo + 136, yo + 16, 102, 5);
///    int minXp = getMinXpPerLevel(traderLevel);
///    if (traderXp >= minXp && canLevelUp(traderLevel)) {
///       float multiplier = 102.0F / (getMaxXpPerLevel(traderLevel) - minXp);
///       int w = Math.min(Mth.floor(multiplier * (traderXp - minXp)), 102);
///       blit(CURRENT, 102, 5, 0, 0, xo + 136, yo + 16, w, 5);
///       int futureXp = getFutureTraderXp();
///       if (futureXp > 0) {
///          int futureXpWidth = Math.min(Mth.floor(futureXp * multiplier), 102 - w);
///          blit(RESULT, 102, 5, w, 0, xo + 136 + w, yo + 16, futureXpWidth, 5);
///       }
///    }
/// }
/// ```
///
/// Returns `(current, future)` widths; the background is drawn whenever the
/// result is `Some`. Three things read backwards:
///
/// * **`traderLevel < 5` hides the whole bar**, background included — a master
///   villager shows nothing rather than a full bar.
/// * the **result segment samples the sprite from `w`**, not from 0, so it
///   continues the gradient where the current segment stopped rather than
///   restarting it. That is why it needs a source offset at all.
/// * `futureXpWidth` is clamped to `102 - w`, the room *left*, so a trade
///   worth more XP than the level needs fills the bar exactly and no further.
pub fn xp_bar(level: i32, xp: i32, future_xp: i32) -> Option<(i32, i32)> {
    if level >= MAX_VILLAGER_LEVEL {
        return None;
    }
    let min_xp = min_xp_per_level(level);
    if !(xp >= min_xp && can_level_up(level)) {
        // The background still draws — the guard is inside it — so this is a
        // bar with no fill rather than no bar.
        return Some((0, 0));
    }
    let multiplier = XP_BAR_W as f32 / (max_xp_per_level(level) - min_xp) as f32;
    let w = ((multiplier * (xp - min_xp) as f32).floor() as i32).min(XP_BAR_W);
    let future = if future_xp > 0 {
        ((future_xp as f32 * multiplier).floor() as i32).min(XP_BAR_W - w)
    } else {
        0
    };
    Some((w, future))
}

/// `MerchantOffer.satisfiedBy` — whether the payment slots match this offer.
///
/// ```java
/// if (!this.baseCostA.test(buyA) || buyA.getCount() < this.getModifiedCostCount(this.baseCostA)) return false;
/// return !this.costB.isPresent() ? buyB.isEmpty()
///     : this.costB.get().test(buyB) && buyB.getCount() >= this.costB.get().count();
/// ```
///
/// **Cost A is compared against its MODIFIED count and cost B against its
/// base** — the same asymmetry `modified_cost_a` carries, one layer up. So a
/// discount lowers what you must hand over for A and never for B.
///
/// And an offer with **no** cost B requires `buyB` to be *empty*: putting
/// anything in the second slot makes a one-item trade stop matching.
pub fn satisfied_by(
    cost_a_ok: bool,
    have_a: i32,
    need_a: i32,
    cost_b: Option<(bool, i32)>,
    have_b: i32,
) -> bool {
    if !cost_a_ok || have_a < need_a {
        return false;
    }
    match cost_b {
        None => have_b == 0,
        Some((ok, need_b)) => ok && have_b >= need_b,
    }
}

/// One offer's inputs to [`satisfied_by`], as the client can resolve them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfferMatch {
    pub cost_a_item: i32,
    /// The **modified** count — a discount lowers what you must hand over.
    pub need_a: i32,
    pub cost_b: Option<(i32, i32)>,
    /// Whether either cost carries a `DataComponentExactPredicate`.
    pub constrained: bool,
}

/// Which offers the payment slots satisfy (M93v).
///
/// **A constrained cost is declined rather than guessed.** `ItemCost.test` is
/// `stack.is(item) && components.test(stack)`, and the second half compares
/// per-component *values* where M41 gives Rewo a digest of the whole patch. So
/// an offer naming one cannot be evaluated, and treating it as matched would
/// show a result segment for a trade the server will refuse.
///
/// The decline is **one-directional**: the bar's result segment is missing
/// where vanilla would show it, never present where vanilla would not.
///
/// Lives here rather than in `live_cmd` because `PlaySession`'s module has no
/// test module anywhere in the repo — M71's finding, and M93t's fix.
pub fn satisfied_offers(
    offers: &[OfferMatch],
    have_a: Option<(i32, i32)>,
    have_b: Option<(i32, i32)>,
) -> Vec<bool> {
    // `MerchantContainer.updateSellItem`: **if slot 0 is empty, slot 1 becomes
    // `buyA` and `buyB` is empty** — so paying with only the second slot still
    // matches a one-item trade.
    let (a, b) = match (have_a, have_b) {
        (None, s1) => (s1, None),
        (s0, s1) => (s0, s1),
    };
    let Some((a_id, a_count)) = a else {
        return vec![false; offers.len()];
    };
    let (b_id, b_count) = b.map_or((0, 0), |(i, c)| (i, c));
    offers
        .iter()
        .map(|o| {
            !o.constrained
                && satisfied_by(
                    o.cost_a_item == a_id,
                    a_count,
                    o.need_a,
                    o.cost_b.map(|(id, n)| (id == b_id, n)),
                    b_count,
                )
        })
        .collect()
}

/// `MerchantOffers.getRecipeFor`'s selection rule.
///
/// ```java
/// if (selectionHint > 0 && selectionHint < this.size()) { … return satisfiedBy ? offer : null; }
/// for (i in 0..size) if (satisfiedBy) return offer;
/// ```
///
/// **`selectionHint > 0` is strictly greater**, so selecting the FIRST trade
/// does not take the fast path — it falls through to the linear scan and can
/// therefore match a *different* offer than the one selected, if an earlier
/// one is satisfied by the same items. Hint 0 is both "nothing selected" and
/// "the first trade", and vanilla cannot tell them apart.
///
/// `satisfied` is indexed by offer.
pub fn recipe_for(selection_hint: i32, satisfied: &[bool]) -> Option<usize> {
    let n = satisfied.len();
    if selection_hint > 0 && (selection_hint as usize) < n {
        return satisfied[selection_hint as usize].then_some(selection_hint as usize);
    }
    satisfied.iter().position(|s| *s)
}

/// `canScroll` — `numberOfOffers > 7`, strictly greater, so exactly seven
/// offers fill the window and do not scroll.
pub fn can_scroll(offers: usize) -> bool {
    offers > TRADE_BUTTONS as usize
}

/// The largest `scrollOff` — `numberOfOffers - 7`.
///
/// Negative for a short list, which vanilla never evaluates because every
/// caller is behind [`can_scroll`]; kept as an `i32` rather than clamped so the
/// arithmetic stays verbatim and the guard stays the caller's.
pub fn max_scroll_off(offers: usize) -> i32 {
    offers as i32 - TRADE_BUTTONS
}

/// `mouseScrolled` — one notch is **one offer**.
///
/// ```java
/// this.scrollOff = Mth.clamp((int)(this.scrollOff - scrollY), 0, maxScrollOff);
/// ```
///
/// Not a fraction of the list, as the stonecutter's is: a 30-trade villager
/// takes 23 notches to reach the bottom. The `(int)` truncates toward zero,
/// so a trackpad's fractional delta below 1 moves nothing at all.
pub fn scroll_off_from_wheel(scroll_off: i32, scroll_y: f64, offers: usize) -> i32 {
    ((scroll_off as f64 - scroll_y) as i32).clamp(0, max_scroll_off(offers))
}

/// `mouseDragged`'s new `scrollOff`.
///
/// ```java
/// float scrolling = ((float)event.y() - fullScrollTopPos - 13.5F) / (fullScrollBottomPos - fullScrollTopPos - 27.0F);
/// scrolling = scrolling * maxScrollOff + 0.5F;
/// this.scrollOff = Mth.clamp((int)scrolling, 0, maxScrollOff);
/// ```
///
/// `13.5` is half the thumb, so the thumb's **centre** tracks the cursor, and
/// the divisor is the track less the thumb — `139 - 27` = 112. The `+ 0.5`
/// then `(int)` is a **round**, not a floor, and it is applied to the offer
/// index rather than to a fraction: the two orderings differ at every
/// half-step.
///
/// Note the clamp is applied *after* the round, so a cursor above the track
/// gives a negative `scrolling` whose `(int)` truncates toward zero — which is
/// why the clamp's lower bound is what stops it, not the cast.
pub fn scroll_off_from_drag(gui_y: f64, offers: usize) -> i32 {
    let max = max_scroll_off(offers);
    let t = (gui_y as f32 - SCROLL_TOP as f32 - 13.5)
        / (SCROLL_HEIGHT - SCROLLER_H) as f32;
    ((t * max as f32 + 0.5) as i32).clamp(0, max)
}

/// Where the thumb is drawn, relative to the panel.
///
/// ```java
/// int steps = offers.size() + 1 - 7;
/// if (steps > 1) {
///    int leftOver = 139 - (27 + (steps - 1) * 139 / steps);
///    int stepHeight = 1 + leftOver / steps + 139 / steps;
///    int scrollerYOff = Math.min(113, this.scrollOff * stepHeight);
///    if (this.scrollOff == steps - 1) scrollerYOff = 113;
/// ```
///
/// **The last position is special-cased, and which lists need it inverts.**
/// The step is integer-divided, and computing `(steps - 1) * stepHeight`
/// across sizes shows two regimes:
///
/// | offers | step | last computed | |
/// |---|---|---|---|
/// | 8 | 91 | 91 | **short** — the override is what reaches the bottom |
/// | 9 | 53 | 106 | **short** |
/// | 10 | 37 | 111 | **short** |
/// | 12 | 24 | 120 | overshoots; `min(113, …)` caps it |
/// | 30 | 6 | 138 | overshoots |
///
/// So the override is load-bearing for **short scrollable lists** and
/// redundant for long ones — the opposite of the intuition that a longer list
/// is the awkward case. `113` is `139 - 27 + 1`, one *past* the track less the
/// thumb: vanilla's own off-by-one, transcribed rather than corrected.
///
/// `None` means no thumb, which is `steps <= 1` — i.e. `size <= 7`, **exactly
/// [`can_scroll`]'s threshold**. Note `steps` is `size + 1 - 7`, one MORE than
/// [`max_scroll_off`], so the two are not interchangeable even though the
/// no-thumb boundary happens to coincide.
pub fn scroller_y(scroll_off: i32, offers: usize) -> Option<i32> {
    let steps = offers as i32 + 1 - TRADE_BUTTONS;
    if steps <= 1 {
        return None;
    }
    let left_over = SCROLL_HEIGHT - (SCROLLER_H + (steps - 1) * SCROLL_HEIGHT / steps);
    let step_height = 1 + left_over / steps + SCROLL_HEIGHT / steps;
    let off = if scroll_off == steps - 1 {
        113
    } else {
        (scroll_off * step_height).min(113)
    };
    Some(SCROLL_TOP + off)
}

/// Whether a press grabs the scrollbar.
///
/// ```java
/// event.x() > xo + 94 && event.x() < xo + 94 + 6 && event.y() > yo + 18 && event.y() <= yo + 18 + 139 + 1
/// ```
///
/// **Every bound is off by one from the obvious rectangle**: x is *strictly*
/// greater than 94 (so the leftmost column does not grab), and y is `<=` its
/// top plus `139 + 1`. Transcribed literally — the asymmetry is not a slip a
/// reader would reproduce by writing a normal hit test.
pub fn scroller_grabbed(gui_x: f64, gui_y: f64) -> bool {
    gui_x > SCROLL_X as f64
        && gui_x < (SCROLL_X + SCROLLER_W) as f64
        && gui_y > SCROLL_TOP as f64
        && gui_y <= (SCROLL_TOP + SCROLL_HEIGHT + 1) as f64
}

/// Which *button* the cursor is over, 0..7, or `None`.
pub fn button_at(gui_x: f64, gui_y: f64) -> Option<i32> {
    if gui_x < TRADE_BUTTON_X as f64 || gui_x >= (TRADE_BUTTON_X + TRADE_BUTTON_W) as f64 {
        return None;
    }
    let dy = gui_y - TRADE_BUTTON_Y as f64;
    if dy < 0.0 {
        return None;
    }
    let i = (dy as i32) / TRADE_BUTTON_H;
    (i < TRADE_BUTTONS && dy < (TRADE_BUTTONS * TRADE_BUTTON_H) as f64).then_some(i)
}

/// Which *offer* a button press selects — `getIndex() + scrollOff`.
///
/// The button's index is its position on screen; the offer's is absolute. The
/// packet carries the absolute one.
pub fn offer_for_button(button: i32, scroll_off: i32) -> i32 {
    button + scroll_off
}

/// A button's y, relative to the panel.
pub fn button_y(index: i32) -> i32 {
    TRADE_BUTTON_Y + index * TRADE_BUTTON_H
}

/// Whether an offer is drawn at all, from `extractOffers`' guard:
///
/// ```java
/// if (!this.canScroll(offers.size()) || currentOfferIndex >= this.scrollOff && currentOfferIndex < 7 + this.scrollOff)
/// ```
///
/// The **short-circuit matters**: with seven or fewer offers the window is
/// ignored entirely, so a stale non-zero `scrollOff` — left behind by a longer
/// list — cannot hide a short one.
pub fn offer_visible(index: i32, scroll_off: i32, offers: usize) -> bool {
    !can_scroll(offers) || (index >= scroll_off && index < TRADE_BUTTONS + scroll_off)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_undiscounted_price_is_one_number_and_a_single_item_is_none() {
        assert_eq!(
            cost_a_display(4, 4),
            CostADisplay {
                at_icon: Some(4),
                at_second: None,
                strikethrough: false
            }
        );
        // `itemCount`'s own rule: a count of 1 draws no digit at all.
        assert_eq!(
            cost_a_display(1, 1),
            CostADisplay {
                at_icon: None,
                at_second: None,
                strikethrough: false
            }
        );
    }

    #[test]
    fn a_discount_forces_BOTH_digits_including_a_one() {
        // The whole point of `countText`: without the override the discounted
        // number would vanish exactly when the discount is deepest.
        assert_eq!(
            cost_a_display(4, 1),
            CostADisplay {
                at_icon: Some(4),
                at_second: Some(1),
                strikethrough: true
            }
        );
        // …and a base of 1 raised by demand is the mirror case.
        assert_eq!(
            cost_a_display(1, 3),
            CostADisplay {
                at_icon: Some(1),
                at_second: Some(3),
                strikethrough: true
            }
        );
        assert_eq!(
            cost_a_display(6, 4),
            CostADisplay {
                at_icon: Some(6),
                at_second: Some(4),
                strikethrough: true
            }
        );
    }

    #[test]
    fn the_strikethrough_crosses_the_FIRST_number_not_the_gap() {
        // The count labels are right-aligned into the icon's 16 px box, so the
        // base digit sits around +9..+16 and a 9-px line from +7 strikes it —
        // rather than sitting in the space between the two numbers.
        assert_eq!(STRIKETHROUGH_DX, 7);
        assert!(STRIKETHROUGH_DX + STRIKETHROUGH_W <= DISCOUNT_SECOND_X + 2);
        assert_eq!((STRIKETHROUGH_W, STRIKETHROUGH_H), (9, 2));
    }

    #[test]
    fn a_master_villager_shows_no_bar_at_all() {
        // `traderLevel < 5` gates the BACKGROUND too, so level 5 is nothing
        // rather than a full bar.
        assert_eq!(xp_bar(5, 999, 0), None);
        assert!(xp_bar(4, 999, 0).is_some());
    }

    #[test]
    fn the_thresholds_are_vanillas_and_the_ends_return_zero() {
        assert_eq!(XP_THRESHOLDS, [0, 10, 70, 150, 250]);
        // Level 1 spans 0..10, level 4 spans 150..250.
        assert_eq!((min_xp_per_level(1), max_xp_per_level(1)), (0, 10));
        assert_eq!((min_xp_per_level(4), max_xp_per_level(4)), (150, 250));
        // BOTH are 0 outside the levelling range, so their difference is 0 —
        // the bar's divisor. Only `extractProgressBar`'s guards keep that
        // division safe.
        assert_eq!((min_xp_per_level(5), max_xp_per_level(5)), (0, 0));
        assert_eq!((min_xp_per_level(0), max_xp_per_level(0)), (0, 0));
        assert!(!can_level_up(0) && !can_level_up(5) && can_level_up(1) && can_level_up(4));
    }

    #[test]
    fn the_fill_is_the_fraction_of_the_level_not_of_the_total() {
        // Level 2 spans 10..70, so 40 xp is halfway through THAT level and not
        // through the villager's career.
        assert_eq!(xp_bar(2, 40, 0), Some((51, 0)));
        assert_eq!(xp_bar(2, 10, 0), Some((0, 0)), "the level's floor is empty");
        assert_eq!(xp_bar(2, 70, 0), Some((102, 0)), "and its ceiling is full");
        // Below the level's own floor the fill guard fails and the bar is
        // empty — but it still DRAWS, which is why this is Some.
        assert_eq!(xp_bar(2, 5, 0), Some((0, 0)));
    }

    #[test]
    fn the_future_segment_is_clamped_to_the_room_left() {
        // Level 1 spans 0..10, so the multiplier is 10.2 px per xp.
        let (w, f) = xp_bar(1, 5, 2).expect("level 1");
        assert_eq!(w, 51);
        assert_eq!(f, 20, "2 xp at 10.2 px each, floored");
        // A trade worth more than the level needs fills the rest exactly and
        // no further — `Math.min(…, 102 - w)`.
        assert_eq!(xp_bar(1, 5, 99), Some((51, 51)));
        assert_eq!(xp_bar(1, 9, 99), Some((91, 11)));
        // Zero future xp draws no result segment.
        assert_eq!(xp_bar(1, 5, 0), Some((51, 0)));
    }

    #[test]
    fn cost_a_is_matched_at_its_MODIFIED_count_and_b_at_its_base() {
        // 3 needed for A after a discount, 2 for B.
        assert!(satisfied_by(true, 3, 3, Some((true, 2)), 2));
        assert!(!satisfied_by(true, 2, 3, Some((true, 2)), 2), "short on A");
        assert!(!satisfied_by(true, 3, 3, Some((true, 2)), 1), "short on B");
        assert!(!satisfied_by(false, 9, 3, Some((true, 2)), 9), "wrong item in A");
        // An offer with NO cost B requires the second slot to be EMPTY —
        // putting anything there stops a one-item trade matching.
        assert!(satisfied_by(true, 3, 3, None, 0));
        assert!(!satisfied_by(true, 3, 3, None, 1));
    }

    #[test]
    fn a_constrained_cost_is_declined_rather_than_guessed() {
        let plain = OfferMatch {
            cost_a_item: 7,
            need_a: 2,
            cost_b: None,
            constrained: false,
        };
        let fancy = OfferMatch {
            constrained: true,
            ..plain
        };
        // The same payment matches the plain offer and not the constrained
        // one, even though their items and counts are identical — Rewo cannot
        // evaluate the component predicate and refuses to assume.
        assert_eq!(
            satisfied_offers(&[plain, fancy], Some((7, 2)), None),
            vec![true, false]
        );
    }

    #[test]
    fn an_empty_first_slot_promotes_the_second() {
        // `updateSellItem`: with slot 0 empty, slot 1 becomes `buyA`.
        let o = OfferMatch {
            cost_a_item: 7,
            need_a: 2,
            cost_b: None,
            constrained: false,
        };
        assert_eq!(satisfied_offers(&[o], None, Some((7, 2))), vec![true]);
        assert_eq!(satisfied_offers(&[o], Some((7, 2)), None), vec![true]);
        // …and with BOTH filled a one-cost offer stops matching, because its
        // `buyB` must be empty.
        assert_eq!(satisfied_offers(&[o], Some((7, 2)), Some((9, 1))), vec![false]);
        // Nothing at all matches nothing.
        assert_eq!(satisfied_offers(&[o], None, None), vec![false]);
    }

    #[test]
    fn selecting_the_FIRST_trade_does_not_take_the_fast_path() {
        // `selectionHint > 0` is strictly greater, so hint 0 falls through to
        // the linear scan — and hint 0 means both "nothing selected" and "the
        // first trade", which vanilla cannot tell apart.
        let sat = [false, true, true];
        assert_eq!(recipe_for(0, &sat), Some(1), "the scan, not offer 0");
        assert_eq!(recipe_for(2, &sat), Some(2), "the hint, exactly");
        // A hint whose offer is NOT satisfied yields nothing rather than
        // falling back to the scan.
        assert_eq!(recipe_for(1, &[true, false, true]), None);
        // An out-of-range hint falls through to the scan.
        assert_eq!(recipe_for(99, &sat), Some(1));
    }

    #[test]
    fn exactly_seven_offers_do_not_scroll() {
        assert!(!can_scroll(7), "strictly greater");
        assert!(can_scroll(8));
        assert_eq!(max_scroll_off(8), 1);
        assert_eq!(max_scroll_off(30), 23);
    }

    #[test]
    fn one_wheel_notch_is_one_OFFER_not_a_fraction() {
        // The stonecutter's notch is a fraction of its range; this is not.
        assert_eq!(scroll_off_from_wheel(0, -1.0, 30), 1);
        assert_eq!(scroll_off_from_wheel(5, -1.0, 30), 6);
        assert_eq!(scroll_off_from_wheel(5, 1.0, 30), 4);
        // Clamped at both ends rather than wrapping.
        assert_eq!(scroll_off_from_wheel(0, 1.0, 30), 0);
        assert_eq!(scroll_off_from_wheel(23, -1.0, 30), 23);
        // `(int)` truncates toward zero, so a sub-notch trackpad delta does
        // nothing at all.
        assert_eq!(scroll_off_from_wheel(3, -0.4, 30), 3);
    }

    #[test]
    fn the_drag_rounds_the_offer_index_rather_than_flooring_it() {
        // 30 offers, max 23. The thumb's CENTRE tracks the cursor: 13.5 is half
        // of 27, and the divisor is 139 - 27 = 112.
        assert_eq!(scroll_off_from_drag(18.0 + 13.5, 30), 0, "the very top");
        assert_eq!(scroll_off_from_drag(18.0 + 13.5 + 112.0, 30), 23, "the bottom");
        // Halfway is 11.5 offers, and the `+ 0.5` then `(int)` rounds it to 12
        // where a floor would give 11.
        assert_eq!(scroll_off_from_drag(18.0 + 13.5 + 56.0, 30), 12);
        // Outside the track, the clamp is what stops it.
        assert_eq!(scroll_off_from_drag(0.0, 30), 0);
        assert_eq!(scroll_off_from_drag(999.0, 30), 23);
    }

    /// Which lists need the bottom override, computed rather than assumed.
    ///
    /// The first draft of this test asserted that the step "falls short" and
    /// used 30 offers to show it. At 30 the step OVERSHOOTS (138) and the
    /// `min(113, …)` is what caps it; the override is redundant there. It is
    /// the SHORT scrollable lists that fall short — the opposite of the
    /// intuition, and only visible by computing both regimes.
    #[test]
    fn the_bottom_override_is_load_bearing_for_SHORT_lists_and_redundant_for_long() {
        let last_computed = |offers: i32| {
            let steps = offers + 1 - TRADE_BUTTONS;
            let left_over = SCROLL_HEIGHT - (SCROLLER_H + (steps - 1) * SCROLL_HEIGHT / steps);
            let step = 1 + left_over / steps + SCROLL_HEIGHT / steps;
            (steps, (steps - 1) * step)
        };
        // Short: the arithmetic never reaches the end, so without the override
        // the thumb would stop 22 pixels up.
        for (offers, want) in [(8, 91), (9, 106), (10, 111)] {
            let (steps, last) = last_computed(offers);
            assert_eq!(last, want, "{offers} offers");
            assert!(last < 113, "{offers} offers falls short");
            assert_eq!(
                scroller_y(steps - 1, offers as usize),
                Some(SCROLL_TOP + 113),
                "{offers} offers still reaches the bottom"
            );
        }
        // Long: it overshoots and the `min` caps it, so the override changes
        // nothing.
        for offers in [12, 30, 40] {
            let (steps, last) = last_computed(offers);
            assert!(last > 113, "{offers} offers overshoots ({last})");
            assert_eq!(scroller_y(steps - 1, offers as usize), Some(SCROLL_TOP + 113));
        }
        assert_eq!(scroller_y(0, 30), Some(SCROLL_TOP), "and the top is the top");
        // 113 is 139 - 27 + 1 — one PAST the track less the thumb.
        assert_eq!(113, SCROLL_HEIGHT - SCROLLER_H + 1);
    }

    #[test]
    fn the_thumb_appears_exactly_when_the_list_scrolls() {
        // `steps > 1` is `size + 1 - 7 > 1`, i.e. `size > 7` — the SAME
        // threshold as `can_scroll`, reached by different arithmetic. The two
        // coinciding is worth pinning precisely because `steps` is one more
        // than `max_scroll_off` and the pair look interchangeable.
        for n in 0..12usize {
            assert_eq!(
                scroller_y(0, n).is_some(),
                can_scroll(n),
                "{n} offers: thumb and scrollability must agree"
            );
        }
        assert_eq!(scroller_y(0, 7), None);
        assert!(scroller_y(0, 8).is_some(), "steps is 2 at EIGHT, not nine");
        assert_eq!(max_scroll_off(8), 1, "…while max_scroll_off is 1 there");
    }

    #[test]
    fn the_grab_box_is_off_by_one_on_two_sides() {
        // x is STRICTLY greater than 94, so the leftmost column misses…
        assert!(!scroller_grabbed(94.0, 50.0));
        assert!(scroller_grabbed(94.5, 50.0));
        assert!(!scroller_grabbed(100.0, 50.0), "and 94 + 6 is excluded");
        // …and y runs to `18 + 139 + 1` INCLUSIVE.
        assert!(!scroller_grabbed(95.0, 18.0), "the top is exclusive");
        assert!(scroller_grabbed(95.0, 18.5));
        assert!(scroller_grabbed(95.0, 158.0), "18 + 139 + 1");
        assert!(!scroller_grabbed(95.0, 158.5));
    }

    #[test]
    fn the_items_sit_one_pixel_above_their_button() {
        // `offerY = yo + 16 + 1` against `init`'s `buttonY = yo + 16 + 2`.
        assert_eq!(button_y(0), 18);
        assert_eq!(row_item_y(0), 19, "17 + 2, not 18 + 2");
        assert_eq!(row_item_y(1), 39);
        assert_eq!(row_item_y(0), button_y(0) + 1);
        // …and cost A adds the button's 5 TWICE.
        assert_eq!(COST_A_X, 10, "xo + 5 + 5");
        assert_eq!(COST_B_X, 40);
        assert_eq!(RESULT_X, 73);
    }

    #[test]
    fn a_button_press_names_an_absolute_offer() {
        assert_eq!(button_at(5.0, 18.0), Some(0));
        assert_eq!(button_at(5.0, 37.9), Some(0), "20 tall");
        assert_eq!(button_at(5.0, 38.0), Some(1));
        assert_eq!(button_at(5.0, 18.0 + 6.0 * 20.0), Some(6), "the last of seven");
        assert_eq!(button_at(5.0, 18.0 + 7.0 * 20.0), None);
        assert_eq!(button_at(4.9, 18.0), None);
        assert_eq!(button_at(93.0, 18.0), None, "5 + 88");
        // The button's index is its POSITION; the packet carries the offer's.
        assert_eq!(offer_for_button(0, 0), 0);
        assert_eq!(offer_for_button(0, 5), 5);
        assert_eq!(offer_for_button(6, 5), 11);
    }

    #[test]
    fn a_short_list_ignores_the_scroll_entirely() {
        // The guard short-circuits on `!canScroll`, so a stale scrollOff left
        // by a longer list cannot hide a short one.
        for i in 0..5 {
            assert!(offer_visible(i, 99, 5), "offer {i} with a stale scroll");
        }
        // With a long list the window is real.
        assert!(!offer_visible(4, 5, 30));
        assert!(offer_visible(5, 5, 30));
        assert!(offer_visible(11, 5, 30));
        assert!(!offer_visible(12, 5, 30));
    }
}
