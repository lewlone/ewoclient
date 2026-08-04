//! The tooltip's **second pass** — vanilla's image components (M56).
//!
//! M40 shipped the first: `GuiGraphicsExtractor.tooltip` measures its
//! components, places the box, and walks the list drawing text. What it did not
//! ship is that the method walks the list **twice**:
//!
//! ```java
//! int localY = y;
//! for (int i = 0; i < lines.size(); i++) {
//!    lines.get(i).extractText(this, font, x, localY);
//!    localY += lines.get(i).getHeight(font) + (i == 0 ? 2 : 0);
//! }
//!
//! localY = y;                                   // <- the restart
//!
//! for (int i = 0; i < lines.size(); i++) {
//!    lines.get(i).extractImage(font, x, localY, w, h, this);
//!    localY += lines.get(i).getHeight(font) + (i == 0 ? 2 : 0);
//! }
//! ```
//!
//! The `localY = y` between them is the load-bearing line. The two loops
//! advance identically, so a component's image lands at exactly the y its text
//! would have — the split is a *layering* device (every image draws over every
//! text line, whatever order the components are in), not a layout one. Run it
//! as one continuous cursor and every image drops below the box by the height
//! of all the text, which is the mutation
//! [`continuous_cursor_end`] exists to name.
//!
//! # What a component is
//!
//! `ClientTooltipComponent.create(TooltipComponent)` is a three-arm switch and
//! the third arm **throws**:
//!
//! ```java
//! case BundleTooltip bundleTooltip -> new ClientBundleTooltip(bundleTooltip.contents());
//! case ClientActivePlayersTooltip.ActivePlayersTooltip t -> new ClientActivePlayersTooltip(t);
//! default -> throw new IllegalArgumentException("Unknown TooltipComponent");
//! ```
//!
//! and `BundleItem.getTooltipImage` is the only `getTooltipImage` override in
//! the tree — `Item.getTooltipImage` returns `Optional.empty()` and nothing
//! else overrides it. So the only image an *item* tooltip can carry is a
//! bundle's grid; the active-players component comes from the server list, not
//! from a stack. That is why this module models exactly two kinds.
//!
//! # Sizing
//!
//! `getWidth`/`getHeight` are polymorphic and the measure loop runs over every
//! component regardless of kind, so an image contributes its own 96 to the
//! width and its own grid height to the box with no special case. The one
//! thing that *is* special: `lines.size() == 1 ? -2 : 0` counts **components**,
//! not text lines, so giving a one-line tooltip an image does not merely add
//! the grid's height — it also gives back the two pixels the single-component
//! case subtracts.

/// `ClientTextTooltip.getHeight` — one line of tooltip text.
pub const LINE_HEIGHT: i32 = 10;
/// The gap `localY += … + (i == 0 ? 2 : 0)` opens after the **first**
/// component only, which is what separates a stack's name from its details.
pub const FIRST_GAP: i32 = 2;

// -- `ClientBundleTooltip`'s constants, verbatim ------------------------------

/// The inset of the item inside its 24 px cell — `graphics.item(item,
/// drawX + 4, drawY + 4, …)`, so the 16 px icon is centred.
pub const SLOT_MARGIN: i32 = 4;
/// One grid cell, which is *not* the 18 px an inventory slot uses.
pub const SLOT_SIZE: i32 = 24;
/// `GRID_WIDTH`, and also `ClientBundleTooltip.getWidth`'s fixed return, and
/// also `PROGRESSBAR_WIDTH`. Four 24 px columns.
pub const GRID_WIDTH: i32 = 96;
/// The grid is always four columns; the row count is what varies.
pub const GRID_COLUMNS: i32 = 4;
/// `slotCount` is `min(12, size)` — twelve cells is the whole grid.
pub const MAX_SLOTS: i32 = 12;
pub const PROGRESSBAR_HEIGHT: i32 = 13;
pub const PROGRESSBAR_BORDER: i32 = 1;
pub const PROGRESSBAR_FILL_MAX: i32 = 94;
pub const PROGRESSBAR_MARGIN_Y: i32 = 4;

/// `Mth.positiveCeilDiv(input, divisor)` — `-Math.floorDiv(-input, divisor)`.
///
/// Not `(a + b - 1) / b`: that form differs for a negative input, and while a
/// slot count is never negative, the transcription is free.
fn positive_ceil_div(input: i32, divisor: i32) -> i32 {
    -(-input).div_euclid(divisor)
}

/// `org.apache.commons.lang3.math.Fraction`, reduced to what the progress bar
/// asks of it — a comparison against 0 and 1, and one truncating multiply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fraction {
    pub numerator: i32,
    pub denominator: i32,
}

impl Fraction {
    pub const ZERO: Fraction = Fraction {
        numerator: 0,
        denominator: 1,
    };
    pub const ONE: Fraction = Fraction {
        numerator: 1,
        denominator: 1,
    };

    pub fn new(numerator: i32, denominator: i32) -> Fraction {
        Fraction {
            numerator,
            denominator: if denominator == 0 { 1 } else { denominator },
        }
    }

    /// Cross-multiplied, like `Fraction.compareTo`. Widened to `i64` here:
    /// vanilla's is `int` and can overflow, but a bundle's weight never gets
    /// near that and an overflow would be a silent wrong answer rather than a
    /// faithful one.
    fn cmp_to(self, other: Fraction) -> std::cmp::Ordering {
        (self.numerator as i64 * other.denominator as i64)
            .cmp(&(other.numerator as i64 * self.denominator as i64))
    }

    /// `Mth.mulAndTruncate(fraction, factor)` —
    /// `fraction.getNumerator() * factor / fraction.getDenominator()`, with
    /// Java's truncation toward zero.
    fn mul_and_truncate(self, factor: i32) -> i32 {
        (self.numerator as i64 * factor as i64 / self.denominator as i64) as i32
    }
}

/// The parts of `BundleContents` that `ClientBundleTooltip` reads.
///
/// Held as counts rather than stacks because every number the grid needs is a
/// count: `getAmountOfHiddenItems` sums the hidden stacks' `count()`, and
/// everything else is `size()`.
///
/// Deliberately not `Default`: `selected` would fall to 0, which is a real
/// index, and a bundle that quietly claims its first stack is highlighted is
/// exactly the sort of wrong-by-omission this crate keeps failing loud on.
#[derive(Clone, Debug)]
pub struct Bundle {
    /// One entry per stack in `contents.items()`, holding its `count()`.
    pub counts: Vec<i32>,
    /// `getSelectedItemIndex`, `-1` for none.
    pub selected: i32,
    /// `contents.weight()`, already unwrapped. An *errored* weight
    /// (`"Excessive total bundle weight"`) draws nothing at all — see
    /// [`Bundle::weight_ok`].
    pub weight: Fraction,
    /// `weight().isError()` inverted. `extractImage` returns early on an
    /// error, so an over-weight bundle contributes its size to the box and
    /// then draws none of it. That is vanilla, and it is why this is a flag
    /// rather than an `Option` folded into the height.
    pub weight_ok: bool,
    /// `font.split(BUNDLE_EMPTY_DESCRIPTION, 96).size()` — the wrapped line
    /// count of the empty bundle's blurb. Passed in because `rewo-gpu` holds
    /// no font: the caller splits the translated string, the same arrangement
    /// the sign text uses.
    pub empty_description_lines: i32,
}

impl Bundle {
    pub fn size(&self) -> i32 {
        self.counts.len() as i32
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// `slotCount` — `Math.min(12, contents.size())`.
    pub fn slot_count(&self) -> i32 {
        MAX_SLOTS.min(self.size())
    }

    /// `gridSizeY` — `Mth.positiveCeilDiv(slotCount(), 4)`.
    pub fn grid_size_y(&self) -> i32 {
        positive_ceil_div(self.slot_count(), GRID_COLUMNS)
    }

    /// `itemGridHeight` — `gridSizeY() * 24`.
    pub fn item_grid_height(&self) -> i32 {
        self.grid_size_y() * SLOT_SIZE
    }

    /// `isOverflowing` — `contents.size() > 12`, which is what puts the `+N`
    /// badge in the first cell. Note the boundary: exactly twelve stacks fill
    /// the grid and show no badge.
    pub fn is_overflowing(&self) -> bool {
        self.size() > MAX_SLOTS
    }

    /// `BundleContents.getNumberOfItemsToShow`, which is **not** `min(12,
    /// size)` and not `11` when overflowing:
    ///
    /// ```java
    /// int numberOfItemStacks = this.size();
    /// int availableItemsToShow = numberOfItemStacks > 12 ? 11 : 12;
    /// int itemsOnNonFullRow = numberOfItemStacks % 4;
    /// int emptySpaceOnNonFullRow = itemsOnNonFullRow == 0 ? 0 : 4 - itemsOnNonFullRow;
    /// return Math.min(numberOfItemStacks, availableItemsToShow - emptySpaceOnNonFullRow);
    /// ```
    ///
    /// The ragged-row subtraction is keyed off the **total** stack count, not
    /// off `availableItemsToShow`, so thirteen stacks show **eight** items and
    /// leave the grid's top row empty beside the badge. Reading it as "eleven
    /// items and a badge" fills three cells vanilla leaves blank.
    pub fn number_of_items_to_show(&self) -> i32 {
        let stacks = self.size();
        let available = if stacks > MAX_SLOTS { 11 } else { 12 };
        let on_non_full_row = stacks.rem_euclid(GRID_COLUMNS);
        let empty_space = if on_non_full_row == 0 {
            0
        } else {
            GRID_COLUMNS - on_non_full_row
        };
        stacks.min(available - empty_space)
    }

    /// `getShownItems(amountOfItemsToShow)` — the sublist's length.
    pub fn shown_items(&self) -> i32 {
        self.size().min(self.number_of_items_to_show()).max(0)
    }

    /// `getAmountOfHiddenItems` — the **sum of the hidden stacks' counts**,
    /// not the number of hidden stacks. Thirteen single-item stacks show eight
    /// and badge `+5`; thirteen stacks of 64 badge `+320`.
    pub fn hidden_item_count(&self) -> i32 {
        self.counts
            .iter()
            .skip(self.shown_items().max(0) as usize)
            .sum()
    }

    /// `getEmptyBundleDescriptionTextHeight` — `split(…, 96).size() * 9`.
    fn empty_description_height(&self) -> i32 {
        self.empty_description_lines * 9
    }

    /// `getHeight` — the grid or the blurb, then the bar and its two margins.
    fn height(&self) -> i32 {
        let content = if self.is_empty() {
            self.empty_description_height()
        } else {
            self.item_grid_height()
        };
        content + PROGRESSBAR_HEIGHT + 2 * PROGRESSBAR_MARGIN_Y
    }
}

/// One entry of the `List<ClientTooltipComponent>` the tooltip walks.
#[derive(Clone, Debug)]
pub enum Component {
    /// `ClientTextTooltip` — width is the font's, height is a flat 10.
    Text { width: i32 },
    /// `ClientBundleTooltip`.
    Bundle(Bundle),
}

impl Component {
    pub fn text(width: i32) -> Component {
        Component::Text { width }
    }

    /// `getWidth(font)`. The bundle's is a **fixed 96** — it does not measure
    /// its contents, so a bundle holding one item is as wide as one holding
    /// twelve.
    pub fn width(&self) -> i32 {
        match self {
            Component::Text { width } => *width,
            Component::Bundle(_) => GRID_WIDTH,
        }
    }

    /// `getHeight(font)`.
    pub fn height(&self) -> i32 {
        match self {
            Component::Text { .. } => LINE_HEIGHT,
            Component::Bundle(b) => b.height(),
        }
    }

    /// `showTooltipWithItemInHand` — `false` by default, and overridden to
    /// `true` by `ClientBundleTooltip` alone. `AbstractContainerScreen` asks
    /// it to decide whether a tooltip survives a stack on the cursor, which is
    /// why a bundle's grid still shows while you are carrying something.
    pub fn show_with_item_in_hand(&self) -> bool {
        matches!(self, Component::Bundle(_))
    }
}

// ── Styled tooltip lines (M52b) ───────────────────────────────────────────
//
// A tooltip line was `(String, [f32; 3])` -- one string, one colour, no
// italic. That is strictly *less* than vanilla, which styles per run: a line
// can change colour mid-way, and `ItemLore.LORE_STYLE` is
// `withColor(DARK_PURPLE).withItalic(true)`. Rewo carried the colour and
// silently dropped the italic, because it had nowhere to put it.
//
// So a line is a sequence of [`Span`]s. This type is deliberately
// **font-agnostic**: it says *what* is styled, not which typeface renders it,
// so the same line can go to the vanilla bitmap pass or through
// `velvet_glyph::StyledSpan` to the Velvet type stack. Changing which one a
// tooltip uses is then a rendering decision rather than a model change --
// which matters, because the HUD's visual direction is not settled and this
// model should outlive it.

/// One styled fragment of a tooltip line.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    /// Linear-ish sRGB triple, as the rest of the tooltip path uses.
    pub color: [f32; 3],
    /// Vanilla styles lore italic. Previously unrepresentable.
    pub italic: bool,
}

impl Span {
    pub fn new(text: impl Into<String>, color: [f32; 3]) -> Self {
        Self { text: text.into(), color, italic: false }
    }

    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }
}

/// One tooltip line: spans laid end to end on a shared baseline.
pub type Line = Vec<Span>;

/// The plain text of a line, spans concatenated.
///
/// The width path still measures this with the vanilla font's advances, so
/// tooltip *geometry* is unchanged by the span model -- the box is sized
/// exactly as it was. Only the styling is richer.
pub fn line_text(line: &Line) -> String {
    line.iter().map(|s| s.text.as_str()).collect()
}

/// Convert a styled line into Velvet [`StyledSpan`]s on one baseline.
///
/// Italic selects the **italic face**, not a skew: Velvet ships
/// `Newsreader-Italic.ttf` as its own file, and faking the slant would look
/// like a different typeface next to the real one.
///
/// [`StyledSpan`]: crate::velvet_glyph::StyledSpan
pub fn to_velvet_spans(
    line: &Line,
    size_px: f32,
) -> Vec<crate::velvet_glyph::StyledSpan> {
    use crate::velvet_glyph::{Axes, Family, ScalerKey, StyledSpan};
    line.iter()
        .map(|s| {
            let key = ScalerKey::new(Family::Newsreader, s.italic, size_px, Axes::DEFAULT);
            StyledSpan::new(s.text.clone(), key, s.color)
        })
        .collect()
}

// ── The advanced block (M66) ──────────────────────────────────────────────
//
// `Screen.getTooltipFromItem` picks the flag:
//
// ```java
// itemStack.getTooltipLines(
//    Item.TooltipContext.of(minecraft.level),
//    minecraft.player,
//    minecraft.options.advancedItemTooltips ? TooltipFlag.Default.ADVANCED
//                                           : TooltipFlag.Default.NORMAL);
// ```
//
// and `ItemStack.addDetailsToTooltip` ends with the block this models.

/// `TooltipFlag.Default` — two booleans, and only the first is reachable from
/// a key press.
///
/// `creative` is set by `asCreative()` for the creative inventory's own
/// tooltips; Rewo has no creative screen, so nothing constructs it and it is
/// here to keep the record straight rather than to be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooltipFlag {
    pub advanced: bool,
    pub creative: bool,
}

impl TooltipFlag {
    /// `TooltipFlag.NORMAL = new Default(false, false)`.
    pub const NORMAL: TooltipFlag = TooltipFlag {
        advanced: false,
        creative: false,
    };
    /// `TooltipFlag.ADVANCED = new Default(true, false)` — **advanced is not
    /// creative**. F3+H turns on the first alone.
    pub const ADVANCED: TooltipFlag = TooltipFlag {
        advanced: true,
        creative: false,
    };

    /// `options.advancedItemTooltips ? ADVANCED : NORMAL`.
    pub fn of(advanced_item_tooltips: bool) -> TooltipFlag {
        if advanced_item_tooltips {
            TooltipFlag::ADVANCED
        } else {
            TooltipFlag::NORMAL
        }
    }
}

/// `ChatFormatting.DARK_GRAY` — the two lines under the durability.
pub const DARK_GRAY: [f32; 3] = [
    0x55 as f32 / 255.0,
    0x55 as f32 / 255.0,
    0x55 as f32 / 255.0,
];

/// One line of the advanced block, before translation.
///
/// A description rather than a string, because `rewo-gpu` holds no language
/// file: the caller resolves `item.durability` and `item.components` and knows
/// the registry key. What is decided *here* is the order, the arguments and
/// which lines exist at all — the three things a transcription can get wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdvancedLine {
    /// `Component.translatable("item.durability", getMaxDamage() -
    /// getDamageValue(), getMaxDamage())` — **remaining first, then max**.
    ///
    /// `item.durability` is `"Durability: %s / %s"`, so passing the damage
    /// instead of the remaining reads as a nearly-full tool on a nearly-broken
    /// one and vice versa. The two arguments are also easy to swap and the
    /// result still looks like a durability line.
    Durability { remaining: i32, max: i32 },
    /// `Component.literal(BuiltInRegistries.ITEM.getKey(getItem()).toString())`
    /// in DARK_GRAY.
    ///
    /// A **literal**, not a translation key — `minecraft:diamond_sword` is
    /// printed verbatim. Running it through the language file would find
    /// nothing and (per `TranslatableContents`' fallback) print the key
    /// anyway, which is the same string by luck rather than by rule; a
    /// datapack that happened to define `minecraft:diamond_sword` as a key
    /// would show its value.
    RegistryId,
    /// `Component.translatable("item.components", count)` in DARK_GRAY,
    /// emitted only when `count > 0`.
    Components { count: i32 },
}

/// `ItemStack.addDetailsToTooltip`'s advanced block, verbatim:
///
/// ```java
/// if (tooltipFlag.isAdvanced()) {
///    if (this.isDamaged() && display.shows(DataComponents.DAMAGE)) {
///       builder.accept(Component.translatable("item.durability",
///           this.getMaxDamage() - this.getDamageValue(), this.getMaxDamage()));
///    }
///    builder.accept(Component.literal(BuiltInRegistries.ITEM.getKey(this.getItem()).toString())
///        .withStyle(ChatFormatting.DARK_GRAY));
///    int count = this.components.size();
///    if (count > 0) {
///       builder.accept(Component.translatable("item.components", count)
///           .withStyle(ChatFormatting.DARK_GRAY));
///    }
/// }
/// ```
///
/// Three things the shape says that a paraphrase loses. The registry id is
/// **unconditional** — every advanced tooltip has it, damaged or not, and it
/// is the one line that cannot be absent. `item.nbt_tags` is **gone**: 26.x
/// removed it when components replaced NBT, so a client still emitting it
/// prints a line vanilla does not. And `count` is
/// [`super::tooltip`]'s only input that is not on the wire — see
/// `StackComponents::component_count`, which is the merged map's size and not
/// the patch's.
///
/// `component_count` is `None` when this build cannot resolve the item's
/// prototype, in which case the line is dropped rather than guessed.
pub fn advanced_lines(
    flag: TooltipFlag,
    damaged: DurabilityState,
    shows_damage: bool,
    component_count: Option<i32>,
) -> Vec<AdvancedLine> {
    if !flag.advanced {
        return Vec::new();
    }
    let mut out = Vec::new();
    if damaged.is_damaged() && shows_damage {
        out.push(AdvancedLine::Durability {
            remaining: damaged.max - damaged.damage_value(),
            max: damaged.max,
        });
    }
    out.push(AdvancedLine::RegistryId);
    if let Some(count) = component_count {
        if count > 0 {
            out.push(AdvancedLine::Components { count });
        }
    }
    out
}

/// The three components `ItemStack.isDamaged()` reads, kept together because
/// two of them are easy to forget.
///
/// ```java
/// isDamageableItem() = has(MAX_DAMAGE) && !has(UNBREAKABLE) && has(DAMAGE)
/// isDamaged()        = isDamageableItem() && getDamageValue() > 0
/// getDamageValue()   = Mth.clamp(getOrDefault(DAMAGE, 0), 0, getMaxDamage())
/// ```
///
/// **An `Unbreakable` tool is never damaged**, however much damage it carries
/// — which is also what `isBarVisible()` reads, so it draws no durability bar
/// either. And the damage is **clamped** to the maximum, so a server sending a
/// value past it reports a full-max line rather than a negative remainder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DurabilityState {
    /// `getOrDefault(DAMAGE, 0)`, before the clamp.
    pub damage: i32,
    /// `getOrDefault(MAX_DAMAGE, 0)`.
    pub max: i32,
    /// `has(MAX_DAMAGE)` — the *prototype* counts, not just the patch.
    pub has_max_damage: bool,
    /// `has(DAMAGE)`. Also a prototype component on every damageable item.
    pub has_damage: bool,
    /// `has(UNBREAKABLE)`.
    pub unbreakable: bool,
}

impl DurabilityState {
    pub fn damage_value(&self) -> i32 {
        self.damage.clamp(0, self.max)
    }

    pub fn is_damageable_item(&self) -> bool {
        self.has_max_damage && !self.unbreakable && self.has_damage
    }

    /// `isDamaged()`, which is also `isBarVisible()`.
    pub fn is_damaged(&self) -> bool {
        self.is_damageable_item() && self.damage_value() > 0
    }
}

// ── The container's lines (M66) ───────────────────────────────────────────

/// How many `item.container.item_count` lines a container draws, and what the
/// trailing `item.container.more_items` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContainerPlan {
    /// How many of the present stacks get a line. At most five.
    pub shown: usize,
    /// `itemCount - lineCount`, the argument to `item.container.more_items`.
    /// Zero means **no** trailing line.
    pub more: i32,
}

/// `ItemContainerContents.addToTooltip`, verbatim:
///
/// ```java
/// int lineCount = 0;
/// int itemCount = 0;
/// for (Optional<ItemStackTemplate> item : this.items) {
///    if (!item.isEmpty()) {
///       itemCount++;
///       if (lineCount <= 4) {
///          lineCount++;
///          ItemStack itemStack = item.get().create();
///          consumer.accept(Component.translatable("item.container.item_count",
///              itemStack.getHoverName(), itemStack.getCount()));
///       }
///    }
/// }
/// if (itemCount - lineCount > 0) {
///    consumer.accept(Component.translatable("item.container.more_items", itemCount - lineCount)
///        .withStyle(ChatFormatting.ITALIC));
/// }
/// ```
///
/// **`lineCount <= 4` with the increment inside gives five lines, not four.**
/// The guard reads 0,1,2,3,4 before it reads 5, so a five-stack shulker box
/// lists everything and says nothing more; a six-stack one lists five and says
/// "and 1 more…". Writing it as `lineCount < 4` loses a line *and* moves the
/// remainder by one, which still looks plausible.
pub fn container_plan(present_stacks: usize) -> ContainerPlan {
    let mut line_count = 0i32;
    let mut item_count = 0i32;
    for _ in 0..present_stacks {
        item_count += 1;
        if line_count <= 4 {
            line_count += 1;
        }
    }
    ContainerPlan {
        shown: line_count as usize,
        more: item_count - line_count,
    }
}

/// `optionalImage.ifPresent(image -> components.add(components.isEmpty() ? 0 : 1, …))`
///
/// **Index 1** — immediately after the name, before the enchantments and the
/// lore. The `isEmpty` guard is not decoration: `List.add(1, …)` on an empty
/// list throws `IndexOutOfBoundsException`, and a stack whose tooltip display
/// hides its name reaches here with nothing in the list.
pub fn insert_image(components: &mut Vec<Component>, image: Component) {
    let at = if components.is_empty() { 0 } else { 1 };
    components.insert(at, image);
}

/// The measure loop — the widest component and the sum of the heights.
///
/// The height seed is `lines.size() == 1 ? -2 : 0`, counting **components**.
/// So a bare name measures 8 for its 10 px line, and the moment an image joins
/// it the seed becomes 0 and the box gains those two pixels back on top of the
/// image's own height.
pub fn measure(components: &[Component]) -> (i32, i32) {
    let mut width = 0;
    let mut height = if components.len() == 1 { -2 } else { 0 };
    for c in components {
        let w = c.width();
        if w > width {
            width = w;
        }
        height += c.height();
    }
    (width, height)
}

/// The `localY` each component is drawn at, for **one** pass, from the box's
/// top-left `y`.
pub fn pass_offsets(components: &[Component], y: i32) -> Vec<i32> {
    let mut out = Vec::with_capacity(components.len());
    let mut local_y = y;
    for (i, c) in components.iter().enumerate() {
        out.push(local_y);
        local_y += c.height() + if i == 0 { FIRST_GAP } else { 0 };
    }
    out
}

/// The first loop — `line.extractText(this, font, x, localY)`.
pub fn text_pass_offsets(components: &[Component], y: i32) -> Vec<i32> {
    pass_offsets(components, y)
}

/// The second loop. The whole of its difference from the first is the
/// `localY = y;` that precedes it, which is expressed here by starting from
/// `y` again rather than from [`continuous_cursor_end`].
pub fn image_pass_offsets(components: &[Component], y: i32) -> Vec<i32> {
    pass_offsets(components, y)
}

/// Where a single continuous cursor would leave the image pass — the mutation
/// partner for the restart, and nothing production calls.
pub fn continuous_cursor_end(components: &[Component], y: i32) -> i32 {
    let mut local_y = y;
    for (i, c) in components.iter().enumerate() {
        local_y += c.height() + if i == 0 { FIRST_GAP } else { 0 };
    }
    local_y
}

// -- `ClientBundleTooltip.extractImage` ---------------------------------------

/// `getContentXOffset(tooltipWidth)` — `(tooltipWidth - 96) / 2`.
///
/// The grid is centred in the **whole box**, not left-aligned under the name,
/// because `extractImage` is handed the tooltip's measured `w` rather than the
/// component's own. A long enchantment line therefore slides the grid right.
pub fn content_x_offset(tooltip_width: i32) -> i32 {
    (tooltip_width - GRID_WIDTH) / 2
}

/// What a visited grid cell turned out to hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellKind {
    /// `extractCount` — the surplus badge, `"+" + hiddenItemCount`.
    Badge { hidden: i32 },
    /// `extractSlot`. `item` is `shownItems.size() - slotNumber`, so the cells
    /// are filled from the **end** of the list: the last stack lands in the
    /// first cell visited.
    Slot { item: usize, selected: bool },
}

/// One occupied cell of the grid, at its top-left in tooltip coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub x: i32,
    pub y: i32,
    pub kind: CellKind,
}

impl Cell {
    /// `graphics.item(item, drawX + 4, drawY + 4, …)` — a 16 px icon inset by
    /// `SLOT_MARGIN` in the 24 px cell.
    pub fn icon(&self) -> (i32, i32) {
        (self.x + SLOT_MARGIN, self.y + SLOT_MARGIN)
    }

    /// `extractCount`'s `centeredText(font, text, drawX + 12, drawY + 10, -1)`
    /// — the horizontal anchor is the cell's centre, the vertical is a flat
    /// **10** and not the cell's 12.
    pub fn badge_anchor(&self) -> (i32, i32) {
        (self.x + SLOT_SIZE / 2, self.y + 10)
    }
}

/// `extractProgressbar`'s three blits, resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgressBar {
    /// The border sprite's top-left; it is 96x13. The **fill** is blitted one
    /// pixel right of this (`x + 1`) and at the same `y`, under the border.
    pub x: i32,
    pub y: i32,
    /// `getProgressBarFill` — `clamp(mulAndTruncate(weight, 94), 0, 94)`.
    pub fill: i32,
    /// `getProgressBarTexture` — the *full* sprite once the weight reaches 1.
    pub full: bool,
    /// `getProgressBarFillText`: the "empty" label at exactly zero, the "full"
    /// label at one or more, and **no label at all** in between.
    pub label: Option<BarLabel>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarLabel {
    /// `item.minecraft.bundle.empty`
    Empty,
    /// `item.minecraft.bundle.full`
    Full,
}

impl ProgressBar {
    /// `centeredText(font, text, x + 48, y + 3, -1)` — the label's anchor.
    pub fn label_anchor(&self) -> (i32, i32) {
        (self.x + GRID_WIDTH / 2, self.y + 3)
    }
}

/// Everything `ClientBundleTooltip.extractImage` draws, in tooltip
/// coordinates: `x`/`y` are the component's own origin (the box's left and the
/// `localY` the image pass reached), `w` is the **tooltip's** measured width.
#[derive(Clone, Debug, Default)]
pub struct BundleImage {
    /// In visit order — bottom-right first, top-left last.
    pub cells: Vec<Cell>,
    /// `None` when `weight().isError()`, which draws nothing.
    pub bar: Option<ProgressBar>,
    /// `extractEmptyBundleDescriptionText`'s top-left, for an empty bundle.
    /// The blurb wraps at [`GRID_WIDTH`].
    pub description: Option<(i32, i32)>,
}

/// `ClientBundleTooltip.extractImage`, resolved to geometry.
///
/// The cell walk is verbatim:
///
/// ```java
/// int xStartPos = x + getContentXOffset(w) + 96;
/// int yStartPos = y + this.gridSizeY() * 24;
/// int slotNumber = 1;
/// for (int rowNumber = 1; rowNumber <= this.gridSizeY(); rowNumber++) {
///    for (int columnNumber = 1; columnNumber <= 4; columnNumber++) {
///       int drawX = xStartPos - columnNumber * 24;
///       int drawY = yStartPos - rowNumber * 24;
/// ```
///
/// Both start positions are the grid's **far** edge and both subtract, so
/// `(column 1, row 1)` is the **bottom-right** cell and `(4, gridSizeY)` the
/// top-left. That is the whole reason `shouldRenderSurplusText`'s
/// `column * row == 1` puts the `+N` badge bottom-right — the first cell
/// visited, not the first one read.
pub fn bundle_image(bundle: &Bundle, x: i32, y: i32, w: i32) -> BundleImage {
    // `if (!weight.isError())` wraps the entire body.
    if !bundle.weight_ok {
        return BundleImage::default();
    }
    let left = x + content_x_offset(w);
    if bundle.is_empty() {
        // `extractEmptyBundleTooltip`: the blurb, then the bar below it.
        return BundleImage {
            cells: Vec::new(),
            bar: Some(progress_bar(
                left,
                y + bundle.empty_description_height() + PROGRESSBAR_MARGIN_Y,
                Fraction::ZERO,
            )),
            description: Some((left, y)),
        };
    }

    let is_overflowing = bundle.is_overflowing();
    let shown = bundle.shown_items();
    let grid_size_y = bundle.grid_size_y();
    let x_start = left + GRID_WIDTH;
    let y_start = y + grid_size_y * SLOT_SIZE;
    let mut cells = Vec::new();
    let mut slot_number = 1i32;
    for row in 1..=grid_size_y {
        for column in 1..=GRID_COLUMNS {
            let draw_x = x_start - column * SLOT_SIZE;
            let draw_y = y_start - row * SLOT_SIZE;
            if is_overflowing && column * row == 1 {
                cells.push(Cell {
                    x: draw_x,
                    y: draw_y,
                    kind: CellKind::Badge {
                        hidden: bundle.hidden_item_count(),
                    },
                });
            } else if shown >= slot_number {
                // `itemVisualOrderIndex = shownItems.size() - slotNumber`.
                let item = (shown - slot_number) as usize;
                cells.push(Cell {
                    x: draw_x,
                    y: draw_y,
                    kind: CellKind::Slot {
                        item,
                        selected: item as i32 == bundle.selected,
                    },
                });
                slot_number += 1;
            }
        }
    }
    BundleImage {
        cells,
        // `y + this.itemGridHeight() + 4` — and note this is measured from the
        // component's own `y`, not from the last cell, so it sits under a full
        // grid even when the top row came out empty.
        bar: Some(progress_bar(
            left,
            y + bundle.item_grid_height() + PROGRESSBAR_MARGIN_Y,
            bundle.weight,
        )),
        description: None,
    }
}

fn progress_bar(x: i32, y: i32, weight: Fraction) -> ProgressBar {
    use std::cmp::Ordering;
    let fill = weight
        .mul_and_truncate(PROGRESSBAR_FILL_MAX)
        .clamp(0, PROGRESSBAR_FILL_MAX);
    let full = weight.cmp_to(Fraction::ONE) != Ordering::Less;
    let label = match weight.cmp_to(Fraction::ZERO) {
        Ordering::Equal => Some(BarLabel::Empty),
        _ if full => Some(BarLabel::Full),
        _ => None,
    };
    ProgressBar {
        x,
        y,
        fill,
        full,
        label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `DARK_GRAY` against `TextColor`'s own literal.
    ///
    /// ```java
    /// public static final TextColor DARK_GRAY = named("dark_gray", 5592405);
    /// ```
    ///
    /// Added by M93q's sweep, which found it unguarded: `inventoryshot`'s
    /// advanced-tooltip witness asserts `dark_gray == Some(DARK_GRAY)` while
    /// reading `DARK_GRAY` for the expectation, so the two move together.
    /// Mutating it `0x555555` → `0x3F3F3F` left the gate's 152 witnesses green
    /// — and `0x3F3F3F` is the exact wrong grey M93p shipped for the loom,
    /// which is the point: a plausible flat grey is what a guess produces, and
    /// no amount of pixel-reading catches one when the reader shares the value.
    ///
    /// Note 26.2 moved the RGB **off** `ChatFormatting`, whose enum rows now
    /// carry only the legacy char (`DARK_GRAY('8')`); reading the colour there
    /// finds nothing, which is why the citation is `TextColor`.
    #[test]
    fn dark_gray_is_text_colors_own_value() {
        assert_eq!(5592405, 0x555555, "the decompile's decimal, spelled out");
        for (i, c) in DARK_GRAY.iter().enumerate() {
            assert_eq!(
                (c * 255.0).round() as u32,
                0x55,
                "channel {i} of ChatFormatting.DARK_GRAY"
            );
        }
        // Not GRAY (0xAAAAAA) — the neighbouring formatting code, and the
        // confusion this constant's name invites.
        assert_ne!((DARK_GRAY[0] * 255.0).round() as u32, 0xAA);
    }

    fn bundle(n: usize) -> Bundle {
        Bundle {
            counts: vec![1; n],
            selected: -1,
            weight: Fraction::new(1, 2),
            weight_ok: true,
            empty_description_lines: 2,
        }
    }

    #[test]
    fn the_image_goes_after_the_name() {
        let mut c = vec![Component::text(40), Component::text(55)];
        insert_image(&mut c, Component::Bundle(bundle(3)));
        assert!(matches!(c[0], Component::Text { width: 40 }));
        assert!(matches!(c[1], Component::Bundle(_)));
        assert!(matches!(c[2], Component::Text { width: 55 }));

        // …and at 0 when there is nothing to go after, because `add(1, …)`
        // on an empty list throws.
        let mut empty = Vec::new();
        insert_image(&mut empty, Component::Bundle(bundle(3)));
        assert!(matches!(empty[0], Component::Bundle(_)));
    }

    #[test]
    fn a_bundle_widens_the_box_to_the_grid() {
        let name = vec![Component::text(40)];
        assert_eq!(measure(&name), (40, 8));
        let mut with = name.clone();
        insert_image(&mut with, Component::Bundle(bundle(3)));
        // One row of cells (24) + the bar (13) + two margins (8) = 45, and the
        // single-component -2 is gone because there are two components now.
        assert_eq!(measure(&with), (96, 10 + 45));
    }

    #[test]
    fn the_image_pass_restarts_at_the_top() {
        let mut c = vec![Component::text(40)];
        insert_image(&mut c, Component::Bundle(bundle(3)));
        let text = text_pass_offsets(&c, 100);
        let image = image_pass_offsets(&c, 100);
        assert_eq!(text, image);
        assert_eq!(image[0], 100);
        assert_ne!(image[1], continuous_cursor_end(&c, 100));
    }

    #[test]
    fn the_grid_fills_bottom_right_first() {
        let b = bundle(3);
        let img = bundle_image(&b, 0, 0, GRID_WIDTH);
        assert_eq!(img.cells.len(), 3);
        // One row, so every cell is at y 0; the columns walk right to left.
        assert_eq!((img.cells[0].x, img.cells[0].y), (72, 0));
        assert_eq!((img.cells[1].x, img.cells[1].y), (48, 0));
        assert_eq!((img.cells[2].x, img.cells[2].y), (24, 0));
        // …and the *last* stack lands in the *first* cell.
        assert_eq!(img.cells[0].kind, CellKind::Slot { item: 2, selected: false });
        assert_eq!(img.cells[2].kind, CellKind::Slot { item: 0, selected: false });
    }

    #[test]
    fn thirteen_stacks_badge_and_twelve_do_not() {
        let twelve = bundle(12);
        assert!(!twelve.is_overflowing());
        assert_eq!(twelve.shown_items(), 12);
        assert!(bundle_image(&twelve, 0, 0, GRID_WIDTH)
            .cells
            .iter()
            .all(|c| matches!(c.kind, CellKind::Slot { .. })));

        let thirteen = bundle(13);
        assert!(thirteen.is_overflowing());
        // Eight, not eleven: the ragged-row subtraction is keyed off 13 % 4.
        assert_eq!(thirteen.shown_items(), 8);
        assert_eq!(thirteen.hidden_item_count(), 5);
        let img = bundle_image(&thirteen, 0, 0, GRID_WIDTH);
        assert_eq!(img.cells[0].kind, CellKind::Badge { hidden: 5 });
        assert_eq!((img.cells[0].x, img.cells[0].y), (72, 48));
        assert_eq!(img.cells.len(), 9);
    }

    #[test]
    fn the_bar_labels_only_at_the_ends() {
        let mut b = bundle(1);
        b.weight = Fraction::new(1, 2);
        let mid = bundle_image(&b, 0, 0, GRID_WIDTH).bar.unwrap();
        assert_eq!((mid.fill, mid.full, mid.label), (47, false, None));
        b.weight = Fraction::ONE;
        let full = bundle_image(&b, 0, 0, GRID_WIDTH).bar.unwrap();
        assert_eq!(
            (full.fill, full.full, full.label),
            (94, true, Some(BarLabel::Full))
        );
        b.weight = Fraction::ZERO;
        let empty = bundle_image(&b, 0, 0, GRID_WIDTH).bar.unwrap();
        assert_eq!(
            (empty.fill, empty.full, empty.label),
            (0, false, Some(BarLabel::Empty))
        );
    }

    #[test]
    fn an_errored_weight_draws_nothing() {
        let mut b = bundle(3);
        b.weight_ok = false;
        let img = bundle_image(&b, 0, 0, GRID_WIDTH);
        assert!(img.cells.is_empty() && img.bar.is_none());
        // …but it still measures, so the box keeps the hole.
        assert_eq!(Component::Bundle(b).height(), 24 + 13 + 8);
    }
}

#[cfg(test)]
mod advanced_tests {
    use super::*;

    fn tool(damage: i32, max: i32) -> DurabilityState {
        DurabilityState {
            damage,
            max,
            has_max_damage: true,
            has_damage: true,
            unbreakable: false,
        }
    }

    #[test]
    fn the_durability_arguments_are_remaining_then_max() {
        let lines = advanced_lines(TooltipFlag::ADVANCED, tool(61, 1561), true, Some(19));
        assert_eq!(
            lines[0],
            AdvancedLine::Durability {
                remaining: 1500,
                max: 1561
            }
        );
    }

    #[test]
    fn a_pristine_tool_has_no_durability_line_and_an_unbreakable_one_never_does() {
        let lines = advanced_lines(TooltipFlag::ADVANCED, tool(0, 1561), true, Some(19));
        assert_eq!(lines[0], AdvancedLine::RegistryId);

        let mut u = tool(61, 1561);
        u.unbreakable = true;
        assert!(!u.is_damaged());
        let lines = advanced_lines(TooltipFlag::ADVANCED, u, true, Some(19));
        assert_eq!(lines[0], AdvancedLine::RegistryId);
    }

    #[test]
    fn the_damage_value_is_clamped_to_the_maximum() {
        // A server sending more damage than the item can take reports a
        // remainder of zero, not a negative one.
        assert_eq!(tool(9999, 1561).damage_value(), 1561);
        let lines = advanced_lines(TooltipFlag::ADVANCED, tool(9999, 1561), true, Some(19));
        assert_eq!(
            lines[0],
            AdvancedLine::Durability {
                remaining: 0,
                max: 1561
            }
        );
    }

    #[test]
    fn the_order_is_durability_then_the_id_then_the_count() {
        let lines = advanced_lines(TooltipFlag::ADVANCED, tool(61, 1561), true, Some(19));
        assert_eq!(
            lines,
            vec![
                AdvancedLine::Durability {
                    remaining: 1500,
                    max: 1561
                },
                AdvancedLine::RegistryId,
                AdvancedLine::Components { count: 19 },
            ]
        );
    }

    #[test]
    fn the_count_line_is_suppressed_at_zero_and_when_unknown() {
        let zero = advanced_lines(TooltipFlag::ADVANCED, tool(0, 0), true, Some(0));
        assert_eq!(zero, vec![AdvancedLine::RegistryId]);
        let unknown = advanced_lines(TooltipFlag::ADVANCED, tool(0, 0), true, None);
        assert_eq!(unknown, vec![AdvancedLine::RegistryId]);
        // …but the id is unconditional, so the block is never empty.
        assert_eq!(zero.len(), 1);
    }

    #[test]
    fn the_normal_flag_emits_nothing_at_all() {
        assert!(advanced_lines(TooltipFlag::NORMAL, tool(61, 1561), true, Some(19)).is_empty());
        assert_eq!(TooltipFlag::of(false), TooltipFlag::NORMAL);
        assert_eq!(TooltipFlag::of(true), TooltipFlag::ADVANCED);
        // Advanced is not creative.
        assert!(!TooltipFlag::ADVANCED.creative);
    }

    #[test]
    fn five_lines_fit_and_the_sixth_becomes_a_remainder() {
        assert_eq!(container_plan(0), ContainerPlan { shown: 0, more: 0 });
        assert_eq!(container_plan(1), ContainerPlan { shown: 1, more: 0 });
        // The boundary the `<= 4` guard decides: five fit exactly.
        assert_eq!(container_plan(5), ContainerPlan { shown: 5, more: 0 });
        assert_eq!(container_plan(6), ContainerPlan { shown: 5, more: 1 });
        assert_eq!(container_plan(27), ContainerPlan { shown: 5, more: 22 });
    }
}

#[cfg(test)]
mod span_tests {
    use super::*;

    #[test]
    fn a_line_can_change_colour_and_slant_mid_way() {
        // The three things the old (String, [f32;3]) model could not say.
        let line: Line = vec![
            Span::new("Sharpness ", [0.66, 0.66, 0.66]),
            Span::new("V", [1.0, 1.0, 1.0]),
            Span::new(" cursed", [1.0, 0.33, 0.33]).italic(),
        ];
        assert_eq!(line.len(), 3);
        assert_ne!(line[0].color, line[1].color, "per-run colour");
        assert!(line[2].italic, "per-run italic");
        assert!(!line[0].italic);
    }

    #[test]
    fn line_text_concatenates_for_the_unchanged_width_path() {
        // Geometry must not move: the box is still measured from the plain
        // text with the vanilla advances, so adding spans cannot resize a
        // tooltip that used to fit.
        let line: Line = vec![
            Span::new("Unbreak", [0.0; 3]),
            Span::new("able", [0.0; 3]).italic(),
        ];
        assert_eq!(line_text(&line), "Unbreakable");
    }

    #[test]
    fn italic_selects_the_italic_face_not_a_skew() {
        let line: Line = vec![
            Span::new("a", [1.0; 3]),
            Span::new("b", [1.0; 3]).italic(),
        ];
        let spans = to_velvet_spans(&line, 12.0);
        assert_eq!(spans.len(), 2);
        // The two must reach the cache under different keys, or the italic
        // silently renders upright.
        assert_ne!(spans[0].key, spans[1].key);
    }
}
