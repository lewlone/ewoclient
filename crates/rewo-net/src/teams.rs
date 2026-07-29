//! Scoreboard teams — `ClientboundSetPlayerTeamPacket` (M62).
//!
//! Transcribed from the 26.2 decompile:
//! `net/minecraft/network/protocol/game/ClientboundSetPlayerTeamPacket.java`
//! (the wire), `net/minecraft/client/multiplayer/ClientPacketListener
//! ::handleSetPlayerTeamPacket` (the order the effects apply in), and
//! `net/minecraft/world/scores/Scoreboard.java` (the state machine).
//!
//! This is the third of the tab list's four sort keys. It is a **separate
//! packet** from `player_info_update`, and it keys its members by **player
//! name**, not UUID — see `Teams::team_of_member` and the note on
//! `PlaySession::team_of` for how that resolves.
//!
//! Nothing here is wired to a renderer. Decode + state only.

use std::collections::{BTreeSet, HashMap};

use rewo_proto::nbt::Nbt;
use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

// ── The wire ──────────────────────────────────────────────────────────────

/// The packet's `method` byte.
///
/// Vanilla stores it as an **int read from a signed byte** and then compares
/// it against the five literals, so anything outside 0..=4 is neither a team
/// action nor a player action: `shouldHaveParameters` and
/// `shouldHavePlayerList` are both false, so such a packet is three bytes of
/// header and nothing else, and the handler does nothing with it. Modelling
/// that as `Unknown` rather than as a decode error is what keeps the reader
/// in step with a server that sends one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamMethod {
    /// 0 — create the team, set its parameters, **and** add the listed
    /// players. The only method that carries both trailing sections.
    Add,
    /// 1 — delete the team.
    Remove,
    /// 2 — update an existing team's parameters. No player list.
    Change,
    /// 3 — add the listed players to an existing team.
    Join,
    /// 4 — remove the listed players from an existing team.
    Leave,
    /// Anything else. Inert, and it carries no trailing sections.
    Unknown(i8),
}

impl TeamMethod {
    pub fn from_byte(b: i8) -> TeamMethod {
        match b {
            0 => TeamMethod::Add,
            1 => TeamMethod::Remove,
            2 => TeamMethod::Change,
            3 => TeamMethod::Join,
            4 => TeamMethod::Leave,
            other => TeamMethod::Unknown(other),
        }
    }

    /// `shouldHaveParameters` — methods 0 and 2.
    pub fn has_parameters(self) -> bool {
        matches!(self, TeamMethod::Add | TeamMethod::Change)
    }

    /// `shouldHavePlayerList` — methods 0, 3 and 4.
    ///
    /// Note it includes **Add**: a create packet carries the founding roster,
    /// which is why reading the parameters one byte wrong eats the roster.
    pub fn has_player_list(self) -> bool {
        matches!(self, TeamMethod::Add | TeamMethod::Join | TeamMethod::Leave)
    }
}

/// `Team.Visibility` / `Team.CollisionRule` — both are `ByIdMap.continuous`
/// with `OutOfBoundsStrategy.ZERO`, so an id outside 0..=3 is **`Always`**,
/// not an error. Keeping the raw id would lose that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Always,
    Never,
    HideForOtherTeams,
    HideForOwnTeam,
}

impl Visibility {
    pub fn by_id(id: i32) -> Visibility {
        match id {
            1 => Visibility::Never,
            2 => Visibility::HideForOtherTeams,
            3 => Visibility::HideForOwnTeam,
            _ => Visibility::Always,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollisionRule {
    Always,
    Never,
    PushOtherTeams,
    PushOwnTeam,
}

impl CollisionRule {
    pub fn by_id(id: i32) -> CollisionRule {
        match id {
            1 => CollisionRule::Never,
            2 => CollisionRule::PushOtherTeams,
            3 => CollisionRule::PushOwnTeam,
            _ => CollisionRule::Always,
        }
    }
}

/// `ClientboundSetPlayerTeamPacket.Parameters`, in composite order.
///
/// The three components are `ComponentSerialization.TRUSTED_STREAM_CODEC`,
/// which is `fromCodecWithRegistriesTrusted` — i.e. **one network-NBT tag
/// each**, the same shape `player_info_update`'s display name uses. They are
/// kept rather than skipped because a tab list that renders a team's prefix
/// and suffix needs them and re-deriving the walk later is the risk.
#[derive(Debug, Clone, PartialEq)]
pub struct TeamParameters {
    pub display_name: Nbt,
    pub player_prefix: Nbt,
    pub player_suffix: Nbt,
    pub name_tag_visibility: Visibility,
    pub collision_rule: CollisionRule,
    /// `ByteBufCodecs.optional(TeamColor.STREAM_CODEC)` — a bool, then a
    /// var-int id when present. 0..=15 in `TeamColor` declaration order; an
    /// out-of-range id is `BLACK` (0) by the same ZERO strategy, so this is
    /// clamped at decode rather than kept raw.
    pub color: Option<u8>,
    /// `PlayerTeam.packOptions`: bit 0 = allow friendly fire, bit 1 = see
    /// friendly invisibles. Kept packed because `unpackOptions` is the only
    /// consumer and it takes the byte.
    pub options: u8,
}

impl TeamParameters {
    pub fn allow_friendly_fire(&self) -> bool {
        self.options & 1 != 0
    }
    pub fn see_friendly_invisibles(&self) -> bool {
        self.options & 2 != 0
    }
}

/// One decoded `set_player_team` packet.
#[derive(Debug, Clone, PartialEq)]
pub struct SetPlayerTeam {
    pub name: String,
    pub method: TeamMethod,
    pub parameters: Option<TeamParameters>,
    /// **Player names, not UUIDs.** Vanilla's scoreboard is keyed by
    /// `ScoreHolder::getScoreboardName`, which for a player is the profile
    /// name and for most other entities is its UUID string.
    pub players: Vec<String>,
}

/// Decode a `set_player_team` body.
///
/// Standalone and pure so a test drives the **real** walk. The parameters
/// block is the fragile part: it sits between the header and the player
/// list, so a mis-sized read there does not fail — it hands the roster
/// reader whatever bytes are left, and a plausible-looking team with the
/// wrong members is much worse than an error.
pub fn parse_set_player_team(body: &[u8]) -> Result<SetPlayerTeam> {
    let mut r = PacketReader::new(body);
    // `input.readUtf()` with no argument is the 32767 default.
    let name = r.string(32767)?;
    // `input.readByte()` — signed, and widened to int before the comparisons.
    let method = TeamMethod::from_byte(r.i8()?);

    let parameters = if method.has_parameters() {
        Some(TeamParameters {
            display_name: r.nbt()?,
            player_prefix: r.nbt()?,
            player_suffix: r.nbt()?,
            name_tag_visibility: Visibility::by_id(r.varint()?),
            collision_rule: CollisionRule::by_id(r.varint()?),
            color: if r.bool()? {
                // ZERO strategy: out of range is BLACK.
                let id = r.varint()?;
                Some(if (0..=15).contains(&id) { id as u8 } else { 0 })
            } else {
                None
            },
            options: r.u8()?,
        })
    } else {
        None
    };

    let players = if method.has_player_list() {
        // `readList(FriendlyByteBuf::readUtf)` — a var-int count, then one
        // length-prefixed string each. A name is at least one byte.
        let count = r.count("team players", 1)?;
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(r.string(32767)?);
        }
        out
    } else {
        Vec::new()
    };

    Ok(SetPlayerTeam {
        name,
        method,
        parameters,
        players,
    })
}

// ── The state ─────────────────────────────────────────────────────────────

/// One team as the client holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Team {
    pub name: String,
    pub parameters: Option<TeamParameters>,
    /// Member scoreboard names. A `BTreeSet` so iteration is deterministic —
    /// vanilla's is insertion-ordered and nothing observable depends on it,
    /// but a test that iterates should not depend on hash order either.
    pub members: BTreeSet<String>,
}

/// The client-side scoreboard's team half — `Scoreboard`'s `teamsByName` and
/// `teamsByPlayer`.
///
/// The second map is not a convenience index: vanilla keeps it, and it is
/// what makes "a player is on exactly one team" true, because
/// `addPlayerToTeam` consults it and evicts the previous membership first.
#[derive(Debug, Default, Clone)]
pub struct Teams {
    by_name: HashMap<String, Team>,
    by_member: HashMap<String, String>,
}

impl Teams {
    pub fn new() -> Teams {
        Teams::default()
    }

    /// `ClientPacketListener::handleSetPlayerTeamPacket`, in its own order.
    ///
    /// Returns false when the packet was dropped for naming a team that does
    /// not exist — vanilla logs and **returns**, so the player list of such a
    /// packet is discarded too. That early return is the easy thing to miss:
    /// applying the roster anyway would create members of a team that was
    /// never created, and nothing downstream would complain.
    pub fn apply(&mut self, p: &SetPlayerTeam) -> bool {
        // Step 1 — `getTeamAction`: ADD for method 0, REMOVE for method 1.
        if p.method == TeamMethod::Add {
            // `addPlayerTeam`: an existing team is reused (vanilla warns),
            // NOT reset. Its current members survive and the packet's roster
            // is added on top.
            self.by_name.entry(p.name.clone()).or_insert_with(|| Team {
                name: p.name.clone(),
                parameters: None,
                members: BTreeSet::new(),
            });
        } else if !self.by_name.contains_key(&p.name) {
            log::debug!("play: set_player_team for unknown team {}", p.name);
            return false;
        }

        // Step 2 — parameters, if the method carried them.
        if let Some(params) = &p.parameters {
            if let Some(team) = self.by_name.get_mut(&p.name) {
                team.parameters = Some(params.clone());
            }
        }

        // Step 3 — `getPlayerAction`: ADD for methods 0 and 3, REMOVE for 4.
        match p.method {
            TeamMethod::Add | TeamMethod::Join => {
                for player in &p.players {
                    self.add_member(player, &p.name);
                }
            }
            TeamMethod::Leave => {
                for player in &p.players {
                    self.remove_member(player, &p.name);
                }
            }
            _ => {}
        }

        // Step 4 — and only now is a REMOVE actually removed, which is why a
        // remove packet for an unknown team returned early above.
        if p.method == TeamMethod::Remove {
            self.remove_team(&p.name);
        }
        true
    }

    /// `Scoreboard::addPlayerToTeam` — evict the prior membership first, so a
    /// player is never on two teams at once.
    fn add_member(&mut self, player: &str, team: &str) {
        if let Some(prev) = self.by_member.get(player).cloned() {
            if let Some(t) = self.by_name.get_mut(&prev) {
                t.members.remove(player);
            }
        }
        self.by_member.insert(player.to_string(), team.to_string());
        if let Some(t) = self.by_name.get_mut(team) {
            t.members.insert(player.to_string());
        }
    }

    /// `Scoreboard::removePlayerFromTeam(player, team)`.
    ///
    /// Vanilla **throws** `IllegalStateException` when the player is not on
    /// that team, which on the client would drop the connection. A server
    /// does not send it, so this is a documented divergence rather than an
    /// omission: we log and leave the state alone.
    fn remove_member(&mut self, player: &str, team: &str) {
        if self.by_member.get(player).map(String::as_str) != Some(team) {
            log::debug!("play: set_player_team LEAVE for {player} not on team {team}");
            return;
        }
        self.by_member.remove(player);
        if let Some(t) = self.by_name.get_mut(team) {
            t.members.remove(player);
        }
    }

    /// `Scoreboard::removePlayerTeam` — the team goes, and so does every
    /// membership pointing at it.
    fn remove_team(&mut self, name: &str) {
        if let Some(team) = self.by_name.remove(name) {
            for member in &team.members {
                self.by_member.remove(member);
            }
        }
    }

    /// `Scoreboard::getPlayersTeam` — the whole of what the tab list's third
    /// sort key reads.
    pub fn team_of_member(&self, member: &str) -> Option<&str> {
        self.by_member.get(member).map(String::as_str)
    }

    pub fn team(&self, name: &str) -> Option<&Team> {
        self.by_name.get(name)
    }

    pub fn team_names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

/// `Team.Visibility` (the wire enum) → `rewo_world::label::NameTagVisibility`
/// (the label predicate's) — M70.
///
/// Two enums rather than one because `rewo-world` must not depend on
/// `rewo-net`. Exhaustive on purpose: adding a variant to either side becomes
/// a compile error here rather than a silently wrong arm.
pub fn label_visibility(v: Visibility) -> rewo_world::label::NameTagVisibility {
    use rewo_world::label::NameTagVisibility as L;
    match v {
        Visibility::Always => L::Always,
        Visibility::Never => L::Never,
        Visibility::HideForOtherTeams => L::HideForOtherTeams,
        Visibility::HideForOwnTeam => L::HideForOwnTeam,
    }
}

/// `Entity.getTeam()` reduced to what the label predicate reads, for a
/// scoreboard name the caller has already resolved (M70).
///
/// Pure and standalone so a gate can drive the real mapping from a real
/// `Teams` without a live session.
///
/// A team whose parameters have somehow never arrived falls back to
/// `PlayerTeam`'s own **constructor** defaults — `ALWAYS` and
/// `seeFriendlyInvisibles = true`. That second one is not `false`, which is
/// the easy assumption: a bare `new PlayerTeam(...)` sees friendly invisibles
/// until a parameters packet says otherwise. In practice method 0 always
/// carries parameters, so this is a defensive arm rather than a live path.
pub fn label_team<'a>(
    teams: &'a Teams,
    member: &str,
) -> Option<rewo_world::label::TeamView<'a>> {
    let team = teams.team(teams.team_of_member(member)?)?;
    let (visibility, see_friendly_invisibles) = match &team.parameters {
        Some(p) => (
            label_visibility(p.name_tag_visibility),
            p.see_friendly_invisibles(),
        ),
        None => (rewo_world::label::NameTagVisibility::Always, true),
    };
    Some(rewo_world::label::TeamView {
        name: &team.name,
        visibility,
        see_friendly_invisibles,
    })
}

#[cfg(test)]
mod tests {
    //! Every body here is built by hand and run through the real
    //! `parse_set_player_team` / `Teams::apply`, so the walk and the state
    //! machine are the things under test rather than a local copy.

    use super::*;

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

    /// The `Parameters` composite, in wire order.
    fn params(out: &mut Vec<u8>, color: Option<i32>, options: u8) {
        nbt_string(out, "Display");
        nbt_string(out, "pre");
        nbt_string(out, "suf");
        varint(out, 0); // nameTagVisibility ALWAYS
        varint(out, 0); // collisionRule ALWAYS
        match color {
            Some(c) => {
                out.push(1);
                varint(out, c);
            }
            None => out.push(0),
        }
        out.push(options);
    }

    fn body(name: &str, method: i8, with_params: Option<(Option<i32>, u8)>, players: &[&str]) -> Vec<u8> {
        let mut b = Vec::new();
        utf(&mut b, name);
        b.push(method as u8);
        if let Some((color, options)) = with_params {
            params(&mut b, color, options);
        }
        if TeamMethod::from_byte(method).has_player_list() {
            varint(&mut b, players.len() as i32);
            for p in players {
                utf(&mut b, p);
            }
        }
        b
    }

    #[test]
    fn an_add_packet_carries_both_parameters_and_the_founding_roster() {
        let p = parse_set_player_team(&body("red", 0, Some((Some(12), 3)), &["alice", "bob"])).unwrap();
        assert_eq!(p.name, "red");
        assert_eq!(p.method, TeamMethod::Add);
        assert_eq!(p.players, ["alice", "bob"]);
        let params = p.parameters.expect("method 0 has parameters");
        assert_eq!(params.color, Some(12));
        assert!(params.allow_friendly_fire());
        assert!(params.see_friendly_invisibles());
        assert_eq!(params.display_name, Nbt::String("Display".into()));
    }

    #[test]
    fn reading_the_parameters_one_field_short_would_eat_the_roster() {
        // The sensitivity partner for the parameters walk. Feed the real
        // parser an ADD body, then feed the SAME body to a walk that forgets
        // the trailing options byte: it does not fail, it reports a
        // different, entirely plausible roster.
        let bytes = body("red", 0, Some((None, 0)), &["alice"]);
        assert_eq!(parse_set_player_team(&bytes).unwrap().players, ["alice"]);

        let mut r = PacketReader::new(&bytes);
        let _ = r.string(32767).unwrap();
        let _ = r.i8().unwrap();
        let _ = r.nbt().unwrap();
        let _ = r.nbt().unwrap();
        let _ = r.nbt().unwrap();
        let _ = r.varint().unwrap();
        let _ = r.varint().unwrap();
        let _ = r.bool().unwrap();
        // `options` deliberately not read.
        let count = r.count("team players", 1).unwrap();
        assert_ne!(count, 1, "a mis-sized parameters walk must be observable");
    }

    #[test]
    fn a_remove_packet_carries_neither_parameters_nor_a_roster() {
        let p = parse_set_player_team(&body("red", 1, None, &[])).unwrap();
        assert_eq!(p.method, TeamMethod::Remove);
        assert!(p.parameters.is_none());
        assert!(p.players.is_empty());
    }

    #[test]
    fn a_change_packet_carries_parameters_but_no_roster() {
        let p = parse_set_player_team(&body("red", 2, Some((Some(0), 0)), &[])).unwrap();
        assert_eq!(p.method, TeamMethod::Change);
        assert!(p.parameters.is_some());
        assert!(p.players.is_empty());
    }

    #[test]
    fn an_unknown_method_reads_a_three_field_packet_and_stays_inert() {
        // Vanilla widens a signed byte and compares against five literals, so
        // 7 is neither a team action nor a player action and the packet ends
        // after the method byte. Treating it as a decode error would drop a
        // connection a vanilla client survives.
        let mut b = Vec::new();
        utf(&mut b, "red");
        b.push(7);
        let p = parse_set_player_team(&b).unwrap();
        assert_eq!(p.method, TeamMethod::Unknown(7));
        assert!(p.parameters.is_none() && p.players.is_empty());

        let mut teams = Teams::new();
        assert!(!teams.apply(&p), "no team named red exists");
        assert!(teams.is_empty());
    }

    #[test]
    fn an_out_of_range_color_is_black_rather_than_an_error() {
        // `ByIdMap.continuous(..., ZERO)` — the id maps to values[0].
        let p = parse_set_player_team(&body("red", 0, Some((Some(99), 0)), &[])).unwrap();
        assert_eq!(p.parameters.unwrap().color, Some(0));
    }

    #[test]
    fn an_out_of_range_visibility_is_always_rather_than_an_error() {
        assert_eq!(Visibility::by_id(99), Visibility::Always);
        assert_eq!(Visibility::by_id(-1), Visibility::Always);
        assert_eq!(CollisionRule::by_id(99), CollisionRule::Always);
    }

    #[test]
    fn a_join_for_an_unknown_team_is_dropped_whole_including_its_roster() {
        // The early return. Applying the roster anyway would leave `alice`
        // pointing at a team that was never created.
        let mut teams = Teams::new();
        let join = parse_set_player_team(&body("red", 3, None, &["alice"])).unwrap();
        assert!(!teams.apply(&join));
        assert_eq!(teams.team_of_member("alice"), None);
    }

    #[test]
    fn a_player_added_to_a_second_team_leaves_the_first() {
        let mut teams = Teams::new();
        assert!(teams.apply(&parse_set_player_team(&body("red", 0, Some((None, 0)), &["alice"])).unwrap()));
        assert!(teams.apply(&parse_set_player_team(&body("blue", 0, Some((None, 0)), &[])).unwrap()));
        assert!(teams.apply(&parse_set_player_team(&body("blue", 3, None, &["alice"])).unwrap()));

        assert_eq!(teams.team_of_member("alice"), Some("blue"));
        assert!(
            !teams.team("red").unwrap().members.contains("alice"),
            "the old team's roster must lose her too, not just the index"
        );
    }

    #[test]
    fn removing_a_team_clears_its_members_memberships() {
        let mut teams = Teams::new();
        teams.apply(&parse_set_player_team(&body("red", 0, Some((None, 0)), &["alice", "bob"])).unwrap());
        teams.apply(&parse_set_player_team(&body("red", 1, None, &[])).unwrap());

        assert!(teams.team("red").is_none());
        assert_eq!(teams.team_of_member("alice"), None);
        assert_eq!(teams.team_of_member("bob"), None);
    }

    #[test]
    fn a_leave_removes_only_the_named_players() {
        let mut teams = Teams::new();
        teams.apply(&parse_set_player_team(&body("red", 0, Some((None, 0)), &["alice", "bob"])).unwrap());
        teams.apply(&parse_set_player_team(&body("red", 4, None, &["alice"])).unwrap());

        assert_eq!(teams.team_of_member("alice"), None);
        assert_eq!(teams.team_of_member("bob"), Some("red"));
    }

    #[test]
    fn a_leave_for_a_player_on_another_team_does_not_move_them() {
        // Vanilla throws here; we log. Either way the state must not change,
        // and in particular `bob` must not be evicted from `blue`.
        let mut teams = Teams::new();
        teams.apply(&parse_set_player_team(&body("red", 0, Some((None, 0)), &[])).unwrap());
        teams.apply(&parse_set_player_team(&body("blue", 0, Some((None, 0)), &["bob"])).unwrap());
        teams.apply(&parse_set_player_team(&body("red", 4, None, &["bob"])).unwrap());

        assert_eq!(teams.team_of_member("bob"), Some("blue"));
    }

    #[test]
    fn re_adding_an_existing_team_keeps_its_current_members() {
        // `addPlayerTeam` warns and reuses; it does not reset. A reset would
        // silently drop every member the server never re-sent.
        let mut teams = Teams::new();
        teams.apply(&parse_set_player_team(&body("red", 0, Some((None, 0)), &["alice"])).unwrap());
        teams.apply(&parse_set_player_team(&body("red", 0, Some((Some(4), 1)), &["bob"])).unwrap());

        let red = teams.team("red").unwrap();
        assert!(red.members.contains("alice") && red.members.contains("bob"));
        assert_eq!(red.parameters.as_ref().unwrap().color, Some(4));
    }

    #[test]
    fn a_change_updates_parameters_without_touching_the_roster() {
        let mut teams = Teams::new();
        teams.apply(&parse_set_player_team(&body("red", 0, Some((Some(1), 0)), &["alice"])).unwrap());
        teams.apply(&parse_set_player_team(&body("red", 2, Some((Some(9), 2)), &[])).unwrap());

        let red = teams.team("red").unwrap();
        assert_eq!(red.members.iter().collect::<Vec<_>>(), ["alice"]);
        assert_eq!(red.parameters.as_ref().unwrap().color, Some(9));
        assert!(red.parameters.as_ref().unwrap().see_friendly_invisibles());
    }

    #[test]
    fn a_truncated_body_is_an_error_rather_than_a_half_team() {
        // A method-0 header with no parameters at all.
        let mut b = Vec::new();
        utf(&mut b, "red");
        b.push(0);
        assert!(parse_set_player_team(&b).is_err());
    }
}
