//! `StackedContents` — vanilla's "can I make this from what I hold" solver
//! (M96).
//!
//! Named as a blocker by M35, M94 and M95: the recipe book greys a recipe you
//! cannot afford, and every slot Rewo drew wore the *uncraftable* chrome
//! because nothing could answer the question. This answers it.
//!
//! # It is a bipartite matching, not a subtraction
//!
//! The obvious implementation — walk the ingredients and decrement a count per
//! item — is wrong whenever two ingredients can be satisfied by overlapping
//! sets. A recipe wanting one `#planks` and one `oak_planks`, held against a
//! single stack of oak planks and a stack of birch, is craftable only if the
//! `#planks` slot takes the birch; a greedy walk that spends the oak on
//! `#planks` first reports it uncraftable. Vanilla models it as a matching
//! between ingredient slots and *distinct item types*, and finds it with
//! augmenting paths — `RecipePicker`, transcribed below.
//!
//! # The whole state is one bit vector
//!
//! Five regions in a fixed order — visited-ingredient, visited-item,
//! satisfied, connection, residual — laid out by the `*_offset`/`*_count`
//! helpers. The two `ingredientCount * itemCount` regions are indexed
//! `item * ingredientCount + ingredient`, which reads backwards from their
//! names (`getConnectionIndex(item, ingredient)`) and is the one place a
//! transposition would compile and silently answer for a different pair.

/// One ingredient slot: the set of item ids that satisfy it.
///
/// Vanilla's `IngredientInfo<T>` is a predicate, and a `HolderSet` behind it.
/// A set of ids is the same thing for a client, and it makes the solver a pure
/// function of numbers — which is what lets its tests state a matching problem
/// rather than a Minecraft one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ingredient {
    pub accepts: Vec<i32>,
}

impl Ingredient {
    pub fn of(ids: &[i32]) -> Self {
        Self { accepts: ids.to_vec() }
    }

    fn accepts_item(&self, item: i32) -> bool {
        self.accepts.contains(&item)
    }
}

/// What the player is holding, as counts per item type.
///
/// Insertion-ordered, unlike vanilla's `Reference2IntOpenHashMap`. **That is a
/// deliberate strengthening, not a transcription error**: the map's iteration
/// order decides the *item indices*, and so which augmenting path is found
/// first — but a maximum matching has a unique *size*, so the boolean answer is
/// order-independent either way. Insertion order makes Rewo's answer
/// reproducible run to run, where vanilla's chosen items are not.
#[derive(Debug, Clone, Default)]
pub struct StackedContents {
    /// `(item id, count)`, first-seen order.
    amounts: Vec<(i32, i32)>,
}

impl StackedContents {
    pub fn new() -> Self {
        Self::default()
    }

    /// `account(item, count)` — `addTo`, so repeated calls accumulate.
    pub fn account(&mut self, item: i32, count: i32) {
        match self.amounts.iter_mut().find(|(i, _)| *i == item) {
            Some((_, c)) => *c += count,
            None => self.amounts.push((item, count)),
        }
    }

    /// `accountStack(stack, maxCount)` — **`min(maxCount, count)`**, so a stack
    /// holding more than its own maximum (which a command can produce) counts
    /// only its maximum.
    pub fn account_stack(&mut self, item: i32, count: i32, max_stack_size: i32) {
        if count <= 0 {
            return;
        }
        self.account(item, count.min(max_stack_size));
    }

    pub fn clear(&mut self) {
        self.amounts.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.amounts.is_empty()
    }

    fn get(&self, item: i32) -> i32 {
        self.amounts
            .iter()
            .find(|(i, _)| *i == item)
            .map_or(0, |(_, c)| *c)
    }

    fn has_at_least(&self, item: i32, count: i32) -> bool {
        self.get(item) >= count
    }

    fn add_to(&mut self, item: i32, delta: i32) {
        match self.amounts.iter_mut().find(|(i, _)| *i == item) {
            Some((_, c)) => *c += delta,
            None => self.amounts.push((item, delta)),
        }
    }

    /// `tryPick(ingredients, amount, output)`.
    ///
    /// Returns whether every ingredient can be satisfied at once, `amount`
    /// of each. **The contents are restored before it returns** — the picker
    /// takes and puts back — so this is a query, not a consumption.
    pub fn try_pick(&mut self, ingredients: &[Ingredient], amount: i32) -> bool {
        RecipePicker::new(self, ingredients).try_pick(self, amount)
    }

    /// `getResultUpperBound` — the largest batch any single ingredient could
    /// possibly support, which bounds the binary search in [`try_pick_all`].
    fn result_upper_bound(&self, ingredients: &[Ingredient]) -> i32 {
        let mut min = i32::MAX;
        'outer: for ing in ingredients {
            let mut max = 0;
            for &(item, count) in &self.amounts {
                if count > max {
                    if ing.accepts_item(item) {
                        max = count;
                    }
                    // Vanilla's `continue label31` — this early exit is inside
                    // the `count > max` guard, so it is reached only after a
                    // candidate has been considered.
                    if max >= min {
                        continue 'outer;
                    }
                }
            }
            min = max;
            if min == 0 {
                break;
            }
        }
        min
    }

    /// `tryPickAll(maxSize)` — the largest batch craftable, by binary search
    /// over [`try_pick`].
    ///
    /// The search is vanilla's exactly, including that `max` starts at the
    /// upper bound **plus one** and that the loop exits on `max - min <= 1`
    /// *after* a successful pick rather than before.
    pub fn try_pick_all(&mut self, ingredients: &[Ingredient], max_size: i32) -> i32 {
        let mut min = 0;
        let mut max = max_size.min(self.result_upper_bound(ingredients)).saturating_add(1);
        loop {
            let mid = (min + max) / 2;
            if self.try_pick(ingredients, mid) {
                if max - min <= 1 {
                    return mid;
                }
                min = mid;
            } else {
                max = mid;
            }
        }
    }
}

/// `RecipePicker` — the augmenting-path matcher.
struct RecipePicker {
    ingredient_count: usize,
    /// `getUniqueAvailableIngredientItems`: the item types the player holds
    /// that **some** ingredient accepts. Anything else cannot participate, and
    /// leaving it out is what keeps the matrix small.
    items: Vec<i32>,
    item_count: usize,
    data: Vec<bool>,
    path: Vec<usize>,
}

impl RecipePicker {
    fn new(contents: &StackedContents, ingredients: &[Ingredient]) -> Self {
        let items: Vec<i32> = contents
            .amounts
            .iter()
            .filter(|(item, count)| {
                *count > 0 && ingredients.iter().any(|i| i.accepts_item(*item))
            })
            .map(|(item, _)| *item)
            .collect();
        let ingredient_count = ingredients.len();
        let item_count = items.len();
        let mut p = Self {
            ingredient_count,
            items,
            item_count,
            data: vec![false; ingredient_count * 2 + item_count + ingredient_count * item_count * 2],
            path: Vec::new(),
        };
        // `setInitialConnections`.
        for (ingredient, info) in ingredients.iter().enumerate() {
            for item in 0..p.item_count {
                if info.accepts_item(p.items[item]) {
                    let i = p.connection_index(item, ingredient);
                    p.data[i] = true;
                }
            }
        }
        p
    }

    // -- the bit layout, in vanilla's order ---------------------------------
    fn visited_ingredient_offset(&self) -> usize {
        0
    }
    fn visited_item_offset(&self) -> usize {
        self.ingredient_count
    }
    fn satisfied_offset(&self) -> usize {
        self.visited_item_offset() + self.item_count
    }
    fn connection_offset(&self) -> usize {
        self.satisfied_offset() + self.ingredient_count
    }
    fn residual_offset(&self) -> usize {
        self.connection_offset() + self.ingredient_count * self.item_count
    }

    /// **`item * ingredientCount + ingredient`** — which reads backwards from
    /// the argument order and is the one index a transposition would get
    /// wrong silently, answering for a different (item, ingredient) pair.
    fn connection_index(&self, item: usize, ingredient: usize) -> usize {
        self.connection_offset() + item * self.ingredient_count + ingredient
    }
    fn residual_index(&self, item: usize, ingredient: usize) -> usize {
        self.residual_offset() + item * self.ingredient_count + ingredient
    }

    fn has_connection(&self, item: usize, ingredient: usize) -> bool {
        self.data[self.connection_index(item, ingredient)]
    }
    fn is_assigned(&self, item: usize, ingredient: usize) -> bool {
        self.data[self.residual_index(item, ingredient)]
    }
    fn assign(&mut self, item: usize, ingredient: usize) {
        let i = self.residual_index(item, ingredient);
        self.data[i] = true;
    }
    fn unassign(&mut self, item: usize, ingredient: usize) {
        let i = self.residual_index(item, ingredient);
        self.data[i] = false;
    }
    fn is_satisfied(&self, ingredient: usize) -> bool {
        self.data[self.satisfied_offset() + ingredient]
    }
    fn set_satisfied(&mut self, ingredient: usize) {
        let i = self.satisfied_offset() + ingredient;
        self.data[i] = true;
    }
    fn clear_satisfied(&mut self) {
        let (o, n) = (self.satisfied_offset(), self.ingredient_count);
        self.data[o..o + n].fill(false);
    }
    fn visit_ingredient(&mut self, ingredient: usize) {
        let i = self.visited_ingredient_offset() + ingredient;
        self.data[i] = true;
    }
    fn has_visited_ingredient(&self, ingredient: usize) -> bool {
        self.data[self.visited_ingredient_offset() + ingredient]
    }
    fn visit_item(&mut self, item: usize) {
        let i = self.visited_item_offset() + item;
        self.data[i] = true;
    }
    fn has_visited_item(&self, item: usize) -> bool {
        self.data[self.visited_item_offset() + item]
    }
    fn clear_all_visited(&mut self) {
        let n = self.ingredient_count + self.item_count;
        self.data[0..n].fill(false);
    }

    /// `isPathIndexItem` — the path alternates item, ingredient, item, …, so
    /// an EVEN index holds an item.
    fn is_path_index_item(index: usize) -> bool {
        index % 2 == 0
    }

    fn try_pick(&mut self, contents: &mut StackedContents, capacity: i32) -> bool {
        // A zero-or-negative batch is trivially craftable — and `try_pick_all`
        // relies on it, because its binary search starts at 0.
        if capacity <= 0 {
            return true;
        }
        let mut satisfied_ingredient_count = 0usize;
        loop {
            let Some(path) = self.try_assigning_new_item(contents, capacity) else {
                let is_valid = satisfied_ingredient_count == self.ingredient_count;
                self.clear_all_visited();
                self.clear_satisfied();
                // Put back everything taken. `break` after the first assigned
                // item for an ingredient: an ingredient holds at most one
                // assignment, and the loop is over ITEMS for a fixed
                // ingredient.
                for ingredient in 0..self.ingredient_count {
                    for item in 0..self.item_count {
                        if self.is_assigned(item, ingredient) {
                            self.unassign(item, ingredient);
                            contents.add_to(self.items[item], capacity);
                            break;
                        }
                    }
                }
                return is_valid;
            };
            let assigned_item = path[0];
            contents.add_to(self.items[assigned_item], -capacity);
            let last = path[path.len() - 1];
            self.set_satisfied(last);
            satisfied_ingredient_count += 1;
            for i in 0..path.len() - 1 {
                if Self::is_path_index_item(i) {
                    self.assign(path[i], path[i + 1]);
                } else {
                    self.unassign(path[i + 1], path[i]);
                }
            }
        }
    }

    fn try_assigning_new_item(
        &mut self,
        contents: &StackedContents,
        capacity: i32,
    ) -> Option<Vec<usize>> {
        self.clear_all_visited();
        for item in 0..self.item_count {
            if contents.has_at_least(self.items[item], capacity) {
                if let Some(p) = self.find_new_item_assignment_path(item) {
                    return Some(p);
                }
            }
        }
        None
    }

    fn find_new_item_assignment_path(&mut self, starting_item: usize) -> Option<Vec<usize>> {
        self.path.clear();
        self.visit_item(starting_item);
        self.path.push(starting_item);
        while !self.path.is_empty() {
            let path_length = self.path.len();
            if Self::is_path_index_item(path_length - 1) {
                let item_to_assign = self.path[path_length - 1];
                for ingredient in 0..self.ingredient_count {
                    if !self.has_visited_ingredient(ingredient)
                        && self.has_connection(item_to_assign, ingredient)
                        && !self.is_assigned(item_to_assign, ingredient)
                    {
                        self.visit_ingredient(ingredient);
                        self.path.push(ingredient);
                        break;
                    }
                }
            } else {
                let last = self.path[path_length - 1];
                // An UNSATISFIED ingredient ends the path: that is the
                // augmenting path's far end. A satisfied one has to be bumped,
                // which is what the item loop below starts.
                if !self.is_satisfied(last) {
                    return Some(self.path.clone());
                }
                for item in 0..self.item_count {
                    if !self.has_visited_item(item) && self.is_assigned(item, last) {
                        self.visit_item(item);
                        self.path.push(item);
                        break;
                    }
                }
            }
            // Nothing was pushed, so this end is exhausted — back up.
            if self.path.len() == path_length {
                self.path.pop();
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contents(items: &[(i32, i32)]) -> StackedContents {
        let mut c = StackedContents::new();
        for &(item, count) in items {
            c.account(item, count);
        }
        c
    }

    #[test]
    fn nothing_held_makes_nothing() {
        let mut c = StackedContents::new();
        assert!(!c.try_pick(&[Ingredient::of(&[1])], 1));
    }

    #[test]
    fn a_single_ingredient_is_satisfied_by_a_single_item() {
        let mut c = contents(&[(1, 1)]);
        assert!(c.try_pick(&[Ingredient::of(&[1])], 1));
        assert!(!c.try_pick(&[Ingredient::of(&[2])], 1), "the wrong item");
    }

    /// The query RESTORES what it took, so asking twice gives the same answer.
    /// A solver that consumed would report the second call uncraftable.
    #[test]
    fn asking_does_not_consume() {
        let mut c = contents(&[(1, 1)]);
        let ing = [Ingredient::of(&[1])];
        assert!(c.try_pick(&ing, 1));
        assert!(c.try_pick(&ing, 1), "and again");
        assert_eq!(c.get(1), 1, "the count is untouched");
    }

    /// One item type serves SEVERAL slots, as long as the count holds.
    ///
    /// This witness first asserted the opposite — that a type could satisfy
    /// one slot only — and the code was right: `try_pick` loops, `take`ing
    /// `capacity` per satisfied ingredient, and `hasAtLeast` re-reads the
    /// decremented amount. The matching is over (item, ingredient) PAIRS, so
    /// the same type may be assigned to many ingredients; what runs out is the
    /// count, not the type. A model in which it could not would report a stack
    /// of dirt unable to fill a nine-slot recipe.
    #[test]
    fn one_item_type_serves_as_many_slots_as_its_count_allows() {
        let ing = [Ingredient::of(&[1]), Ingredient::of(&[1])];
        assert!(contents(&[(1, 5)]).try_pick(&ing, 1));
        assert!(contents(&[(1, 2)]).try_pick(&ing, 1), "exactly enough");
        assert!(!contents(&[(1, 1)]).try_pick(&ing, 1), "one short");
    }

    /// The case a greedy subtraction gets wrong: overlapping ingredient sets
    /// where the assignment has to be chosen, not taken in order.
    #[test]
    fn overlapping_ingredients_need_a_real_matching() {
        // Ingredient 0 takes oak(1) or birch(2); ingredient 1 takes oak only.
        let ing = [Ingredient::of(&[1, 2]), Ingredient::of(&[1])];
        // Holding one of each, the only assignment is birch->0, oak->1. A
        // greedy walk spends oak on ingredient 0 and then fails.
        assert!(contents(&[(1, 1), (2, 1)]).try_pick(&ing, 1));
        // Holding two birch and no oak, ingredient 1 cannot be satisfied.
        assert!(!contents(&[(2, 2)]).try_pick(&ing, 1));
    }

    /// And the same case with the held items in the OTHER order, because the
    /// item indices come from insertion order and a matcher that only worked
    /// for one of the two would be a fluke.
    #[test]
    fn the_matching_does_not_depend_on_the_order_items_were_added() {
        let ing = [Ingredient::of(&[1, 2]), Ingredient::of(&[1])];
        assert!(contents(&[(1, 1), (2, 1)]).try_pick(&ing, 1));
        assert!(contents(&[(2, 1), (1, 1)]).try_pick(&ing, 1));
    }

    /// A chain that needs the augmenting path to BUMP an earlier assignment:
    /// ingredient 0 accepts everything, 1 accepts {1,2}, 2 accepts {1}. The
    /// only assignment is 3->0, 2->1, 1->2, which a first-fit never finds.
    #[test]
    fn an_augmenting_path_reassigns_what_it_has_to() {
        let ing = [
            Ingredient::of(&[1, 2, 3]),
            Ingredient::of(&[1, 2]),
            Ingredient::of(&[1]),
        ];
        assert!(contents(&[(1, 1), (2, 1), (3, 1)]).try_pick(&ing, 1));
        // Drop the item only ingredient 0 can use and it becomes impossible.
        assert!(!contents(&[(1, 1), (2, 1)]).try_pick(&ing, 1));
    }

    /// An ingredient that accepts nothing at all can never be satisfied.
    #[test]
    fn an_empty_ingredient_is_impossible() {
        let mut c = contents(&[(1, 64)]);
        assert!(!c.try_pick(&[Ingredient::default()], 1));
    }

    /// A zero-or-negative batch is trivially true — and `try_pick_all`'s
    /// binary search starts at 0, so this is load-bearing rather than a
    /// curiosity.
    #[test]
    fn a_batch_of_zero_is_always_craftable() {
        let mut c = StackedContents::new();
        assert!(c.try_pick(&[Ingredient::of(&[1])], 0));
        assert!(c.try_pick(&[Ingredient::of(&[1])], -1));
    }

    #[test]
    fn a_bigger_batch_needs_proportionally_more() {
        let ing = [Ingredient::of(&[1])];
        let mut c = contents(&[(1, 5)]);
        assert!(c.try_pick(&ing, 5));
        assert!(!c.try_pick(&ing, 6));
    }

    #[test]
    fn try_pick_all_finds_the_biggest_batch() {
        let ing = [Ingredient::of(&[1]), Ingredient::of(&[2])];
        assert_eq!(contents(&[(1, 5), (2, 9)]).try_pick_all(&ing, 64), 5);
        assert_eq!(contents(&[(1, 5), (2, 9)]).try_pick_all(&ing, 3), 3, "capped");
        assert_eq!(contents(&[(1, 0), (2, 9)]).try_pick_all(&ing, 64), 0);
    }

    /// `min(maxCount, count)` — a stack holding more than its own maximum
    /// counts only its maximum.
    #[test]
    fn a_stack_counts_at_most_its_own_maximum() {
        let mut c = StackedContents::new();
        c.account_stack(1, 100, 64);
        assert_eq!(c.get(1), 64);
        c.clear();
        c.account_stack(1, 3, 64);
        assert_eq!(c.get(1), 3);
        c.clear();
        // An empty stack accounts nothing at all.
        c.account_stack(1, 0, 64);
        assert!(c.is_empty());
    }

    #[test]
    fn accounting_the_same_item_twice_adds_up() {
        let mut c = StackedContents::new();
        c.account_stack(1, 30, 64);
        c.account_stack(1, 30, 64);
        assert_eq!(c.get(1), 60);
        assert!(c.try_pick(&[Ingredient::of(&[1])], 60));
    }

    /// An item held with a count of zero cannot participate.
    ///
    /// The filter that drops it is an OPTIMISATION, not a correctness rule, and
    /// a mutation removing it survives: a zero-count item enters the matrix but
    /// `hasAtLeast` refuses it for every positive capacity, so the answer is
    /// unchanged and only the matrix is bigger. Written down because "this
    /// guard is load-bearing" was the natural assumption and it is false.
    #[test]
    fn a_zero_count_item_is_inert_rather_than_excluded() {
        let mut c = StackedContents::new();
        c.account(1, 0);
        c.account(2, 1);
        assert!(!c.try_pick(&[Ingredient::of(&[1])], 1));
        assert!(c.try_pick(&[Ingredient::of(&[2])], 1));
    }

    /// A brute-force reference: can every ingredient be assigned an item type
    /// it accepts, such that no type is used for more slots than its count
    /// allows?
    ///
    /// This is the whole model stated independently — an exhaustive search over
    /// assignments rather than an augmenting-path matcher — so it shares no
    /// code, no order and no bit layout with the thing it grades.
    fn brute_force(items: &[(i32, i32)], ingredients: &[Ingredient], capacity: i32) -> bool {
        if capacity <= 0 {
            return true;
        }
        fn go(items: &[(i32, i32)], ings: &[Ingredient], cap: i32, i: usize, used: &mut Vec<i32>) -> bool {
            if i == ings.len() {
                return true;
            }
            for (k, &(item, count)) in items.iter().enumerate() {
                if ings[i].accepts_item(item) && used[k] + cap <= count {
                    used[k] += cap;
                    if go(items, ings, cap, i + 1, used) {
                        used[k] -= cap;
                        return true;
                    }
                    used[k] -= cap;
                }
            }
            false
        }
        go(items, ingredients, capacity, 0, &mut vec![0; items.len()])
    }

    /// Every small problem, graded against the reference.
    ///
    /// Three item types with counts 0..=2, three ingredient slots each
    /// accepting any subset of the three types: 27 x 343 = 9,261 problems,
    /// exhaustively. Two mutations survived the hand-written fixtures above -
    /// dropping the zero-count filter and dropping the already-assigned guard -
    /// and this is what settles whether either is equivalent, rather than an
    /// argument about it.
    #[test]
    fn the_matcher_agrees_with_brute_force_on_every_small_problem() {
        let subsets: Vec<Ingredient> = (0..8)
            .map(|m: u32| {
                Ingredient::of(
                    &(0..3)
                        .filter(|b| m >> b & 1 == 1)
                        .map(|b| b as i32 + 1)
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let mut checked = 0u32;
        for counts in 0..27u32 {
            let items: Vec<(i32, i32)> = (0..3)
                .map(|k| (k as i32 + 1, (counts / 3u32.pow(k)) as i32 % 3))
                .collect();
            for a in 0..8usize {
                for b in 0..8usize {
                    for d in 0..8usize {
                        let ings = [subsets[a].clone(), subsets[b].clone(), subsets[d].clone()];
                        for capacity in 1..=2 {
                            let mut c = StackedContents::new();
                            for &(item, count) in &items {
                                c.account(item, count);
                            }
                            let got = c.try_pick(&ings, capacity);
                            let want = brute_force(&items, &ings, capacity);
                            assert_eq!(
                                got, want,
                                "items {items:?} ingredients {ings:?} capacity {capacity}"
                            );
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(checked, 27 * 8 * 8 * 8 * 2);
    }

    /// A nine-slot recipe against one stack — the shape a crafting table
    /// actually produces, and the sanity check on the rule above.
    #[test]
    fn a_stack_of_sixty_four_fills_a_nine_slot_recipe() {
        let ing: Vec<_> = (0..9).map(|_| Ingredient::of(&[1])).collect();
        assert!(contents(&[(1, 64)]).try_pick(&ing, 1));
        assert!(contents(&[(1, 9)]).try_pick(&ing, 1), "exactly nine");
        assert!(!contents(&[(1, 8)]).try_pick(&ing, 1), "eight is one short");
        // ...and a batch of 7 needs 63.
        assert!(contents(&[(1, 63)]).try_pick(&ing, 7));
        assert!(!contents(&[(1, 62)]).try_pick(&ing, 7));
    }
}
