use super::*;

impl PlaySession {
    /// `LocalPlayer.onUpdateAbilities()` — tell the server we changed `flying`.
    ///
    /// Sent only when the *client* made the change (a toggle, the spectator
    /// force-on, or the landing clause); a change that arrived in a
    /// `ClientboundPlayerAbilitiesPacket` is already the server's own view and
    /// echoing it back would be noise.
    pub(super) fn send_abilities(&mut self) -> Result<(), String> {
        let p =
            crate::abilities::serverbound(self.ids.sb_play_player_abilities, self.abilities.flying);
        self.send(p)
    }

    /// Decompiled `LocalPlayer.sendPosition` cadence + tick_end + input.
    pub(super) fn send_movement(&mut self, input: &TickInput) -> Result<(), String> {
        // player_input on change (Input.STREAM_CODEC flag order).
        let flags = (input.forward > 0.0) as u8
            | (((input.forward < 0.0) as u8) << 1)
            | (((input.strafe > 0.0) as u8) << 2)
            | (((input.strafe < 0.0) as u8) << 3)
            | ((input.jump as u8) << 4)
            | ((input.sneak as u8) << 5)
            | ((input.sprint as u8) << 6);
        if flags != self.last_input_flags {
            if let Some(id) = self.ids.sb_play_player_input {
                let mut p = PacketWriter::packet(id);
                p.u8(flags);
                self.send(p)?;
            }
            self.last_input_flags = flags;
        }

        let (px, py, pz) = (self.player.x, self.player.y, self.player.z);
        let (yaw, pitch) = (self.player.yaw, self.player.pitch);
        let dx = px - self.last_pos.0;
        let dy = py - self.last_pos.1;
        let dz = pz - self.last_pos.2;
        self.reminder += 1;
        let moved = dx * dx + dy * dy + dz * dz > 4.0e-8 || self.reminder >= 20;
        let rotated = yaw != self.last_rot.0 || pitch != self.last_rot.1;
        let move_flags =
            self.player.on_ground as u8 | ((self.player.horizontal_collision as u8) << 1);

        if moved && rotated {
            let mut p = PacketWriter::packet(self.ids.sb_play_move_pos_rot);
            p.f64(px).f64(py).f64(pz).f32(yaw).f32(pitch).u8(move_flags);
            self.send(p)?;
        } else if moved {
            let mut p = PacketWriter::packet(self.ids.sb_play_move_pos);
            p.f64(px).f64(py).f64(pz).u8(move_flags);
            self.send(p)?;
        } else if rotated {
            let mut p = PacketWriter::packet(self.ids.sb_play_move_rot);
            p.f32(yaw).f32(pitch).u8(move_flags);
            self.send(p)?;
        } else if self.last_on_ground != self.player.on_ground
            || self.last_horiz != self.player.horizontal_collision
        {
            let mut p = PacketWriter::packet(self.ids.sb_play_move_status);
            p.u8(move_flags);
            self.send(p)?;
        }
        if moved {
            self.last_pos = (px, py, pz);
            self.reminder = 0;
        }
        if rotated {
            self.last_rot = (yaw, pitch);
        }
        self.last_on_ground = self.player.on_ground;
        self.last_horiz = self.player.horizontal_collision;

        if let Some(id) = self.ids.sb_play_client_tick_end {
            self.send(PacketWriter::packet(id))?;
        }
        Ok(())
    }

    /// `LocalPlayer.sendRidingJump` (M169): `ServerboundPlayerCommandPacket(
    /// this, START_RIDING_JUMP, Mth.floor(getJumpRidingScale() * 100))`.
    pub(super) fn send_riding_jump(&mut self, data: i32) -> Result<(), String> {
        let (Some(id), Some(me)) = (self.ids.sb_play_player_command, self.player_id) else {
            return Ok(());
        };
        let mut p = PacketWriter::packet(id);
        p.raw(&crate::jump_riding::player_command_body(
            me,
            crate::jump_riding::START_RIDING_JUMP,
            data,
        ));
        self.send(p)?;
        self.riding_jumps_sent += 1;
        Ok(())
    }

    pub fn send_chat(&mut self, message: &str) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_chat else {
            return Err("chat packet unavailable".into());
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let millis = now.as_millis() as i64;
        // `sendChat`: the last-seen update is generated (and applied) for
        // every message, signed or not; a signature commits to the
        // seconds-precision timestamp, a random salt and the acknowledged
        // signatures, all of which go on the wire exactly as signed.
        let last_seen = self.last_seen.generate_and_apply_update();
        let (salt, signature) = match self.signer.as_mut() {
            Some(signer) => {
                let mut salt_bytes = [0u8; 8];
                rand::Rng::fill(&mut rand::thread_rng(), &mut salt_bytes);
                let salt = i64::from_be_bytes(salt_bytes);
                let sig = signer.sign(message, salt, now.as_secs() as i64, &last_seen.last_seen);
                (salt, Some(sig))
            }
            None => (0, None),
        };
        let mut p = PacketWriter::packet(id);
        p.string(message).i64(millis).i64(salt);
        match &signature {
            Some(sig) => {
                p.bool(true).raw(sig); // MessageSignature: fixed 256 bytes
            }
            None => {
                p.bool(false);
            }
        }
        last_seen.write(&mut p);
        self.send(p)
    }

    /// `sendChatAcknowledgement` — only when there is something to report.
    pub(super) fn send_chat_ack(&mut self) -> Result<(), String> {
        let offset = self.last_seen.get_and_clear_offset();
        if offset > 0 {
            if let Some(id) = self.ids.sb_play_chat_ack {
                let mut p = PacketWriter::packet(id);
                p.varint(offset);
                self.send(p)?;
            }
        }
        Ok(())
    }

    /// `ServerboundPlaceRecipePacket` (M98) — click a recipe in the book.
    ///
    /// `use_max_items` is shift-held. The container is the SHOWN menu's, on
    /// M89's rule: a book click belongs to whatever screen is up.
    /// `AbstractSignEditScreen.removed()` (M174) — the editor's one commit.
    /// Sent UNCONDITIONALLY on every exit (Done, Esc, the validity tick):
    /// there is no dirty check and no cancel path in vanilla.
    pub fn send_sign_update(
        &mut self,
        pos: (i32, i32, i32),
        is_front_text: bool,
        lines: &[String; 4],
    ) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_sign_update else {
            return Err("sign_update unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        p.buf
            .extend_from_slice(&crate::sign_update_body(pos, is_front_text, lines));
        self.send(p)
    }

    /// `StatsScreen.init()`'s last line —
    /// `send(new ServerboundClientCommandPacket(REQUEST_STATS))` (M84).
    ///
    /// The screen asks; the server answers with `award_stats`. **Vanilla sends
    /// this from `init()`, so it is re-sent on every window resize**, because
    /// `init()` is what `repositionElements` runs. Rewo sends it only when the
    /// screen opens, which is a deliberate deviation: a resize costs a round
    /// trip in vanilla and buys nothing the client does not already hold.
    pub fn request_stats(&mut self) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_client_command else {
            return Err("client_command unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        p.buf.extend_from_slice(&crate::client_command_body(
            crate::ClientCommand::RequestStats,
        ));
        self.send(p)
    }

    /// `ServerboundSeenAdvancementsPacket.openedTab` (M178) — the screen's
    /// open/init path sends it for the tab it shows.
    pub fn send_seen_advancements_opened_tab(&mut self, tab: &str) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_seen_advancements else {
            return Err("seen_advancements unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        // Action enum ordinal 0 = OPENED_TAB; then the identifier. CLOSED_SCREEN
        // writes the ordinal alone (`write`'s guard skips the tab).
        p.varint(0);
        p.string(tab);
        self.send(p)
    }

    /// `ServerboundSeenAdvancementsPacket.closedScreen` — `removed()` sends it
    /// unconditionally, no dirty check and no cancel path.
    pub fn send_seen_advancements_closed_screen(&mut self) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_seen_advancements else {
            return Err("seen_advancements unavailable".into());
        };
        let mut p = PacketWriter::packet(id);
        p.varint(1); // CLOSED_SCREEN
        self.send(p)
    }

    /// Start digging (creative servers break the block on START).
    /// `ServerboundCommandSuggestionPacket` — ask what completes `command`.
    ///
    /// The id and the pending slot are the provider's; see
    /// [`crate::suggestion_wire`] for why there is only one outstanding
    /// request and what happens to a reply that misses it.
    pub fn request_command_suggestions(&mut self, command: &str) -> Result<(), String> {
        let Some(id) = self.ids.sb_play_command_suggestion else {
            return Err("command_suggestion unavailable".into());
        };
        let (_req, body) = self.suggestions.begin_request(command);
        let mut p = PacketWriter::packet(id);
        p.raw(&body);
        self.send(p)
    }

    /// Run a server command (unsigned `chat_command`, the string without the
    /// leading `/`). Used for verification (`/summon …`) when the account is
    /// op; a normal client mostly sends these too.
    /// `ClientPacketListener.sendCommand`. `command` has no leading slash.
    ///
    /// A command whose parse contains `minecraft:message` arguments goes out as
    /// `chat_command_signed` with one signature per argument (none without a
    /// chat session, as vanilla); everything else as plain `chat_command`.
    pub fn send_command(&mut self, command: &str) -> Result<(), String> {
        let units: Vec<u16> = command.encode_utf16().collect();
        let ctx = crate::dispatcher::CommandCtx { names: &[], blocks: None, items: None };
        let parsed = crate::dispatcher::parse(&self.commands, &units, 0, ctx);
        let signable = crate::signed_command::signable_arguments(&self.commands, &parsed, &units);
        if signable.is_empty() {
            let Some(id) = self.ids.sb_play_chat_command else {
                return Err("chat_command unavailable".into());
            };
            let mut p = PacketWriter::packet(id);
            p.string(command);
            return self.send(p);
        }
        let Some(id) = self.ids.sb_play_chat_command_signed else {
            return Err("chat_command_signed unavailable".into());
        };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let salt: i64 = rand::Rng::gen(&mut rand::thread_rng());
        let last_seen = self.last_seen.generate_and_apply_update();
        let signatures: Vec<(String, Vec<u8>)> = match self.signer.as_mut() {
            Some(signer) => signable
                .into_iter()
                .map(|(name, value)| {
                    let sig = signer.sign(&value, salt, now.as_secs() as i64, &last_seen.last_seen);
                    (name, sig)
                })
                .collect(),
            None => Vec::new(),
        };
        let mut p = PacketWriter::packet(id);
        crate::signed_command::write_signed_command(
            &mut p,
            command,
            now.as_millis() as i64,
            salt,
            &signatures,
            &last_seen,
        );
        self.send(p)
    }
}
