//! The title overlay and two HUD gauges (M79): `clear_titles` (14),
//! `cooldown` (22), `set_action_bar_text` (87), `set_experience` (103),
//! `set_subtitle_text` (112), `set_title_text` (114) and
//! `set_titles_animation` (115).
//!
//! Seven `REWO_PACKET_COVERAGE.md` class **B** packets that all land on
//! vanilla's `Gui` / `Hud`. The decode is the easy half; what is transcribed
//! here is the *state machine* each one drives, because in five separate
//! places vanilla's behaviour is the opposite of the obvious reading.
//!
//! # The five inversions
//!
//! ## 1. `set_subtitle_text` alone shows nothing
//!
//! ```java
//! public void setSubtitle(final Component subtitle) {
//!    this.subtitle = subtitle;
//! }
//!
//! public void setTitle(final Component title) {
//!    this.title = title;
//!    this.titleTime = this.titleFadeInTime + this.titleStayTime + this.titleFadeOutTime;
//! }
//! ```
//!
//! Only `setTitle` arms the clock, and `extractTitle`'s whole body is gated on
//! `this.title != null && this.titleTime > 0` — the subtitle is drawn *inside*
//! that block. So a server that sends a subtitle and no title displays
//! nothing at all, and one that sends the subtitle *after* the title (the
//! order `/title` uses is the other way round) is relying on the subtitle
//! surviving into a countdown that is already running.
//!
//! An implementation that armed the clock from either packet would show a
//! bare subtitle vanilla never shows, and would restart the countdown on the
//! second half of every two-packet title.
//!
//! ## 2. A negative time means "leave unchanged", and a live title is re-armed
//!
//! ```java
//! public void setTimes(final int fadeInTime, final int stayTime, final int fadeOutTime) {
//!    if (fadeInTime >= 0)  this.titleFadeInTime  = fadeInTime;
//!    if (stayTime >= 0)    this.titleStayTime    = stayTime;
//!    if (fadeOutTime >= 0) this.titleFadeOutTime = fadeOutTime;
//!    if (this.titleTime > 0) {
//!       this.titleTime = this.titleFadeInTime + this.titleStayTime + this.titleFadeOutTime;
//!    }
//! }
//! ```
//!
//! Two things, and the second is the surprising one. A negative field is a
//! *skip*, per axis — so `-1, -1, 40` changes only the fade-out. And the
//! trailing `if` **restarts a title that is already on screen at its full
//! duration**, so `/title @a times` sent mid-title does not retime the
//! remainder, it hands the title its whole life back. Zero is not negative and
//! is a legal set.
//!
//! ## 3. `clear_titles`' boolean does something the clear does not
//!
//! ```java
//! this.minecraft.gui.hud.clearTitles();
//! if (packet.shouldResetTimes()) {
//!    this.minecraft.gui.hud.resetTitleTimes();
//! }
//! ```
//!
//! `clearTitles` drops the text and zeroes the countdown; it leaves
//! `fadeIn` / `stay` / `fadeOut` exactly as the last `set_titles_animation`
//! left them. Only `resetTimes` puts them back to **10 / 70 / 20**. So a
//! `/title @a reset` and a `/title @a clear` differ in what the *next* title
//! does, not in what is on screen now.
//!
//! ## 4. `set_experience`'s wire order is not its declaration order
//!
//! ```java
//! public record-ish ClientboundSetExperiencePacket(
//!    float experienceProgress, int totalExperience, int experienceLevel)
//!
//! private ClientboundSetExperiencePacket(final FriendlyByteBuf input) {
//!    this.experienceProgress = input.readFloat();
//!    this.experienceLevel    = input.readVarInt();   // <- level SECOND
//!    this.totalExperience    = input.readVarInt();   // <- total THIRD
//! }
//! ```
//!
//! The field declaration order is progress / total / level and the **wire**
//! order is progress / level / total. Reading the fields top to bottom swaps
//! the last two, which decodes without erroring (both are var-ints) and puts
//! the player's lifetime XP total in the level display. [`SetExperience`]
//! therefore names its fields rather than being a tuple.
//!
//! ## 5. `cooldown` is a *group*, not an item, and duration 0 is a removal
//!
//! ```java
//! public record ClientboundCooldownPacket(Identifier cooldownGroup, int duration)
//!
//! if (packet.duration() == 0) {
//!    this.minecraft.player.getCooldowns().removeCooldown(packet.cooldownGroup());
//! } else {
//!    this.minecraft.player.getCooldowns().addCooldown(packet.cooldownGroup(), packet.duration());
//! }
//! ```
//!
//! The wire carries **no start tick and no end tick** — just a group name and
//! a length in ticks, and the client's own `tickCount` supplies the start:
//!
//! ```java
//! public void addCooldown(final Identifier cooldownGroup, final int time) {
//!    this.cooldowns.put(cooldownGroup, new CooldownInstance(this.tickCount, this.tickCount + time));
//! }
//! ```
//!
//! and `duration == 0` is routed to `removeCooldown`, so it cancels rather
//! than starting a zero-length one. The distinction is observable: an
//! `addCooldown(group, 0)` would leave an instance whose `endTime == startTime`
//! and whose percent is `0/0` — a NaN through `Mth.clamp`.
//!
//! The *group* is `ItemCooldowns.getCooldownGroup(stack)`: the stack's
//! `minecraft:use_cooldown` component's `cooldownGroup` when it sets one, and
//! the item's registry name otherwise. That resolution lives one layer up (the
//! renderer needs a stack); this module only ever sees the name.
//!
//! # What is *not* inverted, and is still worth writing down
//!
//! - The three animation fields are **fixed big-endian i32s**, not var-ints
//!   (`Packet.codec(write, new)` over `FriendlyByteBuf.readInt`). Twelve bytes,
//!   always.
//! - All three text packets use `ComponentSerialization.TRUSTED_STREAM_CODEC`
//!   — one NBT tag, the same shape M78's `disguised_chat` reads.
//! - `setActionBarText` passes `animate = false`
//!   (`setOverlayMessage(packet.text(), false)`). The animated rainbow is
//!   `setNowPlaying`'s, i.e. a jukebox, and is unreachable from this packet.
//!   The flag is modelled anyway, because the *renderer* branches on it.
//! - `handleSetExperience` re-arms the XP bar's display-priority window only
//!   when **`experienceProgress` changed** — a pure level change with the same
//!   progress does not. See [`ExperienceState::set_values`].
//!
//! # Ground truth (bundled 26.2 decompile, `%APPDATA%/EwoClient/rewo/26.2/decompiled/`)
//!
//! - `net/minecraft/network/protocol/game/Clientbound{ClearTitles,Cooldown,
//!   SetActionBarText,SetExperience,SetSubtitleText,SetTitleText,
//!   SetTitlesAnimation}Packet.java`
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java`
//!   (`handleTitlesClear`, `setActionBarText`, `setTitleText`,
//!   `setSubtitleText`, `setTitlesAnimation`, `handleSetExperience`,
//!   `handleItemCooldown`)
//! - `net/minecraft/client/gui/Hud.java` (the fields, `tick`, `setTimes`,
//!   `setTitle`, `setSubtitle`, `clearTitles`, `resetTitleTimes`,
//!   `onDisconnected`)
//! - `net/minecraft/client/player/LocalPlayer.java` (`setExperienceValues`)
//! - `net/minecraft/world/item/ItemCooldowns.java`

use std::collections::HashMap;

use rewo_proto::nbt::Nbt;
use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

use crate::chat_style;

/// `Hud.resetTitleTimes()` — `titleFadeInTime = 10`.
pub const DEFAULT_FADE_IN: i32 = 10;
/// `Hud.resetTitleTimes()` — `titleStayTime = 70`.
pub const DEFAULT_STAY: i32 = 70;
/// `Hud.resetTitleTimes()` — `titleFadeOutTime = 20`.
pub const DEFAULT_FADE_OUT: i32 = 20;

/// `Hud.setOverlayMessage` — `this.overlayMessageTime = 60`.
///
/// A constant, unrelated to the title times: an action bar always lives three
/// seconds regardless of what `set_titles_animation` last said.
pub const OVERLAY_MESSAGE_TICKS: i32 = 60;

/// Flatten a component to plain text — [`chat_style::flatten`] with no table.
///
/// **No language table**, because this runs at packet-decode time inside
/// `PlaySession` and the table is the app's (`BakedAssets::lang`). A
/// `translate` component therefore flattens to its key here. That is not a
/// regression — it is what this function has always done — and it does not
/// reach the screen for the components that matter: the title, subtitle and
/// action bar are kept as raw [`Nbt`] precisely so the app can resolve and
/// style them at render time. This is for the callers that want a string now,
/// mostly logging and witnesses.
///
/// It **delegates** rather than open-coding `plain_text(parse_component(..))`:
/// that expression had grown four spellings across two crates by M163, which
/// is the census that milestone's own doc got wrong. `None` is the whole of
/// what distinguishes this one, so it is the whole of what this body says.
pub fn plain(component: &Nbt) -> String {
    chat_style::flatten(component, None)
}

// ---------------------------------------------------------------------------
// The title overlay — `Hud`'s title / subtitle / action-bar fields.
// ---------------------------------------------------------------------------

/// `Hud`'s title, subtitle and overlay-message state, and the three
/// server-settable durations.
///
/// The components are kept as raw [`Nbt`] rather than flattened strings so the
/// renderer can style each span: vanilla's `graphics.text(font, Component, …)`
/// takes the fade colour as a *default* that a span's own `color` replaces —
/// and, load-bearingly, `Font.StringRenderOutput.getTextColor` keeps the
/// **caller's alpha** when it does, so a coloured title still fades.
#[derive(Clone, Debug, PartialEq)]
pub struct TitleOverlay {
    /// `Hud.title`.
    pub title: Option<Nbt>,
    /// `Hud.subtitle`. Never arms [`Self::title_time`] on its own.
    pub subtitle: Option<Nbt>,
    /// `Hud.titleTime` — ticks remaining, counted **down**.
    pub title_time: i32,
    /// `Hud.titleFadeInTime`.
    pub fade_in: i32,
    /// `Hud.titleStayTime`.
    pub stay: i32,
    /// `Hud.titleFadeOutTime`.
    pub fade_out: i32,
    /// `Hud.overlayMessageString` — the action bar.
    pub overlay_message: Option<Nbt>,
    /// `Hud.overlayMessageTime`.
    pub overlay_message_time: i32,
    /// `Hud.animateOverlayMessageColor`. Always `false` from
    /// `set_action_bar_text`; `true` only from `setNowPlaying`.
    pub animate_overlay_message_color: bool,
}

impl Default for TitleOverlay {
    /// `Hud`'s constructor calls `resetTitleTimes()`, so a fresh HUD already
    /// carries 10 / 70 / 20 rather than three zeros.
    fn default() -> Self {
        Self {
            title: None,
            subtitle: None,
            title_time: 0,
            fade_in: DEFAULT_FADE_IN,
            stay: DEFAULT_STAY,
            fade_out: DEFAULT_FADE_OUT,
            overlay_message: None,
            overlay_message_time: 0,
            animate_overlay_message_color: false,
        }
    }
}

impl TitleOverlay {
    /// `Hud.setTitle` — sets the text **and** arms the countdown at the full
    /// `fadeIn + stay + fadeOut`.
    pub fn set_title(&mut self, title: Nbt) {
        self.title = Some(title);
        self.title_time = self.fade_in + self.stay + self.fade_out;
    }

    /// `Hud.setSubtitle` — sets the text and **nothing else**. See inversion 1.
    pub fn set_subtitle(&mut self, subtitle: Nbt) {
        self.subtitle = Some(subtitle);
    }

    /// `Hud.setTimes` — per-axis "negative means unchanged", then re-arm a
    /// live title at its full duration. See inversion 2.
    pub fn set_times(&mut self, fade_in: i32, stay: i32, fade_out: i32) {
        if fade_in >= 0 {
            self.fade_in = fade_in;
        }
        if stay >= 0 {
            self.stay = stay;
        }
        if fade_out >= 0 {
            self.fade_out = fade_out;
        }
        if self.title_time > 0 {
            self.title_time = self.fade_in + self.stay + self.fade_out;
        }
    }

    /// `Hud.clearTitles` — drops both components and zeroes the countdown.
    /// **Leaves the three durations alone**; see inversion 3.
    pub fn clear_titles(&mut self) {
        self.title = None;
        self.subtitle = None;
        self.title_time = 0;
    }

    /// `Hud.resetTitleTimes` — 10 / 70 / 20, and nothing else.
    pub fn reset_title_times(&mut self) {
        self.fade_in = DEFAULT_FADE_IN;
        self.stay = DEFAULT_STAY;
        self.fade_out = DEFAULT_FADE_OUT;
    }

    /// `Hud.setOverlayMessage(string, animate)`.
    pub fn set_overlay_message(&mut self, message: Nbt, animate: bool) {
        self.overlay_message = Some(message);
        self.overlay_message_time = OVERLAY_MESSAGE_TICKS;
        self.animate_overlay_message_color = animate;
    }

    /// One `Hud.tick()`:
    ///
    /// ```java
    /// if (this.overlayMessageTime > 0) this.overlayMessageTime--;
    /// if (this.titleTime > 0) {
    ///    this.titleTime--;
    ///    if (this.titleTime <= 0) { this.title = null; this.subtitle = null; }
    /// }
    /// ```
    ///
    /// Both decrements are guarded, so neither counter runs negative — and the
    /// expiry drops **both** components, which is why a subtitle cannot outlive
    /// the title it was shown under.
    pub fn tick(&mut self) {
        if self.overlay_message_time > 0 {
            self.overlay_message_time -= 1;
        }
        if self.title_time > 0 {
            self.title_time -= 1;
            if self.title_time <= 0 {
                self.title = None;
                self.subtitle = None;
            }
        }
    }

    /// `Hud.onDisconnected` — `clearTitles(); resetTitleTimes();`, the one
    /// place vanilla does both.
    pub fn on_disconnected(&mut self) {
        self.clear_titles();
        self.reset_title_times();
    }

    /// `extractTitle`'s guard: `this.title != null && this.titleTime > 0`.
    pub fn showing_title(&self) -> bool {
        self.title.is_some() && self.title_time > 0
    }
}

// ---------------------------------------------------------------------------
// The experience gauge — `LocalPlayer`'s three fields plus the display window.
// ---------------------------------------------------------------------------

/// `ClientboundSetExperiencePacket`, in **wire** order.
///
/// Named fields rather than a tuple because the declaration order and the wire
/// order disagree — see inversion 4.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SetExperience {
    /// `experienceProgress` — 0..1 through the current level.
    pub progress: f32,
    /// `experienceLevel` — the green number. **Second** on the wire.
    pub level: i32,
    /// `totalExperience` — lifetime XP. **Third** on the wire, and read by no
    /// client renderer at all: `LocalPlayer.java`'s only reference is the
    /// assignment in `setExperienceValues`.
    pub total: i32,
}

/// `LocalPlayer`'s experience fields plus the `experienceDisplayStartTick`
/// window `Hud.willPrioritizeExperienceInfo` reads.
#[derive(Clone, Debug, PartialEq)]
pub struct ExperienceState {
    pub progress: f32,
    pub total: i32,
    pub level: i32,
    /// `LocalPlayer.experienceDisplayStartTick`, initialised to
    /// `Integer.MIN_VALUE`.
    pub display_start_tick: i32,
    /// `Entity.tickCount` for the local player — the clock the window above is
    /// measured against. Kept here rather than borrowed from the session for
    /// the same reason `effects::VisualEffects` keeps its own: a respawn builds
    /// a fresh `LocalPlayer` and this counter restarts with it.
    pub tick_count: i32,
}

impl Default for ExperienceState {
    fn default() -> Self {
        Self {
            progress: 0.0,
            total: 0,
            level: 0,
            display_start_tick: i32::MIN,
            tick_count: 0,
        }
    }
}

/// `Hud.EXPERIENCE_BAR_DISPLAY_TICKS` — the window in which a recent XP change
/// out-ranks the locator bar for the one contextual-bar slot.
pub const EXPERIENCE_BAR_DISPLAY_TICKS: i32 = 100;

impl ExperienceState {
    /// `LocalPlayer.setExperienceValues`:
    ///
    /// ```java
    /// if (experienceProgress != this.experienceProgress) {
    ///    this.setExperienceDisplayStartTickToTickCount();
    /// }
    /// this.experienceProgress = experienceProgress;
    /// this.totalExperience = totalExp;
    /// this.experienceLevel = experienceLevel;
    /// ```
    ///
    /// **The re-arm keys on `progress` only.** Levelling up with the bar
    /// landing on the same fraction — or a server that resends the same
    /// progress with a new level — leaves the window where it was.
    pub fn set_values(&mut self, v: SetExperience) {
        if v.progress != self.progress {
            self.set_display_start_tick();
        }
        self.progress = v.progress;
        self.total = v.total;
        self.level = v.level;
    }

    /// `LocalPlayer.setExperienceDisplayStartTickToTickCount`:
    ///
    /// ```java
    /// if (this.experienceDisplayStartTick == Integer.MIN_VALUE) {
    ///    this.experienceDisplayStartTick = -2147483647;
    /// } else {
    ///    this.experienceDisplayStartTick = this.tickCount;
    /// }
    /// ```
    ///
    /// **The first change is deliberately not a change.** The sentinel is
    /// replaced with `Integer.MIN_VALUE + 1`, which is still ~2.1 billion ticks
    /// in the past — so the join-time `set_experience` a vanilla server always
    /// sends does not pop the XP bar over the locator bar. Every change after
    /// it does.
    fn set_display_start_tick(&mut self) {
        self.display_start_tick = if self.display_start_tick == i32::MIN {
            i32::MIN + 1
        } else {
            self.tick_count
        };
    }

    /// `Hud.willPrioritizeExperienceInfo`:
    /// `experienceDisplayStartTick + 100 > tickCount`.
    ///
    /// `wrapping_add` because Java's `int` addition wraps and the sentinel sits
    /// one above `Integer.MIN_VALUE` — the sum is fine, but a panic in a debug
    /// build would be a divergence introduced by Rust's arithmetic rules
    /// rather than by the transcription.
    pub fn will_prioritize(&self) -> bool {
        self.display_start_tick
            .wrapping_add(EXPERIENCE_BAR_DISPLAY_TICKS)
            > self.tick_count
    }

    /// `Player.getXpNeededForNextLevel`:
    ///
    /// ```java
    /// if (this.experienceLevel >= 30) return 112 + (this.experienceLevel - 30) * 9;
    /// return this.experienceLevel >= 15 ? 37 + (this.experienceLevel - 15) * 5
    ///                                   : 7 + this.experienceLevel * 2;
    /// ```
    ///
    /// `ExperienceBar.extractBackground` draws nothing at all when this is
    /// `<= 0`, which is reachable only at level `<= -4` — i.e. never from a
    /// vanilla server, and the guard is transcribed rather than dropped
    /// because a hostile or buggy one can send it.
    pub fn xp_needed_for_next_level(&self) -> i32 {
        if self.level >= 30 {
            112 + (self.level - 30) * 9
        } else if self.level >= 15 {
            37 + (self.level - 15) * 5
        } else {
            7 + self.level * 2
        }
    }

    /// One local-player tick.
    pub fn tick(&mut self) {
        self.tick_count = self.tick_count.wrapping_add(1);
    }
}

// ---------------------------------------------------------------------------
// The cooldown gauge — `ItemCooldowns`.
// ---------------------------------------------------------------------------

/// `ItemCooldowns` — the per-group `(startTime, endTime)` map and its own tick
/// counter.
///
/// The counter is `ItemCooldowns.tickCount`, which is **not** the player's:
/// it is a private field advanced by `ItemCooldowns.tick()` alone, and every
/// start/end is expressed in it. Rewo keeps that separation rather than
/// borrowing the session's tick number, so the arithmetic is exactly vanilla's
/// even if the two clocks were ever started apart.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemCooldowns {
    cooldowns: HashMap<String, (i32, i32)>,
    tick_count: i32,
}

impl ItemCooldowns {
    /// `ItemCooldowns.addCooldown(Identifier, int)`.
    pub fn add(&mut self, group: &str, time: i32) {
        self.cooldowns
            .insert(group.to_string(), (self.tick_count, self.tick_count + time));
    }

    /// `ItemCooldowns.removeCooldown(Identifier)`.
    pub fn remove(&mut self, group: &str) {
        self.cooldowns.remove(group);
    }

    /// `ItemCooldowns.tick()`:
    ///
    /// ```java
    /// this.tickCount++;
    /// … remove every entry whose endTime <= this.tickCount …
    /// ```
    ///
    /// The increment happens **first**, so an entry added with `time = 1` is
    /// gone at the end of the very next tick.
    pub fn tick(&mut self) {
        self.tick_count = self.tick_count.wrapping_add(1);
        let now = self.tick_count;
        self.cooldowns.retain(|_, (_, end)| *end > now);
    }

    /// `ItemCooldowns.getCooldownPercent(stack, partialTick)`, for a group
    /// name already resolved by the caller:
    ///
    /// ```java
    /// float duration  = cooldown.endTime - cooldown.startTime;
    /// float remaining = cooldown.endTime - (this.tickCount + a);
    /// return Mth.clamp(remaining / duration, 0.0F, 1.0F);
    /// ```
    ///
    /// An absent group is `0.0F`, which is also what `isOnCooldown` tests.
    pub fn percent(&self, group: &str, partial: f32) -> f32 {
        let Some(&(start, end)) = self.cooldowns.get(group) else {
            return 0.0;
        };
        let duration = (end - start) as f32;
        let remaining = end as f32 - (self.tick_count as f32 + partial);
        (remaining / duration).clamp(0.0, 1.0)
    }

    /// `ItemCooldowns.isOnCooldown` — `getCooldownPercent(item, 0.0F) > 0.0F`.
    pub fn is_on_cooldown(&self, group: &str) -> bool {
        self.percent(group, 0.0) > 0.0
    }

    pub fn len(&self) -> usize {
        self.cooldowns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cooldowns.is_empty()
    }

    /// `ItemCooldowns.tickCount` — exposed so a witness can grade the clock
    /// the starts and ends are expressed in.
    pub fn tick_count(&self) -> i32 {
        self.tick_count
    }
}

// ---------------------------------------------------------------------------
// The three together.
// ---------------------------------------------------------------------------

/// Everything M79's seven packets write.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HudState {
    pub titles: TitleOverlay,
    pub experience: ExperienceState,
    pub cooldowns: ItemCooldowns,
}

impl HudState {
    /// One client tick. All three counters advance together because all three
    /// are driven by the same 20 Hz loop; they are independent of each other,
    /// so the order within this function carries no meaning.
    pub fn tick(&mut self) {
        self.titles.tick();
        self.experience.tick();
        self.cooldowns.tick();
    }

    /// `handleRespawn`'s asymmetry, and it runs the *opposite* way for the two
    /// halves of this struct.
    ///
    /// `Hud` is `Minecraft.gui.hud` — it belongs to the client, not the level,
    /// and `handleRespawn` never touches it. A title survives a death and a
    /// dimension change; only `onDisconnected` clears it.
    ///
    /// The experience fields and the cooldown map are `LocalPlayer` /
    /// `Player` state, and `handleRespawn` builds a **new** `LocalPlayer`.
    /// Neither is in the explicitly-carried list (`dataToKeep` bit 2 carries
    /// `SynchedEntityData`, the delta movement and the rotation; bit 1 carries
    /// the attributes), so both reset — including
    /// `experienceDisplayStartTick`'s sentinel, which means the first
    /// `set_experience` after a respawn again does not prioritise the bar.
    pub fn reset_for_respawn(&mut self) {
        self.experience = ExperienceState::default();
        self.cooldowns = ItemCooldowns::default();
    }
}

// ---------------------------------------------------------------------------
// Readers.
// ---------------------------------------------------------------------------

/// A trusted `Component` — `ComponentSerialization.TRUSTED_STREAM_CODEC`, one
/// NBT tag. Shared by `set_title_text`, `set_subtitle_text` and
/// `set_action_bar_text`, whose codecs are byte-for-byte the same.
pub fn read_component(body: &[u8]) -> Result<Nbt> {
    PacketReader::new(body).nbt()
}

/// `ClientboundSetTitlesAnimationPacket` — three `readInt()`s, twelve fixed
/// big-endian bytes among a protocol that is mostly var-ints.
pub fn read_titles_animation(body: &[u8]) -> Result<(i32, i32, i32)> {
    let mut r = PacketReader::new(body);
    let fade_in = r.i32()?;
    let stay = r.i32()?;
    let fade_out = r.i32()?;
    Ok((fade_in, stay, fade_out))
}

/// `ClientboundClearTitlesPacket` — one `readBoolean()`.
pub fn read_clear_titles(body: &[u8]) -> Result<bool> {
    PacketReader::new(body).bool()
}

/// `ClientboundSetExperiencePacket` — float, then **level**, then total.
pub fn read_set_experience(body: &[u8]) -> Result<SetExperience> {
    let mut r = PacketReader::new(body);
    let progress = r.f32()?;
    let level = r.varint()?;
    let total = r.varint()?;
    Ok(SetExperience {
        progress,
        level,
        total,
    })
}

/// `ClientboundCooldownPacket` — `Identifier.STREAM_CODEC` then
/// `ByteBufCodecs.VAR_INT`.
pub fn read_cooldown(body: &[u8]) -> Result<(String, i32)> {
    let mut r = PacketReader::new(body);
    let group = r.identifier()?;
    let duration = r.varint()?;
    Ok((group, duration))
}

// ---------------------------------------------------------------------------
// The dispatch seam.
// ---------------------------------------------------------------------------

/// Which of M79's seven packets an id names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudPacket {
    ClearTitles,
    Cooldown,
    SetActionBarText,
    SetExperience,
    SetSubtitleText,
    SetTitleText,
    SetTitlesAnimation,
}

/// The seven resolved ids, lifted out of [`crate::ids::Ids`] so this module
/// does not depend on the whole table.
#[derive(Clone, Copy, Debug)]
pub struct HudIds {
    pub clear_titles: i32,
    pub cooldown: i32,
    pub set_action_bar_text: i32,
    pub set_experience: i32,
    pub set_subtitle_text: i32,
    pub set_title_text: i32,
    pub set_titles_animation: i32,
}

/// The id → packet map.
///
/// Three of these bodies are a bare NBT tag and are therefore
/// **indistinguishable on the wire** — only the id says whether a component is
/// a title, a subtitle or an action bar. That is the same reason
/// [`crate::view_area`] and [`crate::ticking`] key on the id rather than
/// sniffing the body.
pub fn kind_for_id(id: i32, ids: HudIds) -> Option<HudPacket> {
    if id == ids.clear_titles {
        Some(HudPacket::ClearTitles)
    } else if id == ids.cooldown {
        Some(HudPacket::Cooldown)
    } else if id == ids.set_action_bar_text {
        Some(HudPacket::SetActionBarText)
    } else if id == ids.set_experience {
        Some(HudPacket::SetExperience)
    } else if id == ids.set_subtitle_text {
        Some(HudPacket::SetSubtitleText)
    } else if id == ids.set_title_text {
        Some(HudPacket::SetTitleText)
    } else if id == ids.set_titles_animation {
        Some(HudPacket::SetTitlesAnimation)
    } else {
        None
    }
}

/// Apply one packet. Returns whether the body decoded — a `false` leaves the
/// state untouched, exactly as a vanilla decode failure kills the packet
/// before the handler ever runs.
pub fn apply(kind: HudPacket, body: &[u8], state: &mut HudState) -> bool {
    match kind {
        HudPacket::SetTitleText => match read_component(body) {
            Ok(c) => {
                state.titles.set_title(c);
                true
            }
            Err(err) => {
                log::debug!("net: set_title_text decode: {err}");
                false
            }
        },
        HudPacket::SetSubtitleText => match read_component(body) {
            Ok(c) => {
                state.titles.set_subtitle(c);
                true
            }
            Err(err) => {
                log::debug!("net: set_subtitle_text decode: {err}");
                false
            }
        },
        HudPacket::SetActionBarText => match read_component(body) {
            Ok(c) => {
                // `setOverlayMessage(packet.text(), false)` — the literal
                // `false` is the whole reason the packet's action bar never
                // does the jukebox rainbow.
                state.titles.set_overlay_message(c, false);
                true
            }
            Err(err) => {
                log::debug!("net: set_action_bar_text decode: {err}");
                false
            }
        },
        HudPacket::SetTitlesAnimation => match read_titles_animation(body) {
            Ok((fade_in, stay, fade_out)) => {
                state.titles.set_times(fade_in, stay, fade_out);
                true
            }
            Err(err) => {
                log::debug!("net: set_titles_animation decode: {err}");
                false
            }
        },
        HudPacket::ClearTitles => match read_clear_titles(body) {
            Ok(reset_times) => {
                state.titles.clear_titles();
                if reset_times {
                    state.titles.reset_title_times();
                }
                true
            }
            Err(err) => {
                log::debug!("net: clear_titles decode: {err}");
                false
            }
        },
        HudPacket::SetExperience => match read_set_experience(body) {
            Ok(v) => {
                state.experience.set_values(v);
                true
            }
            Err(err) => {
                log::debug!("net: set_experience decode: {err}");
                false
            }
        },
        HudPacket::Cooldown => match read_cooldown(body) {
            Ok((group, duration)) => {
                if duration == 0 {
                    state.cooldowns.remove(&group);
                } else {
                    state.cooldowns.add(&group, duration);
                }
                true
            }
            Err(err) => {
                log::debug!("net: cooldown decode: {err}");
                false
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Nbt {
        Nbt::String(s.to_string())
    }

    /// A trusted `Component` on the wire: NBT tag 8 (String) with a bare
    /// payload — the network form has no name.
    fn component_body(s: &str) -> Vec<u8> {
        let mut out = vec![8u8];
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s.as_bytes());
        out
    }

    fn varint(v: i32, out: &mut Vec<u8>) {
        let mut n = v as u32;
        loop {
            let b = (n & 0x7F) as u8;
            n >>= 7;
            if n == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
    }

    const IDS: HudIds = HudIds {
        clear_titles: 14,
        cooldown: 22,
        set_action_bar_text: 87,
        set_experience: 103,
        set_subtitle_text: 112,
        set_title_text: 114,
        set_titles_animation: 115,
    };

    #[test]
    fn a_subtitle_alone_shows_nothing() {
        let mut s = HudState::default();
        assert!(apply(HudPacket::SetSubtitleText, &component_body("go"), &mut s));
        assert_eq!(s.titles.subtitle, Some(text("go")));
        // MUTATION partner: arming the clock from `set_subtitle` too makes
        // this 100.
        assert_eq!(s.titles.title_time, 0);
        assert!(!s.titles.showing_title());
    }

    #[test]
    fn a_title_after_a_subtitle_shows_both() {
        let mut s = HudState::default();
        apply(HudPacket::SetSubtitleText, &component_body("sub"), &mut s);
        apply(HudPacket::SetTitleText, &component_body("main"), &mut s);
        assert!(s.titles.showing_title());
        assert_eq!(s.titles.title_time, 100);
        assert_eq!(s.titles.subtitle, Some(text("sub")));
    }

    #[test]
    fn a_negative_time_leaves_that_axis_alone() {
        let mut s = HudState::default();
        let mut body = Vec::new();
        body.extend_from_slice(&(-1i32).to_be_bytes());
        body.extend_from_slice(&(-1i32).to_be_bytes());
        body.extend_from_slice(&40i32.to_be_bytes());
        assert!(apply(HudPacket::SetTitlesAnimation, &body, &mut s));
        // MUTATION partner: assigning unconditionally makes fade_in -1.
        assert_eq!(
            (s.titles.fade_in, s.titles.stay, s.titles.fade_out),
            (DEFAULT_FADE_IN, DEFAULT_STAY, 40)
        );
    }

    #[test]
    fn zero_is_a_legal_time_and_is_not_a_skip() {
        let mut s = HudState::default();
        s.titles.set_times(0, 0, 0);
        assert_eq!((s.titles.fade_in, s.titles.stay, s.titles.fade_out), (0, 0, 0));
    }

    #[test]
    fn set_times_re_arms_a_live_title_at_its_full_duration() {
        let mut s = HudState::default();
        s.titles.set_title(text("hi"));
        for _ in 0..50 {
            s.titles.tick();
        }
        assert_eq!(s.titles.title_time, 50);
        s.titles.set_times(10, 70, 20);
        // MUTATION partner: dropping the trailing `if` leaves this at 50.
        assert_eq!(s.titles.title_time, 100);
    }

    #[test]
    fn set_times_does_not_arm_a_title_that_is_not_showing() {
        let mut s = HudState::default();
        s.titles.set_times(1, 2, 3);
        assert_eq!(s.titles.title_time, 0);
    }

    #[test]
    fn clear_keeps_the_times_and_reset_restores_them() {
        let mut s = HudState::default();
        s.titles.set_times(5, 6, 7);
        s.titles.set_title(text("hi"));
        // `resetTimes = false`.
        assert!(apply(HudPacket::ClearTitles, &[0], &mut s));
        assert_eq!(s.titles.title, None);
        assert_eq!(s.titles.title_time, 0);
        // MUTATION partner: resetting unconditionally makes this (10, 70, 20).
        assert_eq!((s.titles.fade_in, s.titles.stay, s.titles.fade_out), (5, 6, 7));
        // `resetTimes = true`.
        assert!(apply(HudPacket::ClearTitles, &[1], &mut s));
        assert_eq!(
            (s.titles.fade_in, s.titles.stay, s.titles.fade_out),
            (DEFAULT_FADE_IN, DEFAULT_STAY, DEFAULT_FADE_OUT)
        );
    }

    #[test]
    fn the_expiry_drops_the_subtitle_too() {
        let mut s = HudState::default();
        s.titles.set_times(0, 1, 0);
        s.titles.set_title(text("t"));
        s.titles.set_subtitle(text("s"));
        assert_eq!(s.titles.title_time, 1);
        s.titles.tick();
        assert_eq!(s.titles.title_time, 0);
        assert_eq!(s.titles.title, None);
        assert_eq!(s.titles.subtitle, None);
        // …and the counter does not run negative.
        s.titles.tick();
        assert_eq!(s.titles.title_time, 0);
    }

    #[test]
    fn the_action_bar_lives_sixty_ticks_and_never_animates_from_the_packet() {
        let mut s = HudState::default();
        assert!(apply(
            HudPacket::SetActionBarText,
            &component_body("bar"),
            &mut s
        ));
        assert_eq!(s.titles.overlay_message_time, OVERLAY_MESSAGE_TICKS);
        // MUTATION partner: passing `true` to `set_overlay_message` flips this.
        assert!(!s.titles.animate_overlay_message_color);
        for _ in 0..60 {
            s.titles.tick();
        }
        assert_eq!(s.titles.overlay_message_time, 0);
        // The string is NOT cleared on expiry — only `titleTime` does that.
        assert_eq!(s.titles.overlay_message, Some(text("bar")));
    }

    #[test]
    fn the_action_bar_clock_is_independent_of_the_title_times() {
        let mut s = HudState::default();
        s.titles.set_times(0, 5, 0);
        s.titles.set_overlay_message(text("bar"), false);
        assert_eq!(s.titles.overlay_message_time, 60);
    }

    #[test]
    fn set_experience_reads_level_before_total() {
        let mut body = Vec::new();
        body.extend_from_slice(&0.25f32.to_be_bytes());
        varint(7, &mut body); // level
        varint(1000, &mut body); // total
        let v = read_set_experience(&body).unwrap();
        // MUTATION partner: reading the fields in declaration order swaps
        // these two and decodes without error.
        assert_eq!(v.level, 7);
        assert_eq!(v.total, 1000);
        assert_eq!(v.progress, 0.25);
    }

    #[test]
    fn the_first_experience_update_does_not_prioritise_the_bar() {
        let mut s = ExperienceState::default();
        for _ in 0..50 {
            s.tick();
        }
        s.set_values(SetExperience {
            progress: 0.5,
            level: 3,
            total: 30,
        });
        assert_eq!(s.display_start_tick, i32::MIN + 1);
        // MUTATION partner: writing `tick_count` on the first change makes
        // this true.
        assert!(!s.will_prioritize());
        // The second change does arm it.
        s.set_values(SetExperience {
            progress: 0.6,
            level: 3,
            total: 32,
        });
        assert_eq!(s.display_start_tick, 50);
        assert!(s.will_prioritize());
    }

    #[test]
    fn a_level_change_with_unchanged_progress_does_not_re_arm() {
        let mut s = ExperienceState::default();
        // Burn the sentinel so the next change would write a real tick.
        s.set_values(SetExperience {
            progress: 0.5,
            level: 1,
            total: 10,
        });
        for _ in 0..200 {
            s.tick();
        }
        s.set_values(SetExperience {
            progress: 0.5,
            level: 9,
            total: 400,
        });
        // MUTATION partner: keying the re-arm on any field makes this 200.
        assert_eq!(s.display_start_tick, i32::MIN + 1);
        assert_eq!(s.level, 9);
        assert_eq!(s.total, 400);
    }

    #[test]
    fn the_xp_curve_has_its_three_segments() {
        let at = |level: i32| ExperienceState {
            level,
            ..ExperienceState::default()
        }
        .xp_needed_for_next_level();
        assert_eq!(at(0), 7);
        assert_eq!(at(14), 35);
        assert_eq!(at(15), 37);
        assert_eq!(at(29), 107);
        assert_eq!(at(30), 112);
        assert_eq!(at(31), 121);
        // The `> 0` guard is reachable only below -3.
        assert_eq!(at(-3), 1);
        assert_eq!(at(-4), -1);
    }

    #[test]
    fn a_zero_duration_cooldown_is_a_removal() {
        let mut s = HudState::default();
        let mut body = Vec::new();
        let name = "minecraft:ender_pearl";
        varint(name.len() as i32, &mut body);
        body.extend_from_slice(name.as_bytes());
        varint(40, &mut body);
        assert!(apply(HudPacket::Cooldown, &body, &mut s));
        assert!(s.cooldowns.is_on_cooldown(name));

        let mut zero = Vec::new();
        varint(name.len() as i32, &mut zero);
        zero.extend_from_slice(name.as_bytes());
        varint(0, &mut zero);
        assert!(apply(HudPacket::Cooldown, &zero, &mut s));
        // MUTATION partner: routing 0 through `add` leaves an instance whose
        // percent is 0/0 — NaN, which `clamp` does not rescue.
        assert!(s.cooldowns.is_empty());
        assert_eq!(s.cooldowns.percent(name, 0.0), 0.0);
    }

    #[test]
    fn a_cooldown_runs_down_and_is_dropped_the_tick_it_ends() {
        let mut c = ItemCooldowns::default();
        c.add("g", 4);
        assert_eq!(c.percent("g", 0.0), 1.0);
        c.tick();
        assert_eq!(c.percent("g", 0.0), 0.75);
        c.tick();
        c.tick();
        assert_eq!(c.percent("g", 0.0), 0.25);
        // MUTATION partner: incrementing after the sweep keeps it one tick
        // longer.
        c.tick();
        assert!(c.is_empty());
        assert_eq!(c.percent("g", 0.0), 0.0);
    }

    #[test]
    fn the_partial_tick_interpolates_between_whole_ticks() {
        let mut c = ItemCooldowns::default();
        c.add("g", 10);
        c.tick();
        assert!((c.percent("g", 0.0) - 0.9).abs() < 1e-6);
        assert!((c.percent("g", 0.5) - 0.85).abs() < 1e-6);
        // …and it never runs past the ends.
        assert_eq!(c.percent("g", -100.0), 1.0);
        assert_eq!(c.percent("g", 100.0), 0.0);
    }

    #[test]
    fn re_adding_a_group_restarts_it_from_the_current_tick() {
        let mut c = ItemCooldowns::default();
        c.add("g", 10);
        for _ in 0..5 {
            c.tick();
        }
        assert!((c.percent("g", 0.0) - 0.5).abs() < 1e-6);
        c.add("g", 10);
        assert_eq!(c.percent("g", 0.0), 1.0);
        assert_eq!(c.tick_count(), 5);
    }

    #[test]
    fn a_respawn_keeps_the_title_and_drops_the_player_state() {
        let mut s = HudState::default();
        s.titles.set_times(5, 6, 7);
        s.titles.set_title(text("welcome"));
        s.cooldowns.add("g", 40);
        s.experience.set_values(SetExperience {
            progress: 0.5,
            level: 30,
            total: 900,
        });
        s.reset_for_respawn();
        // MUTATION partner: clearing the titles here loses a title vanilla
        // keeps across a death.
        assert_eq!(s.titles.title, Some(text("welcome")));
        assert_eq!(s.titles.title_time, 18);
        assert_eq!((s.titles.fade_in, s.titles.stay, s.titles.fade_out), (5, 6, 7));
        // …and the player's halves are gone, sentinel included.
        assert!(s.cooldowns.is_empty());
        assert_eq!(s.experience.level, 0);
        assert_eq!(s.experience.display_start_tick, i32::MIN);
    }

    #[test]
    fn a_disconnect_clears_both_halves_of_the_title_state() {
        let mut t = TitleOverlay::default();
        t.set_times(5, 6, 7);
        t.set_title(text("hi"));
        t.on_disconnected();
        assert_eq!(t.title, None);
        assert_eq!(t.title_time, 0);
        assert_eq!(
            (t.fade_in, t.stay, t.fade_out),
            (DEFAULT_FADE_IN, DEFAULT_STAY, DEFAULT_FADE_OUT)
        );
    }

    #[test]
    fn the_seven_ids_map_to_seven_distinct_packets() {
        let kinds = [
            (14, HudPacket::ClearTitles),
            (22, HudPacket::Cooldown),
            (87, HudPacket::SetActionBarText),
            (103, HudPacket::SetExperience),
            (112, HudPacket::SetSubtitleText),
            (114, HudPacket::SetTitleText),
            (115, HudPacket::SetTitlesAnimation),
        ];
        for (id, want) in kinds {
            assert_eq!(kind_for_id(id, IDS), Some(want), "id {id}");
        }
        assert_eq!(kind_for_id(0, IDS), None);
        assert_eq!(kind_for_id(999, IDS), None);
    }

    #[test]
    fn the_animation_triple_is_twelve_fixed_bytes_not_var_ints() {
        let mut body = Vec::new();
        body.extend_from_slice(&10i32.to_be_bytes());
        body.extend_from_slice(&70i32.to_be_bytes());
        body.extend_from_slice(&20i32.to_be_bytes());
        assert_eq!(body.len(), 12);
        assert_eq!(read_titles_animation(&body).unwrap(), (10, 70, 20));
        // A var-int reading would have consumed one byte per value and
        // succeeded on a short body; the fixed reader rejects it.
        assert!(read_titles_animation(&[10, 70, 20]).is_err());
    }

    #[test]
    fn a_decode_failure_leaves_the_state_untouched() {
        let mut s = HudState::default();
        s.titles.set_title(text("kept"));
        let before = s.clone();
        assert!(!apply(HudPacket::SetTitlesAnimation, &[1, 2], &mut s));
        assert!(!apply(HudPacket::ClearTitles, &[], &mut s));
        assert!(!apply(HudPacket::SetExperience, &[0, 0], &mut s));
        assert!(!apply(HudPacket::Cooldown, &[], &mut s));
        assert_eq!(s, before);
    }

    #[test]
    fn a_component_flattens_through_extra_the_way_chat_does() {
        let tag = Nbt::Compound(vec![
            ("text".into(), Nbt::String("a".into())),
            (
                "extra".into(),
                Nbt::List(vec![Nbt::String("b".into()), Nbt::String("c".into())]),
            ),
        ]);
        assert_eq!(plain(&tag), "abc");
    }
}
