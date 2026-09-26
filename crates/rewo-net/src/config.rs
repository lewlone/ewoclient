//! The configuration state, shared by the initial login-time configuration
//! (`Connection::run_configuration`) and a mid-session re-entry after play's
//! `start_configuration` (`PlaySession`, what a proxy server switch does).
//!
//! One handler, [`handle_config_packet`], answers every configuration packet;
//! the two callers differ only in how they send and where the synced data
//! lands when `finish_configuration` arrives. Vanilla builds a fresh
//! `ClientConfigurationPacketListenerImpl` (and a fresh `RegistryDataCollector`)
//! for each entry, so a re-entry starts from an empty [`ConfigData`] too; only
//! the common-listener state ([`crate::session::SessionState`]) carries across.

use rewo_data::GameData;
use rewo_proto::nbt::Nbt;
use rewo_proto::reader::PacketReader;
use rewo_proto::writer::PacketWriter;
use rewo_world::dimension::DimensionTypeDef;

use crate::ids::Ids;
use crate::{config_tasks, dimension_parse};

/// Raw `minecraft:mob_effect` registry ids of the four effects the local
/// player's movement physics reads (`MoveAttributes`'s effect fields). Same
/// capture as the M13 lightmap's night-vision / darkness ids: from the datagen
/// report, overridden if a server syncs the registry. `None` = that effect can
/// never match here.
#[derive(Clone, Copy, Debug, Default)]
pub struct MovementEffectIds {
    pub jump_boost: Option<i32>,
    pub slow_falling: Option<i32>,
    pub dolphins_grace: Option<i32>,
    pub levitation: Option<i32>,
}

/// What one configuration pass synced — the registries and tags a play
/// session is built from.
pub struct ConfigData {
    /// The `minecraft:dimension_type` registry in raw wire order — index *is*
    /// the holder registry id.
    pub dim_types: Vec<DimensionTypeDef>,
    /// Registry id of the `minecraft:overworld` world clock; `None` on a server
    /// that syncs no clocks.
    pub overworld_clock_id: Option<i32>,
    /// The whole `minecraft:world_clock` registry in raw wire order.
    pub world_clock_ids: Vec<String>,
    /// `mob_effect` ids for the M13 lightmap. A built-in registry, so these come
    /// from the datagen report (M92c); a server that does sync the registry
    /// overrides them.
    pub night_vision_id: Option<i32>,
    pub darkness_id: Option<i32>,
    pub swing_effect_ids: crate::SwingEffectIds,
    /// The four movement effects (jump boost, slow falling, dolphin's grace,
    /// levitation) the local player's physics reads.
    pub movement_effect_ids: MovementEffectIds,
    /// `minecraft:worldgen/biome` registry in raw wire order (M14).
    pub biome_defs: Vec<rewo_world::biome::BiomeDef>,
    pub enchantments: Vec<crate::enchantment_parse::EnchantmentDef>,
    pub chat_types: Vec<crate::chat_type_parse::ChatTypeDef>,
    pub trim_materials: Vec<crate::trim_parse::TrimMaterialDef>,
    pub trim_patterns: Vec<crate::trim_parse::TrimPatternDef>,
    pub cat_variants: Vec<crate::variant_parse::MobVariantDef>,
    pub wolf_variants: Vec<crate::variant_parse::MobVariantDef>,
    pub frog_variants: Vec<crate::variant_parse::MobVariantDef>,
    /// The server's datapack tags (M69).
    pub tags: crate::tags::TagOverrides,
}

impl ConfigData {
    pub fn new(data: &GameData) -> Self {
        Self {
            dim_types: Vec::new(),
            overworld_clock_id: None,
            world_clock_ids: Vec::new(),
            night_vision_id: data.mob_effects.id_of("minecraft:night_vision"),
            darkness_id: data.mob_effects.id_of("minecraft:darkness"),
            swing_effect_ids: crate::SwingEffectIds {
                haste: data.mob_effects.id_of("minecraft:haste"),
                conduit_power: data.mob_effects.id_of("minecraft:conduit_power"),
                mining_fatigue: data.mob_effects.id_of("minecraft:mining_fatigue"),
            },
            movement_effect_ids: MovementEffectIds {
                jump_boost: data.mob_effects.id_of("minecraft:jump_boost"),
                slow_falling: data.mob_effects.id_of("minecraft:slow_falling"),
                dolphins_grace: data.mob_effects.id_of("minecraft:dolphins_grace"),
                levitation: data.mob_effects.id_of("minecraft:levitation"),
            },
            biome_defs: Vec::new(),
            enchantments: Vec::new(),
            chat_types: Vec::new(),
            trim_materials: Vec::new(),
            trim_patterns: Vec::new(),
            cat_variants: Vec::new(),
            wolf_variants: Vec::new(),
            frog_variants: Vec::new(),
            tags: crate::tags::TagOverrides::default(),
        }
    }

    /// A fresh collector for a re-entry, keeping the report-derived effect ids
    /// (a built-in registry the server does not resend).
    pub fn fresh_with_effect_ids(
        night_vision_id: Option<i32>,
        darkness_id: Option<i32>,
        swing_effect_ids: crate::SwingEffectIds,
        movement_effect_ids: MovementEffectIds,
    ) -> Self {
        Self {
            dim_types: Vec::new(),
            overworld_clock_id: None,
            world_clock_ids: Vec::new(),
            night_vision_id,
            darkness_id,
            swing_effect_ids,
            movement_effect_ids,
            biome_defs: Vec::new(),
            enchantments: Vec::new(),
            chat_types: Vec::new(),
            trim_materials: Vec::new(),
            trim_patterns: Vec::new(),
            cat_variants: Vec::new(),
            wolf_variants: Vec::new(),
            frog_variants: Vec::new(),
            tags: crate::tags::TagOverrides::default(),
        }
    }

    /// The biome registry the play session builds its biome context from.
    pub fn biome_registry(&self) -> Option<rewo_world::biome::BiomeRegistry> {
        if self.biome_defs.is_empty() {
            None
        } else {
            Some(rewo_world::biome::BiomeRegistry::new(self.biome_defs.clone()))
        }
    }

    /// Decode one Configuration `registry_data` packet body.
    ///
    /// The `minecraft:dimension_type` registry is the one that can fail the
    /// connection: it is the only registry here whose entries the client
    /// *must* understand exactly (a wrong vertical shape mis-decodes every
    /// chunk, a wrong `has_skylight` invents light), so `dimension_parse`
    /// returns a `Result` and it propagates. The remaining registries are
    /// id-capture only and stay tolerant.
    pub fn parse_registry_data(&mut self, body: &[u8]) -> Result<(), String> {
        let mut r = PacketReader::new(body);
        let registry = match r.identifier() {
            Ok(v) => v,
            Err(e) => {
                log::warn!("net: registry_data: bad registry id: {e}");
                return Ok(());
            }
        };
        let count = match r.count("registry entries", 1) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("net: registry_data {registry}: bad count: {e}");
                return Ok(());
            }
        };
        if registry == crate::enchantment_parse::ENCHANTMENT_REGISTRY {
            // Datapack-driven, so both the contents and the id order are the
            // server's — nothing here may be assumed from bootstrap order.
            self.enchantments = crate::enchantment_parse::parse_enchantment_registry(&mut r, count);
            log::info!("net: {} enchantment(s) synced", self.enchantments.len());
            return Ok(());
        }
        // M127: the chat-type registry, datapack-driven for the same reason —
        // the index is the id `ChatType.Bound`'s `holder` VarInt names.
        if registry == crate::chat_type_parse::CHAT_TYPE_REGISTRY {
            self.chat_types = crate::chat_type_parse::parse_chat_type_registry(&mut r, count);
            log::info!("net: {} chat type(s) synced", self.chat_types.len());
            return Ok(());
        }
        // M48: the two trim registries, datapack-driven for the same reason.
        if registry == crate::trim_parse::TRIM_MATERIAL_REGISTRY {
            self.trim_materials = crate::trim_parse::parse_trim_material_registry(&mut r, count);
            log::info!("net: {} trim material(s) synced", self.trim_materials.len());
            return Ok(());
        }
        if registry == crate::trim_parse::TRIM_PATTERN_REGISTRY {
            self.trim_patterns = crate::trim_parse::parse_trim_pattern_registry(&mut r, count);
            log::info!("net: {} trim pattern(s) synced", self.trim_patterns.len());
            return Ok(());
        }
        // M64: the three mob-variant registries, datapack-driven for the
        // same reason — the index is the raw holder id the metadata carries.
        if registry == crate::variant_parse::CAT_VARIANT_REGISTRY {
            self.cat_variants = crate::variant_parse::parse_single_asset_registry(&mut r, count);
            log::info!("net: {} cat variant(s) synced", self.cat_variants.len());
            return Ok(());
        }
        if registry == crate::variant_parse::WOLF_VARIANT_REGISTRY {
            self.wolf_variants = crate::variant_parse::parse_wolf_variant_registry(&mut r, count);
            log::info!("net: {} wolf variant(s) synced", self.wolf_variants.len());
            return Ok(());
        }
        if registry == crate::variant_parse::FROG_VARIANT_REGISTRY {
            self.frog_variants = crate::variant_parse::parse_single_asset_registry(&mut r, count);
            log::info!("net: {} frog variant(s) synced", self.frog_variants.len());
            return Ok(());
        }
        if registry == dimension_parse::DIMENSION_TYPE_REGISTRY {
            self.dim_types = dimension_parse::parse_dimension_registry(&mut r, count)?;
            log::info!("net: {} dimension type(s) synced", self.dim_types.len());
            return Ok(());
        }
        // The day/night timeline runs on the `minecraft:overworld` world
        // clock, and `set_time` keys its clock map by raw registry id. The id
        // is capture-able here rather than assumed from bootstrap order.
        let is_clock = registry == "minecraft:world_clock";
        if is_clock {
            self.world_clock_ids.clear();
        }
        // The M13 camera lightmap keys night-vision / darkness off their raw
        // `mob_effect` registry ids, captured here rather than assumed from
        // bootstrap order (exactly like the world clock above).
        let is_mob_effect = registry == "minecraft:mob_effect";
        // M14: the biome registry, in raw wire order, drives per-biome tint.
        let is_biome = registry == "minecraft:worldgen/biome";
        if is_biome {
            self.biome_defs.clear();
        }
        for idx in 0..count {
            let entry_name = match r.identifier() {
                Ok(v) => v,
                Err(e) => {
                    log::warn!("net: registry_data {registry}: entry {idx}: {e}");
                    return Ok(());
                }
            };
            if is_clock {
                if entry_name == "minecraft:overworld" {
                    self.overworld_clock_id = Some(idx as i32);
                }
                // Pushed in iteration order, so the position is the id. Never
                // sorted, and never derived from bootstrap order — M64's
                // alphabetisation trap.
                self.world_clock_ids.push(entry_name.clone());
            }
            if is_mob_effect {
                match entry_name.as_str() {
                    "minecraft:night_vision" => self.night_vision_id = Some(idx as i32),
                    "minecraft:darkness" => self.darkness_id = Some(idx as i32),
                    // M19: `getCurrentSwingDuration`'s dig-speed / fatigue terms.
                    "minecraft:haste" => self.swing_effect_ids.haste = Some(idx as i32),
                    "minecraft:conduit_power" => {
                        self.swing_effect_ids.conduit_power = Some(idx as i32)
                    }
                    "minecraft:mining_fatigue" => {
                        self.swing_effect_ids.mining_fatigue = Some(idx as i32)
                    }
                    // The local player's movement physics.
                    "minecraft:jump_boost" => {
                        self.movement_effect_ids.jump_boost = Some(idx as i32)
                    }
                    "minecraft:slow_falling" => {
                        self.movement_effect_ids.slow_falling = Some(idx as i32)
                    }
                    "minecraft:dolphins_grace" => {
                        self.movement_effect_ids.dolphins_grace = Some(idx as i32)
                    }
                    "minecraft:levitation" => {
                        self.movement_effect_ids.levitation = Some(idx as i32)
                    }
                    _ => {}
                }
            }
            let has_nbt = r.bool().unwrap_or(false);
            if !has_nbt {
                if is_biome {
                    // A biome with no NBT is degenerate; keep raw order intact
                    // with a neutral default so indices still line up.
                    self.biome_defs
                        .push(crate::biome_parse::parse_biome(&entry_name, &Nbt::End));
                }
                continue;
            }
            let nbt = match r.nbt() {
                Ok(v) => v,
                Err(e) => {
                    log::warn!("net: registry_data {registry}: {entry_name}: {e}");
                    return Ok(());
                }
            };
            if is_biome {
                self.biome_defs
                    .push(crate::biome_parse::parse_biome(&entry_name, &nbt));
            }
        }
        if is_biome {
            log::info!("net: {} biome(s) synced", self.biome_defs.len());
        }
        Ok(())
    }
}

/// What a configuration packet asks of its caller.
#[derive(Debug, PartialEq, Eq)]
pub enum ConfigStep {
    /// Handled; stay in configuration.
    Continue,
    /// `finish_configuration` arrived and was acknowledged: switch to play.
    Finished,
    /// `disconnect` arrived; the caller decodes the reason from the body.
    Disconnect,
}

/// Everything [`handle_config_packet`] reads and writes besides the packet.
pub struct ConfigCtx<'a> {
    pub ids: &'a Ids,
    pub cfg: &'a mut ConfigData,
    pub session: &'a mut crate::session::SessionState,
    pub tasks: &'a mut config_tasks::ConfigTaskLog,
    pub keepalives: &'a mut u64,
}

/// Answer one clientbound configuration packet. `send` writes one packet on
/// the connection. Unknown or inert packets return [`ConfigStep::Continue`].
pub fn handle_config_packet(
    ctx: ConfigCtx<'_>,
    id: i32,
    body: &[u8],
    send: &mut dyn FnMut(PacketWriter) -> Result<(), String>,
) -> Result<ConfigStep, String> {
    let ids = ctx.ids;
    let de = |what: &str, e: rewo_proto::ProtoError| format!("config {what}: {e}");
    if id == ids.cb_config_keep_alive {
        // A keep-alive that cannot be answered is a timeout in 15 s anyway;
        // failing now at least names the cause.
        let ka = PacketReader::new(body).i64().map_err(|e| de("keep_alive", e))?;
        let mut resp = PacketWriter::packet(ids.sb_config_keep_alive);
        resp.i64(ka);
        send(resp)?;
        *ctx.keepalives += 1;
    } else if id == ids.cb_config_ping {
        let ping = PacketReader::new(body).i32().map_err(|e| de("ping", e))?;
        let mut resp = PacketWriter::packet(ids.sb_config_pong);
        resp.i32(ping);
        send(resp)?;
    } else if id == ids.cb_config_select_known_packs {
        // Reply with an empty list = "I have none cached, send me
        // everything" (the full RegistryData follows).
        let mut resp = PacketWriter::packet(ids.sb_config_select_known_packs);
        resp.varint(0);
        send(resp)?;
    } else if id == ids.cb_config_registry_data {
        ctx.cfg.parse_registry_data(body)?;
    } else if id == ids.cb_config_update_tags {
        // M69 — the server's datapack tags. This is where a vanilla server
        // sends them on a normal join; the play copy (`route_tags`) is the
        // datapack-reload case. Both reach the same walk.
        if !crate::apply_update_tags(body, &mut ctx.cfg.tags) {
            log::warn!("net: config update_tags decode failed");
        }
    } else if id == ids.cb_config_code_of_conduct {
        // M166 — the FIRST of the two blocking tasks (`addOptionalTasks`
        // appends it ahead of the resource pack). Until this reply exists the
        // server's task queue never advances and `finish_configuration` never
        // arrives.
        match config_tasks::read_code_of_conduct(body) {
            Ok(text) => {
                log::info!(
                    "net: accepting the server's code of conduct ({} chars)",
                    text.chars().count()
                );
                ctx.tasks.codes_of_conduct.push(text);
            }
            Err(err) => {
                // Answer anyway: the reply carries none of the body, so a
                // failed decode costs a log line, whereas going silent costs
                // the whole connection.
                log::warn!("net: code_of_conduct decode: {err} — accepting regardless");
                ctx.tasks.codes_of_conduct.push(String::new());
            }
        }
        send(config_tasks::write_code_of_conduct_accept(
            ids.sb_config_accept_code_of_conduct,
        ))?;
    } else if id == ids.cb_config_resource_pack_push {
        // M166 — the second blocking task. See `config_tasks` for why the
        // reply is FAILED_DOWNLOAD and not DECLINED.
        let (pack, action) = config_tasks::answer_pack_push(body, ctx.tasks);
        send(config_tasks::write_pack_reply(ids.sb_config_resource_pack, pack, action))?;
    } else if id == ids.cb_config_finish {
        send(PacketWriter::packet(ids.sb_config_finish))?;
        return Ok(ConfigStep::Finished);
    } else if Some(id) == ids.cb_config_cookie_request {
        // `handleRequestCookie` answers with whatever the jar holds.
        let key = match PacketReader::new(body).identifier() {
            Ok(k) => k,
            Err(e) => return Err(de("cookie_request", e)),
        };
        let payload = ctx.session.cookie(&key).map(<[u8]>::to_vec);
        send(crate::session::write_cookie_response(
            ids.sb_config_cookie_response,
            &key,
            payload.as_deref(),
        ))?;
    } else if id == ids.cb_config_custom_payload {
        // M78 — the vanilla server sends `minecraft:brand` from its
        // configuration listener's opening burst and never sends another.
        crate::session::apply(crate::session::SessionPacket::CustomPayload, body, ctx.session);
    } else if id == ids.cb_config_store_cookie {
        crate::session::apply(crate::session::SessionPacket::StoreCookie, body, ctx.session);
    } else if id == ids.cb_config_server_links {
        // M85 — `serverLinks` is a field of the common listener, so it crosses
        // into play with the brand and the cookie jar.
        crate::session::apply(crate::session::SessionPacket::ServerLinks, body, ctx.session);
    } else if id == ids.cb_config_disconnect {
        return Ok(ConfigStep::Disconnect);
    }
    // What is left is inert here: enabled_features, reset_chat, transfer,
    // custom_report_details, the dialog pair, and resource_pack_pop
    // (deliberately unresolved — see `config_tasks`). None of them blocks the
    // server's task queue.
    Ok(ConfigStep::Continue)
}
