//! `net.minecraft.client.gui.layouts` — the layout system (M85).
//!
//! [`crate::death_screen`] did not need this. `DeathScreen.init()` writes
//! literal arithmetic (`bounds(this.width / 2 - 100, this.height / 4 + 72, 200,
//! 20)`) and M82 transcribed the arithmetic. **`PauseScreen` and
//! `DialogScreen` do not** — they build a tree of `GridLayout` /
//! `LinearLayout` / `FrameLayout` and call `arrangeElements()`, and the
//! coordinates that come out are not writable in closed form: they depend on
//! the widths of siblings, on a `Divisor`'s remainder distribution, and on two
//! rounding rules that disagree with each other.
//!
//! So this is a transcription of the layout classes themselves rather than of
//! their output. Four of them, plus `LayoutSettings`:
//!
//! | vanilla | here |
//! |---|---|
//! | `LayoutSettings.LayoutSettingsImpl` | [`Settings`] |
//! | `AbstractLayout.AbstractChildWrapper` | [`Child`] |
//! | `GridLayout` (+ `RowHelper`) | [`Grid`] (+ [`RowHelper`]) |
//! | `LinearLayout` | [`Linear`] |
//! | `FrameLayout` | [`Frame`] |
//! | `HeaderAndFooterLayout` | [`HeaderAndFooter`] |
//!
//! # Three things that invert
//!
//! 1. **`setX` truncates and `setY` rounds.** In the same class, four lines
//!    apart:
//!
//!    ```java
//!    // AbstractChildWrapper.setX
//!    int offset = (int)Mth.lerp(this.layoutSettings.xAlignment, leastOffset, mostOffset);
//!    // AbstractChildWrapper.setY
//!    int offset = Math.round(Mth.lerp(this.layoutSettings.yAlignment, leastOffset, mostOffset));
//!    ```
//!
//!    With `align = 0.5F` — which is what every centred cell uses — the two
//!    differ by a pixel on every odd leftover. Using one rule for both axes is
//!    the kind of tidy-up that is invisible until a button is a pixel low.
//!    `FrameLayout.alignInDimension`, which is what actually places the pause
//!    menu on the screen, is a **third** call site and it truncates.
//!
//! 2. **`GridLayout` sizes a spanning child through a `Divisor`, not a
//!    division.** A child occupying two columns contributes
//!    `Divisor(childWidth, 2)` to the two column widths — which yields
//!    `ceil` for the first column and `floor` for the second when the width is
//!    odd, because `Divisor` carries the remainder forward. Dividing evenly
//!    loses a pixel; giving it all to one column misplaces the other. The pause
//!    menu is full of 204-wide children spanning two 98-wide columns, so this
//!    is the arithmetic that decides where the *narrow* buttons sit.
//!
//! 3. **`AbstractLayout.setX` is a *translation*, not an assignment.** It walks
//!    every child adding `x - getX()` first and only then stores the new `x`,
//!    so moving a laid-out tree preserves the relative positions inside it.
//!    Assigning the children's `x` instead would collapse the tree onto one
//!    column, and the collapse would look plausible for a single-column layout
//!    — which the server-links dialog is.
//!
//! # The model
//!
//! Java's layout tree is `LayoutElement`s holding each other by interface.
//! Rust's is an [`Element`] enum, because the set is closed (three shapes) and
//! an enum makes `arrange` a plain recursion with no dynamic dispatch and no
//! interior mutability. A widget is a [`Element::Leaf`] carrying a `key`; the
//! screen builder arranges the tree, then reads the leaves back by key and
//! writes those rectangles onto its [`crate::screen::Widget`]s. That read-back
//! is [`Element::leaves`].
//!
//! # Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/client/gui/layouts/{Layout,LayoutElement,LayoutSettings,
//!   AbstractLayout,GridLayout,LinearLayout,FrameLayout,
//!   HeaderAndFooterLayout}.java`
//! - `com/mojang/math/Divisor.java`
//! - `net/minecraft/util/Mth.java` — `lerp`, `roundToward`, `positiveCeilDiv`

/// `LayoutSettings.LayoutSettingsImpl`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Settings {
    pub padding_left: i32,
    pub padding_top: i32,
    pub padding_right: i32,
    pub padding_bottom: i32,
    pub x_alignment: f32,
    pub y_alignment: f32,
}

impl Settings {
    /// `LayoutSettings.defaults()` — every field zero, i.e. top-left aligned
    /// with no padding.
    pub const fn defaults() -> Self {
        Self {
            padding_left: 0,
            padding_top: 0,
            padding_right: 0,
            padding_bottom: 0,
            x_alignment: 0.0,
            y_alignment: 0.0,
        }
    }

    /// `padding(left, top, right, bottom)`.
    pub const fn padding4(mut self, left: i32, top: i32, right: i32, bottom: i32) -> Self {
        self.padding_left = left;
        self.padding_top = top;
        self.padding_right = right;
        self.padding_bottom = bottom;
        self
    }

    /// `padding(p)` — all four sides.
    pub const fn padding(self, p: i32) -> Self {
        self.padding4(p, p, p, p)
    }

    pub const fn padding_top(mut self, p: i32) -> Self {
        self.padding_top = p;
        self
    }

    /// `align(x, y)`.
    pub const fn align(mut self, x: f32, y: f32) -> Self {
        self.x_alignment = x;
        self.y_alignment = y;
        self
    }

    /// `alignHorizontallyCenter()`.
    pub const fn align_h_center(mut self) -> Self {
        self.x_alignment = 0.5;
        self
    }

    /// `alignVerticallyMiddle()`.
    pub const fn align_v_middle(mut self) -> Self {
        self.y_alignment = 0.5;
        self
    }
}

/// `Mth.lerp(alpha, p0, p1)` — `p0 + alpha * (p1 - p0)`, in that order, because
/// the algebraically equal `p0*(1-alpha) + p1*alpha` rounds differently.
fn lerp(alpha: f32, p0: f32, p1: f32) -> f32 {
    p0 + alpha * (p1 - p0)
}

/// Java's `Math.round(float)` → `int`: `floor(x + 0.5)`, which is **not**
/// Rust's `f32::round` (ties away from zero). They differ at exactly `-0.5`,
/// which a negative leftover reaches.
fn java_round(x: f32) -> i32 {
    (x + 0.5).floor() as i32
}

/// `com.mojang.math.Divisor` — split `numerator` into `denominator` parts whose
/// sizes differ by at most one, distributing the remainder to the **later**
/// parts.
///
/// `Divisor(5, 2)` yields `2, 3` — not `3, 2` and not `2, 2`. The remainder
/// accumulates *before* the comparison (`remainder += mod; if (remainder >=
/// denominator) next++`), so the carry lands on the last part, not the first.
/// The obvious `ceil`-then-`floor` reading is the transposed one, and this
/// module's own test asserted it until the implementation disagreed.
#[derive(Debug)]
pub struct Divisor {
    denominator: i32,
    quotient: i32,
    modulo: i32,
    returned: i32,
    remainder: i32,
}

impl Divisor {
    pub fn new(numerator: i32, denominator: i32) -> Self {
        let (quotient, modulo) = if denominator > 0 {
            (numerator / denominator, numerator % denominator)
        } else {
            (0, 0)
        };
        Self {
            denominator,
            quotient,
            modulo,
            returned: 0,
            remainder: 0,
        }
    }

    /// `nextInt()`. Returns `None` past the end, where vanilla throws.
    pub fn next_int(&mut self) -> Option<i32> {
        if self.returned >= self.denominator {
            return None;
        }
        let mut next = self.quotient;
        self.remainder += self.modulo;
        if self.remainder >= self.denominator {
            self.remainder -= self.denominator;
            next += 1;
        }
        self.returned += 1;
        Some(next)
    }
}

/// One node of a layout tree.
///
/// A `Leaf` is a widget's slot: `key` is how the screen builder finds it again
/// after [`Self::arrange`], and `w`/`h` are the widget's *intrinsic* size (a
/// button's declared width and `Button.DEFAULT_HEIGHT`, a label's
/// `font.width(message)` and 9).
#[derive(Clone, Debug, PartialEq)]
pub enum Element {
    Leaf {
        key: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    },
    Grid(Grid),
    Frame(Frame),
}

impl Element {
    pub fn leaf(key: u32, w: i32, h: i32) -> Self {
        Element::Leaf {
            key,
            x: 0,
            y: 0,
            w,
            h,
        }
    }

    pub fn x(&self) -> i32 {
        match self {
            Element::Leaf { x, .. } => *x,
            Element::Grid(g) => g.x,
            Element::Frame(f) => f.x,
        }
    }

    pub fn y(&self) -> i32 {
        match self {
            Element::Leaf { y, .. } => *y,
            Element::Grid(g) => g.y,
            Element::Frame(f) => f.y,
        }
    }

    pub fn width(&self) -> i32 {
        match self {
            Element::Leaf { w, .. } => *w,
            Element::Grid(g) => g.width,
            Element::Frame(f) => f.width,
        }
    }

    pub fn height(&self) -> i32 {
        match self {
            Element::Leaf { h, .. } => *h,
            Element::Grid(g) => g.height,
            Element::Frame(f) => f.height,
        }
    }

    /// `AbstractLayout.setX` — **translate** the subtree, then store.
    pub fn set_x(&mut self, new_x: i32) {
        match self {
            Element::Leaf { x, .. } => *x = new_x,
            Element::Grid(g) => {
                let d = new_x - g.x;
                for c in &mut g.children {
                    let cx = c.child.element.x();
                    c.child.element.set_x(cx + d);
                }
                g.x = new_x;
            }
            Element::Frame(f) => {
                let d = new_x - f.x;
                for c in &mut f.children {
                    let cx = c.element.x();
                    c.element.set_x(cx + d);
                }
                f.x = new_x;
            }
        }
    }

    /// `AbstractLayout.setY`.
    pub fn set_y(&mut self, new_y: i32) {
        match self {
            Element::Leaf { y, .. } => *y = new_y,
            Element::Grid(g) => {
                let d = new_y - g.y;
                for c in &mut g.children {
                    let cy = c.child.element.y();
                    c.child.element.set_y(cy + d);
                }
                g.y = new_y;
            }
            Element::Frame(f) => {
                let d = new_y - f.y;
                for c in &mut f.children {
                    let cy = c.element.y();
                    c.element.set_y(cy + d);
                }
                f.y = new_y;
            }
        }
    }

    /// `LayoutElement.setPosition`.
    pub fn set_position(&mut self, x: i32, y: i32) {
        self.set_x(x);
        self.set_y(y);
    }

    /// `Layout.arrangeElements`. A leaf is not a `Layout`, so it is a no-op —
    /// which is the whole of vanilla's `instanceof Layout` test.
    pub fn arrange(&mut self) {
        match self {
            Element::Leaf { .. } => {}
            Element::Grid(g) => g.arrange(),
            Element::Frame(f) => f.arrange(),
        }
    }

    /// Every `Leaf` in the tree as `(key, x, y, w, h)`, depth first in
    /// insertion order — the read-back a screen builder uses to move its
    /// widgets onto the arranged rectangles.
    pub fn leaves(&self, out: &mut Vec<(u32, i32, i32, i32, i32)>) {
        match self {
            Element::Leaf { key, x, y, w, h } => out.push((*key, *x, *y, *w, *h)),
            Element::Grid(g) => {
                for c in &g.children {
                    c.child.element.leaves(out);
                }
            }
            Element::Frame(f) => {
                for c in &f.children {
                    c.element.leaves(out);
                }
            }
        }
    }

    /// One leaf's arranged rectangle.
    pub fn leaf_rect(&self, key: u32) -> Option<(i32, i32, i32, i32)> {
        let mut v = Vec::new();
        self.leaves(&mut v);
        v.into_iter()
            .find(|(k, ..)| *k == key)
            .map(|(_, x, y, w, h)| (x, y, w, h))
    }
}

/// `AbstractLayout.AbstractChildWrapper`.
#[derive(Clone, Debug, PartialEq)]
pub struct Child {
    pub element: Element,
    pub settings: Settings,
}

impl Child {
    /// `getWidth()` — the child's own width **plus its horizontal padding**.
    fn outer_width(&self) -> i32 {
        self.element.width() + self.settings.padding_left + self.settings.padding_right
    }

    /// `getHeight()`.
    fn outer_height(&self) -> i32 {
        self.element.height() + self.settings.padding_top + self.settings.padding_bottom
    }

    /// `setX(x, availableSpace)` — truncating.
    fn place_x(&mut self, x: i32, available: i32) {
        let least = self.settings.padding_left as f32;
        let most = (available - self.element.width() - self.settings.padding_right) as f32;
        let offset = lerp(self.settings.x_alignment, least, most) as i32;
        self.element.set_x(offset + x);
    }

    /// `setY(y, availableSpace)` — **rounding**. See the module docs.
    fn place_y(&mut self, y: i32, available: i32) {
        let least = self.settings.padding_top as f32;
        let most = (available - self.element.height() - self.settings.padding_bottom) as f32;
        let offset = java_round(lerp(self.settings.y_alignment, least, most));
        self.element.set_y(offset + y);
    }
}

/// `GridLayout.ChildContainer`.
#[derive(Clone, Debug, PartialEq)]
struct GridChild {
    child: Child,
    row: i32,
    column: i32,
    occupied_rows: i32,
    occupied_columns: i32,
}

impl GridChild {
    fn last_row(&self) -> i32 {
        self.row + self.occupied_rows - 1
    }
    fn last_column(&self) -> i32 {
        self.column + self.occupied_columns - 1
    }
}

/// `GridLayout`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Grid {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    row_spacing: i32,
    column_spacing: i32,
    default_cell: Settings,
    children: Vec<GridChild>,
}

impl Grid {
    pub fn new() -> Self {
        Self {
            default_cell: Settings::defaults(),
            ..Default::default()
        }
    }

    pub fn row_spacing(mut self, s: i32) -> Self {
        self.row_spacing = s;
        self
    }

    pub fn column_spacing(mut self, s: i32) -> Self {
        self.column_spacing = s;
        self
    }

    /// `defaultCellSetting()` — the settings every `newCellSettings()` is
    /// **copied from**, so changing it after adding a child does not
    /// retroactively change that child.
    pub fn default_cell(mut self, s: Settings) -> Self {
        self.default_cell = s;
        self
    }

    /// `newCellSettings()`.
    pub fn new_cell_settings(&self) -> Settings {
        self.default_cell
    }

    /// `addChild(child, row, column, rows, columns, cellSettings)`.
    pub fn add(
        &mut self,
        element: Element,
        row: i32,
        column: i32,
        rows: i32,
        columns: i32,
        settings: Settings,
    ) {
        assert!(rows >= 1, "Occupied rows must be at least 1");
        assert!(columns >= 1, "Occupied columns must be at least 1");
        self.children.push(GridChild {
            child: Child { element, settings },
            row,
            column,
            occupied_rows: rows,
            occupied_columns: columns,
        });
    }

    /// `arrangeElements()`, verbatim.
    pub fn arrange(&mut self) {
        // `super.arrangeElements()` — recurse first, so a nested layout has
        // reported its size before this one measures it.
        for c in &mut self.children {
            c.child.element.arrange();
        }
        if self.children.is_empty() {
            // Vanilla would index `maxColumnWidths[0]` of a 1-length array of
            // zeroes and come out 0x0; the explicit branch says the same thing
            // without an allocation.
            self.width = 0;
            self.height = 0;
            return;
        }
        let max_row = self.children.iter().map(|c| c.last_row()).max().unwrap_or(0);
        let max_col = self
            .children
            .iter()
            .map(|c| c.last_column())
            .max()
            .unwrap_or(0);
        let mut col_widths = vec![0i32; (max_col + 1) as usize];
        let mut row_heights = vec![0i32; (max_row + 1) as usize];

        for c in &self.children {
            let child_h = c.child.outer_height() - (c.occupied_rows - 1) * self.row_spacing;
            let mut hd = Divisor::new(child_h, c.occupied_rows);
            for row in c.row..=c.last_row() {
                let part = hd.next_int().unwrap_or(0);
                let slot = &mut row_heights[row as usize];
                *slot = (*slot).max(part);
            }
            let child_w = c.child.outer_width() - (c.occupied_columns - 1) * self.column_spacing;
            let mut wd = Divisor::new(child_w, c.occupied_columns);
            for col in c.column..=c.last_column() {
                let part = wd.next_int().unwrap_or(0);
                let slot = &mut col_widths[col as usize];
                *slot = (*slot).max(part);
            }
        }

        let mut col_x = vec![0i32; (max_col + 1) as usize];
        let mut row_y = vec![0i32; (max_row + 1) as usize];
        for col in 1..=max_col as usize {
            col_x[col] = col_x[col - 1] + col_widths[col - 1] + self.column_spacing;
        }
        for row in 1..=max_row as usize {
            row_y[row] = row_y[row - 1] + row_heights[row - 1] + self.row_spacing;
        }

        let (gx, gy) = (self.x, self.y);
        for c in &mut self.children {
            let mut available_w = 0;
            for col in c.column..=c.last_column() {
                available_w += col_widths[col as usize];
            }
            available_w += self.column_spacing * (c.occupied_columns - 1);
            c.child.place_x(gx + col_x[c.column as usize], available_w);

            let mut available_h = 0;
            for row in c.row..=c.last_row() {
                available_h += row_heights[row as usize];
            }
            available_h += self.row_spacing * (c.occupied_rows - 1);
            c.child.place_y(gy + row_y[c.row as usize], available_h);
        }

        self.width = col_x[max_col as usize] + col_widths[max_col as usize];
        self.height = row_y[max_row as usize] + row_heights[max_row as usize];
    }
}

/// `Mth.roundToward(input, multiple)` = `positiveCeilDiv(input, multiple) *
/// multiple`.
///
/// **Public since M84**, which needs it for `MenuTabBar.arrangeElements` and
/// had shipped a second copy. Written as `-Math.floorDiv(-input, divisor)`
/// verbatim rather than as `(input + multiple - 1) / multiple`: the two agree
/// on every non-negative input — which is all a `RowHelper` index or a tab
/// width can be — and part company on a negative even one, where Rust's `/`
/// truncates toward zero and Java's `floorDiv` does not.
pub fn round_toward(input: i32, multiple: i32) -> i32 {
    -(-input).div_euclid(multiple) * multiple
}

/// `GridLayout.RowHelper` — the cursor `PauseScreen` fills its grid through.
///
/// Carries the grid rather than borrowing it, because vanilla's is an inner
/// class of the grid and every use here is "build the grid, then take it".
pub struct RowHelper {
    pub grid: Grid,
    columns: i32,
    index: i32,
}

impl RowHelper {
    /// `GridLayout.createRowHelper(columns)`.
    pub fn new(grid: Grid, columns: i32) -> Self {
        Self {
            grid,
            columns,
            index: 0,
        }
    }

    pub fn default_cell_settings(&self) -> Settings {
        self.grid.new_cell_settings()
    }

    /// `addChild(widget, columnWidth, layoutSettings)`.
    ///
    /// The wrap is not a plain `index / columns`: a child too wide for what is
    /// left of the current row moves to the next row **and rounds the index up
    /// to a row boundary**, so the cells it skipped stay empty rather than
    /// being back-filled by the next child.
    pub fn add(&mut self, element: Element, column_width: i32, settings: Settings) {
        let mut row = self.index / self.columns;
        let mut column_begin = self.index % self.columns;
        if column_begin + column_width > self.columns {
            row += 1;
            column_begin = 0;
            self.index = round_toward(self.index, self.columns);
        }
        self.index += column_width;
        self.grid
            .add(element, row, column_begin, 1, column_width, settings);
    }

    /// `addChild(widget)` — one column, the grid's default cell settings.
    pub fn add1(&mut self, element: Element) {
        let s = self.default_cell_settings();
        self.add(element, 1, s);
    }
}

/// `FrameLayout.ChildContainer`'s owner.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    min_width: i32,
    min_height: i32,
    default_child: Settings,
    children: Vec<Child>,
}

impl Frame {
    /// `new FrameLayout()` — and note the default child settings are
    /// `align(0.5F, 0.5F)`, not the zeroed `LayoutSettings.defaults()` a
    /// `GridLayout` uses. A frame centres by default; a grid does not.
    pub fn new() -> Self {
        Self {
            default_child: Settings::defaults().align(0.5, 0.5),
            ..Default::default()
        }
    }

    pub fn set_min_width(&mut self, w: i32) {
        self.min_width = w;
    }

    pub fn set_min_height(&mut self, h: i32) {
        self.min_height = h;
    }

    /// `addChild(child)` — with the frame's default (centred) settings.
    pub fn add(&mut self, element: Element) {
        let settings = self.default_child;
        self.children.push(Child { element, settings });
    }

    /// `arrangeElements()`.
    pub fn arrange(&mut self) {
        for c in &mut self.children {
            c.element.arrange();
        }
        let mut w = self.min_width;
        let mut h = self.min_height;
        for c in &self.children {
            w = w.max(c.outer_width());
            h = h.max(c.outer_height());
        }
        let (fx, fy) = (self.x, self.y);
        for c in &mut self.children {
            c.place_x(fx, w);
            c.place_y(fy, h);
        }
        self.width = w;
        self.height = h;
    }
}

/// `FrameLayout.alignInDimension` — **truncating**, unlike
/// `AbstractChildWrapper.setY`.
///
/// ```java
/// int offset = (int)Mth.lerp(align, 0.0F, length - widgetLength);
/// setWidgetPos.accept(pos + offset);
/// ```
pub fn align_in_dimension(pos: i32, length: i32, widget_length: i32, align: f32) -> i32 {
    let offset = lerp(align, 0.0, (length - widget_length) as f32) as i32;
    pos + offset
}

/// `FrameLayout.alignInRectangle(widget, x, y, width, height, alignX, alignY)`.
pub fn align_in_rectangle(
    element: &mut Element,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    align_x: f32,
    align_y: f32,
) {
    element.set_x(align_in_dimension(x, width, element.width(), align_x));
    element.set_y(align_in_dimension(y, height, element.height(), align_y));
}

/// `FrameLayout.centerInRectangle`.
pub fn center_in_rectangle(element: &mut Element, x: i32, y: i32, width: i32, height: i32) {
    align_in_rectangle(element, x, y, width, height, 0.5, 0.5);
}

/// `HeaderAndFooterLayout` — three stacked frames sized to the screen.
///
/// `DEFAULT_HEADER_AND_FOOTER_HEIGHT` is **33** and `CONTENT_MARGIN_TOP` is
/// **30**, and the content's `y` is
/// `min(headerHeight + 30, screenHeight - footerHeight - contentHeight)` — so a
/// short body sits 30 px below the header and a tall one is pushed up until it
/// touches the footer. Not a clamp to a range: the `min` alone, with no lower
/// bound, so a body taller than the whole content band gets a **negative** `y`
/// and overflows upward. That is vanilla's behaviour, and vanilla avoids it by
/// wrapping the body in a `ScrollableLayout` — see
/// [`crate::server_links_screen`] for what Rewo does instead.
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderAndFooter {
    pub screen_width: i32,
    pub screen_height: i32,
    pub header_height: i32,
    pub footer_height: i32,
    pub header: Frame,
    pub contents: Frame,
    pub footer: Frame,
}

/// `HeaderAndFooterLayout.DEFAULT_HEADER_AND_FOOTER_HEIGHT`.
pub const DEFAULT_HEADER_AND_FOOTER_HEIGHT: i32 = 33;
/// `HeaderAndFooterLayout.CONTENT_MARGIN_TOP`.
pub const CONTENT_MARGIN_TOP: i32 = 30;

impl HeaderAndFooter {
    pub fn new(screen_width: i32, screen_height: i32) -> Self {
        Self {
            screen_width,
            screen_height,
            header_height: DEFAULT_HEADER_AND_FOOTER_HEIGHT,
            footer_height: DEFAULT_HEADER_AND_FOOTER_HEIGHT,
            header: Frame::new(),
            contents: Frame::new(),
            footer: Frame::new(),
        }
    }

    /// `getContentHeight()` — the space the body is allowed, read *before*
    /// the body is arranged (`DialogScreen.init` passes it to the
    /// `ScrollableLayout`'s constructor).
    pub fn content_height(&self) -> i32 {
        self.screen_height - self.header_height - self.footer_height
    }

    /// `arrangeElements()`.
    pub fn arrange(&mut self) {
        let header_height = self.header_height;
        let footer_height = self.footer_height;
        self.header.set_min_width(self.screen_width);
        self.header.set_min_height(header_height);
        let mut header = Element::Frame(std::mem::take(&mut self.header));
        header.set_position(0, 0);
        header.arrange();
        let Element::Frame(header) = header else {
            unreachable!()
        };
        self.header = header;

        self.footer.set_min_width(self.screen_width);
        self.footer.set_min_height(footer_height);
        let mut footer = Element::Frame(std::mem::take(&mut self.footer));
        footer.arrange();
        // **`setY` after `arrangeElements`**, so it translates the arranged
        // children rather than being overwritten by them.
        footer.set_y(self.screen_height - footer_height);
        let Element::Frame(footer) = footer else {
            unreachable!()
        };
        self.footer = footer;

        self.contents.set_min_width(self.screen_width);
        let mut contents = Element::Frame(std::mem::take(&mut self.contents));
        contents.arrange();
        let preferred_y = header_height + CONTENT_MARGIN_TOP;
        let max_y = self.screen_height - footer_height - contents.height();
        contents.set_position(0, preferred_y.min(max_y));
        let Element::Frame(contents) = contents else {
            unreachable!()
        };
        self.contents = contents;
    }

    /// Every leaf across the three frames.
    pub fn leaves(&self) -> Vec<(u32, i32, i32, i32, i32)> {
        let mut out = Vec::new();
        for f in [&self.header, &self.contents, &self.footer] {
            for c in &f.children {
                c.element.leaves(&mut out);
            }
        }
        out
    }
}

/// `LinearLayout` — a [`Grid`] with one row (horizontal) or one column
/// (vertical) and an auto-incrementing index.
///
/// Vanilla's is a *wrapper* around `GridLayout`, not a subclass, and every
/// method forwards. Reproduced that way so the arithmetic has exactly one
/// implementation.
pub struct Linear {
    pub grid: Grid,
    vertical: bool,
    next: i32,
}

impl Linear {
    pub fn vertical() -> Self {
        Self {
            grid: Grid::new(),
            vertical: true,
            next: 0,
        }
    }

    pub fn horizontal() -> Self {
        Self {
            grid: Grid::new(),
            vertical: false,
            next: 0,
        }
    }

    /// `spacing(n)` — **row** spacing when vertical, **column** spacing when
    /// horizontal. Setting both would insert a gap on the axis that has only
    /// one cell, which is invisible; setting the wrong one alone is not.
    pub fn spacing(mut self, n: i32) -> Self {
        self.grid = if self.vertical {
            self.grid.row_spacing(n)
        } else {
            self.grid.column_spacing(n)
        };
        self
    }

    pub fn default_cell(mut self, s: Settings) -> Self {
        self.grid = self.grid.default_cell(s);
        self
    }

    /// `addChild(child, cellSettings)`.
    pub fn add(&mut self, element: Element, settings: Settings) {
        let i = self.next;
        self.next += 1;
        if self.vertical {
            self.grid.add(element, i, 0, 1, 1, settings);
        } else {
            self.grid.add(element, 0, i, 1, 1, settings);
        }
    }

    /// `addChild(child)` — the grid's default cell settings.
    pub fn add1(&mut self, element: Element) {
        let s = self.grid.new_cell_settings();
        self.add(element, s);
    }

    /// Finish, yielding the element to place.
    pub fn into_element(self) -> Element {
        Element::Grid(self.grid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Divisor` distributes the remainder to the **later** parts.
    ///
    /// MUTATION: `numerator / denominator` for every part (which loses the
    /// remainder), `ceil` for every part (which gains one), or the transposed
    /// `ceil`-first reading. `Divisor(5, 2)` separates all four: 2+3 against
    /// 2+2, 3+3 and 3+2. A grid child spanning two columns is sized by exactly
    /// this, so a lost pixel moves the column beside it — and the transposition
    /// is the one that survives an even-width test, which is why 5 and 7 are
    /// sampled rather than 204.
    #[test]
    fn a_divisor_gives_the_remainder_to_the_earlier_parts() {
        let take = |n, d| {
            let mut it = Divisor::new(n, d);
            std::iter::from_fn(move || it.next_int()).collect::<Vec<_>>()
        };
        assert_eq!(take(5, 2), vec![2, 3]);
        assert_eq!(take(4, 2), vec![2, 2]);
        assert_eq!(take(7, 3), vec![2, 2, 3]);
        assert_eq!(take(204, 2), vec![102, 102]);
        // A zero or negative denominator yields nothing at all rather than
        // dividing by zero.
        assert_eq!(take(5, 0), Vec::<i32>::new());
    }

    /// The two rounding rules, on the same numbers.
    ///
    /// MUTATION: using one rule for both axes. With `align = 0.5` and an odd
    /// leftover the truncating and rounding forms differ by a pixel, so a
    /// centred cell would sit one pixel off on whichever axis was changed.
    #[test]
    fn the_x_axis_truncates_where_the_y_axis_rounds() {
        // available 21, child 20, no padding, centred: leftover 1, lerp → 0.5.
        let mk = || Child {
            element: Element::leaf(0, 20, 20),
            settings: Settings::defaults().align(0.5, 0.5),
        };
        let mut cx = mk();
        cx.place_x(0, 21);
        assert_eq!(cx.element.x(), 0, "(int)0.5 truncates to 0");
        let mut cy = mk();
        cy.place_y(0, 21);
        assert_eq!(cy.element.y(), 1, "Math.round(0.5) is 1");
        // And `alignInDimension` is a third call site that truncates.
        assert_eq!(align_in_dimension(0, 21, 20, 0.5), 0);
    }

    /// `AbstractLayout.setX` translates the subtree.
    ///
    /// MUTATION: assigning the children's `x` instead of adding the delta.
    /// A single-column layout would look identical — every child would land on
    /// the same `x` it should have — which is exactly the shape of the
    /// server-links dialog, so the bug would only surface on the pause menu's
    /// two-column row.
    #[test]
    fn moving_a_layout_translates_its_children_rather_than_collapsing_them() {
        let mut g = Grid::new();
        g.add(Element::leaf(0, 40, 20), 0, 0, 1, 1, Settings::defaults());
        g.add(Element::leaf(1, 40, 20), 0, 1, 1, 1, Settings::defaults());
        let mut e = Element::Grid(g);
        e.arrange();
        let mut before = Vec::new();
        e.leaves(&mut before);
        assert_eq!(before[0].1, 0);
        assert_eq!(before[1].1, 40);
        e.set_x(100);
        let mut after = Vec::new();
        e.leaves(&mut after);
        assert_eq!(after[0].1, 100);
        assert_eq!(after[1].1, 140, "the relative offset survives the move");
    }

    /// A child spanning two columns sizes both of them, and the narrow
    /// children in those columns are placed against the spanning child's half.
    ///
    /// This is the pause menu's shape: 204-wide full-row buttons over a pair of
    /// 98-wide half buttons, with 4 px of column spacing.
    #[test]
    fn a_two_column_child_sizes_both_columns_through_the_divisor() {
        let mut g = Grid::new();
        let cell = Settings::defaults().padding4(4, 4, 4, 0);
        g.add(Element::leaf(0, 204, 20), 0, 0, 1, 2, cell);
        g.add(Element::leaf(1, 98, 20), 1, 0, 1, 1, cell);
        g.add(Element::leaf(2, 98, 20), 1, 1, 1, 1, cell);
        let mut e = Element::Grid(g);
        e.arrange();
        // The wide child's outer width is 204 + 8 = 212, minus 0 column
        // spacing (the grid's own is 0 here) → Divisor(212, 2) = 106, 106.
        // The narrow children are 98 + 8 = 106 each, so both columns are 106.
        assert_eq!(e.width(), 212);
        assert_eq!(e.leaf_rect(0), Some((4, 4, 204, 20)));
        assert_eq!(e.leaf_rect(1), Some((4, 28, 98, 20)));
        assert_eq!(e.leaf_rect(2), Some((110, 28, 98, 20)));
    }

    /// `RowHelper` rounds its index up to a row boundary when a wide child does
    /// not fit in what is left.
    ///
    /// MUTATION: `index += columns - columnBegin` or a plain `index = (row+1) *
    /// columns` — both land on the same cell here. The tell is the *next*
    /// child: after a wide child forced a new row, the following narrow child
    /// must start at column 0 of the row after it, not back-fill the gap.
    #[test]
    fn a_wide_child_that_does_not_fit_starts_a_new_row_and_leaves_a_gap() {
        let mut h = RowHelper::new(Grid::new(), 2);
        h.add1(Element::leaf(0, 10, 10)); // row 0, col 0
        h.add(Element::leaf(1, 20, 10), 2, Settings::defaults()); // does not fit → row 1
        h.add1(Element::leaf(2, 10, 10)); // row 2, col 0
        let mut e = Element::Grid(h.grid);
        e.arrange();
        let mut v = Vec::new();
        e.leaves(&mut v);
        assert_eq!(v[0].2, 0, "first child on row 0");
        assert_eq!(v[1].2, 10, "the wide child moved to row 1");
        assert_eq!(v[2].2, 20, "and the next narrow child is on row 2, not row 1");
        assert_eq!(v[2].1, 0, "at column 0");
    }

    /// A frame centres its children by default and a grid does not.
    ///
    /// MUTATION: giving `Frame` the grid's zeroed defaults. Every screen this
    /// module serves centres through a frame, so the whole layout would slide
    /// to the left edge — visible, but only once something is rendered.
    #[test]
    fn a_frame_centres_by_default_where_a_grid_aligns_top_left() {
        let mut f = Frame::new();
        f.set_min_width(100);
        f.set_min_height(40);
        f.add(Element::leaf(0, 20, 10));
        let mut e = Element::Frame(f);
        e.arrange();
        assert_eq!(e.leaf_rect(0), Some((40, 15, 20, 10)));

        let mut g = Grid::new();
        g.add(Element::leaf(0, 20, 10), 0, 0, 1, 1, Settings::defaults());
        let mut e = Element::Grid(g);
        e.arrange();
        assert_eq!(e.leaf_rect(0), Some((0, 0, 20, 10)));
    }

    /// The header/footer bands, and the content's `min`.
    ///
    /// MUTATION: clamping the content `y` into `[preferred, max]` instead of
    /// taking the `min`. A short body is placed identically either way; a body
    /// taller than the band is what separates them, and vanilla lets it go
    /// negative.
    #[test]
    fn the_header_and_footer_bands_are_thirty_three_and_the_content_takes_a_min() {
        let mut hf = HeaderAndFooter::new(320, 240);
        hf.header.add(Element::leaf(0, 60, 9));
        hf.contents.add(Element::leaf(1, 200, 40));
        hf.footer.add(Element::leaf(2, 200, 20));
        assert_eq!(hf.content_height(), 240 - 33 - 33);
        hf.arrange();
        let v = hf.leaves();
        let by = |k: u32| v.iter().find(|e| e.0 == k).copied().unwrap();
        // Header: centred in a 320x33 band at (0, 0).
        assert_eq!(by(0).1, (320 - 60) / 2);
        assert_eq!(by(0).2, (33 - 9) / 2);
        // Contents: 33 + 30 = 63, and the max is 240 - 33 - 40 = 167.
        assert_eq!(by(1).2, 63);
        // Footer: centred in a 320x33 band whose top is 240 - 33 = 207 — and
        // the vertical centring **rounds**, so the 13-px leftover gives 7 and
        // not the 6 an integer division would. That one pixel is the module's
        // first inversion, showing up in the first real screen that uses it.
        assert_eq!(by(2).2, 207 + java_round(13.0 / 2.0));
        assert_eq!(by(2).2, 214);

        // A body taller than the band goes *negative* rather than clamping.
        let mut hf = HeaderAndFooter::new(320, 240);
        hf.contents.add(Element::leaf(1, 200, 400));
        hf.arrange();
        let v = hf.leaves();
        assert!(
            v.iter().find(|e| e.0 == 1).unwrap().2 < 0,
            "min() has no lower bound"
        );
    }

    /// A vertical `Linear` stacks with row spacing and a horizontal one with
    /// column spacing.
    ///
    /// MUTATION: `spacing` setting both. The gap on the single-cell axis is
    /// invisible, so the mutation survives any test that only checks the axis
    /// being stacked — this one checks the *other* axis's extent as well.
    #[test]
    fn a_linear_layout_spaces_only_the_axis_it_stacks_on() {
        let mut v = Linear::vertical().spacing(10);
        v.add1(Element::leaf(0, 30, 20));
        v.add1(Element::leaf(1, 30, 20));
        let mut e = v.into_element();
        e.arrange();
        assert_eq!(e.leaf_rect(1), Some((0, 30, 30, 20)));
        assert_eq!(e.width(), 30, "no horizontal gap on a vertical stack");
        assert_eq!(e.height(), 50);

        let mut h = Linear::horizontal().spacing(4);
        h.add1(Element::leaf(0, 20, 20));
        h.add1(Element::leaf(1, 20, 20));
        h.add1(Element::leaf(2, 20, 20));
        h.add1(Element::leaf(3, 20, 20));
        let mut e = h.into_element();
        e.arrange();
        // The pause menu's icon row: 4 * 20 + 3 * 4 = 92.
        assert_eq!(e.width(), 92);
        assert_eq!(e.height(), 20);
        assert_eq!(e.leaf_rect(3), Some((72, 0, 20, 20)));
    }
}
