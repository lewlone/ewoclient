//! `AdvancementsScreen` — the advancements tree's layout model (M177).
//!
//! A verbatim transcription of the geometry and clocks in
//! `client/gui/screens/advancements/{AdvancementsScreen,AdvancementTab,
//! AdvancementWidget,AdvancementTabType}.java`, kept pure and testable the way
//! [`crate::book_view_screen`] is: the render lowers these values into blits
//! in `live_cmd`, and clicks read the same rects back.
//!
//! What the model deliberately does NOT own: anything needing the font. The
//! title/description line splits, the tooltip's `width` (which folds measured
//! line widths together with the progress counter's worst case — see
//! [`DisplayInput`]) and the rendered strings arrive pre-measured from the
//! app, where `BakedAssets::lang` and the glyph metrics live. Everything else
//! — tab-strip geometry, the scroll clamps, the connectivity runs, the fade
//! clock, the tooltip's three-way progress-bar rule and both flip axes — is
//! here, exact.
//!
//! # The inversions a tidy rewrite loses
//!
//! - **The tab sprite's first/middle/last choice compares against the TYPE'S
//!   max, not the live tab count** (`AdvancementTabType.extractRenderState`,
//!   `:101-107`). A lone ABOVE tab draws the *left*-cap sprite, and a fourth
//!   tab still draws the middle one.
//! - **The scroll clamp's lower bound is `-(maxX - 234)`, not
//!   `-(maxX - minX)`** (`AdvancementTab.scroll`, `:181`). The two disagree
//!   whenever `minX != 0`, and `Mth.clamp(v, min, max)` with `min > max`
//!   answers `min` — it does not panic like Rust's `f64::clamp`.
//! - **The widget bounding box extends 28 x 27 past each origin**, not the
//!   drawn frame's 26 (`AdvancementTab.addWidget`, `:207-210`). Centering and
//!   scrollability are computed off the larger box.
//! - **A widget's hover box is inclusive on all four edges** (`>=` / `<=`,
//!   `AdvancementWidget.isMouseOver`, `:296`) while the TAB strip's
//!   `isMouseOver` is exclusive (`>` / `<`, `AdvancementTabType.java:157`) —
//!   one pixel apart, different predicates, same screen.
//! - **A hidden advancement renders nothing until done**, including its hover
//!   target (`AdvancementWidget.extractRenderState`, `:155`) — but it still
//!   routes its DESCENDANTS' parent-links through itself
//!   (`getFirstVisibleParent` walks node objects, which is why the model
//!   keeps every node's chain even for nodes that never become widgets).
//! - **The empty-tab labels sit at `56 - 9/2` and `113 - 9`** inside the
//!   contents area — integer division makes the first 52
//!   (`AdvancementsScreen.extractInside`, `:195-198`).
//! - **The connectivity underlay runs 3px wide and the core 1px, and they are
//!   two full passes over the tree** (`extractContents` calls
//!   `extractConnectivity` twice, `:142-143`) — black first, white second, so
//!   the core paints OVER the underlay where they coincide.

/// `WINDOW_WIDTH` / `WINDOW_HEIGHT`; the window.png sheet is 256x256 and the
/// blit samples 252x140 of it.
pub const WINDOW_W: i32 = 252;
pub const WINDOW_H: i32 = 140;
/// `WINDOW_INSIDE_X/Y/W/H`.
pub const INSIDE_X: i32 = 9;
pub const INSIDE_Y: i32 = 18;
pub const INSIDE_W: i32 = 234;
pub const INSIDE_H: i32 = 113;
/// `WINDOW_TITLE_X/Y`.
pub const TITLE_X: i32 = 8;
pub const TITLE_Y: i32 = 6;
/// `BACKGROUND_TILE_WIDTH/HEIGHT` — the root backdrop tiles.
pub const BACKGROUND_TILE: i32 = 16;
/// `SCROLL_SPEED` — the wheel multiplier (`mouseScrolled`, `:185`).
pub const SCROLL_SPEED: f64 = 16.0;

/// The backdrop grid's extents: `x in -1..=15`, `y in -1..=8`
/// (`extractContents`, `:136-139`) — 17 columns x 10 rows of 16px tiles,
/// offset by `scroll mod 16`.
pub const BACKDROP_TILES_X: i32 = 17;
pub const BACKDROP_TILES_Y: i32 = 10;

/// The window rect for a screen of `(width, height)` — centred by integer
/// division (`repositionElements`, `:83-84`).
pub fn window_origin(width: i32, height: i32) -> (i32, i32) {
    ((width - WINDOW_W) / 2, (height - WINDOW_H) / 2)
}

/// The contents-area origin in screen pixels.
pub fn inside_origin(width: i32, height: i32) -> (i32, i32) {
    let (xo, yo) = window_origin(width, height);
    (xo + INSIDE_X, yo + INSIDE_Y)
}

/// `AdvancementType` — mirrored rather than imported so this crate does not
/// depend on the net crate (the wire side lives in
/// `rewo_net::advancements::Frame`; the app converts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    Task,
    Challenge,
    Goal,
}

impl Frame {
    /// `getChatColor` — the description text's colour, read through the
    /// shared named-colour table so the values cannot drift from chat's.
    pub fn chat_color(self) -> u32 {
        let name = match self {
            Frame::Task | Frame::Goal => "green",
            Frame::Challenge => "dark_purple",
        };
        crate::chat_style::NAMED_COLORS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, rgb)| *rgb)
            .unwrap_or(0xFF_FFFF)
    }
}

/// Which sprite group a tab-strip cell uses, in `AdvancementTabType`
/// declaration order. Widths/heights/capacities are the constructor
/// arguments (`AdvancementTabType.java:9-68`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabKind {
    Above,
    Below,
    Left,
    Right,
}

impl TabKind {
    pub fn width(self) -> i32 {
        match self {
            TabKind::Above | TabKind::Below => 28,
            TabKind::Left | TabKind::Right => 32,
        }
    }

    pub fn height(self) -> i32 {
        match self {
            TabKind::Above | TabKind::Below => 32,
            TabKind::Left | TabKind::Right => 28,
        }
    }

    /// `getMax` — how many cells this type holds before the next type starts.
    pub fn max(self) -> usize {
        match self {
            TabKind::Above | TabKind::Below => 8,
            TabKind::Left | TabKind::Right => 5,
        }
    }

    /// `getX(index)`.
    pub fn x_at(self, index: usize) -> i32 {
        match self {
            TabKind::Above | TabKind::Below => (self.width() + 4) * index as i32,
            TabKind::Left => -self.width() + 4,
            TabKind::Right => 248,
        }
    }

    /// `getY(index)`.
    pub fn y_at(self, index: usize) -> i32 {
        match self {
            TabKind::Above => -self.height() + 4,
            TabKind::Below => 136,
            TabKind::Left | TabKind::Right => self.height() * index as i32,
        }
    }

    /// The cell's rect relative to the window origin.
    pub fn rect_at(self, index: usize) -> (i32, i32, i32, i32) {
        (self.x_at(index), self.y_at(index), self.width(), self.height())
    }

    /// `extractIcon`'s per-type icon inset.
    pub fn icon_offset(self) -> (i32, i32) {
        match self {
            TabKind::Above => (6, 9),
            TabKind::Below => (6, 6),
            TabKind::Left => (10, 5),
            TabKind::Right => (6, 5),
        }
    }

    /// `isMouseOver` — **strict** inequalities on every edge.
    pub fn is_mouse_over(self, xo: i32, yo: i32, index: usize, mx: f64, my: f64) -> bool {
        let (x, y, w, h) = self.rect_at(index);
        mx > (xo + x) as f64
            && mx < (xo + x + w) as f64
            && my > (yo + y) as f64
            && my < (yo + y + h) as f64
    }

    /// `extractRenderState`'s cap choice: `index == 0` first, `index ==
    /// max - 1` last, else middle — against the TYPE's capacity, whatever the
    /// live tab count is.
    pub fn sprite_cap(self, index: usize) -> Cap {
        if index == 0 {
            Cap::First
        } else if index == self.max() - 1 {
            Cap::Last
        } else {
            Cap::Middle
        }
    }
}

/// Which of a type's three sprites a cell draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    First,
    Middle,
    Last,
}

/// `AdvancementTab.create` — walk the types in declaration order subtracting
/// capacities until the index fits; `None` past the last (26 tabs).
pub fn tab_slot_for_index(mut index: usize) -> Option<(TabKind, usize)> {
    for kind in [TabKind::Above, TabKind::Below, TabKind::Left, TabKind::Right] {
        if index < kind.max() {
            return Some((kind, index));
        }
        index -= kind.max();
    }
    None
}

/// An icon reference — enough for the GUI-item path to draw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Icon {
    /// Raw registry id (`holderRegistry`).
    pub item: i32,
    pub count: i32,
}

/// One node of the tab's subtree, exactly what the session tree holds plus
/// the app-resolved text. Displayless nodes are carried too — their chain
/// position matters even though they never draw.
#[derive(Debug, Clone)]
pub struct NodeInput {
    pub id: String,
    /// The TREE parent id — possibly an undisplayed ancestor.
    pub parent: Option<String>,
    pub display: Option<DisplayInput>,
    /// Whether this advancement's progress is complete.
    pub done: bool,
    /// `progress.getPercent()` — 0.0 when none was ever sent.
    pub percent: f32,
}

/// The app-resolved display payload (font work lives app-side).
#[derive(Debug, Clone)]
pub struct DisplayInput {
    pub frame: Frame,
    pub hidden: bool,
    /// Grid coordinates straight off the wire (`display.getX()/getY()`).
    pub gx: f32,
    pub gy: f32,
    pub icon: Icon,
    pub background: Option<String>,
    /// The flattened title (the tab strip's label and the tooltip's head).
    pub title: String,
    /// Pre-split title lines (wrap width 163).
    pub title_lines: Vec<String>,
    /// Pre-split description lines, found by the app's `findOptimalLines`
    /// equivalent.
    pub description_lines: Vec<String>,
    /// The tooltip width — `longestDescLine + 3 + 5` after folding in the
    /// measured title width (min 80) and the progress counter's worst case.
    pub width: i32,
    /// `progress.getProgressText()` flattened + measured, iff it shows.
    pub progress_text: Option<(String, i32)>,
}

/// One placed widget — `AdvancementWidget`.
#[derive(Debug, Clone)]
pub struct Widget {
    pub id: String,
    /// The TREE parent id (may be undisplayed); resolved lazily into
    /// [`Self::parent_widget`].
    pub parent_id: Option<String>,
    /// `floor(gx * 28)`, `floor(gy * 27)` — px positions in tab space.
    pub x: i32,
    pub y: i32,
    pub frame: Frame,
    /// `!hidden || done` — gates the draw AND the hover target.
    pub visible: bool,
    pub percent: f32,
    pub done: bool,
    pub icon: Icon,
    pub background: Option<String>,
    pub title: String,
    pub title_lines: Vec<String>,
    pub description_lines: Vec<String>,
    pub width: i32,
    pub progress_text: Option<(String, i32)>,
    /// Index of this widget's first ancestor WITH a display, if any —
    /// `getFirstVisibleParent`, resolved through the node chain, which can
    /// hop OVER undisplayed intermediates.
    pub parent_widget: Option<usize>,
}

impl Widget {
    /// `isMouseOver` — **inclusive** bounds on the 26x26 box.
    pub fn is_mouse_over(&self, sx: i32, sy: i32, mx: i32, my: i32) -> bool {
        if !self.visible {
            return false;
        }
        let x0 = sx + self.x;
        let x1 = x0 + 26;
        let y0 = sy + self.y;
        let y1 = y0 + 26;
        mx >= x0 && mx <= x1 && my >= y0 && my <= y1
    }

    /// The three-way half/frame rule of `extractHover` (`:196-220`), split
    /// out because it is the tooltip's least guessable piece. Returns
    /// `(first_half_width, first_obtained, second_obtained, icon_obtained)`;
    /// `first_half_width` is reset to `width / 2` in every degenerate branch,
    /// which is what makes both bars read as one full-width bar there.
    pub fn bar_split(&self, width: i32) -> (i32, bool, bool, bool) {
        let raw = (self.percent * width as f32).floor() as i32;
        if self.percent >= 1.0 {
            (width / 2, true, true, true)
        } else if raw < 2 {
            (width / 2, false, false, false)
        } else if raw > width - 2 {
            (width / 2, true, true, false)
        } else {
            (raw, true, false, false)
        }
    }
}

/// One tab of the strip — `AdvancementTab`.
#[derive(Debug, Clone)]
pub struct Tab {
    pub kind: TabKind,
    pub index: usize,
    pub root_id: String,
    /// The root's display title.
    pub title: String,
    pub icon: Icon,
    pub background: Option<String>,
    pub widgets: Vec<Widget>,
    /// Every subtree member's `(id, tree-parent)` pair, INCLUDING displayless
    /// ones — `getFirstVisibleParent` walks node objects, so the chain must
    /// survive nodes that never become widgets.
    chain: Vec<(String, Option<String>)>,
    /// Content bounds — note the +28/+27 extents, not the frame's 26.
    pub min_x: i32,
    pub min_y: i32,
    pub max_x: i32,
    pub max_y: i32,
    /// First-view centring latch (`centered`).
    pub centered: bool,
    pub scroll_x: f64,
    pub scroll_y: f64,
    /// Hover-brighten clock: `+= 0.06` toward 0.3 while hovering, `-= 0.12`
    /// toward 0.0 otherwise (`tick`, `:97-104`). The caps mean a hover fades
    /// OUT twice as fast as in, and never past 0.3 while held.
    pub fade: f32,
    pub hovered: Option<usize>,
}

impl Tab {
    /// `AdvancementTab::create` + the root's `AdvancementWidget`. The root
    /// MUST have a display (the caller filters, matching `create`'s null
    /// return).
    pub fn new(kind: TabKind, index: usize, root: &NodeInput) -> Tab {
        let display = root
            .display
            .as_ref()
            .expect("tab roots always have a display");
        let mut tab = Tab {
            kind,
            index,
            root_id: root.id.clone(),
            title: display.title.clone(),
            icon: display.icon,
            background: display.background.clone(),
            widgets: Vec::new(),
            chain: Vec::new(),
            min_x: i32::MAX,
            min_y: i32::MAX,
            max_x: i32::MIN,
            max_y: i32::MIN,
            centered: false,
            scroll_x: 0.0,
            scroll_y: 0.0,
            fade: 0.0,
            hovered: None,
        };
        tab.add_node(root);
        tab
    }

    /// `addAdvancement` + `addWidget` for a subtree member. Displayless nodes
    /// record their chain entry and stop there.
    pub fn add_node(&mut self, input: &NodeInput) {
        // The chain bookkeeping comes first: `attach_all` below may need it.
        self.chain.push((input.id.clone(), input.parent.clone()));
        if let Some(display) = &input.display {
            let x = (display.gx * 28.0).floor() as i32;
            let y = (display.gy * 27.0).floor() as i32;
            let widget = Widget {
                id: input.id.clone(),
                parent_id: input.parent.clone(),
                x,
                y,
                frame: display.frame,
                visible: !display.hidden || input.done,
                percent: input.percent,
                done: input.done,
                icon: display.icon,
                background: display.background.clone(),
                title: display.title.clone(),
                title_lines: display.title_lines.clone(),
                description_lines: display.description_lines.clone(),
                width: display.width,
                progress_text: display.progress_text.clone(),
                parent_widget: None,
            };
            self.widgets.push(widget);

            // `addWidget`'s bounds — +28/+27, NOT the frame's 26.
            let (x1, y1) = (x + 28, y + 27);
            self.min_x = self.min_x.min(x);
            self.max_x = self.max_x.max(x1);
            self.min_y = self.min_y.min(y);
            self.max_y = self.max_y.max(y1);
        }

        // Every existing widget retries `attachToParent` after each add —
        // idempotent, since a resolved parent never resets.
        self.attach_all();
    }

    /// `attachToParent` for every unresolved widget: walk the NODE parent
    /// chain (through undisplayed members) to the first one with a display,
    /// then link to ITS widget. That ancestor is necessarily in this tab —
    /// same subtree — which is why a plain index lookup suffices.
    fn attach_all(&mut self) {
        for i in 0..self.widgets.len() {
            if self.widgets[i].parent_widget.is_some() {
                continue;
            }
            let mut cursor = self.widgets[i].parent_id.clone();
            let mut hops = 0usize;
            while let Some(pid) = cursor {
                hops += 1;
                if hops > self.widgets.len() + 1 {
                    break; // cycle guard; cannot happen in a tree
                }
                match self.widgets.iter().position(|w| w.id == pid) {
                    // The named ancestor IS displayed here: attach to it.
                    Some(idx) => {
                        if idx != i {
                            self.widgets[i].parent_widget = Some(idx);
                        }
                        break;
                    }
                    // Undisplayed (or not yet arrived) — keep walking.
                    None => {
                        cursor = self
                            .chain
                            .iter()
                            .find(|(id, _)| id == &pid)
                            .and_then(|(_, p)| p.clone());
                    }
                }
            }
        }
    }

    /// `extractContents`' centring latch: `scrollX = 117 - (maxX+minX)/2`,
    /// `scrollY = 56 - (maxY+minY)/2` — INTEGER division, truncating toward
    /// zero.
    pub fn ensure_centered(&mut self) {
        if !self.centered {
            self.scroll_x = 117.0 - ((self.max_x + self.min_x) / 2) as f64;
            self.scroll_y = 56.0 - ((self.max_y + self.min_y) / 2) as f64;
            self.centered = true;
        }
    }

    /// `canScrollHorizontally`.
    pub fn can_scroll_horizontally(&self) -> bool {
        self.max_x - self.min_x > INSIDE_W
    }

    /// `canScrollVertically`.
    pub fn can_scroll_vertically(&self) -> bool {
        self.max_y - self.min_y > INSIDE_H
    }

    /// `scroll` — the lower bound is `-(maxX - 234)`, and the clamp is Java's
    /// (a `min` above `max` ANSWERS `min`; Rust's `f64::clamp` would panic).
    pub fn scroll(&mut self, dx: f64, dy: f64) {
        if self.can_scroll_horizontally() {
            let lo = -((self.max_x - INSIDE_W) as f64);
            self.scroll_x = java_clamp(self.scroll_x + dx, lo, 0.0);
        }
        if self.can_scroll_vertically() {
            let lo = -((self.max_y - INSIDE_H) as f64);
            self.scroll_y = java_clamp(self.scroll_y + dy, lo, 0.0);
        }
    }

    /// `tick` — hover detection over the contents-relative mouse. Strict
    /// window (`0 < x < 234`, `0 < y < 113`), inclusive widget boxes, first
    /// hit wins in insertion order.
    pub fn tick(&mut self, rel_mx: i32, rel_my: i32) {
        let mut hovering = false;
        if rel_mx > 0 && rel_mx < INSIDE_W && rel_my > 0 && rel_my < INSIDE_H {
            let (sx, sy) = self.scroll_int();
            for (i, w) in self.widgets.iter().enumerate() {
                if w.is_mouse_over(sx, sy, rel_mx, rel_my) {
                    hovering = true;
                    self.hovered = Some(i);
                    break;
                }
            }
        }
        if hovering {
            self.fade = (self.fade + 0.06).clamp(0.0, 0.3);
        } else {
            self.fade = (self.fade - 0.12).clamp(0.0, 1.0);
            if self.hovered.is_some() {
                self.hovered = None;
            }
        }
    }

    /// The scroll offsets floored to ints — what every draw consumer reads.
    pub fn scroll_int(&self) -> (i32, i32) {
        (self.scroll_x.floor() as i32, self.scroll_y.floor() as i32)
    }

    /// `extractConnectivity` — every widget's link to its parent, as
    /// normalised `(x, y, w, h)` fill rects in CONTENTS space (add the
    /// inside origin and the scroll offset when drawing). Two passes:
    /// `background=true` is the 3px black underlay, `false` the 1px white
    /// core.
    pub fn connectivity(&self, sx: i32, sy: i32, background: bool) -> Vec<(i32, i32, i32, i32)> {
        let mut out = Vec::new();
        for w in &self.widgets {
            let Some(p) = w.parent_widget else { continue };
            let parent = &self.widgets[p];
            // Vanilla's names, kept: dep = the parent's centre, mine = ours,
            // split = where the elbow turns.
            let dep_x = sx + parent.x + 13;
            let split_x = sx + parent.x + 26 + 4;
            let dep_y = sy + parent.y + 13;
            let my_x = sx + w.x + 13;
            let my_y = sy + w.y + 13;
            if background {
                push_hline(&mut out, split_x, dep_x, dep_y - 1);
                push_hline(&mut out, split_x + 1, dep_x, dep_y);
                push_hline(&mut out, split_x, dep_x, dep_y + 1);
                push_hline(&mut out, my_x, split_x - 1, my_y - 1);
                push_hline(&mut out, my_x, split_x - 1, my_y);
                push_hline(&mut out, my_x, split_x - 1, my_y + 1);
                push_vline(&mut out, split_x - 1, my_y, dep_y);
                push_vline(&mut out, split_x + 1, my_y, dep_y);
            } else {
                push_hline(&mut out, split_x, dep_x, dep_y);
                push_hline(&mut out, my_x, split_x, my_y);
                push_vline(&mut out, split_x, my_y, dep_y);
            }
        }
        out
    }
}

/// `Mth.clamp(f64)` — answers `min` when `min > max`, where Rust's
/// `f64::clamp` panics. The scroll call can genuinely construct that
/// (`-(maxX - 234) > 0` while the range's upper end is 0).
fn java_clamp(v: f64, min: f64, max: f64) -> f64 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

/// `horizontalLine(minX, maxX, y)` fills min..=max — normalise to a rect.
fn push_hline(out: &mut Vec<(i32, i32, i32, i32)>, a: i32, b: i32, y: i32) {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    out.push((lo, y, hi - lo + 1, 1));
}

/// `verticalLine(x, minY, maxY)` fills min..=max — normalise to a rect.
fn push_vline(out: &mut Vec<(i32, i32, i32, i32)>, x: i32, a: i32, b: i32) {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    out.push((x, lo, 1, hi - lo + 1));
}

/// The whole screen's model: tabs in insertion order plus the selection.
///
/// Vanilla keys tabs by holder object; Rewo keys by INDEX IN THE TAB LIST,
/// which is what the strip draws and what clicks send back.
#[derive(Debug, Clone, Default)]
pub struct AdvancementsScreen {
    pub tabs: Vec<Tab>,
    /// Index into `tabs`.
    pub selected: Option<usize>,
}

impl AdvancementsScreen {
    /// `onAddAdvancementRoot` — a displayed root becomes a tab; a displayless
    /// one is skipped whole (`AdvancementTab.create` returning null).
    pub fn add_root(&mut self, root: &NodeInput) {
        let Some((kind, index)) = tab_slot_for_index(self.tabs.len()) else {
            log::warn!("advancements: more than 26 tabs — dropping {}", root.id);
            return;
        };
        self.tabs.push(Tab::new(kind, index, root));
    }

    /// `onAddAdvancementTask` — route to the tab whose ROOT matches the
    /// node's subtree root. Returns whether any tab took it.
    pub fn add_task(&mut self, root_id: &str, task: &NodeInput) -> bool {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.root_id == root_id) {
            tab.add_node(task);
            true
        } else {
            false
        }
    }

    /// `onSelectedTabChanged`.
    pub fn select(&mut self, index: Option<usize>) {
        self.selected = index.filter(|i| *i < self.tabs.len());
    }

    /// `extractInside`'s empty state — the two centred labels' positions,
    /// absolute in screen pixels. `56 - 9/2` is 52 (integer division);
    /// `113 - 9` is 104.
    pub fn empty_labels(width: i32, height: i32) -> [(i32, i32); 2] {
        let (xo, yo) = window_origin(width, height);
        let mid_x = xo + INSIDE_X + INSIDE_W / 2;
        [
            (mid_x, yo + INSIDE_Y + 56 - 9 / 2),
            (mid_x, yo + INSIDE_Y + INSIDE_H - 9),
        ]
    }
}

/// The hovered widget's full tooltip geometry — `extractHover`, returned in
/// CONTENTS space (the caller adds the inside origin; tooltips draw OUTSIDE
/// the scissor, which is why the box may hang past the window).
///
/// `leftSide` asks whether the box's RIGHT edge would leave the SCREEN:
/// `screenxo + scrollX + x + width + 26 >= screenWidth` where `screenxo` is
/// the WINDOW's left — note it mixes the window origin, the scroll and the
/// widget position in one sum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TooltipGeom {
    pub widget: usize,
    /// Contents-space positions.
    pub title_left: i32,
    pub title_top: i32,
    pub title_bar_height: i32,
    pub description_left: i32,
    pub description_y: i32,
    pub box_y: i32,
    pub box_height: i32,
    pub left_side: bool,
    pub top_side: bool,
    pub first_half_w: i32,
    pub second_bar_w: i32,
    pub first_obtained: bool,
    pub second_obtained: bool,
    pub icon_obtained: bool,
    /// `if (!this.description.isEmpty())` gates the BOX, not the texts.
    pub show_box: bool,
    /// The 26x26 frame + icon position, `(xo + x + 3, yo + y)` and
    /// `(xo + x + 8, yo + y + 5)`.
    pub frame_pos: (i32, i32),
    pub icon_pos: (i32, i32),
}

/// `extractHover` for a tab's current hover, given the WINDOW left and the
/// screen width for the flip test.
pub fn tooltip_geom(tab: &Tab, window_left: i32, screen_width: i32) -> Option<TooltipGeom> {
    let wi = tab.hovered?;
    let w = &tab.widgets[wi];
    if !w.visible {
        return None;
    }
    let (sx, sy) = tab.scroll_int();
    let left_side =
        window_left + sx + w.x + w.width + 26 >= screen_width;
    let title_bar_height = 9 * w.title_lines.len() as i32 + 9 + 8;
    let title_top = sy + w.y + (26 - title_bar_height) / 2;
    let title_bar_bottom = title_top + title_bar_height;
    let desc_text_height = w.description_lines.len() as i32 * 9;
    let desc_height = 6 + desc_text_height;
    // `titleBarBottom + descriptionHeight >= 113` — against the INSIDE
    // height, not the screen's.
    let top_side = title_bar_bottom + desc_height >= INSIDE_H;
    let title_left = if left_side {
        sx + w.x - w.width + 26 + 6
    } else {
        sx + w.x
    };
    let (first_half_w, first_obt, second_obt, icon_obt) = w.bar_split(w.width);
    let box_height = title_bar_height + desc_height;
    let box_y = if top_side {
        title_bar_bottom - box_height
    } else {
        title_top
    };
    Some(TooltipGeom {
        widget: wi,
        title_left,
        title_top,
        title_bar_height,
        description_left: title_left + 5,
        description_y: if top_side {
            title_top - desc_text_height + 1
        } else {
            title_bar_bottom
        },
        box_y,
        box_height,
        left_side,
        top_side,
        first_half_w,
        second_bar_w: w.width - first_half_w,
        first_obtained: first_obt,
        second_obtained: second_obt,
        icon_obtained: icon_obt,
        show_box: !w.description_lines.is_empty(),
        frame_pos: (sx + w.x + 3, sy + w.y),
        icon_pos: (sx + w.x + 8, sy + w.y + 5),
    })
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn display(x: f32, y: f32) -> DisplayInput {
        DisplayInput {
            frame: Frame::Task,
            hidden: false,
            gx: x,
            gy: y,
            icon: Icon { item: 1, count: 1 },
            background: None,
            title: "T".into(),
            title_lines: vec!["T".into()],
            description_lines: vec![],
            width: 100,
            progress_text: None,
        }
    }

    fn node(id: &str, parent: Option<&str>, x: f32, y: f32) -> NodeInput {
        NodeInput {
            id: id.into(),
            parent: parent.map(str::to_string),
            display: Some(display(x, y)),
            done: false,
            percent: 0.0,
        }
    }

    #[test]
    fn tab_slots_walk_the_type_table_and_exhaust_at_26() {
        assert_eq!(tab_slot_for_index(0), Some((TabKind::Above, 0)));
        assert_eq!(tab_slot_for_index(7), Some((TabKind::Above, 7)));
        assert_eq!(tab_slot_for_index(8), Some((TabKind::Below, 0)));
        assert_eq!(tab_slot_for_index(15), Some((TabKind::Below, 7)));
        assert_eq!(tab_slot_for_index(16), Some((TabKind::Left, 0)));
        // Left's five cells run 16..=20; Right starts at 21.
        assert_eq!(tab_slot_for_index(20), Some((TabKind::Left, 4)));
        assert_eq!(tab_slot_for_index(21), Some((TabKind::Right, 0)));
        assert_eq!(tab_slot_for_index(25), Some((TabKind::Right, 4)));
        assert_eq!(tab_slot_for_index(26), None);
    }

    #[test]
    fn tab_origins_match_getx_gety_per_type() {
        assert_eq!(TabKind::Above.rect_at(0), (0, -28, 28, 32));
        assert_eq!(TabKind::Above.x_at(3), 32 * 3);
        assert_eq!(TabKind::Below.y_at(0), 136);
        assert_eq!(TabKind::Left.x_at(0), -28);
        assert_eq!(TabKind::Left.y_at(2), 56);
        assert_eq!(TabKind::Right.x_at(0), 248);
        assert_eq!(TabKind::Right.y_at(1), 28);
    }

    #[test]
    fn sprite_cap_compares_against_the_types_max_not_the_tab_count() {
        // A lone ABOVE tab (index 0) draws the LEFT-cap sprite…
        assert_eq!(TabKind::Above.sprite_cap(0), Cap::First);
        // …and a fourth draws middle even though only four tabs exist…
        assert_eq!(TabKind::Above.sprite_cap(3), Cap::Middle);
        // …while index 7 draws LAST regardless of how many are live, because
        // the comparison is against the type's declared capacity of 8.
        assert_eq!(TabKind::Above.sprite_cap(7), Cap::Last);
        assert_eq!(TabKind::Right.sprite_cap(4), Cap::Last);
    }

    #[test]
    fn tab_strip_hover_is_strict_and_widget_hover_is_inclusive() {
        // Tab cell at (0, -28) size 28x32: its TOP edge is y=-28. The strip's
        // hover is STRICT, so x=0 itself is outside.
        assert!(TabKind::Above.is_mouse_over(0, 0, 0, 1.0, -27.5));
        assert!(!TabKind::Above.is_mouse_over(0, 0, 0, 0.0, -27.5), "left edge excluded");
        assert!(!TabKind::Above.is_mouse_over(0, 0, 0, 1.0, -28.0), "top edge excluded");

        let mut t = Tab::new(TabKind::Above, 0, &node("r", None, 1.0, 1.0));
        let (sx, sy) = t.scroll_int();
        let w = &t.widgets[0];
        // Widget at floor(28)=28, floor(27)=27; box 26 wide INCLUSIVE.
        assert!(w.is_mouse_over(sx, sy, sx + 28 + 26, sy + 27 + 26), "+26 included");
        assert!(!w.is_mouse_over(sx, sy, sx + 28 + 27, sy + 27), "+27 excluded");
    }

    #[test]
    fn widget_positions_floor_grid_times_pitch() {
        let t = Tab::new(TabKind::Above, 0, &node("r", None, 1.5, 2.0));
        assert_eq!((t.widgets[0].x, t.widgets[0].y), (42, 54)); // 1.5*28, 2*27
    }

    #[test]
    fn bounds_extend_28_by_27_not_the_frames_26() {
        let mut s = AdvancementsScreen::default();
        s.add_root(&node("r", None, 0.0, 0.0));
        let t = &s.tabs[0];
        // Only the root: maxX = 0+28 (not 26), maxY = 0+27 (not 26).
        assert_eq!((t.min_x, t.max_x, t.min_y, t.max_y), (0, 28, 0, 27));
    }

    #[test]
    fn centring_latch_divides_the_sum_as_integers() {
        let mut s = AdvancementsScreen::default();
        // Root at grid 0 (x=0, bound extends to 28); children at grid -1
        // (x=-28) and +1 (x=28, bound to 56). minX=-28, maxX=56:
        // scrollX = 117 - (56 + -28)/2 = 117 - 14 = 103.
        s.add_root(&node("r", None, 0.0, 0.0));
        s.add_task("r", &node("l", Some("r"), -1.0, 0.0));
        s.add_task("r", &node("c", Some("r"), 1.0, 0.0));
        let t = &mut s.tabs[0];
        t.ensure_centered();
        assert_eq!(t.scroll_x, 103.0);
        // This tree spans only 84px, so it cannot scroll at all — a wheel
        // event leaves the centred offset alone.
        assert!(!t.can_scroll_horizontally());
        t.scroll(-16.0, 0.0);
        assert_eq!(t.scroll_x, 103.0);
        // And the latch fires once regardless.
        t.ensure_centered();
        assert_eq!(t.scroll_x, 103.0);
        // Odd sums truncate toward zero: floor positions only.
        s.add_root(&node("r2", None, 0.036, 0.0)); // x = floor(0.036*28)=1
        let t2 = &s.tabs[1];
        assert_eq!(t2.widgets[0].x, 1);
    }

    #[test]
    fn scroll_clamps_to_java_semantics_when_the_lower_bound_turns_positive() {
        let mut s = AdvancementsScreen::default();
        // A wide-but-leftward tree: minX=-200, maxX=0 → span 228 ≤ 234, so
        // make it wider: children out to grid -8 → x=-224, maxX=28.
        s.add_root(&node("r", None, 0.0, 0.0));
        for g in -8i32..=-1 {
            s.add_task("r", &node(&format!("c{g}"), Some("r"), g as f32, 0.0));
        }
        let t = &mut s.tabs[0];
        assert!(t.can_scroll_horizontally());
        // lo = -(28 - 234) = +206 > max 0: Java answers MIN for anything
        // below it. Rust's clamp would panic; vanilla's would snap right.
        t.ensure_centered();
        t.scroll(-16.0, 0.0);
        assert!(
            t.scroll_x >= 0.0 || t.scroll_x == java_clamp(t.scroll_x, 206.0, 0.0),
            "the value must come from the Java clamp, got {}",
            t.scroll_x
        );
        assert_eq!(t.scroll_x, 206.0, "min>max answers min");
    }

    #[test]
    fn vertical_scroll_clamps_normally_when_bounds_are_negative() {
        let mut s = AdvancementsScreen::default();
        s.add_root(&node("r", None, 0.0, 0.0));
        for g in 1..=12 {
            s.add_task("r", &node(&format!("c{g}"), Some("r"), 0.0, g as f32));
        }
        let t = &mut s.tabs[0];
        assert!(t.can_scroll_vertically()); // maxY = 12*27+27 = 351; span > 113
        t.ensure_centered();
        let bottom = -((t.max_y - INSIDE_H) as f64);
        t.scroll(0.0, 999.0);
        assert_eq!(t.scroll_y, 0.0, "upper end pins at 0");
        t.scroll(0.0, -999.0);
        assert_eq!(t.scroll_y, bottom);
    }

    #[test]
    fn hidden_widgets_draw_nothing_and_hover_nothing_until_done_but_still_route_links() {
        let mut s = AdvancementsScreen::default();
        s.add_root(&node("r", None, 0.0, 0.0));
        let mut hidden = node("h", Some("r"), 1.0, 0.0);
        hidden.display.as_mut().unwrap().hidden = true;
        s.add_task("r", &hidden);
        let child = node("c", Some("h"), 2.0, 0.0);
        s.add_task("r", &child);

        let t = &s.tabs[0];
        let h = t.widgets.iter().find(|w| w.id == "h").unwrap();
        assert!(!h.visible);
        let c = t.widgets.iter().find(|w| w.id == "c").unwrap();
        assert!(c.visible);
        // The visible grandchild's link hops THROUGH the hidden parent to…
        // the hidden one is a widget too (it just does not draw), so vanilla
        // attaches to IT: getFirstVisibleParent stops at the first node with
        // a DISPLAY, and hidden still has one.
        assert_eq!(c.parent_widget.map(|p| t.widgets[p].id.as_str()), Some("h"));

        // …but a displayLESS intermediate is skipped entirely.
        let mut s2 = AdvancementsScreen::default();
        s2.add_root(&node("r", None, 0.0, 0.0));
        s2.add_task("r", &NodeInput {
            id: "mid".into(),
            parent: Some("r".into()),
            display: None,
            done: false,
            percent: 0.0,
        });
        s2.add_task("r", &node("leaf", Some("mid"), 2.0, 0.0));
        let t2 = &s2.tabs[0];
        let leaf = t2.widgets.iter().find(|w| w.id == "leaf").unwrap();
        assert_eq!(leaf.parent_widget.map(|p| t2.widgets[p].id.as_str()), Some("r"));
    }

    #[test]
    fn connectivity_runs_match_the_two_pass_shapes() {
        let mut s = AdvancementsScreen::default();
        s.add_root(&node("r", None, 0.0, 0.0));
        s.add_task("r", &node("c", Some("r"), 1.0, 0.0));
        let t = &s.tabs[0];
        let bg = t.connectivity(0, 0, true);
        let fg = t.connectivity(0, 0, false);
        // Background: 6 hlines + 2 vlines; foreground: 2 hlines + 1 vline.
        assert_eq!(bg.len(), 8);
        assert_eq!(fg.len(), 3);
        // Parent at (0,0): dep=(13,13), split=30, mine at (28,0): my=(41,13).
        // First background run: hline(split..dep, depY-1) = x 13..30, y 12,
        // width 30-13+1 = 18.
        assert_eq!(bg[0], (13, 12, 18, 1));
        // Foreground core: hline(split..dep, depY) = (13,13,18,1).
        assert_eq!(fg[0], (13, 13, 18, 1));
        // The vertical: vline(split, myY..depY) = (30, 13, 1, 1) — both ends
        // equal, height 1.
        assert_eq!(fg[2], (30, 13, 1, 1));
    }

    #[test]
    fn fade_rises_slowly_capped_and_falls_fast() {
        let mut s = AdvancementsScreen::default();
        s.add_root(&node("r", None, 1.0, 1.0));
        let t = &mut s.tabs[0];
        let (sx, sy) = t.scroll_int();
        // Hover the root's box (inside-relative coords need +1: strict > 0).
        let mx = sx + t.widgets[0].x + 5;
        let my = sy + t.widgets[0].y + 5;
        for _ in 0..10 {
            t.tick(mx, my);
        }
        assert_eq!(t.fade, 0.3, "caps at 0.3 while held");
        assert_eq!(t.hovered, Some(0));
        t.tick(-5, -5);
        assert_eq!(t.fade, 0.18, "one fall step drops 0.12");
        assert_eq!(t.hovered, None, "cleared the moment the cursor leaves");
    }

    fn bar_widget(percent: f32, width: i32) -> Widget {
        Widget {
            id: "w".into(),
            parent_id: None,
            x: 0,
            y: 0,
            frame: Frame::Task,
            visible: true,
            percent,
            done: false,
            icon: Icon { item: 1, count: 1 },
            background: None,
            title: String::new(),
            title_lines: vec![],
            description_lines: vec![],
            width,
            progress_text: None,
            parent_widget: None,
        }
    }

    #[test]
    #[test]
    fn bar_split_follows_the_three_way_rule() {
        // width 100: raw = floor(p * 100).
        assert_eq!(bar_widget(1.0, 100).bar_split(100), (50, true, true, true), "complete");
        assert_eq!(
            bar_widget(0.0, 100).bar_split(100),
            (50, false, false, false),
            "raw 0 < 2: both halves unobtained"
        );
        assert_eq!(
            bar_widget(0.5, 100).bar_split(100),
            (50, true, false, false),
            "the normal split"
        );
        // raw 98 is NOT > width-2 (98): still the normal split.
        assert_eq!(bar_widget(0.98, 100).bar_split(100), (98, true, false, false));
        // raw 99 > 98: the odd branch — obtained BAR but unobtained ICON.
        assert_eq!(bar_widget(0.99, 100).bar_split(100), (50, true, true, false));
    }

    #[test]
    fn bar_boundaries_are_exclusive_on_both_sides() {
        // width 10: branches at raw < 2 and raw > 8.
        assert_eq!(bar_widget(0.19, 10).bar_split(10), (5, false, false, false), "raw=1");
        assert_eq!(bar_widget(0.20, 10).bar_split(10), (2, true, false, false), "raw=2 normal");
        assert_eq!(bar_widget(0.89, 10).bar_split(10), (8, true, false, false), "raw=8 normal");
        assert_eq!(
            bar_widget(0.90, 10).bar_split(10),
            (5, true, true, false),
            "raw=9 odd branch"
        );
    }

    #[test]
    fn tooltip_geometry_flips_on_both_axes() {
        let mut s = AdvancementsScreen::default();
        let mut d = display(1.0, 1.0);
        d.title_lines = vec!["Title".into()];
        d.description_lines = vec!["desc line".into()];
        d.width = 60;
        s.add_root(&NodeInput {
            id: "r".into(),
            parent: None,
            display: Some(d),
            done: false,
            percent: 0.5,
        });
        let t = &mut s.tabs[0];
        let (sx, sy) = t.scroll_int();
        t.hovered = Some(0);

        // Window at left 0, screen 300 wide: right edge 0+sx+28+60+26 = 114 <
        // 300 → no flip. titleBarHeight = 9+9+8 = 26; titleTop = sy+27+(26-26)/2.
        let g = tooltip_geom(t, 0, 300).unwrap();
        assert!(!g.left_side);
        assert_eq!(g.title_top, sy + t.widgets[0].y);
        assert_eq!(g.frame_pos, (sx + t.widgets[0].x + 3, sy + t.widgets[0].y));

        // Narrow screen: right edge past 300 flips the box to the left.
        let g = tooltip_geom(t, 0, 100).unwrap();
        assert!(g.left_side);
        assert_eq!(g.title_left, sx + t.widgets[0].x - 60 + 26 + 6);
        assert_eq!(g.description_left, g.title_left + 5);
    }

    #[test]
    fn empty_screen_labels_sit_at_52_and_104_inside() {
        // 800x600 screen → window at (274, 230).
        let labels = AdvancementsScreen::empty_labels(800, 600);
        let (xo, yo) = window_origin(800, 600);
        assert_eq!(labels[0], (xo + 9 + 117, yo + 18 + 52));
        assert_eq!(labels[1], (xo + 9 + 117, yo + 18 + 104));
    }
}
