//! `StatsScreen` (M84) — the second consumer of [`crate::screen`], and the one
//! that made it grow a scrolling list and a tab bar.
//!
//! Everything here is
//! `net/minecraft/client/gui/screens/achievement/StatsScreen.java` plus the
//! four layout classes it stands on (`MenuTabBar`, `TabNavigationBar`,
//! `HeaderAndFooterLayout`, `AbstractSelectionList`). Nothing screen-specific
//! belongs in the framework module, which is the split M82 set up.
//!
//! # The header is 24 and the footer is 33, and only one of them is a constant
//!
//! `HeaderAndFooterLayout`'s `DEFAULT_HEADER_AND_FOOTER_HEIGHT` is 33 and the
//! footer keeps it. The **header does not**: `repositionElements` overwrites it
//! with `tabNavigationBar.getRectangle().bottom()`, and the bar is built at
//! `(0, 0, width, 24)`, so the header is **24**. Taking 33 for both — the
//! reading the constant's name invites — moves every row nine pixels down and
//! shortens every list by nine.
//!
//! # `isInGameUi()` is false, so this screen does *not* dim the world
//!
//! `Screen.extractBackground` branches on `isInGameUi()`, and the only override
//! in the tree is `AbstractContainerScreen`'s. So the **inventory** gets the
//! translucent `0xC0101010 → 0xD0101010` gradient and the statistics screen
//! gets the opaque menu-background tiles — the reverse of the intuition that an
//! in-game screen is a translucent overlay. What it does instead is cover the
//! world with two tiled sheets: `tab_header_background` over the header strip
//! and `inworld_menu_background` over everything below it.
//!
//! # Three column orders, and they disagree
//!
//! * the `minecraft:stat_type` **registry** is mined, crafted, used, broken,
//!   picked_up, dropped;
//! * the items tab's **columns** are mined, broken, crafted, used, picked_up,
//!   dropped (`blockColumns` then `itemColumns`);
//! * the general tab's rows are sorted by their **translated** text, not by
//!   registry id.
//!
//! [`COLUMNS`] is the second of the three, and it is the one every `getColumnX`
//! index means.
//!
//! # The one deviation, and why
//!
//! Vanilla keys an items row by an `Item` and draws it as an icon alone,
//! joining a block's `mined` count onto it through `Item.BY_BLOCK`
//! (`block.asItem()`). Rewo keys the row by **registry name** and draws the
//! name. Two reasons, both recorded rather than hidden:
//!
//! 1. Rewo has no `BY_BLOCK` table. Its only source is `Items.java`'s 643
//!    `registerBlock(BlockItemIds.X, Blocks.Y)` lines, which need
//!    `Blocks.java`'s *constant → registry name* map to resolve — and that map
//!    is irregular exactly where M10 records it being irregular (`copper_block`
//!    against `exposed_copper`), so deriving it by lower-casing the constant is
//!    wrong in a direction no gate here would see.
//! 2. A row identified only by an icon is identified by nothing at all in the
//!    windowed client, where `LiveApp::self.baked` is `None` and no item icon
//!    renders (a bug a sibling milestone is fixing this session).
//!
//! The failure mode of matching by name is a *split* row rather than a wrong
//! number: a block whose item is named differently would appear twice, once
//! with only a `mined` count. Every vanilla `BlockItem` shares its block's
//! name, so that is a narrow set.

use crate::screen::{
    Backdrop, ScrollList, Screen, ScreenKind, Sprite, Widget, WidgetId, WidgetKind, WidgetSprites,
    BUTTON_HEIGHT, BUTTON_WIDTH,
};
use crate::layout;
use crate::stats::{self, StatKey, StatsCounter};
use rewo_data::stats::{Formatter, StatRegistries};

// ---------------------------------------------------------------------------
// Widget ids.
// ---------------------------------------------------------------------------

/// The footer's `CommonComponents.GUI_DONE`.
pub const DONE: WidgetId = 0;
/// The three `MenuTabButton`s, in `addTabs` order.
pub const TAB_GENERAL: WidgetId = 1;
pub const TAB_ITEMS: WidgetId = 2;
pub const TAB_MOBS: WidgetId = 3;
/// The six `StatSortButton`s. `SORT_FIRST + column`.
pub const SORT_FIRST: WidgetId = 8;

// ---------------------------------------------------------------------------
// Layout constants, each from its own class.
// ---------------------------------------------------------------------------

/// `MenuTabBar.HEIGHT`, and therefore `layout.getHeaderHeight()`.
pub const HEADER_HEIGHT: i32 = 24;
/// `HeaderAndFooterLayout.DEFAULT_HEADER_AND_FOOTER_HEIGHT`, kept by the footer
/// — M85's constant, because there is one of it.
pub const FOOTER_HEIGHT: i32 = layout::DEFAULT_HEADER_AND_FOOTER_HEIGHT;
/// `MenuTabBar.MAX_WIDTH`.
const TAB_BAR_MAX_WIDTH: i32 = 400;
/// `MenuTabBar.MARGIN` — subtracted **doubled**, `- 28` in `arrangeElements`.
const TAB_BAR_MARGIN: i32 = 14;
/// Every list's `getRowWidth()` override. `AbstractSelectionList`'s own default
/// is 220; all three statistics lists return 280.
pub const LIST_ROW_WIDTH: i32 = 280;
/// `GeneralStatisticsList`'s `defaultEntryHeight`.
pub const GENERAL_ROW_HEIGHT: i32 = 14;
/// `ItemStatisticsList.SLOT_STAT_HEIGHT`.
pub const ITEM_ROW_HEIGHT: i32 = 22;
/// `MobsStatisticsList`'s `9 * 4`.
pub const MOB_ROW_HEIGHT: i32 = 9 * 4;
/// `ItemStatisticsList.SLOT_BG_SIZE`, and the sort buttons' size.
pub const SLOT_SIZE: i32 = 18;

/// `-1` and `-4539718` — the alternating row colours, and `-8355712` for a
/// mob line with a zero count.
pub const ROW_WHITE: [f32; 3] = [1.0, 1.0, 1.0];
pub const ROW_DIM: [f32; 3] = [0xBA as f32 / 255.0, 0xBA as f32 / 255.0, 0xBA as f32 / 255.0];
pub const ROW_GREY: [f32; 3] = [0.5, 0.5, 0.5];

/// `getColumnX(col)` — `75 + 40 * col`, the column's **right** edge, since
/// every number is drawn at `x - font.width(msg)`.
pub fn column_x(col: usize) -> i32 {
    75 + 40 * col as i32
}

/// The items tab's six columns, `blockColumns` then `itemColumns`.
///
/// Registry names, so the ids come from the report rather than from a second
/// hard-coded table.
pub const COLUMNS: [&str; 6] = [
    "minecraft:mined",
    "minecraft:broken",
    "minecraft:crafted",
    "minecraft:used",
    "minecraft:picked_up",
    "minecraft:dropped",
];

/// Which tab is up. `MenuTabBar`'s three, in `addTabs` order.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StatsTab {
    #[default]
    General,
    Items,
    Mobs,
}

impl StatsTab {
    pub const ALL: [StatsTab; 3] = [StatsTab::General, StatsTab::Items, StatsTab::Mobs];

    pub fn index(self) -> usize {
        match self {
            StatsTab::General => 0,
            StatsTab::Items => 1,
            StatsTab::Mobs => 2,
        }
    }

    pub fn widget_id(self) -> WidgetId {
        TAB_GENERAL + self.index() as WidgetId
    }

    pub fn from_widget(id: WidgetId) -> Option<Self> {
        StatsTab::ALL.into_iter().find(|t| t.widget_id() == id)
    }
}

/// The strings the screen needs, resolved by the caller.
///
/// Passed in for the same reason `DeathLabels` is: this module stays free of
/// the asset pipeline and can be unit-tested without a jar.
#[derive(Clone, Debug, PartialEq)]
pub struct StatsLabels {
    /// `gui.stats`.
    pub title: String,
    /// `stat.generalButton` / `stat.itemsButton` / `stat.mobsButton`.
    pub tabs: [String; 3],
    /// `multiplayer.downloadingStats`.
    pub pending: String,
    /// `gui.done`.
    pub done: String,
    /// `NO_VALUE_DISPLAY` — `stats.none`, which is a lone `-` in English and a
    /// **translated** string all the same, so a resource pack can change it.
    pub none: String,
    /// The six column tooltips — `StatType.getDisplayName()`.
    pub columns: [String; 6],
}

pub const KEY_TITLE: &str = "gui.stats";
pub const KEY_GENERAL: &str = "stat.generalButton";
pub const KEY_ITEMS: &str = "stat.itemsButton";
pub const KEY_MOBS: &str = "stat.mobsButton";
pub const KEY_PENDING: &str = "multiplayer.downloadingStats";
pub const KEY_NONE_FOUND: &str = "gui.stats.none_found";
/// `NO_VALUE_DISPLAY` — a lone `-`.
pub const KEY_NONE: &str = "stats.none";
pub const KEY_DONE: &str = "gui.done";

impl StatsLabels {
    pub fn resolve(lang: &rewo_data::lang::Language) -> Self {
        Self {
            title: lang.or_key(KEY_TITLE).to_string(),
            tabs: [
                lang.or_key(KEY_GENERAL).to_string(),
                lang.or_key(KEY_ITEMS).to_string(),
                lang.or_key(KEY_MOBS).to_string(),
            ],
            pending: lang.or_key(KEY_PENDING).to_string(),
            done: lang.or_key(KEY_DONE).to_string(),
            none: lang.or_key(KEY_NONE).to_string(),
            columns: std::array::from_fn(|i| {
                lang.or_key(&stats::stat_type_key(COLUMNS[i])).to_string()
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Rows.
// ---------------------------------------------------------------------------

/// One `GeneralStatisticsList.Entry`.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneralRow {
    /// `Component.translatable(getTranslationKey(stat))`.
    pub label: String,
    /// `stat.format(stats.getValue(stat))`.
    pub value: String,
}

/// One `MobsStatisticsList.MobRow`.
#[derive(Clone, Debug, PartialEq)]
pub struct MobRow {
    pub name: String,
    pub kills: String,
    pub has_kills: bool,
    pub killed_by: String,
    pub was_killed_by: bool,
}

/// One `ItemStatisticsList.ItemRow`, keyed by registry name — see the module
/// docs for the deviation.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemRow {
    pub name: String,
    /// The short name, without the `minecraft:` namespace, which is what the
    /// row draws in place of vanilla's icon.
    pub short_name: String,
    pub item_id: Option<i32>,
    pub block_id: Option<i32>,
    /// Per column, in [`COLUMNS`] order: the raw count, or `None` where the
    /// stat does not exist for this row at all (vanilla's `stat == null`,
    /// which draws `stats.none`).
    pub counts: [Option<i32>; 6],
    /// The already-formatted cells. All six use `StatFormatter.DEFAULT`.
    pub cells: [Option<String>; 6],
    /// The stand-in for `Item.getId(item)` in the comparator's tie-break.
    pub order: i32,
}

// ---------------------------------------------------------------------------
// The model.
// ---------------------------------------------------------------------------

/// Everything the statistics screen holds across frames.
#[derive(Clone, Debug, Default)]
pub struct StatsModel {
    pub tab: StatsTab,
    /// `StatsScreen.isLoading`, which starts **true** and is cleared by the
    /// first `award_stats`. Vanilla's tab bar is three `LoadingTab`s until
    /// then, all disabled.
    pub loading: bool,
    pub general: Vec<GeneralRow>,
    pub items: Vec<ItemRow>,
    pub mobs: Vec<MobRow>,
    /// One per tab, in [`StatsTab::ALL`] order.
    pub lists: [ScrollList; 3],
    /// `sortColumn` as a column index, and `sortOrder` (-1 / 0 / 1).
    pub sort_column: Option<usize>,
    pub sort_order: i32,
    /// The `updates` watermark this model was built from, so a later packet
    /// rebuilds it and an unchanged one does not.
    pub built_from: u64,
}

impl StatsModel {
    /// The list for whichever tab is up.
    pub fn list(&self) -> &ScrollList {
        &self.lists[self.tab.index()]
    }

    pub fn list_mut(&mut self) -> &mut ScrollList {
        let i = self.tab.index();
        &mut self.lists[i]
    }

    /// How many rows the current tab has. The items tab's header counts, since
    /// it is a real entry.
    pub fn row_count(&self) -> usize {
        match self.tab {
            StatsTab::General => self.general.len(),
            StatsTab::Items => {
                if self.items.is_empty() {
                    0
                } else {
                    self.items.len() + 1
                }
            }
            StatsTab::Mobs => self.mobs.len(),
        }
    }

    /// `setTabActiveStateAndTooltip` — a tab whose list has no children at all
    /// is **disabled**, with a `gui.stats.none_found` tooltip. Only tabs 1 and
    /// 2 are tested: `General` is never disabled, because
    /// `Stats.CUSTOM` always has all 77 entries whatever the counts are.
    pub fn tab_active(&self, tab: StatsTab) -> bool {
        if self.loading {
            return false;
        }
        match tab {
            StatsTab::General => true,
            StatsTab::Items => !self.items.is_empty(),
            StatsTab::Mobs => !self.mobs.is_empty(),
        }
    }

    /// `sortByColumn` — the three-state cycle.
    ///
    /// A different column: sort **descending** (`sortOrder = -1`). The same
    /// column again: ascending. A third time: no sort at all, and the column
    /// arrow disappears. So the first click on a fresh column is descending,
    /// not ascending.
    pub fn sort_by_column(&mut self, column: usize) {
        if self.sort_column != Some(column) {
            self.sort_column = Some(column);
            self.sort_order = -1;
        } else if self.sort_order == -1 {
            self.sort_order = 1;
        } else {
            self.sort_column = None;
            self.sort_order = 0;
        }
        self.sort_items();
    }

    /// `sortItems(itemStatSorter)` with `ItemRowComparator`.
    ///
    /// `sortOrder == 0` makes the comparator return 0 for every pair, so
    /// vanilla's stable sort leaves the order alone — clearing the sort does
    /// **not** restore the original order, it freezes the last one.
    pub fn sort_items(&mut self) {
        let (col, order) = (self.sort_column, self.sort_order);
        if order == 0 {
            return;
        }
        let key = |r: &ItemRow| -> i32 {
            match col {
                // `blockColumns.contains(sortColumn)` → a row with no block is
                // **-1**, not 0, so it sorts below a genuine zero.
                Some(0) => {
                    if r.block_id.is_some() {
                        r.counts[0].unwrap_or(0)
                    } else {
                        -1
                    }
                }
                Some(c) => r.counts[c].unwrap_or(0),
                None => 0,
            }
        };
        self.items.sort_by(|a, b| {
            let (ka, kb) = (key(a), key(b));
            let cmp = if ka == kb {
                a.order.cmp(&b.order)
            } else {
                ka.cmp(&kb)
            };
            if order < 0 {
                cmp.reverse()
            } else {
                cmp
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Building the rows.
// ---------------------------------------------------------------------------

/// `Stats.CUSTOM`'s 77 entries, formatted and sorted by their translated name.
///
/// The sort is `Comparator.comparing(k -> I18n.get(getTranslationKey(k)))` —
/// by the **translated** string, so a resource pack reorders the list. Java's
/// `String.compareTo` is UTF-16 code-unit order; for ASCII, Rust's `str` `Ord`
/// is the same order, and every vanilla `stat.*` value is ASCII.
pub fn build_general(
    counter: &StatsCounter,
    reg: &StatRegistries,
    lang: &rewo_data::lang::Language,
) -> Vec<GeneralRow> {
    let Some(custom) = reg.stat_type.id_of("minecraft:custom") else {
        return Vec::new();
    };
    let mut rows: Vec<(String, GeneralRow)> = reg
        .custom_stat
        .iter()
        .map(|(id, name)| {
            let key = stats::custom_stat_key(name);
            let label = lang.or_key(&key).to_string();
            let value = stats::format_stat(
                rewo_data::stats::custom_formatter(name),
                counter.value(StatKey::new(custom, id)),
            );
            (label.clone(), GeneralRow { label, value })
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.into_iter().map(|(_, r)| r).collect()
}

/// `MobsStatisticsList`'s rows — every entity type with a non-zero
/// `killed` or `killed_by`, in registry-id order.
///
/// **The two templates take their arguments in opposite orders.**
/// `killed` is `translatable(key, kills, mobName)` and `killed_by` is
/// `translatable(key, mobName, killedBy)`, which the English strings confirm
/// ("You killed %s %s" against "%s killed you %s time(s)"). Passing one
/// order to both produces two sentences that both read plausibly and are
/// both wrong.
pub fn build_mobs(
    counter: &StatsCounter,
    reg: &StatRegistries,
    types: &rewo_data::entity_types::EntityTypes,
    lang: &rewo_data::lang::Language,
) -> Vec<MobRow> {
    let (Some(killed), Some(killed_by)) = (
        reg.stat_type.id_of("minecraft:killed"),
        reg.stat_type.id_of("minecraft:killed_by"),
    ) else {
        return Vec::new();
    };
    let mut ids: Vec<i32> = types.ids().collect();
    ids.sort_unstable();
    ids.into_iter()
        .filter_map(|id| {
            let kills = counter.value(StatKey::new(killed, id));
            let died = counter.value(StatKey::new(killed_by, id));
            if kills == 0 && died == 0 {
                return None;
            }
            let name_key = format!("entity.{}", types.name(id)?.replace(':', "."));
            let mob = lang.or_key(&name_key).to_string();
            let kills_text = if kills == 0 {
                lang.translate("stat_type.minecraft.killed.none", &[&mob])
            } else {
                lang.translate("stat_type.minecraft.killed", &[&kills.to_string(), &mob])
            };
            let died_text = if died == 0 {
                lang.translate("stat_type.minecraft.killed_by.none", &[&mob])
            } else {
                lang.translate(
                    "stat_type.minecraft.killed_by",
                    &[&mob, &died.to_string()],
                )
            };
            Some(MobRow {
                name: mob,
                kills: kills_text,
                has_kills: kills != 0,
                killed_by: died_text,
                was_killed_by: died != 0,
            })
        })
        .collect()
}

/// `ItemStatisticsList`'s rows, keyed by registry name — see the module docs.
///
/// A row exists when **any** of the six columns is non-zero for that name,
/// which is vanilla's `addToList` over the item columns unioned with the same
/// over the block column. `minecraft:air` is dropped, as `items.remove(Items.AIR)`
/// does.
pub fn build_items(
    counter: &StatsCounter,
    reg: &StatRegistries,
    items: &rewo_data::items::Items,
) -> Vec<ItemRow> {
    let type_ids: Vec<Option<i32>> = COLUMNS.iter().map(|c| reg.stat_type.id_of(c)).collect();
    let mut names: std::collections::BTreeMap<String, (Option<i32>, Option<i32>)> =
        Default::default();
    // Column 0 is the block registry; 1..6 are the item registry.
    if let Some(t) = type_ids[0] {
        for (value_id, _) in counter.of_type(t) {
            if let Some(name) = reg.block.name(value_id) {
                names.entry(name.to_string()).or_default().1 = Some(value_id);
            }
        }
    }
    for t in type_ids.iter().skip(1).flatten() {
        for (value_id, _) in counter.of_type(*t) {
            if let Some(name) = items.name(value_id) {
                names.entry(name.to_string()).or_default().0 = Some(value_id);
            }
        }
    }
    names.remove("minecraft:air");
    let mut rows: Vec<ItemRow> = names
        .into_iter()
        .map(|(name, (found_item, found_block))| {
            // Match the two registries by name, in both directions, so a row
            // discovered through one gets the other's column too.
            let item_id = found_item.or_else(|| items.id(&name));
            let block_id = found_block.or_else(|| reg.block.id_of(&name));
            let counts: [Option<i32>; 6] = std::array::from_fn(|c| {
                let t = type_ids[c]?;
                let value_id = if c == 0 { block_id? } else { item_id? };
                Some(counter.value(StatKey::new(t, value_id)))
            });
            let short_name = name
                .split_once(':')
                .map(|(_, s)| s.to_string())
                .unwrap_or_else(|| name.clone());
            ItemRow {
                short_name,
                cells: std::array::from_fn(|c| {
                    counts[c].map(|v| stats::format_stat(Formatter::Default, v))
                }),
                // `Item.getId(item)`'s stand-in. A block-only row sorts after
                // every item, deterministically.
                order: item_id.unwrap_or_else(|| items.len() as i32 + block_id.unwrap_or(0)),
                name,
                item_id,
                block_id,
                counts,
            }
        })
        .collect();
    rows.sort_by_key(|r| r.order);
    rows
}

// ---------------------------------------------------------------------------
// The screen itself.
// ---------------------------------------------------------------------------

/// `MenuTabBar.arrangeElements(width)` — `(x, tab_width)`.
///
/// ```java
/// int tabsWidth = Math.min(400, width) - 28;
/// int tabWidth  = Mth.roundToward(tabsWidth / this.tabs.size(), 2);
/// this.layout.setX(Mth.roundToward((width - tabsWidth) / 2, 2));
/// ```
///
/// `Mth.roundToward(v, 2)` is `ceil(v / 2) * 2`, so it rounds **up** to an even
/// number rather than to the nearest one — three 98-wide tabs overrun their own
/// 292-wide band by two pixels, and that is vanilla.
pub fn tab_bar_layout(gui_w: i32, tabs: i32) -> (i32, i32) {
    let tabs_width = gui_w.min(TAB_BAR_MAX_WIDTH) - 2 * TAB_BAR_MARGIN;
    let tab_width = layout::round_toward(tabs_width / tabs, 2);
    let x = layout::round_toward((gui_w - tabs_width) / 2, 2);
    (x, tab_width)
}

/// The footer `Done` button's rect — `FrameLayout` centring inside the
/// footer frame.
///
/// `alignInDimension` is `(int) Mth.lerp(align, 0, length - widgetLength)`, a
/// **truncating** cast of a float, so the vertical offset in a 33-high footer
/// around a 20-high button is `(int) 6.5 == 6` and the button sits one pixel
/// above centre.
pub fn done_bounds(gui_w: i32, gui_h: i32) -> (i32, i32) {
    (
        layout::align_in_dimension(0, gui_w, BUTTON_WIDTH, 0.5),
        layout::align_in_dimension(gui_h - FOOTER_HEIGHT, FOOTER_HEIGHT, BUTTON_HEIGHT, 0.5),
    )
}

/// The content band the lists live in: `(y, height)`.
pub fn content_band(gui_h: i32) -> (i32, i32) {
    (HEADER_HEIGHT, gui_h - HEADER_HEIGHT - FOOTER_HEIGHT)
}

impl StatsModel {
    /// `StatsScreen.init()` + `onStatsUpdated()`'s rebuild, as one builder.
    ///
    /// Vanilla splits them because it must: `init()` runs before any stats have
    /// arrived and builds three `LoadingTab`s, and `onStatsUpdated()` swaps in
    /// the real ones exactly once. Rewo rebuilds from the counter whenever the
    /// counter moves, which reaches the same states and additionally survives a
    /// second `award_stats` — vanilla's `if (this.isLoading)` guard means a
    /// later packet updates the *numbers* but never re-derives which rows
    /// exist.
    pub fn build(
        counter: &StatsCounter,
        reg: &StatRegistries,
        items: &rewo_data::items::Items,
        types: &rewo_data::entity_types::EntityTypes,
        lang: &rewo_data::lang::Language,
        tab: StatsTab,
        sort: (Option<usize>, i32),
        gui_w: i32,
        gui_h: i32,
    ) -> Self {
        let loading = counter.updates == 0;
        let general = if loading {
            Vec::new()
        } else {
            build_general(counter, reg, lang)
        };
        let mobs = if loading {
            Vec::new()
        } else {
            build_mobs(counter, reg, types, lang)
        };
        let item_rows = if loading {
            Vec::new()
        } else {
            build_items(counter, reg, items)
        };
        let (y, h) = content_band(gui_h);
        let mut lists = [
            ScrollList::new(gui_w, h, y, GENERAL_ROW_HEIGHT, LIST_ROW_WIDTH),
            ScrollList::new(gui_w, h, y, ITEM_ROW_HEIGHT, LIST_ROW_WIDTH),
            ScrollList::new(gui_w, h, y, MOB_ROW_HEIGHT, LIST_ROW_WIDTH),
        ];
        lists[0].rows = vec![GENERAL_ROW_HEIGHT; general.len()];
        // The header entry is a real child with the list's own row height, and
        // it exists only when there is at least one item row.
        lists[1].rows = if item_rows.is_empty() {
            Vec::new()
        } else {
            vec![ITEM_ROW_HEIGHT; item_rows.len() + 1]
        };
        lists[2].rows = vec![MOB_ROW_HEIGHT; mobs.len()];
        let mut model = Self {
            tab,
            loading,
            general,
            items: item_rows,
            mobs,
            lists,
            sort_column: sort.0,
            sort_order: sort.1,
            built_from: counter.updates,
        };
        model.sort_items();
        model
    }

    /// `init()`'s widgets: three tabs, the six sort buttons, and `Done`.
    ///
    /// The sort buttons live inside the items list's header *entry*, so they
    /// move with the scroll and vanish when the tab changes. Modelled as
    /// ordinary widgets whose `visible` tracks both — the framework's routing
    /// already skips an invisible widget, so nothing special is needed for
    /// either.
    pub fn build_screen(&self, labels: &StatsLabels, gui_w: i32, gui_h: i32) -> Screen {
        let mut widgets = Vec::with_capacity(10);
        let (tab_x, tab_w) = tab_bar_layout(gui_w, StatsTab::ALL.len() as i32);
        for (i, tab) in StatsTab::ALL.into_iter().enumerate() {
            let mut w = Widget::button(
                tab.widget_id(),
                tab_x + tab_w * i as i32,
                0,
                tab_w,
                HEADER_HEIGHT,
                labels.tabs[i].clone(),
            )
            .with_kind(WidgetKind::Sprites {
                // `MenuTabButton.SPRITES`, in its declared order.
                sprites: WidgetSprites::four(
                    Sprite::TabSelected,
                    Sprite::Tab,
                    Sprite::TabSelectedHighlighted,
                    Sprite::TabHighlighted,
                ),
                // `SPRITES.get(this.isSelected(), …)` — *selected*, not active.
                first: self.tab == tab,
                overlay: None,
                label: true,
            });
            w.active = self.tab_active(tab);
            widgets.push(w);
        }
        // The header entry's six sort buttons, at
        // `contentX + getColumnX(col) - 18`, `contentY + 1`.
        let list = &self.lists[StatsTab::Items.index()];
        let visible = self.tab == StatsTab::Items && !self.items.is_empty();
        let (cx, cy, ..) = list.content_rect(0);
        for col in 0..COLUMNS.len() {
            let mut w = Widget::button(
                SORT_FIRST + col as WidgetId,
                cx + column_x(col) - SLOT_SIZE,
                cy + 1,
                SLOT_SIZE,
                SLOT_SIZE,
                labels.columns[col].clone(),
            )
            .with_kind(WidgetKind::Sprites {
                // `new WidgetSprites(HEADER_SPRITE, SLOT_SPRITE)`.
                sprites: WidgetSprites::two(Sprite::StatHeader, Sprite::Slot),
                // An `ImageButton` passes `isActive()`, which is the *widget's*
                // flag — the other meaning of the same first argument.
                first: true,
                overlay: Some(Sprite::StatColumn(col as u8)),
                label: false,
            });
            w.visible = visible;
            widgets.push(w);
        }
        let (dx, dy) = done_bounds(gui_w, gui_h);
        widgets.push(Widget::button(
            DONE,
            dx,
            dy,
            BUTTON_WIDTH,
            BUTTON_HEIGHT,
            labels.done.clone(),
        ));
        Screen::new(ScreenKind::Stats, gui_w, gui_h)
            .with_widgets(widgets)
            // `Screen.shouldCloseOnEsc()` is not overridden, so Esc closes;
            // `isPauseScreen()` is not overridden either, so it is a pause
            // screen. Both are the defaults the death screen inverted.
            .with_close_on_esc(true)
            .with_pause(true)
    }

    /// The backdrop, which is [`None`] on purpose.
    ///
    /// `StatsScreen` is not an `AbstractContainerScreen`, so `isInGameUi()` is
    /// false and `extractBackground` takes the *menu* branch — no gradient at
    /// all. The two tiled sheets that replace it are chrome the renderer draws,
    /// not a two-stop fill this type can express.
    pub fn backdrop(&self) -> Option<Backdrop> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lang() -> rewo_data::lang::Language {
        rewo_data::lang::Language::from_map(
            [
                ("stat_type.minecraft.killed", "You killed %s %s"),
                (
                    "stat_type.minecraft.killed.none",
                    "You have never killed %s",
                ),
                ("stat_type.minecraft.killed_by", "%s killed you %s time(s)"),
                (
                    "stat_type.minecraft.killed_by.none",
                    "You have never been killed by %s",
                ),
                ("entity.minecraft.zombie", "Zombie"),
                ("entity.minecraft.creeper", "Creeper"),
                ("stat.minecraft.jump", "Jumps"),
                ("stat.minecraft.play_time", "Time Played"),
                ("stat.minecraft.deaths", "Deaths"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        )
    }

    /// The header is the tab bar's height, not the layout constant that shares
    /// a name with the footer's.
    #[test]
    fn the_header_is_twenty_four_and_the_footer_is_thirty_three() {
        assert_eq!(HEADER_HEIGHT, 24);
        assert_eq!(FOOTER_HEIGHT, 33);
        assert_eq!(content_band(240), (24, 240 - 57));
        assert_ne!(
            content_band(240),
            (33, 240 - 66),
            "taking DEFAULT_HEADER_AND_FOOTER_HEIGHT for both moves every row"
        );
    }

    /// `roundToward` rounds **up**, so the three tabs overrun their band.
    #[test]
    fn the_tab_bar_rounds_its_widths_up_to_even_numbers() {
        // 320 GUI px: tabsWidth 292, 292/3 = 97 -> 98, x = roundToward(14, 2).
        assert_eq!(tab_bar_layout(320, 3), (14, 98));
        assert!(98 * 3 > 292, "the three tabs are two pixels wider than the band");
        // Clamped at MAX_WIDTH: a 600-wide window still gets a 372-wide band.
        let (x, w) = tab_bar_layout(600, 3);
        assert_eq!(w, layout::round_toward(372 / 3, 2));
        assert_eq!(x, layout::round_toward((600 - 372) / 2, 2));
        // Round *up*, not to nearest: 97 -> 98 and 99 -> 100.
        assert_eq!(layout::round_toward(97, 2), 98);
        assert_eq!(layout::round_toward(99, 2), 100);
        assert_eq!(layout::round_toward(98, 2), 98, "an exact multiple stays put");
    }

    /// The float lerp truncates, so the button is a pixel above centre.
    #[test]
    fn the_done_button_is_centred_by_a_truncating_lerp() {
        assert_eq!(done_bounds(320, 240), (60, 240 - 33 + 6));
        assert_ne!(
            done_bounds(320, 240).1,
            240 - 33 + 7,
            "(int) 6.5 is 6, and rounding would put it a pixel low"
        );
        // An odd width truncates the same way.
        assert_eq!(done_bounds(321, 240).0, (121.0f32 * 0.5) as i32);
    }

    #[test]
    fn the_column_order_is_not_the_registry_order() {
        assert_eq!(COLUMNS[1], "minecraft:broken");
        // The registry has crafted(1), used(2), broken(3).
        assert_eq!(column_x(0), 75);
        assert_eq!(column_x(5), 75 + 200);
    }

    /// The three-state cycle, driven through all four transitions.
    #[test]
    fn a_fresh_column_sorts_descending_and_the_third_click_clears() {
        let mut m = StatsModel::default();
        m.sort_by_column(2);
        assert_eq!((m.sort_column, m.sort_order), (Some(2), -1));
        m.sort_by_column(2);
        assert_eq!((m.sort_column, m.sort_order), (Some(2), 1));
        m.sort_by_column(2);
        assert_eq!((m.sort_column, m.sort_order), (None, 0));
        // A different column always restarts at descending.
        m.sort_by_column(4);
        assert_eq!((m.sort_column, m.sort_order), (Some(4), -1));
        m.sort_by_column(0);
        assert_eq!((m.sort_column, m.sort_order), (Some(0), -1));
    }

    fn row(name: &str, order: i32, mined: Option<i32>, used: i32) -> ItemRow {
        let mut counts = [None; 6];
        counts[0] = mined;
        counts[3] = Some(used);
        ItemRow {
            name: name.into(),
            short_name: name.into(),
            item_id: Some(order),
            block_id: mined.map(|_| order),
            counts,
            cells: std::array::from_fn(|c| counts[c].map(|v| v.to_string())),
            order,
        }
    }

    /// `key1 == key2 ? sortOrder * compare(ids) : sortOrder * compare(keys)` —
    /// the tie-break is multiplied by `sortOrder` too, so a descending sort
    /// reverses the ids as well as the values.
    #[test]
    fn the_tie_break_is_reversed_along_with_the_values() {
        let mut m = StatsModel {
            items: vec![row("a", 1, None, 5), row("b", 2, None, 5)],
            ..Default::default()
        };
        m.sort_by_column(3);
        assert_eq!(
            m.items.iter().map(|r| r.order).collect::<Vec<_>>(),
            vec![2, 1],
            "equal counts, descending → the ids reverse too"
        );
        m.sort_by_column(3);
        assert_eq!(m.items.iter().map(|r| r.order).collect::<Vec<_>>(), vec![1, 2]);
    }

    /// A row with no block sorts by **-1** in the block column, below a row
    /// whose count is a genuine zero.
    #[test]
    fn a_row_with_no_block_sorts_below_a_real_zero_in_the_mined_column() {
        let mut m = StatsModel {
            items: vec![row("noblock", 1, None, 0), row("zero", 2, Some(0), 0)],
            ..Default::default()
        };
        m.sort_by_column(0);
        assert_eq!(
            m.items[0].name, "zero",
            "descending: 0 above -1"
        );
        m.sort_by_column(0);
        assert_eq!(m.items[0].name, "noblock", "ascending: -1 first");
    }

    /// Clearing the sort freezes the order rather than restoring it.
    #[test]
    fn clearing_the_sort_does_not_restore_the_original_order() {
        let mut m = StatsModel {
            items: vec![row("a", 1, None, 1), row("b", 2, None, 9)],
            ..Default::default()
        };
        m.sort_by_column(3); // descending → b, a
        assert_eq!(m.items[0].name, "b");
        m.sort_by_column(3); // ascending → a, b
        m.sort_by_column(3); // cleared
        assert_eq!((m.sort_column, m.sort_order), (None, 0));
        assert_eq!(m.items[0].name, "a", "the comparator is a no-op, not an undo");
    }

    #[test]
    fn the_two_mob_templates_take_their_arguments_in_opposite_orders() {
        let l = lang();
        let mut c = StatsCounter::default();
        // Fabricate a registry with `killed` = 6 and `killed_by` = 7, which is
        // what the report says.
        c.apply(&[
            (StatKey::new(6, 54), 3),
            (StatKey::new(7, 54), 2),
            (StatKey::new(6, 20), 0),
        ]);
        let reg = fake_registries();
        let types = fake_types();
        let rows = build_mobs(&c, &reg, &types, &l);
        assert_eq!(rows.len(), 1, "a zero-on-both row is dropped");
        assert_eq!(rows[0].kills, "You killed 3 Zombie");
        assert_eq!(
            rows[0].killed_by, "Zombie killed you 2 time(s)",
            "the name comes first in killed_by and second in killed"
        );
        assert!(rows[0].has_kills && rows[0].was_killed_by);
    }

    #[test]
    fn a_mob_you_never_killed_takes_the_none_template_and_the_grey_colour() {
        let l = lang();
        let mut c = StatsCounter::default();
        c.apply(&[(StatKey::new(7, 54), 1)]);
        let rows = build_mobs(&c, &fake_registries(), &fake_types(), &l);
        assert_eq!(rows[0].kills, "You have never killed Zombie");
        assert!(!rows[0].has_kills);
        assert!(rows[0].was_killed_by);
        assert_ne!(ROW_GREY, ROW_DIM, "the zero line is a third colour");
    }

    /// Two registries small enough to reason about, with the ids the real
    /// report gives so the fixture and the wire agree.
    fn fake_registries() -> StatRegistries {
        StatRegistries::from_pairs(
            &[
                (0, "minecraft:mined"),
                (1, "minecraft:crafted"),
                (2, "minecraft:used"),
                (3, "minecraft:broken"),
                (4, "minecraft:picked_up"),
                (5, "minecraft:dropped"),
                (6, "minecraft:killed"),
                (7, "minecraft:killed_by"),
                (8, "minecraft:custom"),
            ],
            &[
                (0, "minecraft:jump"),
                (1, "minecraft:play_time"),
                (2, "minecraft:deaths"),
            ],
            &[(1, "minecraft:stone"), (2, "minecraft:dirt")],
        )
    }

    fn fake_types() -> rewo_data::entity_types::EntityTypes {
        rewo_data::entity_types::EntityTypes::from_pairs(&[
            (54, "minecraft:zombie"),
            (20, "minecraft:creeper"),
            (149, "minecraft:player"),
        ])
        .expect("fixture entity types")
    }

    /// The general list is sorted by the **translated** label, not by id.
    #[test]
    fn the_general_list_sorts_by_the_translated_name() {
        let l = lang();
        let mut c = StatsCounter::default();
        c.apply(&[
            (StatKey::new(8, 0), 12), // jump -> "Jumps"
            (StatKey::new(8, 1), 40), // play_time -> "Time Played"
            (StatKey::new(8, 2), 3),  // deaths -> "Deaths"
        ]);
        let rows = build_general(&c, &fake_registries(), &l);
        assert_eq!(
            rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(),
            vec!["Deaths", "Jumps", "Time Played"],
            "id order would be Jumps, Time Played, Deaths"
        );
        // …and each takes its own formatter: play_time is TIME.
        assert_eq!(rows[1].value, "12");
        assert_eq!(rows[2].value, "2.0 s", "40 ticks");
    }
}
