//! The screen-space passes: the first-person hand, hotbar item icons, the
//! HUD, the inventory / container screen, the locator bar, the screen-chrome
//! framework and the two text passes — plus [`GuiFrame`], the inputs the
//! `set_*` calls store for them to draw.

use ash::vk;

use super::depth::clear_depth_rect;
use super::{unorm_of, WorldRenderer};
use crate::entities::FontData;
use crate::hud::{HudPass, HudSpritesData};
use crate::text::{TextLine, TextPass};
use crate::Gpu;

/// An owned screen-space text line (the app fills these each frame; the
/// renderer borrows them into `TextLine` at draw time).
pub struct OwnedTextLine {
    pub x: f32,
    pub y: f32,
    pub px: f32,
    /// [`crate::text::TextLine::color_linear`] — **linear**, not the `/255` of
    /// a vanilla colour byte.
    ///
    /// Named for the space rather than called `color` because M130 found nine
    /// of the twenty-two callers handing over the byte: the pass writes this
    /// value straight into an **sRGB** attachment, so a `/255` renders a third
    /// of a stop bright and looks entirely plausible. A field called `color`
    /// admits both readings and cannot be got wrong loudly; this one at least
    /// says which it wants at the point of assignment.
    pub color_linear: [f32; 3],
    pub alpha: f32,
    /// [`crate::text::TextLine::shadow`] — deliberately **not** defaulted, so
    /// adding a caller is a decision rather than an omission. Everything but
    /// M79's XP level number passes `true`.
    pub shadow: bool,
    /// [`crate::text::TextLine::style`] — the five `Style` flags (M126c).
    pub style: crate::text::TextStyle,
    pub text: String,
}

/// One frame's GUI inputs, separate from the renderer's GPU resources.
/// Each field is written by a `WorldRenderer::set_*` call and read when
/// the frame is recorded.
#[derive(Default)]
pub(super) struct GuiFrame {
    /// This frame's screen chrome. Empty = nothing to draw, which is the
    /// in-game case and also a screen that draws only text.
    pub(super) screen_draw: crate::screen::ScreenDraw,
    /// Whether the screen is open, and which slot's GUI-space top-left the
    /// cursor is over.
    pub(super) container_open: Option<Option<(i32, i32)>>,
    /// The open container's panel (M87). `None` means the player's own
    /// inventory, whose 176x166 panel the container pass draws from its own
    /// `inventory.png` rect.
    pub(super) container_panel: Option<crate::container::ContainerPanel>,
    /// The recipe book (M94). Beside the panel, not inside it: the player's
    /// own inventory has no `ContainerPanel` and still has a book.
    pub(super) recipe_book: Option<crate::container::RecipeBookPanel>,
    /// This frame's tooltip: the text block's GUI-space top-left and size,
    /// both measured by the caller (M40), plus the bundle grid inside it if the
    /// hovered stack produced one (M58). The pass owns no font.
    pub(super) container_tooltip: Option<crate::container::TooltipDraw>,
    /// This frame's durability bars, in screen pixels (M41). Independent of
    /// the screen being open — a worn pickaxe shows one in the hotbar.
    pub(super) item_bars: Vec<crate::container::ItemBar>,
    /// This frame's preview matrix and window, or `None` when it is closed.
    pub(super) preview_state: Option<([[f32; 4]; 4], vk::Rect2D)>,
    /// This frame's hand view-projection, or `None` when nothing is drawn.
    pub(super) hand_vp: Option<[[f32; 4]; 4]>,
    pub(super) locator_state: Option<crate::locator_bar::LocatorBarState>,
    /// Live HUD state (health 0..20, food 0..20, selected slot 0..8); when
    /// `None`, no HUD draws (view/demo/bench aren't "playing").
    pub(super) hud_state: Option<(u8, crate::hud::HudGauges)>,
    /// M168 — this frame's survival gauges (`survival_hud::layout`), in GUI
    /// pixels. Replaced every frame with the rest of `hud_state`.
    pub(super) hud_survival: Vec<crate::hud::HudBlit>,
    /// This frame's chat backdrops (M109). A separate field from
    /// [`Self::hud_state`] because it is set on a different cadence — the HUD
    /// state comes from the packet stream and this comes from the chat store's
    /// own geometry — and because an empty slice is a meaningful value (no
    /// chat on screen) rather than an absent one.
    pub(super) hud_fills: Vec<crate::hud::HudFill>,
    /// M151 — this frame's caller-placed HUD sprites (the tab list's ping
    /// icons). Separate from `hud_fills` because they name an atlas rect
    /// rather than a colour.
    pub(super) hud_icons: Vec<crate::hud::HudBlit>,
    pub(super) velvet_runs: Vec<crate::velvet_text::OwnedRun>,
    /// Screen-space text lines to draw this frame (chat, coords); empty →
    /// nothing (view/demo/bench never set them).
    pub(super) text_lines: Vec<OwnedTextLine>,
}

impl WorldRenderer {
    fn retire_container(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.container.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }

    fn retire_hud(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.hud.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }

    fn retire_locator(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.locator.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }

    fn retire_text(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.text.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }

    fn retire_velvet_text(&mut self, gpu: &mut Gpu) {
        if let Some(mut old) = self.velvet_text.take() {
            gpu.wait_idle();
            old.destroy(gpu);
        }
    }

    /// Build the GUI-item pass (M34) over an RGBA atlas the caller packs.
    pub fn init_gui_items(
        &mut self,
        gpu: &mut Gpu,
        atlas_rgba: &[u8],
        atlas_w: u32,
        atlas_h: u32,
    ) -> Result<(), String> {
        // Rebuildable: the atlas is repacked whenever the hotbar needs a
        // texture it does not hold. Same size (the usual case) updates the
        // texels in place; otherwise the old pass — raw handles, not RAII — is
        // retired through deferred destruction, never idled on a gameplay path.
        if let Some(p) = self.gui_items.as_mut() {
            if p.update_atlas(gpu, atlas_rgba, atlas_w, atlas_h)? {
                return Ok(());
            }
        }
        if let Some(mut old) = self.gui_items.take() {
            gpu.defer_destroy(move |g| old.destroy_now(g));
        }
        self.gui_items = Some(crate::gui_item::GuiItemPass::new(
            gpu,
            self.color_format,
            atlas_rgba,
            atlas_w,
            atlas_h,
        )?);
        self.gui_item_generation += 1;
        Ok(())
    }

    /// How many times the GUI-item pass has been (re)built.
    ///
    /// `live --render-check` needs it: a rebuild legitimately discards the
    /// whole ring — `GuiItemPass::destroy` idles first, so nothing in flight is
    /// reading it — and without this the gate would score that as the very
    /// use-after-free it is looking for. It scored exactly one such false
    /// orphan on the hand pass before this existed.
    pub fn gui_item_generation(&self) -> u64 {
        self.gui_item_generation
    }

    /// How many times the hand pass has been (re)built. See
    /// [`Self::gui_item_generation`].
    pub fn hand_generation(&self) -> u64 {
        self.hand_generation
    }

    /// Build the inventory-screen pass (M35).
    pub fn init_container(
        &mut self,
        gpu: &mut Gpu,
        sprites: &crate::container::ContainerSpriteData<'_>,
    ) -> Result<(), String> {
        self.retire_container(gpu);
        self.container = Some(crate::container::ContainerPass::new(
            gpu,
            self.color_format,
            sprites,
        )?);
        Ok(())
    }

    /// Build the screen-framework chrome pass (M82). Idempotent, so the app
    /// can call it the first time any screen opens rather than at startup.
    pub fn init_screen(
        &mut self,
        gpu: &mut Gpu,
        sprites: &crate::screen::WidgetSpriteData<'_>,
    ) -> Result<(), String> {
        if self.screen.is_some() {
            return Ok(());
        }
        self.screen = Some(crate::screen::ScreenPass::new(
            gpu,
            self.color_format,
            sprites,
        )?);
        Ok(())
    }

    pub fn screen_ready(&self) -> bool {
        self.screen.is_some()
    }

    /// This frame's screen chrome. `ScreenDraw::default()` draws nothing.
    pub fn set_screen(&mut self, draw: crate::screen::ScreenDraw) {
        self.gui.screen_draw = draw;
    }

    /// Build the first-person hand pass over an atlas holding the item
    /// textures and the player's skin (M38).
    pub fn init_hand(
        &mut self,
        gpu: &mut Gpu,
        atlas_rgba: &[u8],
        atlas_w: u32,
        atlas_h: u32,
    ) -> Result<(), String> {
        // Rebuildable, like the GUI-item atlas: the resident texture set
        // changes when the held item does. Same-size atlases update in place;
        // otherwise the old pass is retired through deferred destruction.
        if let Some(p) = self.hand.as_mut() {
            if p.update_atlas(gpu, atlas_rgba, atlas_w, atlas_h)? {
                return Ok(());
            }
        }
        if let Some(mut old) = self.hand.take() {
            gpu.defer_destroy(move |g| old.destroy_now(g));
        }
        self.hand = Some(crate::hand_pass::HandPass::new(
            gpu,
            self.color_format,
            atlas_rgba,
            atlas_w,
            atlas_h,
        )?);
        self.hand_generation += 1;
        Ok(())
    }

    pub fn hand_ready(&self) -> bool {
        self.hand.is_some()
    }

    /// Whether the vanilla-font text pass exists (`init_text`) — chat, the
    /// command line, screen labels. Without it `set_text` stores lines that
    /// nothing draws.
    pub fn text_ready(&self) -> bool {
        self.text.is_some()
    }

    /// The raw handle of the buffer the hand pass would bind this frame, or 0.
    ///
    /// For `live --render-check`'s rotation witnesses (M86): the whole point of
    /// [`crate::buf_ring::BufRing`] is that consecutive frames hand back
    /// *different* buffers, and that is only observable from outside if the
    /// handle is. Raw `u64` rather than `vk::Buffer` so the check does not have
    /// to reach into ash's types to compare two of them.
    pub fn hand_vertex_buffer(&self) -> u64 {
        use ash::vk::Handle;
        self.hand
            .as_ref()
            .and_then(|p| p.vertex_buffer())
            .map(|b| b.as_raw())
            .unwrap_or(0)
    }

    /// The raw handle of the buffer the GUI-item pass would bind this frame,
    /// or 0. See [`Self::hand_vertex_buffer`].
    pub fn gui_item_vertex_buffer(&self) -> u64 {
        use ash::vk::Handle;
        self.gui_items
            .as_ref()
            .and_then(|p| p.vertex_buffer())
            .map(|b| b.as_raw())
            .unwrap_or(0)
    }

    /// Every buffer the hand pass is keeping alive. See
    /// [`crate::buf_ring::BufRing::live`] for why the gate needs this rather
    /// than just the current handle.
    pub fn hand_live_buffers(&self) -> Vec<u64> {
        self.hand
            .as_ref()
            .map(|p| p.live_vertex_buffers())
            .unwrap_or_default()
    }

    /// Every buffer the GUI-item pass is keeping alive.
    pub fn gui_item_live_buffers(&self) -> Vec<u64> {
        self.gui_items
            .as_ref()
            .map(|p| p.live_vertex_buffers())
            .unwrap_or_default()
    }

    /// This frame's hand geometry and the matrix that projects it. An empty
    /// vertex list draws nothing, which is the scoping / hidden-hand case.
    pub fn set_hand(
        &mut self,
        gpu: &mut Gpu,
        verts: &[crate::gui_item::GuiItemVertex],
        view_proj: [[f32; 4]; 4],
    ) -> Result<(), String> {
        self.set_hand_with_glint(gpu, verts, &[], view_proj)
    }

    /// The hand plus this frame's enchantment glint (M44), in one buffer.
    pub fn set_hand_with_glint(
        &mut self,
        gpu: &mut Gpu,
        verts: &[crate::gui_item::GuiItemVertex],
        glint: &[crate::gui_item::GuiItemVertex],
        view_proj: [[f32; 4]; 4],
    ) -> Result<(), String> {
        self.gui.hand_vp = (!verts.is_empty()).then_some(view_proj);
        match self.hand.as_mut() {
            Some(p) => p.set_vertices_with_glint(gpu, verts, glint),
            None => Ok(()),
        }
    }

    /// Build the hand glint's pipeline and upload its sheet. Goes with
    /// `init_hand`, which is rebuilt whenever the held item's atlas changes.
    pub fn init_hand_glint(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
    ) -> Result<(), String> {
        // M50: the glint renders into the UNORM view, so its pipeline must
        // declare that format. No counterpart format means no gamma-space
        // target, and the glint is dropped rather than blended in linear light.
        let Some(format) = unorm_of(self.color_format) else {
            return Ok(());
        };
        match self.hand.as_mut() {
            Some(p) => p.init_glint(gpu, rgba, w, h, format),
            None => Ok(()),
        }
    }

    pub fn container_ready(&self) -> bool {
        self.container.is_some()
    }

    /// Open or close the screen for this frame. `hovered` is the GUI-space
    /// top-left of the slot under the cursor, or `None`.
    /// The open container's panel, or `None` for the player's own inventory.
    ///
    /// Separate from [`Self::set_container`] because the two answer different
    /// questions — that one is *whether* a screen is open and what is hovered,
    /// this one is *which* menu — and a container can open and close without
    /// the hover changing.
    pub fn set_container_panel(&mut self, panel: Option<crate::container::ContainerPanel>) {
        self.gui.container_panel = panel;
    }

    /// The panel height the renderer is actually holding, if a container's.
    ///
    /// Read back rather than inferred from the open menu's layout: a fallback
    /// in the panel builder would leave the layout saying 168 while the
    /// renderer drew the player's 166, and a witness that asked the layout
    /// could not tell those apart. This is what the draw uses.
    pub fn container_panel_height(&self) -> Option<f32> {
        self.gui.container_panel.as_ref().map(|p| p.gui_h)
    }

    /// How many overlay sprites this frame's container panel carries (M92).
    ///
    /// Read back OUT of the renderer for M88's reason: asking the open menu
    /// what it *should* draw answers from the model, and the question is
    /// whether the windowed frame loop reached the builder at all.
    pub fn container_panel_overlays(&self) -> usize {
        self.gui
            .container_panel
            .as_ref()
            .map_or(0, |p| p.overlays.len())
    }

    /// How many quads this frame's RECIPE BOOK carries — its panel plus its
    /// sprites — or 0 for a shut book (M94).
    ///
    /// Read out of the renderer for the same reason as the two above: asking
    /// the session whether the book *should* be open answers from the model,
    /// and the question is whether the windowed frame loop reached the book's
    /// builder at all. That is the M86 gap, one feature over.
    pub fn container_panel_book_quads(&self) -> usize {
        self.gui
            .recipe_book
            .as_ref()
            .map_or(0, |b| b.blits.len() + b.overlays.len())
    }

    pub fn set_recipe_book(&mut self, book: Option<crate::container::RecipeBookPanel>) {
        self.gui.recipe_book = book;
    }

    pub fn set_container(&mut self, open: bool, hovered: Option<(i32, i32)>) {
        self.gui.container_open = open.then_some(hovered);
    }

    /// This frame's tooltip, or `None` for no tooltip. The caller measures it
    /// with [`crate::container::tooltip_size`] and places it with
    /// [`crate::container::tooltip_position`]; only the sprites live here,
    /// because the text — the lines, the `+N` badge and the bar's label — goes
    /// through the ordinary text pass.
    pub fn set_container_tooltip(&mut self, tooltip: Option<crate::container::TooltipDraw>) {
        self.gui.container_tooltip = tooltip;
    }

    /// This frame's durability bars (M41). The caller decides which stacks
    /// have one — `isBarVisible` needs the item's max damage, which is not on
    /// the wire.
    pub fn set_item_bars(&mut self, bars: Vec<crate::container::ItemBar>) {
        self.gui.item_bars = bars;
    }

    pub fn gui_items_ready(&self) -> bool {
        self.gui_items.is_some()
    }

    /// This frame's item icons, already placed and shaded by
    /// `gui_item::build_vertices`.
    pub fn set_gui_items(
        &mut self,
        gpu: &mut Gpu,
        verts: &[crate::gui_item::GuiItemVertex],
    ) -> Result<(), String> {
        match self.gui_items.as_mut() {
            Some(p) => p.set_vertices(gpu, verts),
            None => Ok(()),
        }
    }

    /// The icons plus this frame's enchantment glint (M43), which rides in the
    /// same buffer after them.
    pub fn set_gui_items_with_glint(
        &mut self,
        gpu: &mut Gpu,
        verts: &[crate::gui_item::GuiItemVertex],
        glint: &[crate::gui_item::GuiItemVertex],
    ) -> Result<(), String> {
        match self.gui_items.as_mut() {
            Some(p) => p.set_vertices_with_glint(gpu, verts, glint),
            None => Ok(()),
        }
    }

    /// Build the glint's pipeline and upload `misc/enchanted_glint_item.png`.
    ///
    /// The item pass is rebuilt whenever its atlas is repacked, and this goes
    /// with it — hence the call site being beside `init_gui_items` rather than
    /// once at startup.
    pub fn init_gui_glint(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
    ) -> Result<(), String> {
        // M50: the glint renders into the UNORM view, so its pipeline must
        // declare that format. No counterpart format means no gamma-space
        // target, and the glint is dropped rather than blended in linear light.
        let Some(format) = unorm_of(self.color_format) else {
            return Ok(());
        };
        match self.gui_items.as_mut() {
            Some(p) => p.init_glint(gpu, rgba, w, h, format),
            None => Ok(()),
        }
    }

    /// Attach the in-game HUD pass (crosshair/hotbar/hearts/hunger).
    pub fn init_hud(&mut self, gpu: &mut Gpu, sprites: &HudSpritesData<'_>) -> Result<(), String> {
        self.retire_hud(gpu);
        self.hud = Some(HudPass::new(gpu, self.color_format, sprites)?);
        Ok(())
    }

    /// Set this frame's HUD state: the selected slot 0..8, M79's two gauges,
    /// and M168's survival gauges (`survival_hud::layout`'s output, in GUI
    /// pixels). Never called → no HUD (view/demo/bench).
    pub fn set_hud(
        &mut self,
        slot: u8,
        gauges: crate::hud::HudGauges,
        survival: Vec<crate::hud::HudBlit>,
    ) {
        self.gui.hud_state = Some((slot, gauges));
        self.gui.hud_survival = survival;
    }

    /// Set this frame's chat backdrops (M109), in GUI pixels.
    ///
    /// **Replaces**, and an empty slice is how chat says it has nothing to
    /// draw — a stale backdrop under nothing would be a black bar hanging over
    /// the world after the last message faded.
    pub fn set_hud_fills(&mut self, rects: Vec<crate::hud::HudFill>) {
        self.gui.hud_fills = rects;
    }

    /// Set this frame's caller-placed HUD sprites (M151), in GUI pixels.
    ///
    /// **Replaces**, for [`Self::set_hud_fills`]'s reason: the tab list exists
    /// only while its key is held, and a stale list would leave six ping icons
    /// floating over the world the moment it is let go.
    pub fn set_hud_icons(&mut self, icons: Vec<crate::hud::HudBlit>) {
        self.gui.hud_icons = icons;
    }

    /// Attach the locator bar (M83). Independent of `init_hud` because its
    /// sprites bake separately — see `rewo_data::assets::LocatorSprites`.
    pub fn init_locator_bar(
        &mut self,
        gpu: &mut Gpu,
        sprites: &crate::locator_bar::LocatorSpritesData<'_>,
    ) -> Result<(), String> {
        self.retire_locator(gpu);
        self.locator = Some(crate::locator_bar::LocatorBarPass::new(
            gpu,
            self.color_format,
            sprites,
        )?);
        Ok(())
    }

    /// Set (or clear) this frame's locator-bar draw list. `None` is the
    /// contextual bar *not* being the locator bar, which is a different thing
    /// from an empty marker list: `Some(state)` with no markers still paints
    /// the 182x5 strip, because `extractBackground` and `extractRenderState`
    /// are separate calls and only the second one loops.
    pub fn set_locator_bar(&mut self, state: Option<crate::locator_bar::LocatorBarState>) {
        self.gui.locator_state = state;
    }

    /// Attach the screen-space text pass (chat + coords overlay).
    pub fn init_text(&mut self, gpu: &mut Gpu, font: &FontData<'_>) -> Result<(), String> {
        self.retire_text(gpu);
        self.text = Some(TextPass::new(gpu, self.color_format, font)?);
        Ok(())
    }

    /// Replace this frame's screen-space text lines (chat, coords).
    /// Build the Velvet text pass against this target's **UNORM twin**.
    ///
    /// Not the sRGB format: the pass draws inside [`Self::with_gamma_space`],
    /// and Vulkan requires a pipeline's rendering formats to match the
    /// attachment. A target with no mutable-format twin gets no Velvet text at
    /// all rather than text blended in the wrong space.
    pub fn init_velvet_text(
        &mut self,
        gpu: &mut Gpu,
        cache: &crate::velvet_glyph::GlyphCache,
    ) -> Result<(), String> {
        let Some(format) = unorm_of(self.color_format) else {
            return Err("velvet text: target has no UNORM twin".into());
        };
        self.retire_velvet_text(gpu);
        self.velvet_text = Some(crate::velvet_text::VelvetTextPass::new(gpu, format, cache)?);
        Ok(())
    }

    /// Re-upload the glyph atlas if the cache has grown or been written to.
    /// Must run outside a rendering scope — it records a transfer.
    pub fn sync_velvet_atlas(
        &mut self,
        gpu: &mut Gpu,
        cache: &mut crate::velvet_glyph::GlyphCache,
    ) -> Result<(), String> {
        match self.velvet_text.as_mut() {
            Some(p) => p.sync_atlas(gpu, cache),
            None => Ok(()),
        }
    }

    pub fn set_velvet_runs(&mut self, runs: Vec<crate::velvet_text::OwnedRun>) {
        self.gui.velvet_runs = runs;
    }

    pub fn set_text(&mut self, lines: Vec<OwnedTextLine>) {
        self.gui.text_lines = lines;
    }

    /// The screen-space half of [`Self::draw`]: the first-person hand, the HUD,
    /// the inventory screen, the screen chrome and the text passes.
    pub(super) fn draw_gui_passes(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        extent: vk::Extent2D,
    ) {
        // The first-person hand, after the world and before the HUD.
        //
        // Vanilla clears the depth buffer first, which is what stops a wall a
        // block in front of you from cutting your arm in half — the hand is
        // drawn in view space at a fraction of a block, well inside anything
        // the world put there.
        if let (Some(pass), Some(vp)) = (&self.hand, self.gui.hand_vp) {
            clear_depth_rect(gpu, cb, vk::Rect2D::default().extent(extent));
            pass.draw(gpu, cb, vp, extent);
            self.in_gamma_space(gpu, cb, extent, || pass.draw_glint(gpu, cb, vp, extent));
        }
        // HUD, then the inventory screen over it, then text — all screen
        // space. The item icons are one pass drawn once, so when the screen is
        // open they belong to it rather than to the hotbar: the caller feeds
        // `set_gui_items` the screen's 46 slot rects instead of the hotbar's
        // nine, and this order puts them between the two highlight halves,
        // where vanilla puts them.
        let screen = self.gui.container_open.filter(|_| self.container.is_some());
        // The container pass owns the durability bars whether or not the
        // screen is open, because the hotbar draws them too — so its geometry
        // is built unconditionally and only the panel is gated on `open`.
        if let Some(pass) = self.container.as_mut() {
            pass.set_state(
                extent,
                screen.is_some(),
                screen.flatten(),
                self.gui.container_tooltip.as_ref(),
                &self.gui.item_bars,
                self.gui.container_panel.as_ref(),
                self.gui.recipe_book.as_ref(),
            );
        }
        if let (Some(hud), Some((slot, gauges))) = (self.hud.as_mut(), self.gui.hud_state) {
            hud.draw(
                gpu,
                cb,
                extent,
                &self.gui.hud_survival,
                slot,
                gauges,
                &self.gui.hud_fills,
                &self.gui.hud_icons,
            );
            if screen.is_none() {
                // M83 — the contextual bar's slot, which is the XP bar's slot:
                // `Hud.nextContextualInfoState` picks one of them and the
                // caller has already made that choice (`set_locator_bar(None)`
                // when the XP bar won). Behind the hotbar icons for the same
                // reason the XP bar is: it is background chrome.
                if let (Some(pass), Some(state)) =
                    (self.locator.as_mut(), self.gui.locator_state.as_ref())
                {
                    pass.draw(gpu, cb, extent, state);
                }
                if let Some(items) = &self.gui_items {
                    items.draw(gpu, cb, extent);
                    self.in_gamma_space(gpu, cb, extent, || items.draw_glint(gpu, cb, extent));
                }
                // A hotbar bar goes over its icon, same as in the screen.
                if let Some(pass) = &self.container {
                    pass.draw_bars(gpu, cb, extent);
                }
            }
        }
        if let (Some(pass), Some(_)) = (self.container.as_ref(), screen) {
            pass.draw_back(gpu, cb, extent);
            // The player preview sits between the panel and the icons: it is
            // inside the window the panel paints, and an item on the cursor
            // passes over it.
            if let (Some(preview), Some((vp, rect))) = (&self.preview, self.gui.preview_state) {
                clear_depth_rect(gpu, cb, rect);
                preview.draw_solid(gpu, cb, vp, extent);
            }
            if let Some(items) = &self.gui_items {
                items.draw(gpu, cb, extent);
                self.in_gamma_space(gpu, cb, extent, || items.draw_glint(gpu, cb, extent));
            }
            pass.draw_front(gpu, cb, extent);
            pass.draw_bars(gpu, cb, extent);
            // The tooltip box goes over everything else the screen draws. Its
            // text follows in the text pass below, which is drawn last of all.
            pass.draw_tooltip(gpu, cb, extent);
        }
        // The screen framework's chrome (M82), over the HUD and the inventory
        // and under the text pass — vanilla's own order is
        // `extractBackground` then the widgets then the deferred text, and the
        // text pass below is drawn last of all for every caller.
        if let Some(pass) = self.screen.as_mut() {
            pass.set_state(extent, &self.gui.screen_draw);
            pass.draw(gpu, cb, extent);
        }
        if let Some(text) = self.text.as_mut() {
            if !self.gui.text_lines.is_empty() {
                let lines: Vec<TextLine> = self
                    .gui
                    .text_lines
                    .iter()
                    .map(|l| TextLine {
                        x: l.x,
                        y: l.y,
                        px: l.px,
                        color_linear: l.color_linear,
                        alpha: l.alpha,
                        shadow: l.shadow,
                        style: l.style,
                        text: &l.text,
                    })
                    .collect();
                text.draw(gpu, cb, extent, &lines);
            }
        }
        // Velvet text last, and in gamma space. `take` because
        // `in_gamma_space` borrows `&self` while the pass needs `&mut` — the
        // same dance `self.text.take()` does at teardown.
        if !self.gui.velvet_runs.is_empty() {
            if let Some(mut vt) = self.velvet_text.take() {
                let runs: Vec<crate::velvet_text::Run> = self
                    .gui
                    .velvet_runs
                    .iter()
                    .map(|r| crate::velvet_text::Run {
                        glyphs: &r.glyphs,
                        color: r.color,
                        alpha: r.alpha,
                    })
                    .collect();
                self.in_gamma_space(gpu, cb, extent, || vt.draw(gpu, cb, extent, &runs));
                self.velvet_text = Some(vt);
            }
        }
    }
}
