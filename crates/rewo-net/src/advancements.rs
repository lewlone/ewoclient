//! `update_advancements` (130) + `select_advancements_tab` (85) — the
//! advancements screen's two packets, decoded, plus the client-side tree and
//! progress state they feed (M177).
//!
//! `REWO_PACKET_COVERAGE.md` filed both as class C. The decode is ordinary —
//! the reason this waited is the *screen*: a tabbed tree with scissored
//! contents, per-root background textures and hover tooltips. This module is
//! the half that has to come first (M63's split): what the wire says, and what
//! `ClientAdvancements` does with it. The screen model lives in
//! [`rewo_world::advancements_screen`] and the render in `live_cmd`.
//!
//! # Wire shapes, and the two traps in them
//!
//! `ClientboundUpdateAdvancementsPacket.java:41-47`: `bool reset`,
//! `List<AdvancementHolder> added`, `Set<Identifier> removed`,
//! `Map<Identifier, AdvancementProgress> progress`, `bool showAdvancements`.
//!
//! **`announce_to_chat` never crosses the wire.** `DisplayInfo.fromNetwork`
//! (`DisplayInfo.java:141`) hard-codes it to `false` — the flags int carries
//! background-present (bit 0), `showToast` (bit 1) and `hidden` (bit 2) and
//! nothing else. A decoder that invents the fourth flag desyncs on the first
//! display whose bits it misplaces, because the background identifier and the
//! two floats ride *after* the flags word.
//!
//! **`AdvancementType` is `readEnum`** — an array index whose out-of-range
//! read throws (M65's convention; `TASK`=0, `CHALLENGE`=1, `GOAL`=2). The
//! neighbouring flags int, one field later, is a fixed big-endian i32 in a
//! mostly-var-int protocol — M34's trap again.
//!
//! The icon is an `ItemStackTemplate`, decoded through
//! [`crate::component_wire::read_item_template`] so the patch-walk shares one
//! body with every other stack-shaped reader (M61's rule).
//!
//! # The tree, and why insertion runs in passes
//!
//! `AdvancementTree.addAll` (`AdvancementTree.java:54-65`) repeatedly walks
//! the pending list in order, inserting every advancement whose parent is
//! already known, until a pass inserts none; whatever remains is logged and
//! dropped. A server may legitimately send a child before its parent inside
//! one packet, so a single-pass reader drops real advancements. Roots land in
//! first-insertion order and so do tasks — the screen's tab strip and each
//! tab's widget order are exactly those two orders.

use rewo_proto::nbt::Nbt;
use rewo_proto::reader::PacketReader;
use std::collections::HashMap;

/// `AdvancementType` — `readEnum`, so out-of-range is a decode error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    Task,
    Challenge,
    Goal,
}

impl Frame {
    fn from_ordinal(v: i32) -> Result<Frame, String> {
        Ok(match v {
            0 => Frame::Task,
            1 => Frame::Challenge,
            2 => Frame::Goal,
            other => return Err(format!("advancement frame ordinal {other} out of range")),
        })
    }

    /// `getChatColor` — TASK and GOAL are green, CHALLENGE dark purple. The
    /// tooltip's description text is tinted with this
    /// (`AdvancementWidget` ctor, `:69`). Read through the shared named-colour
    /// table rather than re-typed, so the values cannot drift.
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

/// One advancement's `DisplayInfo`, exactly what `DisplayInfo.fromNetwork`
/// builds. The title and description stay NBT components: flattening them
/// needs the language table, which lives app-side (M125's rule).
#[derive(Debug, Clone, PartialEq)]
pub struct WireDisplay {
    pub title: Nbt,
    pub description: Nbt,
    /// The icon template. `item_id` is the **raw** registry id
    /// (`holderRegistry` — M93u's fifth sighting).
    pub icon: crate::component_wire::ItemTemplate,
    pub frame: Frame,
    /// `Optional<ClientAsset.ResourceTexture>` — a texture path identifier
    /// sent only when flags bit 0 is set. A root's tiled backdrop.
    pub background: Option<String>,
    /// Flags bit 1. Defaults `true` in the JSON codec but **is** sent.
    pub show_toast: bool,
    /// Flags bit 2. A hidden advancement renders only once done
    /// (`AdvancementWidget.extractRenderState`, `:155`).
    pub hidden: bool,
    /// Grid coordinates in advancement units — the widget multiplies by
    /// 28 / 27 px (`AdvancementWidget` ctor, `:61-62`). Sent AFTER the
    /// background identifier.
    pub x: f32,
    pub y: f32,
}

/// One `AdvancementHolder` entry of the `added` list.
///
/// The criteria map and rewards are **not on the wire**
/// (`Advancement.read`, `:91-100` decodes both as empty) — the server keeps
/// triggers to itself. What crosses is enough to place and draw the node:
/// parent, display, and the requirement groups that decide "done".
#[derive(Debug, Clone, PartialEq)]
pub struct WireAdvancement {
    pub id: String,
    pub parent: Option<String>,
    pub display: Option<WireDisplay>,
    /// `AdvancementRequirements` — a list of AND-groups, each a list of
    /// criterion names of which any one suffices.
    pub requirements: Vec<Vec<String>>,
    pub sends_telemetry: bool,
}

/// One criterion's completion, `CriterionProgress` — nullable epoch-millis.
pub type CriterionSlot = (String, Option<i64>);

/// One `AdvancementProgress` off the wire: the criteria that have times.
///
/// `update` (below) reshapes this against the tree node's requirements before
/// it is stored, exactly as `ClientAdvancements.update` does — the raw map is
/// never queried directly.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Progress {
    pub criteria: Vec<CriterionSlot>,
}

impl Progress {
    /// `AdvancementProgress.update` (`AdvancementProgress.java:55-64`):
    /// prune every criterion the requirements do not name, then add an empty
    /// slot for every named one missing. After this the criteria set equals
    /// the requirements' name set.
    pub fn update(&mut self, requirements: &[Vec<String>]) {
        let named = |n: &str| requirements.iter().any(|g| g.iter().any(|s| s == n));
        self.criteria.retain(|(name, _)| named(name));
        for group in requirements {
            for name in group {
                if !self.criteria.iter().any(|(n, _)| n == name) {
                    self.criteria.push((name.clone(), None));
                }
            }
        }
    }

    /// `isDone` against requirement groups — any member of a group done
    /// completes the group; every group must complete. **An empty requirement
    /// list is `false`**, not vacuously true (`AdvancementRequirements.test`,
    /// `:64-66`) — the same shape as `getPercent`'s criteria guard.
    pub fn is_done(&self, requirements: &[Vec<String>]) -> bool {
        if requirements.is_empty() {
            return false;
        }
        requirements.iter().all(|group| {
            group
                .iter()
                .any(|name| self.criteria.iter().any(|(n, t)| n == name && t.is_some()))
        })
    }

    /// `hasProgress` — any criterion done at all.
    pub fn has_progress(&self) -> bool {
        self.criteria.iter().any(|(_, t)| t.is_some())
    }

    fn completed_requirement_groups(&self, requirements: &[Vec<String>]) -> usize {
        requirements
            .iter()
            .filter(|group| {
                group
                    .iter()
                    .any(|name| self.criteria.iter().any(|(n, t)| n == name && t.is_some()))
            })
            .count()
    }

    /// `getPercent` — completed requirement groups over total groups. The
    /// guard is on **criteria being empty**, not requirements (`:123-131`);
    /// after `update` the two coincide, so the degenerate division is
    /// unreachable through the stored state.
    pub fn percent(&self, requirements: &[Vec<String>]) -> f32 {
        if self.criteria.is_empty() {
            return 0.0;
        }
        let total = requirements.len() as f32;
        let complete = self.completed_requirement_groups(requirements) as f32;
        complete / total
    }

    /// `getProgressText` — `Some((complete, total))` iff more than one
    /// requirement group exists; rendered as `advancements.progress`.
    pub fn progress_text(&self, requirements: &[Vec<String>]) -> Option<(i32, i32)> {
        if self.criteria.is_empty() {
            return None;
        }
        let total = requirements.len();
        if total <= 1 {
            return None;
        }
        Some((self.completed_requirement_groups(requirements) as i32, total as i32))
    }
}

/// One decoded `ClientboundUpdateAdvancementsPacket`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateAdvancements {
    pub reset: bool,
    pub added: Vec<WireAdvancement>,
    pub removed: Vec<String>,
    pub progress: Vec<(String, Progress)>,
    pub show_advancements: bool,
}

/// `ComponentSerialization.TRUSTED_STREAM_CODEC` — one NBT tag (M125).
fn component(r: &mut PacketReader) -> Result<Nbt, String> {
    r.nbt().map_err(|e| format!("component: {e:?}"))
}

fn identifier(r: &mut PacketReader, what: &'static str) -> Result<String, String> {
    r.identifier().map_err(|e| format!("{what}: {e:?}"))
}

fn parse_display(r: &mut PacketReader) -> Result<WireDisplay, String> {
    let title = component(r)?;
    let description = component(r)?;
    let icon = crate::component_wire::read_item_template(r, 0)
        .map_err(|_| "icon: untranscribed component in the patch".to_string())?
        .ok_or("icon: untranscribed component in the patch")?;
    let frame = Frame::from_ordinal(r.varint().map_err(|e| format!("frame: {e:?}"))?)?;
    // A fixed big-endian i32, not a var-int (`output.writeInt(flags)`).
    let flags = r.i32().map_err(|e| format!("flags: {e:?}"))?;
    let background = if flags & 1 != 0 {
        Some(identifier(r, "background")?)
    } else {
        None
    };
    let show_toast = flags & 2 != 0;
    let hidden = flags & 4 != 0;
    // `announceChat` is NOT on the wire — fromNetwork hard-falses it.
    let x = r.f32().map_err(|e| format!("x: {e:?}"))?;
    let y = r.f32().map_err(|e| format!("y: {e:?}"))?;
    Ok(WireDisplay {
        title,
        description,
        icon,
        frame,
        background,
        show_toast,
        hidden,
        x,
        y,
    })
}

fn parse_advancement(r: &mut PacketReader) -> Result<WireAdvancement, String> {
    let id = identifier(r, "advancement id")?;
    // `readOptional` / `writeOptional` — a bool then the value.
    let parent = if r.bool().map_err(|e| format!("parent flag: {e:?}"))? {
        Some(identifier(r, "parent")?)
    } else {
        None
    };
    let display = if r.bool().map_err(|e| format!("display flag: {e:?}"))? {
        Some(parse_display(r)?)
    } else {
        None
    };
    // `AdvancementRequirements(FriendlyByteBuf)` — list of lists of utf.
    let group_count = r
        .count("requirement groups", 2)
        .map_err(|e| format!("requirements: {e:?}"))?;
    let mut requirements = Vec::with_capacity(group_count.min(1024));
    for _ in 0..group_count {
        let n = r.count("requirement group", 1).map_err(|e| format!("requirement group: {e:?}"))?;
        let mut group = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            group.push(
                r.string(32767)
                    .map_err(|e| format!("criterion name: {e:?}"))?,
            );
        }
        requirements.push(group);
    }
    let sends_telemetry = r.bool().map_err(|e| format!("telemetry: {e:?}"))?;
    Ok(WireAdvancement {
        id,
        parent,
        display,
        requirements,
        sends_telemetry,
    })
}

fn parse_progress(r: &mut PacketReader) -> Result<Progress, String> {
    let n = r
        .count("progress criteria", 3)
        .map_err(|e| format!("progress criteria: {e:?}"))?;
    let mut criteria = Vec::with_capacity(n.min(4096));
    for _ in 0..n {
        let name = r.string(32767).map_err(|e| format!("criterion key: {e:?}"))?;
        // `readNullable(readInstant)` — bool then a BE i64 of epoch millis.
        let obtained = r
            .option(|r| r.i64())
            .map_err(|e| format!("obtained: {e:?}"))?;
        criteria.push((name, obtained));
    }
    Ok(Progress { criteria })
}

/// `ClientboundUpdateAdvancementsPacket.STREAM_CODEC` decode.
pub fn parse_update(body: &[u8]) -> Result<UpdateAdvancements, String> {
    let mut r = PacketReader::new(body);
    let reset = r.bool().map_err(|e| format!("reset: {e:?}"))?;
    let added_count = r
        .count("added advancements", 4)
        .map_err(|e| format!("added: {e:?}"))?;
    let mut added = Vec::with_capacity(added_count.min(8192));
    for _ in 0..added_count {
        added.push(parse_advancement(&mut r)?);
    }
    let removed_count = r
        .count("removed advancements", 2)
        .map_err(|e| format!("removed: {e:?}"))?;
    let mut removed = Vec::with_capacity(removed_count.min(8192));
    for _ in 0..removed_count {
        removed.push(identifier(&mut r, "removed")?);
    }
    let progress_count = r
        .count("progress entries", 4)
        .map_err(|e| format!("progress: {e:?}"))?;
    let mut progress = Vec::with_capacity(progress_count.min(8192));
    for _ in 0..progress_count {
        let id = identifier(&mut r, "progress key")?;
        let p = parse_progress(&mut r)?;
        progress.push((id, p));
    }
    let show_advancements = r.bool().map_err(|e| format!("show: {e:?}"))?;
    Ok(UpdateAdvancements {
        reset,
        added,
        removed,
        progress,
        show_advancements,
    })
}

/// `ClientboundSelectAdvancementsTabPacket` — one nullable identifier. A
/// `None` clears the selection; an id the tree does not know resolves to
/// `null` in `handleSelectAdvancementsTab` and ALSO clears it
/// (`ClientPacketListener.java` — `get` returning null feeds
/// `setSelectedTab(null, false)`), so resolution happens against the tree,
/// not here.
pub fn parse_select_tab(body: &[u8]) -> Result<Option<String>, String> {
    let mut r = PacketReader::new(body);
    if r.bool().map_err(|e| format!("select_tab: {e:?}"))? {
        Ok(Some(identifier(&mut r, "tab")?))
    } else {
        Ok(None)
    }
}

// ─── The client state ────────────────────────────────────────────────────────

/// One node of the client's advancement tree — `AdvancementNode` + the wire
/// payload it wraps.
#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub parent: Option<String>,
    /// Child ids in add order. Not detached on removal (vanilla keeps the
    /// dangling link too); a removal walks children FIRST, so a stale id here
    /// finds an empty map slot and stops.
    pub children: Vec<String>,
    pub advancement: WireAdvancement,
}

impl Node {
    /// Walks up the parent chain to this subtree's root id.
    pub fn root_of<'a>(&self, nodes: &'a HashMap<String, Node>) -> String {
        let mut cur = self;
        let mut guard = 0usize;
        while let Some(p) = &cur.parent {
            match nodes.get(p) {
                Some(next) => cur = next,
                // Unreachable through the tree invariants (a child is only
                // inserted after its parent), guarded anyway.
                None => break,
            }
            guard += 1;
            if guard > nodes.len() {
                break;
            }
        }
        cur.id.clone()
    }
}

/// `ClientAdvancements` + `AdvancementTree` — the client-side state the two
/// packets feed.
///
/// Ordering is load-bearing: roots and tasks are **insertion-ordered** (Java's
/// `ObjectLinkedOpenHashSet`), because the screen's tab strip iterates
/// `roots()` and each tab's widgets arrive in task order. `nodes` itself is a
/// plain map — nothing iterates it.
pub struct ClientAdvancements {
    nodes: HashMap<String, Node>,
    roots: Vec<String>,
    tasks: Vec<String>,
    progress: HashMap<String, Progress>,
    selected_tab: Option<String>,
    /// Mirrors the last `update_advancements`' trailing boolean. Vanilla reads
    /// it inline to gate `AdvancementToast`s — a GUI surface Rewo has not got —
    /// and stores nothing; keeping the last value makes the bit observable
    /// instead of silently dropped.
    pub show_advancements: bool,
}

impl Default for ClientAdvancements {
    fn default() -> Self {
        Self {
            nodes: HashMap::new(),
            roots: Vec::new(),
            tasks: Vec::new(),
            progress: HashMap::new(),
            selected_tab: None,
            show_advancements: false,
        }
    }
}

impl ClientAdvancements {
    /// `ClientAdvancements.update` (`ClientAdvancements.java:36-69`) +
    /// `AdvancementTree`'s halves.
    pub fn apply_update(&mut self, u: UpdateAdvancements) {
        if u.reset {
            // `tree.clear()` + `progress.clear()` — everything, not just
            // displayed nodes.
            self.nodes.clear();
            self.roots.clear();
            self.tasks.clear();
            self.progress.clear();
        }

        self.remove_ids(&u.removed);
        self.add_all(u.added);

        for (id, mut p) in u.progress {
            match self.nodes.get(&id) {
                Some(node) => {
                    // The requirements come from the TREE node — the wire
                    // entry's own copy is empty by construction.
                    p.update(&node.advancement.requirements);
                    self.progress.insert(id, p);
                }
                None => log::warn!(
                    "net: server informed client about progress for unknown advancement {id}"
                ),
            }
        }
        self.show_advancements = u.show_advancements;
    }

    /// `AdvancementTree.remove(Set)` — unknown ids warn; known ones take
    /// their whole subtree with them (children first).
    pub fn remove_ids(&mut self, ids: &[String]) {
        for id in ids {
            if self.nodes.contains_key(id) {
                self.remove_recursive(id);
            } else {
                log::warn!("net: told to remove advancement {id} but I don't know what that is");
            }
        }
    }

    fn remove_recursive(&mut self, id: &str) {
        let children = match self.nodes.get(id) {
            Some(n) => n.children.clone(),
            None => return, // already gone via an explicit earlier removal
        };
        for c in children {
            self.remove_recursive(&c);
        }
        if let Some(node) = self.nodes.remove(id) {
            if node.parent.is_none() {
                self.roots.retain(|r| r != id);
            } else {
                self.tasks.retain(|t| t != id);
            }
            self.progress.remove(id);
        }
    }

    /// `AdvancementTree.addAll` — repeated in-order passes until none can be
    /// inserted; the stuck remainder is logged and dropped (vanilla logs and
    /// breaks).
    pub fn add_all(&mut self, added: Vec<WireAdvancement>) {
        let mut remaining = added;
        while !remaining.is_empty() {
            let mut deferred = Vec::new();
            let before = remaining.len();
            for adv in std::mem::take(&mut remaining) {
                match self.try_insert(adv) {
                    Ok(()) => {}
                    Err(adv) => deferred.push(adv),
                }
            }
            remaining = deferred;
            if remaining.len() == before {
                log::error!(
                    "net: couldn't load {} advancement(s) — no parent ever arrived",
                    remaining.len()
                );
                break;
            }
        }
    }

    /// `AdvancementTree.tryInsert` — `Err(holder)` means "parent not known
    /// yet"; the caller retries it in the next pass.
    fn try_insert(&mut self, adv: WireAdvancement) -> Result<(), WireAdvancement> {
        if let Some(parent) = &adv.parent {
            if !self.nodes.contains_key(parent) {
                return Err(adv);
            }
        }
        let id = adv.id.clone();
        let parent = adv.parent.clone();
        if let Some(pid) = &parent {
            if let Some(pnode) = self.nodes.get_mut(pid) {
                if !pnode.children.iter().any(|c| c == &id) {
                    pnode.children.push(id.clone());
                }
            }
        }
        let is_root = parent.is_none();
        self.nodes.insert(
            id.clone(),
            Node {
                id: id.clone(),
                parent,
                children: Vec::new(),
                advancement: adv,
            },
        );
        if is_root {
            if !self.roots.iter().any(|r| r == &id) {
                self.roots.push(id);
            }
        } else if !self.tasks.iter().any(|t| t == &id) {
            self.tasks.push(id);
        }
        Ok(())
    }

    /// `setSelectedTab(selectedTab, tellServer = false)` — the packet-driven
    /// half, which never tells the server. Returns whether the selection
    /// changed.
    ///
    /// Vanilla compares holder REFERENCES (`this.selectedTab !=
    /// selectedTab`), so re-selecting through an equal-but-distinct holder
    /// object would re-fire the listener; comparing ids cannot observe the
    /// difference through any state query, because the listener only reads
    /// the holder back.
    pub fn select_tab(&mut self, tab: Option<&str>) -> bool {
        if self.selected_tab.as_deref() != tab {
            self.selected_tab = tab.map(str::to_string);
            true
        } else {
            false
        }
    }

    pub fn selected_tab(&self) -> Option<&str> {
        self.selected_tab.as_deref()
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn progress(&self, id: &str) -> Option<&Progress> {
        self.progress.get(id)
    }

    /// `AdvancementProgress.isDone` for a stored advancement — every
    /// requirement group must contain a done criterion; **an empty requirement
    /// list is `false`**, not vacuously true (`AdvancementRequirements.test`,
    /// `:64-66`). A known node with NO progress entry yet is **not done**
    /// (vanilla reads `progress == null ? 0.0F : …`), and an unknown id is
    /// `None`.
    ///
    /// One body, on [`Progress`]: the battery's survivor taught why — a second
    /// copy of the AND/ANY rule on this type had no caller and no witness, so
    /// flipping it to ANY survived a full green suite. Ask through here.
    pub fn is_done(&self, id: &str) -> Option<bool> {
        let node = self.nodes.get(id)?;
        Some(self
            .progress
            .get(id)
            .map_or(false, |p| p.is_done(&node.advancement.requirements)))
    }

    /// The roots, in insertion order — the screen's tab strip.
    ///
    /// Only roots WITH a display become tabs (`AdvancementTab.create` returns
    /// null otherwise, `:162-166`); the filtering happens here so callers see
    /// the tab list, not the tree.
    pub fn tabs(&self) -> Vec<&Node> {
        self.roots
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .filter(|n| n.advancement.display.is_some())
            .collect()
    }

    /// The displayed descendants of `root_id`, in task insertion order — one
    /// tab's widgets. Undisplayed nodes are skipped (`addAdvancement` gates
    /// on the display being present) but still route their children's
    /// parent-links through themselves (`getFirstVisibleParent`).
    pub fn tab_tasks(&self, root_id: &str) -> Vec<&Node> {
        self.tasks
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .filter(|n| n.advancement.display.is_some() && n.root_of(&self.nodes) == root_id)
            .collect()
    }

    pub fn len_roots(&self) -> usize {
        self.roots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_proto::writer::PacketWriter;

    /// A network-NBT string tag: `0x08` + u16 BE length + utf-8. The only
    /// shape the fixtures need — a title or description component.
    fn nbt_string(s: &str) -> Vec<u8> {
        let mut out = vec![0x08];
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s.as_bytes());
        out
    }

    fn display_bytes(title: &str, frame: i32, flags: i32, x: f32, y: f32) -> Vec<u8> {
        let mut w = PacketWriter::default();
        w.raw(&nbt_string(title));
        w.raw(&nbt_string("desc"));
        // icon template: item 1, count 1, empty patch (added 0, removed 0)
        w.varint(1).varint(1).varint(0).varint(0);
        w.varint(frame);
        w.i32(flags);
        if flags & 1 != 0 {
            w.string("minecraft:textures/gui/advancements/backgrounds/stone.png");
        }
        w.f32(x).f32(y);
        w.buf
    }

    fn advancement_bytes(id: &str, parent: Option<&str>, display: Option<&[u8]>) -> Vec<u8> {
        let mut w = PacketWriter::default();
        w.string(id);
        match parent {
            Some(p) => {
                w.bool(true).string(p);
            }
            None => {
                w.bool(false);
            }
        }
        match display {
            Some(d) => {
                w.bool(true).raw(d);
            }
            None => {
                w.bool(false);
            }
        }
        // requirements: one group of one criterion
        w.varint(1);
        w.varint(1);
        w.string("c1");
        w.bool(false); // sendsTelemetryEvent
        w.buf
    }

    fn update_bytes(
        reset: bool,
        added: &[Vec<u8>],
        removed: &[&str],
        progress: &[(&str, &[(&str, Option<i64>)])],
        show: bool,
    ) -> Vec<u8> {
        let mut w = PacketWriter::default();
        w.bool(reset);
        w.varint(added.len() as i32);
        for a in added {
            w.raw(a);
        }
        w.varint(removed.len() as i32);
        for r in removed {
            w.string(r);
        }
        w.varint(progress.len() as i32);
        for (id, criteria) in progress {
            w.string(id);
            w.varint(criteria.len() as i32);
            for (name, t) in *criteria {
                w.string(name);
                match t {
                    Some(millis) => {
                        w.bool(true).i64(*millis);
                    }
                    None => {
                        w.bool(false);
                    }
                }
            }
        }
        w.bool(show);
        w.buf
    }

    const TASK: i32 = 0;
    const CHALLENGE: i32 = 1;

    #[test]
    fn decodes_a_full_update_packet() {
        let body = update_bytes(
            true,
            &[
                advancement_bytes(
                    "a:root",
                    None,
                    Some(&display_bytes("Root", TASK, 0b010, 0.0, 0.0)),
                ),
                advancement_bytes("a:child", Some("a:root"), None),
            ],
            &["a:gone"],
            &[("a:root", &[("c1", Some(1_700_000_000_000))])],
            false,
        );
        let u = parse_update(&body).expect("decodes");
        assert!(u.reset);
        assert_eq!(u.added.len(), 2);
        assert_eq!(u.removed, vec!["a:gone".to_string()]);
        assert!(!u.show_advancements);

        let d = u.added[0].display.as_ref().unwrap();
        assert_eq!(d.title.as_str(), Some("Root"));
        assert_eq!(d.frame, Frame::Task);
        assert!(d.show_toast, "flags bit 1");
        assert!(!d.hidden);
        assert_eq!(d.background, None);
        assert_eq!((d.x, d.y), (0.0, 0.0));
        assert_eq!(d.icon.item_id, 1);
        assert_eq!(d.icon.count, 1);
        assert_eq!(u.added[1].parent.as_deref(), Some("a:root"));
        assert_eq!(
            u.progress[0].1.criteria,
            vec![("c1".to_string(), Some(1_700_000_000_000))]
        );
    }

    #[test]
    fn background_identifier_rides_between_flags_and_xy_only_when_bit0_set() {
        let body = update_bytes(
            false,
            &[advancement_bytes(
                "a:r",
                None,
                Some(&display_bytes(
                    "R",
                    TASK,
                    0b001,
                    1.5,
                    2.5,
                )),
            )],
            &[],
            &[],
            true,
        );
        let u = parse_update(&body).expect("decodes");
        let d = u.added[0].display.as_ref().unwrap();
        assert_eq!(
            d.background.as_deref(),
            Some("minecraft:textures/gui/advancements/backgrounds/stone.png")
        );
        assert!(!d.show_toast, "bit 1 unset");
        assert!(!d.hidden, "bit 2 unset");
        assert_eq!((d.x, d.y), (1.5, 2.5));
    }

    #[test]
    fn frame_ordinal_out_of_range_is_an_error_not_a_default() {
        let body = update_bytes(
            false,
            &[advancement_bytes(
                "a:x",
                None,
                Some(&display_bytes("T", 7, 0, 0.0, 0.0)),
            )],
            &[],
            &[],
            true,
        );
        let err = parse_update(&body).unwrap_err();
        assert!(err.contains("ordinal"), "{err}");
    }

    #[test]
    fn challenge_frame_reads_and_carries_dark_purple() {
        let body = update_bytes(
            false,
            &[advancement_bytes(
                "a:c",
                None,
                Some(&display_bytes("C", CHALLENGE, 0b100, 0.0, 0.0)),
            )],
            &[],
            &[],
            true,
        );
        let u = parse_update(&body).expect("decodes");
        let d = u.added[0].display.as_ref().unwrap();
        assert_eq!(d.frame, Frame::Challenge);
        assert!(d.hidden);
        assert_eq!(d.frame.chat_color(), 0xAA_00AA);
    }

    #[test]
    fn goal_frame_shares_task_green_but_is_its_own_ordinal() {
        let body = update_bytes(
            false,
            &[advancement_bytes(
                "a:g",
                None,
                Some(&display_bytes("G", 2, 0, 0.0, 0.0)),
            )],
            &[],
            &[],
            true,
        );
        let u = parse_update(&body).expect("decodes");
        let d = u.added[0].display.as_ref().unwrap();
        assert_eq!(d.frame, Frame::Goal);
        assert_eq!(d.frame.chat_color(), Frame::Task.chat_color());
    }

    #[test]
    fn select_tab_decodes_both_forms() {
        let mut w = PacketWriter::default();
        w.bool(true).string("minecraft:story/root");
        assert_eq!(
            parse_select_tab(&w.buf).unwrap(),
            Some("minecraft:story/root".to_string())
        );
        assert_eq!(parse_select_tab(&[0]).unwrap(), None);
    }

    // ── Tree state ───────────────────────────────────────────────────────

    fn root_adv(id: &str, with_display: bool) -> WireAdvancement {
        WireAdvancement {
            id: id.into(),
            parent: None,
            display: if with_display {
                Some(WireDisplay {
                    title: Nbt::String(id.into()),
                    description: Nbt::String("d".into()),
                    icon: crate::component_wire::ItemTemplate {
                        item_id: 1,
                        count: 1,
                        patched: false,
                    },
                    frame: Frame::Task,
                    background: None,
                    show_toast: true,
                    hidden: false,
                    x: 0.0,
                    y: 0.0,
                })
            } else {
                None
            },
            requirements: vec![vec!["c1".into()]],
            sends_telemetry: false,
        }
    }

    fn child_adv(id: &str, parent: &str, x: f32, y: f32) -> WireAdvancement {
        let mut a = root_adv(id, true);
        a.parent = Some(parent.into());
        if let Some(d) = a.display.as_mut() {
            d.x = x;
            d.y = y;
        }
        a
    }

    fn apply_progress(t: &mut ClientAdvancements, id: &str, criteria: Vec<(&str, Option<i64>)>) {
        t.apply_update(UpdateAdvancements {
            reset: false,
            added: vec![],
            removed: vec![],
            progress: vec![(
                id.to_string(),
                Progress {
                    criteria: criteria
                        .into_iter()
                        .map(|(n, done)| (n.to_string(), done))
                        .collect(),
                },
            )],
            show_advancements: false,
        });
    }

    #[test]
    fn insertion_passes_place_children_sent_before_their_parents() {
        let mut t = ClientAdvancements::default();
        // Child FIRST — pass one cannot insert it; pass two can.
        t.add_all(vec![child_adv("a:c", "a:r", 1.0, 0.0), root_adv("a:r", true)]);
        assert!(t.node("a:r").is_some(), "root inserted");
        assert!(t.node("a:c").is_some(), "child inserted on a later pass");
        assert!(
            t.node("a:r").unwrap().children.contains(&"a:c".to_string()),
            "parent's children list gained the child"
        );
    }

    #[test]
    fn an_orphan_with_no_parent_anywhere_is_dropped_after_the_passes_stall() {
        let mut t = ClientAdvancements::default();
        t.add_all(vec![child_adv("a:c", "a:missing", 0.0, 0.0)]);
        assert!(t.node("a:c").is_none());
    }

    #[test]
    fn removal_cascades_through_the_subtree_and_drops_progress_with_it() {
        let mut t = ClientAdvancements::default();
        t.apply_update(UpdateAdvancements {
            reset: false,
            added: vec![
                root_adv("a:r", true),
                child_adv("a:m", "a:r", 1.0, 0.0),
                child_adv("a:l", "a:m", 2.0, 0.0),
            ],
            removed: vec![],
            progress: vec![],
            show_advancements: false,
        });
        apply_progress(&mut t, "a:l", vec![("c1", Some(5))]);
        assert!(t.progress("a:l").is_some());

        t.remove_ids(&["a:nobody".to_string(), "a:m".to_string()]);
        assert!(t.node("a:m").is_none());
        assert!(t.node("a:l").is_none(), "grandchild cascaded");
        assert!(t.progress("a:l").is_none(), "progress went with it");
        assert!(t.node("a:r").is_some(), "parent untouched");
    }

    #[test]
    fn progress_for_an_unknown_advancement_is_dropped_whole() {
        let mut t = ClientAdvancements::default();
        t.add_all(vec![root_adv("a:r", true)]);
        apply_progress(&mut t, "a:nope", vec![("c1", Some(9))]);
        assert!(t.progress("a:nope").is_none());
    }

    #[test]
    fn stored_progress_is_pruned_to_requirement_names_and_missing_slots_added() {
        let mut adv = root_adv("a:r", true);
        // Two AND-groups: c1, and c2|c3.
        adv.requirements = vec![vec!["c1".into()], vec!["c2".into(), "c3".into()]];
        let mut t = ClientAdvancements::default();
        t.add_all(vec![adv]);

        // The wire carries c1 done plus an unknown cX that must be pruned;
        // c2 is named by the requirements but absent from the wire.
        apply_progress(
            &mut t,
            "a:r",
            vec![
                ("cX", Some(2)),
                ("c1", Some(1)),
                ("c3", Some(3)),
            ],
        );

        let p = t.progress("a:r").unwrap();
        let names: Vec<&str> = p.criteria.iter().map(|(n, _)| n.as_str()).collect();
        // Order preserved for the survivors, appended for the added slot.
        assert_eq!(names, vec!["c1", "c3", "c2"]);
        assert_eq!(
            p.criteria.iter().find(|(n, _)| n == "c2").unwrap().1,
            None,
            "the filled slot starts undone"
        );
    }

    #[test]
    fn done_requires_every_group_but_only_one_member_of_it() {
        let mut adv = root_adv("a:r", true);
        adv.requirements = vec![vec!["c1".into()], vec!["c2".into(), "c3".into()]];
        let mut t = ClientAdvancements::default();
        t.add_all(vec![adv]);
        let reqs = t.node("a:r").unwrap().advancement.requirements.clone();

        apply_progress(
            &mut t,
            "a:r",
            vec![("c1", Some(1)), ("c2", None), ("c3", None)],
        );
        let p = t.progress("a:r").unwrap();
        assert!(!p.is_done(&reqs));
        assert_eq!(p.percent(&reqs), 0.5);
        assert_eq!(p.progress_text(&reqs), Some((1, 2)));

        apply_progress(
            &mut t,
            "a:r",
            vec![("c1", Some(1)), ("c2", None), ("c3", Some(9))],
        );
        let p = t.progress("a:r").unwrap();
        assert!(p.is_done(&reqs), "one member of each group suffices");
        assert_eq!(p.percent(&reqs), 1.0);
        assert!(p.has_progress());
    }

    #[test]
    fn single_requirement_advancements_show_no_progress_text() {
        let mut t = ClientAdvancements::default();
        t.add_all(vec![root_adv("a:solo", true)]); // one group of one
        apply_progress(&mut t, "a:solo", vec![("c1", None)]);
        let reqs = t.node("a:solo").unwrap().advancement.requirements.clone();
        let p = t.progress("a:solo").unwrap();
        assert_eq!(p.percent(&reqs), 0.0);
        assert_eq!(p.progress_text(&reqs), None, "total <= 1 suppresses the counter");
        assert!(!p.has_progress());
    }

    #[test]
    fn tabs_are_roots_in_insertion_order_and_skip_displayless_ones() {
        let mut t = ClientAdvancements::default();
        t.add_all(vec![
            root_adv("a:first", true),
            child_adv("a:task", "a:first", 1.0, 0.0),
            root_adv("a:second", false), // no display → no tab
            root_adv("a:third", true),
        ]);
        let tabs: Vec<&str> = t.tabs().iter().map(|n| n.id.as_str()).collect();
        assert_eq!(tabs, vec!["a:first", "a:third"]);
        let tasks: Vec<&str> = t.tab_tasks("a:first").iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            tasks,
            vec!["a:task"],
            "tab_tasks routes by subtree, not by direct parent"
        );
    }

    #[test]
    fn select_tab_tracks_changes_and_unknown_ids_resolve_to_none() {
        let mut t = ClientAdvancements::default();
        t.add_all(vec![root_adv("a:r", true)]);
        assert!(t.select_tab(Some("a:r")));
        assert!(!t.select_tab(Some("a:r")), "same id is no change");
        // The dispatch arm resolves through the tree before calling:
        let resolved = t.node("a:ghost").map(|_| "a:ghost".to_string());
        assert!(resolved.is_none());
        t.select_tab(resolved.as_deref());
        assert_eq!(t.selected_tab(), None);
    }

    #[test]
    fn reset_clears_everything() {
        let mut t = ClientAdvancements::default();
        t.apply_update(UpdateAdvancements {
            reset: true,
            added: vec![root_adv("a:r", true)],
            removed: vec![],
            progress: vec![],
            show_advancements: true,
        });
        assert!(!t.is_empty());
        assert!(t.show_advancements);
        t.apply_update(UpdateAdvancements {
            reset: true,
            added: vec![],
            removed: vec![],
            progress: vec![],
            show_advancements: false,
        });
        assert!(t.is_empty());
        assert_eq!(t.len_roots(), 0);
        assert_eq!(t.selected_tab(), None, "reset clears the selection too");
    }

    #[test]
    fn reinserting_a_removed_root_moves_it_to_the_end_of_the_tab_order() {
        // Java's ObjectLinkedOpenHashSet.remove+add appends at the end; the
        // screen's tab strip follows that order.
        let mut t = ClientAdvancements::default();
        t.add_all(vec![root_adv("a:a", true), root_adv("a:b", true), root_adv("a:c", true)]);
        t.remove_ids(&["a:a".to_string()]);
        t.add_all(vec![root_adv("a:a", true)]);
        let order: Vec<&str> = t.tabs().iter().map(|n| n.id.as_str()).collect();
        assert_eq!(order, vec!["a:b", "a:c", "a:a"]);
    }

    #[test]
    fn done_requires_every_group_but_only_one_member_of_it_at_the_state_level() {
        let mut adv = root_adv("a:r", true);
        adv.requirements = vec![vec!["c1".into()], vec!["c2".into(), "c3".into()]];
        let mut t = ClientAdvancements::default();
        t.add_all(vec![adv]);

        // No progress at all is NOT done (an empty criteria map answers false,
        // not vacuous truth — AdvancementRequirements.test's own empty rule).
        assert_eq!(t.is_done("a:r"), Some(false));
        assert_eq!(t.is_done("a:ghost"), None, "unknown ids answer None");

        apply_progress(
            &mut t,
            "a:r",
            vec![("c1", Some(1)), ("c2", None), ("c3", None)],
        );
        assert_eq!(t.is_done("a:r"), Some(false));

        apply_progress(
            &mut t,
            "a:r",
            vec![("c1", Some(1)), ("c2", None), ("c3", Some(9))],
        );
        assert_eq!(
            t.is_done("a:r"),
            Some(true),
            "one member of each AND-group suffices"
        );

        // A single-group advancement flips on its one criterion.
        t.add_all(vec![root_adv("a:solo2", true)]);
        apply_progress(&mut t, "a:solo2", vec![("c1", Some(3))]);
        assert_eq!(t.is_done("a:solo2"), Some(true));
    }

    #[test]
    fn empty_requirements_are_never_done_not_vacuously_true() {
        let mut adv = root_adv("a:e", true);
        adv.requirements = vec![];
        let mut t = ClientAdvancements::default();
        t.add_all(vec![adv]);
        apply_progress(
            &mut t,
            "a:e",
            vec![("anything", Some(1)), ("else", Some(2))],
        );
        // Progress exists and is non-empty, but the requirement list is
        // empty: test() answers false (AdvancementRequirements.test, :64).
        assert_eq!(t.is_done("a:e"), Some(false));
    }
}

