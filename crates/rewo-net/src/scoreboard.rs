//! The scoreboard — objectives, scores and display slots (M65).
//!
//! M62 shipped [`crate::teams`], which is `Scoreboard`'s *team* half. This is
//! the other half, and it is deliberately not a parallel state machine:
//! [`Scoreboard`] owns the `Teams` it already had, because vanilla's
//! `Scoreboard` owns both and the two interact (a `DisplaySlot` is named after
//! a team colour, and `removeObjective` reaches across into the display map).
//!
//! **Decode + state only.** Nothing here is wired to a renderer; a sidebar is
//! [`Scoreboard::display_objective`] plus [`Scoreboard::scores_for_objective`]
//! away, and that is a HUD-visual job the visual freeze
//! (`REWO_VELVET_UI_PLAN.md` §8) defers.
//!
//! ## Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/network/protocol/game/ClientboundSetObjectivePacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundSetScorePacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundResetScorePacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundSetDisplayObjectivePacket.java`
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java` —
//!   `handleAddObjective` / `handleSetScore` / `handleResetScore` /
//!   `handleSetDisplayObjective`, which is where the *client's* rules live
//!   (an unknown objective warns, a null objective name clears a slot)
//! - `net/minecraft/world/scores/Scoreboard.java` + `PlayerScores.java`
//! - `net/minecraft/world/scores/DisplaySlot.java`
//! - `net/minecraft/network/chat/numbers/NumberFormatTypes.java`
//!
//! ## The one thing the whole file turns on
//!
//! A score is keyed by a **scoreboard name**, `ScoreHolder::getScoreboardName`
//! — a player's profile name, and for most other entities its UUID *string*.
//! That is the same key space [`crate::teams`] uses for membership, which is
//! why `PlaySession::team_of` has to go through the profile name and why a
//! score cannot be looked up by uuid either.

use std::collections::HashMap;

use rewo_data::number_formats::NumberFormatTypeIds;
use rewo_proto::nbt::Nbt;
use rewo_proto::reader::PacketReader;
use rewo_proto::{ProtoError, Result};

use crate::teams::Teams;

// ── Shared wire pieces ────────────────────────────────────────────────────

/// `ObjectiveCriteria.RenderType`, read by `input.readEnum`.
///
/// `readEnum` is `getEnumConstants()[readVarInt()]`, so the ordinal *is* the
/// wire value and an out-of-range one throws `ArrayIndexOutOfBoundsException`
/// in vanilla. That makes it an error here, **not** a clamp to `INTEGER` —
/// the opposite of [`DisplaySlot::by_id`] two types down, which really is a
/// clamp. The two conventions sit one field apart in this protocol and the
/// only way to tell them apart is to read which one the decompile used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderType {
    Integer,
    Hearts,
}

impl RenderType {
    pub fn from_ordinal(id: i32) -> Option<RenderType> {
        match id {
            0 => Some(RenderType::Integer),
            1 => Some(RenderType::Hearts),
            _ => None,
        }
    }

    fn read(r: &mut PacketReader<'_>) -> Result<RenderType> {
        let ordinal = r.varint()?;
        RenderType::from_ordinal(ordinal).ok_or(ProtoError::LengthOutOfRange {
            what: "objective render type ordinal",
            len: ordinal as i64,
            max: 1,
        })
    }
}

/// `NumberFormat` — how a score's *number* is rendered, or that it is hidden.
///
/// The dispatch id is a raw `minecraft:number_format_type` registry id and the
/// body's length depends on which type it names, so the ids have to be known
/// before the walk can continue. See
/// [`rewo_data::number_formats`] for why they are resolved by name.
#[derive(Clone, Debug, PartialEq)]
pub enum NumberFormat {
    /// `BlankFormat` — `StreamCodec.unit`, **zero bytes on the wire**. The
    /// score renders as nothing at all, which is how servers hide the numbers
    /// down the right of a sidebar.
    Blank,
    /// `StyledFormat` — a `Style`, one network-NBT tag. Not a `Component`: it
    /// styles the digits the client formats itself.
    Styled(Nbt),
    /// `FixedFormat` — a `Component`, one network-NBT tag. Replaces the number
    /// with arbitrary text.
    Fixed(Nbt),
}

impl NumberFormat {
    fn read(r: &mut PacketReader<'_>, ids: NumberFormatTypeIds) -> Result<NumberFormat> {
        let id = r.varint()?;
        if id == ids.blank {
            Ok(NumberFormat::Blank)
        } else if id == ids.styled {
            Ok(NumberFormat::Styled(r.nbt()?))
        } else if id == ids.fixed {
            Ok(NumberFormat::Fixed(r.nbt()?))
        } else {
            // Not skippable: a type we cannot name has a body of unknown
            // length, so continuing would read the next field out of the
            // middle of this one.
            Err(ProtoError::LengthOutOfRange {
                what: "number format type id",
                len: id as i64,
                max: 2,
            })
        }
    }

    /// `NumberFormatTypes.OPTIONAL_STREAM_CODEC` — `ByteBufCodecs::optional`,
    /// a bool then the value.
    fn read_optional(
        r: &mut PacketReader<'_>,
        ids: NumberFormatTypeIds,
    ) -> Result<Option<NumberFormat>> {
        if r.bool()? {
            Ok(Some(NumberFormat::read(r, ids)?))
        } else {
            Ok(None)
        }
    }
}

/// `net.minecraft.world.scores.DisplaySlot`.
///
/// Nineteen slots: the three real ones, then one per team colour. The team
/// slots exist so a server can show a different sidebar to each team, and they
/// are why this enum is not three variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DisplaySlot {
    List,
    Sidebar,
    BelowName,
    TeamBlack,
    TeamDarkBlue,
    TeamDarkGreen,
    TeamDarkAqua,
    TeamDarkRed,
    TeamDarkPurple,
    TeamGold,
    TeamGray,
    TeamDarkGray,
    TeamBlue,
    TeamGreen,
    TeamAqua,
    TeamRed,
    TeamLightPurple,
    TeamYellow,
    TeamWhite,
}

impl DisplaySlot {
    pub const ALL: [DisplaySlot; 19] = [
        DisplaySlot::List,
        DisplaySlot::Sidebar,
        DisplaySlot::BelowName,
        DisplaySlot::TeamBlack,
        DisplaySlot::TeamDarkBlue,
        DisplaySlot::TeamDarkGreen,
        DisplaySlot::TeamDarkAqua,
        DisplaySlot::TeamDarkRed,
        DisplaySlot::TeamDarkPurple,
        DisplaySlot::TeamGold,
        DisplaySlot::TeamGray,
        DisplaySlot::TeamDarkGray,
        DisplaySlot::TeamBlue,
        DisplaySlot::TeamGreen,
        DisplaySlot::TeamAqua,
        DisplaySlot::TeamRed,
        DisplaySlot::TeamLightPurple,
        DisplaySlot::TeamYellow,
        DisplaySlot::TeamWhite,
    ];

    /// `DisplaySlot.BY_ID = ByIdMap.continuous(..., OutOfBoundsStrategy.ZERO)`
    /// — an id outside 0..=18 is **`LIST`**, not an error.
    ///
    /// This is the same clamp M62 records for team visibility and colour, and
    /// the opposite of the `readEnum` two types up. Rejecting an out-of-range
    /// slot would drop a packet a vanilla client applies (to the tab list).
    pub fn by_id(id: i32) -> DisplaySlot {
        usize::try_from(id)
            .ok()
            .and_then(|i| Self::ALL.get(i))
            .copied()
            .unwrap_or(DisplaySlot::List)
    }

    pub fn id(self) -> i32 {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(0) as i32
    }

    /// `getSerializedName` — the `/scoreboard objectives setdisplay` name.
    /// Not on the wire; kept so a log line or a future HUD reads the slot the
    /// way the operator typed it.
    pub fn name(self) -> &'static str {
        match self {
            DisplaySlot::List => "list",
            DisplaySlot::Sidebar => "sidebar",
            DisplaySlot::BelowName => "below_name",
            DisplaySlot::TeamBlack => "sidebar.team.black",
            DisplaySlot::TeamDarkBlue => "sidebar.team.dark_blue",
            DisplaySlot::TeamDarkGreen => "sidebar.team.dark_green",
            DisplaySlot::TeamDarkAqua => "sidebar.team.dark_aqua",
            DisplaySlot::TeamDarkRed => "sidebar.team.dark_red",
            DisplaySlot::TeamDarkPurple => "sidebar.team.dark_purple",
            DisplaySlot::TeamGold => "sidebar.team.gold",
            DisplaySlot::TeamGray => "sidebar.team.gray",
            DisplaySlot::TeamDarkGray => "sidebar.team.dark_gray",
            DisplaySlot::TeamBlue => "sidebar.team.blue",
            DisplaySlot::TeamGreen => "sidebar.team.green",
            DisplaySlot::TeamAqua => "sidebar.team.aqua",
            DisplaySlot::TeamRed => "sidebar.team.red",
            DisplaySlot::TeamLightPurple => "sidebar.team.light_purple",
            DisplaySlot::TeamYellow => "sidebar.team.yellow",
            DisplaySlot::TeamWhite => "sidebar.team.white",
        }
    }
}

// ── set_objective ─────────────────────────────────────────────────────────

/// `ClientboundSetObjectivePacket`'s method byte.
///
/// Vanilla reads a signed byte into an int and compares it against three
/// literals. Anything else carries **no** trailing block (the decoder's
/// condition is `method != 0 && method != 2`) and does nothing in the handler
/// (`if method == 0 … else if method == 1 … else if method == 2 …`), so it is
/// `Unknown` rather than an error — the same shape as `TeamMethod::Unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectiveMethod {
    /// 0 — create.
    Add,
    /// 1 — delete.
    Remove,
    /// 2 — update the display block of an existing objective.
    Change,
    Unknown(i8),
}

impl ObjectiveMethod {
    pub fn from_byte(b: i8) -> ObjectiveMethod {
        match b {
            0 => ObjectiveMethod::Add,
            1 => ObjectiveMethod::Remove,
            2 => ObjectiveMethod::Change,
            other => ObjectiveMethod::Unknown(other),
        }
    }

    /// Methods 0 and 2 carry the display block; 1 and anything else do not.
    pub fn has_display(self) -> bool {
        matches!(self, ObjectiveMethod::Add | ObjectiveMethod::Change)
    }
}

/// The three trailing fields methods 0 and 2 carry.
///
/// Grouped rather than three `Option`s because they are present or absent
/// together, which is exactly the invariant a reader of this struct wants and
/// exactly what three independent `Option`s would fail to state.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectiveDisplay {
    /// `ComponentSerialization.TRUSTED_STREAM_CODEC` — one network-NBT tag.
    pub display_name: Nbt,
    pub render_type: RenderType,
    pub number_format: Option<NumberFormat>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SetObjective {
    pub name: String,
    pub method: ObjectiveMethod,
    pub display: Option<ObjectiveDisplay>,
}

impl SetObjective {
    /// Reader-based so a witness can assert **exact** byte consumption against
    /// a sentinel; `parse_set_objective` is the whole-body wrapper the
    /// dispatch uses.
    pub fn read(r: &mut PacketReader<'_>, ids: NumberFormatTypeIds) -> Result<SetObjective> {
        let name = r.string(32767)?;
        let method = ObjectiveMethod::from_byte(r.i8()?);
        let display = if method.has_display() {
            Some(ObjectiveDisplay {
                display_name: r.nbt()?,
                render_type: RenderType::read(r)?,
                number_format: NumberFormat::read_optional(r, ids)?,
            })
        } else {
            None
        };
        Ok(SetObjective {
            name,
            method,
            display,
        })
    }
}

/// Decode a `set_objective` body.
pub fn parse_set_objective(body: &[u8], ids: NumberFormatTypeIds) -> Result<SetObjective> {
    SetObjective::read(&mut PacketReader::new(body), ids)
}

// ── set_score ─────────────────────────────────────────────────────────────

/// `ClientboundSetScorePacket` — a composite, so every field is
/// unconditional. The one that reads wrong at a glance is `score`: it is
/// `ByteBufCodecs.VAR_INT`, so a **negative** score is a five-byte var-int,
/// not a fixed i32 and not an error.
#[derive(Clone, Debug, PartialEq)]
pub struct SetScore {
    /// A scoreboard name — see the module doc.
    pub owner: String,
    pub objective_name: String,
    pub score: i32,
    /// `ComponentSerialization.TRUSTED_OPTIONAL_STREAM_CODEC` — an override
    /// for how the *holder* is named on this line, distinct from the score's
    /// number format below.
    pub display: Option<Nbt>,
    pub number_format: Option<NumberFormat>,
}

impl SetScore {
    pub fn read(r: &mut PacketReader<'_>, ids: NumberFormatTypeIds) -> Result<SetScore> {
        Ok(SetScore {
            owner: r.string(32767)?,
            objective_name: r.string(32767)?,
            score: r.varint()?,
            display: r.option(|r| r.nbt())?,
            number_format: NumberFormat::read_optional(r, ids)?,
        })
    }
}

pub fn parse_set_score(body: &[u8], ids: NumberFormatTypeIds) -> Result<SetScore> {
    SetScore::read(&mut PacketReader::new(body), ids)
}

// ── reset_score ───────────────────────────────────────────────────────────

/// `ClientboundResetScorePacket` — `readUtf()` then
/// `readNullable(FriendlyByteBuf::readUtf)`.
///
/// The absent objective name is not "no objective", it is **every** objective:
/// `handleResetScore` calls `resetAllPlayerScores` for the holder. A reader
/// that treated `None` as a no-op would leave a departed player's scores on
/// every sidebar they appeared on.
#[derive(Clone, Debug, PartialEq)]
pub struct ResetScore {
    pub owner: String,
    pub objective_name: Option<String>,
}

impl ResetScore {
    pub fn read(r: &mut PacketReader<'_>) -> Result<ResetScore> {
        Ok(ResetScore {
            owner: r.string(32767)?,
            objective_name: r.option(|r| r.string(32767))?,
        })
    }
}

pub fn parse_reset_score(body: &[u8]) -> Result<ResetScore> {
    ResetScore::read(&mut PacketReader::new(body))
}

// ── set_display_objective ─────────────────────────────────────────────────

/// `ClientboundSetDisplayObjectivePacket`.
///
/// `objective_name` is `None` for the **empty string**, not for an absent
/// field: the packet always writes a string, and `getObjectiveName()` maps
/// `""` to null. That empty string is how a server *clears* a slot, so
/// treating it as an ordinary (unfindable) name would leave a stale sidebar up
/// forever.
#[derive(Clone, Debug, PartialEq)]
pub struct SetDisplayObjective {
    pub slot: DisplaySlot,
    pub objective_name: Option<String>,
}

impl SetDisplayObjective {
    pub fn read(r: &mut PacketReader<'_>) -> Result<SetDisplayObjective> {
        let slot = DisplaySlot::by_id(r.varint()?);
        let name = r.string(32767)?;
        Ok(SetDisplayObjective {
            slot,
            objective_name: if name.is_empty() { None } else { Some(name) },
        })
    }
}

pub fn parse_set_display_objective(body: &[u8]) -> Result<SetDisplayObjective> {
    SetDisplayObjective::read(&mut PacketReader::new(body))
}

// ── The state ─────────────────────────────────────────────────────────────

/// One objective as the client holds it (`net.minecraft.world.scores.Objective`).
///
/// The criterion is absent on purpose: `handleAddObjective` hard-codes
/// `ObjectiveCriteria.DUMMY` for every objective a client is told about, so
/// storing it would be storing a constant.
#[derive(Clone, Debug, PartialEq)]
pub struct Objective {
    pub name: String,
    pub display_name: Nbt,
    pub render_type: RenderType,
    pub number_format: Option<NumberFormat>,
}

/// One holder's score in one objective (`net.minecraft.world.scores.Score`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Score {
    pub value: i32,
    pub display: Option<Nbt>,
    pub number_format: Option<NumberFormat>,
}

/// The client-side scoreboard.
///
/// Holds M62's [`Teams`] rather than sitting beside it, because vanilla's
/// `Scoreboard` is one object and the halves touch: `removeObjective` walks
/// the display map, and the display slots are named after team colours.
#[derive(Debug, Default, Clone)]
pub struct Scoreboard {
    /// The team half (M62). Public because it was `PlaySession::teams` before
    /// this module existed and every caller wants it directly.
    pub teams: Teams,
    objectives: HashMap<String, Objective>,
    /// `Scoreboard.playerScores` — holder name → objective name → score.
    ///
    /// Nested exactly as vanilla nests it (`Map<String, PlayerScores>`) rather
    /// than flattened to a `(holder, objective)` key, because two of the four
    /// operations are per-holder: `resetAllPlayerScores` removes the whole
    /// inner map, and `resetSinglePlayerScore` removes the holder once its
    /// inner map empties.
    scores: HashMap<String, HashMap<String, Score>>,
    /// `Scoreboard.displayObjectives`. A slot with no objective is **absent**
    /// here where vanilla stores an explicit null; `getDisplayObjective`
    /// cannot tell the two apart, so neither can anything downstream.
    display: HashMap<DisplaySlot, String>,
}

impl Scoreboard {
    pub fn new() -> Scoreboard {
        Scoreboard::default()
    }

    /// `ClientPacketListener::handleAddObjective`.
    ///
    /// Returns false when the packet changed nothing — an update or a remove
    /// naming an objective the client does not have, which vanilla logs
    /// nothing for and simply skips.
    pub fn apply_set_objective(&mut self, p: &SetObjective) -> bool {
        match p.method {
            ObjectiveMethod::Add => {
                let Some(display) = &p.display else {
                    // Unreachable through `parse_set_objective` (method 0
                    // always carries the block); guarded rather than
                    // unwrapped so a hand-built value cannot panic.
                    return false;
                };
                if self.objectives.contains_key(&p.name) {
                    // Vanilla `Scoreboard::addObjective` THROWS
                    // IllegalArgumentException here, which on the client means
                    // the connection dies. A server does not send it, so this
                    // is a documented divergence, the same shape as M62's
                    // `removePlayerFromTeam` one: log, and leave the existing
                    // objective exactly as it was. Overwriting instead would
                    // be inventing a behaviour vanilla does not have.
                    log::debug!("play: set_objective ADD for existing objective {}", p.name);
                    return false;
                }
                self.objectives.insert(
                    p.name.clone(),
                    Objective {
                        name: p.name.clone(),
                        display_name: display.display_name.clone(),
                        render_type: display.render_type,
                        number_format: display.number_format.clone(),
                    },
                );
                true
            }
            ObjectiveMethod::Remove => {
                if !self.objectives.contains_key(&p.name) {
                    return false;
                }
                self.remove_objective(&p.name);
                true
            }
            ObjectiveMethod::Change => {
                let Some(display) = &p.display else {
                    return false;
                };
                let Some(objective) = self.objectives.get_mut(&p.name) else {
                    return false;
                };
                objective.render_type = display.render_type;
                objective.display_name = display.display_name.clone();
                objective.number_format = display.number_format.clone();
                true
            }
            ObjectiveMethod::Unknown(_) => false,
        }
    }

    /// `Scoreboard::removeObjective`.
    ///
    /// Three effects, and the last two are the ones a naive `remove` misses:
    /// every display slot pointing at it is cleared, and every holder's score
    /// for it is dropped. The holder itself is **kept even when that empties
    /// it** — unlike `resetSinglePlayerScore`, which drops an emptied holder.
    /// The asymmetry is vanilla's, and it is observable through
    /// [`Self::tracked_holders`].
    fn remove_objective(&mut self, name: &str) {
        self.objectives.remove(name);
        self.display.retain(|_, objective| objective != name);
        for holder in self.scores.values_mut() {
            holder.remove(name);
        }
    }

    /// `ClientPacketListener::handleSetScore`.
    ///
    /// Returns false for an unknown objective, which vanilla warns about and
    /// otherwise ignores — it does **not** create the objective, so a score
    /// arriving before its objective is simply lost.
    pub fn apply_set_score(&mut self, p: &SetScore) -> bool {
        if !self.objectives.contains_key(&p.objective_name) {
            log::debug!(
                "play: set_score for unknown objective {}",
                p.objective_name
            );
            return false;
        }
        let entry = self
            .scores
            .entry(p.owner.clone())
            .or_default()
            .entry(p.objective_name.clone())
            .or_default();
        entry.value = p.score;
        entry.display = p.display.clone();
        entry.number_format = p.number_format.clone();
        true
    }

    /// `ClientPacketListener::handleResetScore`.
    pub fn apply_reset_score(&mut self, p: &ResetScore) -> bool {
        let Some(objective) = &p.objective_name else {
            // `resetAllPlayerScores` — the holder leaves the scoreboard
            // entirely.
            return self.scores.remove(&p.owner).is_some();
        };
        if !self.objectives.contains_key(objective) {
            log::debug!("play: reset_score for unknown objective {objective}");
            return false;
        }
        let Some(holder) = self.scores.get_mut(&p.owner) else {
            return false;
        };
        let removed = holder.remove(objective).is_some();
        // `resetSinglePlayerScore`: an emptied holder is dropped. Note the
        // check is on emptiness, not on `removed` — vanilla drops a holder
        // that was already empty too.
        if holder.is_empty() {
            self.scores.remove(&p.owner);
            return true;
        }
        removed
    }

    /// `ClientPacketListener::handleSetDisplayObjective`.
    ///
    /// An objective name the client does not know resolves to null and
    /// therefore **clears** the slot — it is not ignored. That is the whole
    /// reason this returns `()`: there is no failure case.
    pub fn apply_set_display_objective(&mut self, p: &SetDisplayObjective) {
        match p
            .objective_name
            .as_ref()
            .filter(|name| self.objectives.contains_key(name.as_str()))
        {
            Some(name) => {
                self.display.insert(p.slot, name.clone());
            }
            None => {
                self.display.remove(&p.slot);
            }
        }
    }

    // ── Reads ─────────────────────────────────────────────────────────────

    pub fn objective(&self, name: &str) -> Option<&Objective> {
        self.objectives.get(name)
    }

    pub fn objective_names(&self) -> impl Iterator<Item = &str> {
        self.objectives.keys().map(String::as_str)
    }

    /// The objective shown in a slot, if any — `Scoreboard::getDisplayObjective`.
    pub fn display_objective(&self, slot: DisplaySlot) -> Option<&Objective> {
        self.display.get(&slot).and_then(|name| self.objectives.get(name))
    }

    /// The **name** stored against a slot.
    ///
    /// Deliberately distinct from [`Self::display_objective`], which resolves
    /// through the objective map and so reports `None` for a stale entry as
    /// well as for an absent one. Vanilla stores an `Objective` *reference*,
    /// so there is no such thing as a stale entry there — which is exactly why
    /// `removeObjective` clears the slots, and why a witness has to be able to
    /// see the difference.
    pub fn display_objective_name(&self, slot: DisplaySlot) -> Option<&str> {
        self.display.get(&slot).map(String::as_str)
    }

    pub fn score(&self, owner: &str, objective: &str) -> Option<&Score> {
        self.scores.get(owner)?.get(objective)
    }

    /// Every holder with a score in this objective. Unordered — a sidebar
    /// sorts by value descending, which is a renderer's job, not this one's.
    pub fn scores_for_objective<'a>(
        &'a self,
        objective: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a Score)> + 'a {
        self.scores
            .iter()
            .filter_map(move |(holder, by_objective)| {
                by_objective.get(objective).map(|s| (holder.as_str(), s))
            })
    }

    /// `Scoreboard::getTrackedPlayers` — every holder the scoreboard knows,
    /// including one left empty by [`Self::remove_objective`].
    pub fn tracked_holders(&self) -> impl Iterator<Item = &str> {
        self.scores.keys().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.objectives.is_empty()
            && self.scores.is_empty()
            && self.display.is_empty()
            && self.teams.is_empty()
    }
}

#[cfg(test)]
mod tests {
    //! Bodies are built by hand and run through the real parsers and the real
    //! state machine — there is no second copy of any walk here, which is the
    //! drift M62's report records.

    use super::*;

    /// The 26.2 report's ids. A test that used `NumberFormatTypeIds::load`
    /// would be testing the loader; these pin the *walk*, and the loader is
    /// what fails loud if the real ids ever move.
    const IDS: NumberFormatTypeIds = NumberFormatTypeIds {
        blank: 0,
        styled: 1,
        fixed: 2,
    };

    /// Appended past every body so an over-read shows up as a wrong value and
    /// an under-read shows up as a leftover byte.
    const SENTINEL: u8 = 0xA7;

    fn varint(out: &mut Vec<u8>, mut v: i32) {
        loop {
            let mut b = (v & 0x7F) as u8;
            v = ((v as u32) >> 7) as i32;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                break;
            }
        }
    }

    fn utf(out: &mut Vec<u8>, s: &str) {
        varint(out, s.len() as i32);
        out.extend_from_slice(s.as_bytes());
    }

    /// A network-NBT string tag: tag id 8, then a big-endian u16 length.
    fn nbt_string(out: &mut Vec<u8>, s: &str) {
        out.push(8);
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    /// Run a reader-based decode over `body + SENTINEL` and prove it consumed
    /// **exactly** `body`. Both directions fail here: an under-read leaves
    /// more than one byte, and an over-read either eats the sentinel (so the
    /// trailing check fails) or runs off the end.
    fn exact<T>(body: &[u8], read: impl Fn(&mut PacketReader<'_>) -> Result<T>) -> T {
        let mut with_sentinel = body.to_vec();
        with_sentinel.push(SENTINEL);
        let mut r = PacketReader::new(&with_sentinel);
        let value = read(&mut r).expect("body decodes");
        assert_eq!(r.remaining(), 1, "decode must stop at the sentinel");
        assert_eq!(r.u8().unwrap(), SENTINEL);
        value
    }

    fn objective_body(
        name: &str,
        method: i8,
        display: Option<(&str, i32, Option<i32>)>,
    ) -> Vec<u8> {
        let mut b = Vec::new();
        utf(&mut b, name);
        b.push(method as u8);
        if let Some((display_name, render, format)) = display {
            nbt_string(&mut b, display_name);
            varint(&mut b, render);
            match format {
                Some(id) => {
                    b.push(1);
                    varint(&mut b, id);
                    if id != IDS.blank {
                        nbt_string(&mut b, "fmt");
                    }
                }
                None => b.push(0),
            }
        }
        b
    }

    fn score_body(
        owner: &str,
        objective: &str,
        score: i32,
        display: Option<&str>,
        format: Option<i32>,
    ) -> Vec<u8> {
        let mut b = Vec::new();
        utf(&mut b, owner);
        utf(&mut b, objective);
        varint(&mut b, score);
        match display {
            Some(d) => {
                b.push(1);
                nbt_string(&mut b, d);
            }
            None => b.push(0),
        }
        match format {
            Some(id) => {
                b.push(1);
                varint(&mut b, id);
                if id != IDS.blank {
                    nbt_string(&mut b, "fmt");
                }
            }
            None => b.push(0),
        }
        b
    }

    fn add(name: &str) -> SetObjective {
        parse_set_objective(&objective_body(name, 0, Some((name, 0, None))), IDS).unwrap()
    }

    // ── The wire ──────────────────────────────────────────────────────────

    #[test]
    fn an_add_objective_carries_a_display_block_and_a_remove_does_not() {
        let p = parse_set_objective(&objective_body("kills", 0, Some(("Kills", 1, None))), IDS)
            .unwrap();
        assert_eq!(p.method, ObjectiveMethod::Add);
        let display = p.display.expect("method 0 has a display block");
        assert_eq!(display.display_name, Nbt::String("Kills".into()));
        assert_eq!(display.render_type, RenderType::Hearts);

        let p = parse_set_objective(&objective_body("kills", 1, None), IDS).unwrap();
        assert_eq!(p.method, ObjectiveMethod::Remove);
        assert!(p.display.is_none());
    }

    #[test]
    fn an_unknown_objective_method_ends_the_packet_after_the_method_byte() {
        // `method != 0 && method != 2` covers 7 as well as 1, so the body is
        // two fields long. Reading a display block here would consume bytes
        // that are not in the packet.
        let body = objective_body("kills", 7, None);
        let p = parse_set_objective(&body, IDS).unwrap();
        assert_eq!(p.method, ObjectiveMethod::Unknown(7));
        assert!(p.display.is_none());
    }

    #[test]
    fn a_blank_number_format_occupies_zero_bytes_where_a_fixed_one_carries_a_tag() {
        // The whole reason the registry ids have to be resolved before the
        // walk: these two bodies differ in length by a tag, and only the id
        // says which is which.
        let blank = objective_body("k", 0, Some(("K", 0, Some(IDS.blank))));
        let fixed = objective_body("k", 0, Some(("K", 0, Some(IDS.fixed))));
        assert!(fixed.len() > blank.len());

        let p = parse_set_objective(&blank, IDS).unwrap();
        assert_eq!(p.display.unwrap().number_format, Some(NumberFormat::Blank));
        let p = parse_set_objective(&fixed, IDS).unwrap();
        assert_eq!(
            p.display.unwrap().number_format,
            Some(NumberFormat::Fixed(Nbt::String("fmt".into())))
        );
    }

    #[test]
    fn a_styled_number_format_is_a_style_tag_not_a_component() {
        let p = parse_set_objective(
            &objective_body("k", 0, Some(("K", 0, Some(IDS.styled)))),
            IDS,
        )
        .unwrap();
        assert_eq!(
            p.display.unwrap().number_format,
            Some(NumberFormat::Styled(Nbt::String("fmt".into())))
        );
    }

    #[test]
    fn an_unnameable_number_format_type_is_an_error_rather_than_a_skip() {
        // It has no length, so there is nothing to skip. Guessing would park
        // the reader mid-value; M41 records the same rule for component
        // patches.
        let mut b = Vec::new();
        utf(&mut b, "k");
        b.push(0);
        nbt_string(&mut b, "K");
        varint(&mut b, 0);
        b.push(1);
        varint(&mut b, 99);
        assert!(parse_set_objective(&b, IDS).is_err());
    }

    #[test]
    fn an_out_of_range_render_type_is_an_error_rather_than_integer() {
        // `readEnum` indexes `getEnumConstants()`, so vanilla throws. This is
        // the opposite convention to `DisplaySlot::by_id` one packet away.
        assert_eq!(RenderType::from_ordinal(2), None);
        let mut b = Vec::new();
        utf(&mut b, "k");
        b.push(0);
        nbt_string(&mut b, "K");
        varint(&mut b, 2);
        b.push(0);
        assert!(parse_set_objective(&b, IDS).is_err());
    }

    #[test]
    fn an_out_of_range_display_slot_is_list_rather_than_an_error() {
        // `ByIdMap.continuous(..., ZERO)`.
        assert_eq!(DisplaySlot::by_id(99), DisplaySlot::List);
        assert_eq!(DisplaySlot::by_id(-1), DisplaySlot::List);
        assert_eq!(DisplaySlot::by_id(1), DisplaySlot::Sidebar);
        assert_eq!(DisplaySlot::by_id(18), DisplaySlot::TeamWhite);
        for slot in DisplaySlot::ALL {
            assert_eq!(DisplaySlot::by_id(slot.id()), slot);
        }
    }

    #[test]
    fn an_empty_display_objective_name_is_the_clear_signal_not_a_name() {
        let mut b = Vec::new();
        varint(&mut b, 1);
        utf(&mut b, "");
        let p = parse_set_display_objective(&b).unwrap();
        assert_eq!(p.slot, DisplaySlot::Sidebar);
        assert_eq!(p.objective_name, None);
    }

    #[test]
    fn a_negative_score_is_a_var_int_not_a_short_read() {
        // `ByteBufCodecs.VAR_INT` zig-zags nothing, so -1 is five 0xFF-ish
        // bytes. A reader that assumed a fixed i32 would be four bytes out
        // and would then read the display flag from inside the number.
        let body = score_body("alice", "kills", -1, None, None);
        let p = parse_set_score(&body, IDS).unwrap();
        assert_eq!(p.score, -1);
        assert_eq!(p.owner, "alice");
        assert_eq!(p.display, None);
        assert_eq!(p.number_format, None);
    }

    #[test]
    fn set_score_consumes_exactly_its_body() {
        let body = score_body("alice", "kills", 7, Some("Alice!"), Some(IDS.styled));
        let p = exact(&body, |r| SetScore::read(r, IDS));
        assert_eq!(p.score, 7);
        assert_eq!(p.display, Some(Nbt::String("Alice!".into())));
        assert_eq!(
            p.number_format,
            Some(NumberFormat::Styled(Nbt::String("fmt".into())))
        );
    }

    #[test]
    fn set_objective_consumes_exactly_its_body() {
        let body = objective_body("kills", 2, Some(("Kills", 1, Some(IDS.fixed))));
        let p = exact(&body, |r| SetObjective::read(r, IDS));
        assert_eq!(p.method, ObjectiveMethod::Change);
        assert_eq!(p.display.unwrap().render_type, RenderType::Hearts);
    }

    #[test]
    fn a_blank_number_format_consumes_only_its_type_id() {
        // The zero-byte body is the easiest field in the packet to get wrong,
        // because every other dispatch arm reads a tag. If `Blank` read one
        // too, this eats the sentinel.
        let body = objective_body("k", 0, Some(("K", 0, Some(IDS.blank))));
        let p = exact(&body, |r| SetObjective::read(r, IDS));
        assert_eq!(p.display.unwrap().number_format, Some(NumberFormat::Blank));
    }

    #[test]
    fn an_unknown_method_objective_consumes_only_its_two_header_fields() {
        let body = objective_body("kills", 7, None);
        let p = exact(&body, |r| SetObjective::read(r, IDS));
        assert_eq!(p.method, ObjectiveMethod::Unknown(7));
    }

    #[test]
    fn set_display_objective_consumes_exactly_its_body() {
        let mut b = Vec::new();
        varint(&mut b, 2);
        utf(&mut b, "kills");
        let p = exact(&b, SetDisplayObjective::read);
        assert_eq!(p.slot, DisplaySlot::BelowName);
        assert_eq!(p.objective_name.as_deref(), Some("kills"));
    }

    #[test]
    fn reset_score_reads_a_nullable_objective_name() {
        let mut b = Vec::new();
        utf(&mut b, "alice");
        b.push(0);
        assert_eq!(
            parse_reset_score(&b).unwrap(),
            ResetScore {
                owner: "alice".into(),
                objective_name: None
            }
        );

        let mut b = Vec::new();
        utf(&mut b, "alice");
        b.push(1);
        utf(&mut b, "kills");
        let p = exact(&b, ResetScore::read);
        assert_eq!(p.objective_name.as_deref(), Some("kills"));
    }

    #[test]
    fn a_truncated_objective_body_is_an_error_rather_than_a_half_objective() {
        let mut b = Vec::new();
        utf(&mut b, "kills");
        b.push(0);
        assert!(parse_set_objective(&b, IDS).is_err());
    }

    // ── The state ─────────────────────────────────────────────────────────

    #[test]
    fn a_score_for_an_unknown_objective_is_dropped_and_creates_nothing() {
        let mut sb = Scoreboard::new();
        let p = parse_set_score(&score_body("alice", "kills", 3, None, None), IDS).unwrap();
        assert!(!sb.apply_set_score(&p));
        assert_eq!(sb.score("alice", "kills"), None);
        assert_eq!(sb.tracked_holders().count(), 0);
        assert_eq!(sb.objective_names().count(), 0);
    }

    #[test]
    fn a_score_lands_once_its_objective_exists() {
        let mut sb = Scoreboard::new();
        assert!(sb.apply_set_objective(&add("kills")));
        let p = parse_set_score(&score_body("alice", "kills", 3, None, None), IDS).unwrap();
        assert!(sb.apply_set_score(&p));
        assert_eq!(sb.score("alice", "kills").unwrap().value, 3);
    }

    #[test]
    fn re_adding_an_existing_objective_leaves_the_original_untouched() {
        // Vanilla throws; we log. Either way the stored objective must not
        // change, because a replacement is a behaviour vanilla never shows.
        let mut sb = Scoreboard::new();
        sb.apply_set_objective(&add("kills"));
        let changed = parse_set_objective(
            &objective_body("kills", 0, Some(("Something else", 1, None))),
            IDS,
        )
        .unwrap();
        assert!(!sb.apply_set_objective(&changed));
        let objective = sb.objective("kills").unwrap();
        assert_eq!(objective.display_name, Nbt::String("kills".into()));
        assert_eq!(objective.render_type, RenderType::Integer);
    }

    #[test]
    fn a_change_updates_all_three_display_fields_of_an_existing_objective() {
        let mut sb = Scoreboard::new();
        sb.apply_set_objective(&add("kills"));
        let change = parse_set_objective(
            &objective_body("kills", 2, Some(("Kills!", 1, Some(IDS.blank)))),
            IDS,
        )
        .unwrap();
        assert!(sb.apply_set_objective(&change));
        let objective = sb.objective("kills").unwrap();
        assert_eq!(objective.display_name, Nbt::String("Kills!".into()));
        assert_eq!(objective.render_type, RenderType::Hearts);
        assert_eq!(objective.number_format, Some(NumberFormat::Blank));
    }

    #[test]
    fn a_change_for_an_unknown_objective_does_not_create_it() {
        let mut sb = Scoreboard::new();
        let change =
            parse_set_objective(&objective_body("kills", 2, Some(("K", 0, None))), IDS).unwrap();
        assert!(!sb.apply_set_objective(&change));
        assert!(sb.objective("kills").is_none());
    }

    #[test]
    fn removing_an_objective_clears_its_display_slots_and_every_score_in_it() {
        // The two effects a plain map removal misses.
        let mut sb = Scoreboard::new();
        sb.apply_set_objective(&add("kills"));
        sb.apply_set_objective(&add("deaths"));
        sb.apply_set_score(
            &parse_set_score(&score_body("alice", "kills", 3, None, None), IDS).unwrap(),
        );
        sb.apply_set_score(
            &parse_set_score(&score_body("alice", "deaths", 1, None, None), IDS).unwrap(),
        );
        sb.apply_set_display_objective(&SetDisplayObjective {
            slot: DisplaySlot::Sidebar,
            objective_name: Some("kills".into()),
        });
        sb.apply_set_display_objective(&SetDisplayObjective {
            slot: DisplaySlot::List,
            objective_name: Some("deaths".into()),
        });

        let remove = parse_set_objective(&objective_body("kills", 1, None), IDS).unwrap();
        assert!(sb.apply_set_objective(&remove));

        assert!(sb.objective("kills").is_none());
        // The *stored name*, not the resolved objective: `display_objective`
        // reports None either way once the objective is gone, so asserting on
        // it alone would pass with the slot never cleared at all. The
        // mutation that skips the `retain` is what caught that.
        assert_eq!(sb.display_objective_name(DisplaySlot::Sidebar), None);
        assert!(sb.display_objective(DisplaySlot::Sidebar).is_none());
        assert_eq!(sb.score("alice", "kills"), None);
        // The other objective is untouched — `retain` must not clear every
        // slot, only the ones naming this objective.
        assert_eq!(
            sb.display_objective(DisplaySlot::List).map(|o| o.name.as_str()),
            Some("deaths")
        );
        assert_eq!(sb.score("alice", "deaths").unwrap().value, 1);
        assert_eq!(
            sb.display_objective_name(DisplaySlot::List),
            Some("deaths")
        );
    }

    #[test]
    fn re_creating_a_removed_objective_does_not_resurrect_its_old_sidebar() {
        // The consequence of the slot clear, and the reason it is not
        // cosmetic. Vanilla stores an object *reference*, so a new objective
        // of the same name is a different object and cannot land in a slot it
        // was never put in. Rewo stores the name, so skipping the clear would
        // put the new objective straight back on screen — a sidebar the
        // server never asked for, after a remove/re-add cycle servers run
        // constantly.
        let mut sb = Scoreboard::new();
        sb.apply_set_objective(&add("kills"));
        sb.apply_set_display_objective(&SetDisplayObjective {
            slot: DisplaySlot::Sidebar,
            objective_name: Some("kills".into()),
        });
        sb.apply_set_objective(
            &parse_set_objective(&objective_body("kills", 1, None), IDS).unwrap(),
        );
        sb.apply_set_objective(&add("kills"));

        assert!(sb.objective("kills").is_some());
        assert!(
            sb.display_objective(DisplaySlot::Sidebar).is_none(),
            "a re-created objective must not inherit the removed one's slot"
        );
    }

    #[test]
    fn removing_the_last_objective_keeps_the_now_empty_holder() {
        // vanilla's `removeObjective` calls `PlayerScores::remove` and does
        // NOT drop an emptied holder, where `resetSinglePlayerScore` does.
        // The asymmetry is real and shows through `getTrackedPlayers`.
        let mut sb = Scoreboard::new();
        sb.apply_set_objective(&add("kills"));
        sb.apply_set_score(
            &parse_set_score(&score_body("alice", "kills", 3, None, None), IDS).unwrap(),
        );
        sb.apply_set_objective(
            &parse_set_objective(&objective_body("kills", 1, None), IDS).unwrap(),
        );
        assert_eq!(sb.tracked_holders().collect::<Vec<_>>(), ["alice"]);
    }

    #[test]
    fn a_single_reset_that_empties_a_holder_drops_the_holder() {
        let mut sb = Scoreboard::new();
        sb.apply_set_objective(&add("kills"));
        sb.apply_set_score(
            &parse_set_score(&score_body("alice", "kills", 3, None, None), IDS).unwrap(),
        );
        assert!(sb.apply_reset_score(&ResetScore {
            owner: "alice".into(),
            objective_name: Some("kills".into()),
        }));
        assert_eq!(sb.tracked_holders().count(), 0);
    }

    #[test]
    fn a_single_reset_leaves_a_holders_other_scores_alone() {
        let mut sb = Scoreboard::new();
        sb.apply_set_objective(&add("kills"));
        sb.apply_set_objective(&add("deaths"));
        for (objective, value) in [("kills", 3), ("deaths", 1)] {
            sb.apply_set_score(
                &parse_set_score(&score_body("alice", objective, value, None, None), IDS).unwrap(),
            );
        }
        sb.apply_reset_score(&ResetScore {
            owner: "alice".into(),
            objective_name: Some("kills".into()),
        });
        assert_eq!(sb.score("alice", "kills"), None);
        assert_eq!(sb.score("alice", "deaths").unwrap().value, 1);
        assert_eq!(sb.tracked_holders().collect::<Vec<_>>(), ["alice"]);
    }

    #[test]
    fn a_reset_with_no_objective_name_clears_every_score_the_holder_has() {
        // The invertible one: `None` means ALL, not none.
        let mut sb = Scoreboard::new();
        sb.apply_set_objective(&add("kills"));
        sb.apply_set_objective(&add("deaths"));
        for (objective, value) in [("kills", 3), ("deaths", 1)] {
            sb.apply_set_score(
                &parse_set_score(&score_body("alice", objective, value, None, None), IDS).unwrap(),
            );
        }
        sb.apply_set_score(
            &parse_set_score(&score_body("bob", "kills", 9, None, None), IDS).unwrap(),
        );

        assert!(sb.apply_reset_score(&ResetScore {
            owner: "alice".into(),
            objective_name: None,
        }));
        assert_eq!(sb.score("alice", "kills"), None);
        assert_eq!(sb.score("alice", "deaths"), None);
        assert_eq!(sb.score("bob", "kills").unwrap().value, 9);
    }

    #[test]
    fn an_unnamed_reset_for_an_untracked_holder_is_inert() {
        let mut sb = Scoreboard::new();
        assert!(!sb.apply_reset_score(&ResetScore {
            owner: "nobody".into(),
            objective_name: None,
        }));
    }

    #[test]
    fn a_display_objective_naming_an_unknown_objective_clears_the_slot() {
        // The trap: this does NOT mean "ignore". `handleSetDisplayObjective`
        // resolves the name to null and passes null through, so a stale
        // sidebar comes down.
        let mut sb = Scoreboard::new();
        sb.apply_set_objective(&add("kills"));
        sb.apply_set_display_objective(&SetDisplayObjective {
            slot: DisplaySlot::Sidebar,
            objective_name: Some("kills".into()),
        });
        assert!(sb.display_objective(DisplaySlot::Sidebar).is_some());

        sb.apply_set_display_objective(&SetDisplayObjective {
            slot: DisplaySlot::Sidebar,
            objective_name: Some("never_created".into()),
        });
        assert!(sb.display_objective(DisplaySlot::Sidebar).is_none());
    }

    #[test]
    fn an_empty_name_clears_the_slot_it_names_and_no_other() {
        let mut sb = Scoreboard::new();
        sb.apply_set_objective(&add("kills"));
        for slot in [DisplaySlot::Sidebar, DisplaySlot::List] {
            sb.apply_set_display_objective(&SetDisplayObjective {
                slot,
                objective_name: Some("kills".into()),
            });
        }
        sb.apply_set_display_objective(&SetDisplayObjective {
            slot: DisplaySlot::Sidebar,
            objective_name: None,
        });
        assert!(sb.display_objective(DisplaySlot::Sidebar).is_none());
        assert!(sb.display_objective(DisplaySlot::List).is_some());
    }

    #[test]
    fn scores_for_objective_lists_only_that_objectives_holders() {
        let mut sb = Scoreboard::new();
        sb.apply_set_objective(&add("kills"));
        sb.apply_set_objective(&add("deaths"));
        sb.apply_set_score(
            &parse_set_score(&score_body("alice", "kills", 3, None, None), IDS).unwrap(),
        );
        sb.apply_set_score(
            &parse_set_score(&score_body("bob", "deaths", 1, None, None), IDS).unwrap(),
        );
        let mut rows: Vec<_> = sb
            .scores_for_objective("kills")
            .map(|(h, s)| (h.to_string(), s.value))
            .collect();
        rows.sort();
        assert_eq!(rows, [("alice".to_string(), 3)]);
    }

    #[test]
    fn a_second_set_score_overwrites_the_display_and_format_rather_than_merging() {
        // `score.display(...)` / `numberFormatOverride(...)` are assignments,
        // and the packet always carries both — so a later packet without them
        // clears what an earlier one set.
        let mut sb = Scoreboard::new();
        sb.apply_set_objective(&add("kills"));
        sb.apply_set_score(
            &parse_set_score(
                &score_body("alice", "kills", 3, Some("Alice"), Some(IDS.blank)),
                IDS,
            )
            .unwrap(),
        );
        sb.apply_set_score(
            &parse_set_score(&score_body("alice", "kills", 4, None, None), IDS).unwrap(),
        );
        let score = sb.score("alice", "kills").unwrap();
        assert_eq!(score.value, 4);
        assert_eq!(score.display, None);
        assert_eq!(score.number_format, None);
    }
}
