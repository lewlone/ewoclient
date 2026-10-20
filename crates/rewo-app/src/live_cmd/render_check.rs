use super::*;

/// `ChatFormatting.GRAY`'s 0xAAAAAA, in the LINEAR space the text pass wants.
///
/// Spelled out rather than taken from `parse_color` + `srgb_to_linear`,
/// because a witness that computes its expectation from the functions under
/// test is self-calibrating (§0.0 gotcha 0a) — it would pass with a wrong
/// palette entry and with a wrong transfer function alike.
///
/// **The first version of r41 compared against the sRGB value and read 0 with
/// the feature working**, which is this codebase's own open item (the title
/// path and the death screen hand the text pass `/255` bytes where it wants
/// linear) reproduced inside a new witness. The chat path converts correctly;
/// the witness did not.
///
/// **The second version read 0 too, and for a different reason worth keeping.**
/// It compared bits. The renderer evaluates the transfer function in `f32` at
/// every step and reaches `0.40197787`; the same formula in `f64`, narrowed
/// once at the end, reaches `0.40197778`. Those are different `f32`s. A
/// bit-exact comparison across two evaluation orders of one formula is a
/// **wrong** assertion rather than a strict one — the same shape as M12's
/// finding that JOML's `Math.fma` is non-fused by default and so `lengthSquared`
/// must be read right-associatively. The tolerance below is ~40x tighter than
/// one 8-bit colour step (1/255) and ~1000x looser than the gap above, so it
/// cannot admit a wrong palette entry or a missing conversion.
pub(super) const GRAY_LINEAR: f32 = 0.401_977_78;

/// Well under one 8-bit colour step, well over the f32/f64 evaluation gap.
pub(super) const COLOR_EPS: f32 = 1e-4;
/// r48 — the entity ids `--render-check`'s crowd injection creates.
///
/// Two zombies and a villager, spawned through the production `add_entity`
/// route. They are named here rather than written at the injection site
/// because **the census joins on them**: the two crowd counts are taken over
/// exactly these three draws and nothing else in the frame, so that a fixture
/// elsewhere in the same run which happens to spawn a zombie cannot satisfy
/// r48's threshold on this check's behalf. Far above any id a short
/// render-check session's server will hand out, and far from any real profile.
pub(super) const CROWD_IDS: [i32; 3] = [0x5A00, 0x5A01, 0x5A02];

/// r69 (M192) — the map id `--render-check` injects into the off-hand, with
/// its data and one decoration. Far above any id a short session's server
/// hands out, so the witness is about this injection only.
pub(super) const HELD_MAP_WITNESS_ID: i32 = 0x7E57;

/// r70 (M194) — the item frame `--render-check` injects (a sword in it).
pub(super) const FRAME_WITNESS_EID: i32 = 0x00F7_0000;

/// r72 (M185) — the `(horse, happy ghast)` pair `--render-check` injects, the
/// horse leashed to the ghast.
pub(super) const QUAD_LEASH_EIDS: (i32, i32) = (0x00C0_0010, 0x00C0_0011);

/// M128 — the text of the one clickable message `--render-check` injects.
///
/// Deliberately nothing a server would say, so the drawn-line scan that finds
/// it cannot match a real message.
pub(super) const CLICK_WITNESS_TEXT: &str = "rewo click witness";

/// The command that message's `click_event` runs, **without** its leading
/// slash — `Commands.trimOptionalPrefix` strips one, and the witness asserts
/// the trimmed form, so a dispatcher that forgot to trim fails here.
pub(super) const CLICK_WITNESS_COMMAND: &str = "rewoclickwitness";

/// The `system_chat` body carrying that message, as network NBT.
///
/// Hand-assembled because `rewo_proto::nbt` reads and does not write, and
/// because a *compound* is required: a `click_event` has nowhere to live on a
/// bare string component, which is what every other injected witness uses.
///
/// ```text
/// TAG_Compound
///   TAG_String "text"        = "rewo click witness"
///   TAG_Compound "click_event"
///     TAG_String "action"    = "run_command"
///     TAG_String "command"   = "/rewoclickwitness"
///   TAG_End
/// TAG_End
/// ```
///
/// then the `overlay` boolean the packet's second field is.
pub(super) fn click_witness_body() -> Vec<u8> {
    fn string(out: &mut Vec<u8>, name: &str, value: &str) {
        out.push(8); // TAG_String
        out.extend_from_slice(&(name.len() as u16).to_be_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(value.len() as u16).to_be_bytes());
        out.extend_from_slice(value.as_bytes());
    }
    let mut body: Vec<u8> = vec![10]; // TAG_Compound, unnamed at the root
    string(&mut body, "text", CLICK_WITNESS_TEXT);
    body.push(10);
    body.extend_from_slice(&(11u16).to_be_bytes());
    body.extend_from_slice(b"click_event");
    string(&mut body, "action", "run_command");
    // With the slash, so the trim is exercised rather than assumed.
    string(&mut body, "command", &format!("/{CLICK_WITNESS_COMMAND}"));
    body.push(0); // end of click_event
    body.push(0); // end of root
    body.push(0); // overlay = false
    body
}

/// M166 — the UUID r55's PLAY-state `resource_pack_push` injection carries.
///
/// Deliberately not the one `render_check.py` stages for the configuration
/// task: r55 asserts the two replies in order, so a client that answered only
/// the configuration push, or answered the same id twice, fails on the id
/// rather than on a count.
pub(super) const PLAY_PACK_ID: u128 = 0x00c0_ffee_0000_4000_8000_0000_0bad_f00d;

/// The entity id r48's injected zombie is given.
///
/// Far outside anything a server assigns in a short session, so it cannot
/// collide with a real entity and make the witness read someone else's name.
pub(super) const NAMETAG_WITNESS_ID: i32 = 0x0052_5701;

/// Where r49's injected sign is placed, relative to the player's block.
///
/// Two east and one up, so it sits in air rather than replacing whatever the
/// player is standing on — a sign whose block is overwritten by the server's
/// next chunk update would make the witness flap.
pub(super) const SIGN_WITNESS_OFFSET: (i32, i32, i32) = (2, 1, 0);

/// The translation key r49's sign carries.
pub(super) const SIGN_WITNESS_KEY: &str = "block.minecraft.dirt";

/// What `en_us.json` says that key is.
///
/// A literal rather than a `baked.lang` lookup ON PURPOSE: a witness that asks
/// the table under test what to expect grades everything except the table
/// (§0.0 gotcha 0a). This is the jar's own answer, read once and written down.
pub(super) const SIGN_WITNESS_RESOLVED: &str = "Dirt";

/// M247/M248 — the block entities `render_check.py` stages (the chest by the
/// anvil, the bell beyond it, the book on M184's lectern),
/// which r109 requires to draw every quad of their models.
pub(super) const RC_BLOCK_ENTITY_MODELS: [&str; 3] =
    ["rewo:be/chest", "rewo:be/bell", "rewo:be/lectern_book"];

/// What `live --render-check` observed (M86).
///
/// Every field is a **count of frames on which something happened**, never a
/// snapshot, because the failure this gate exists for is "the branch never ran
/// at all" and a snapshot taken on the wrong frame cannot tell that from "it
/// ran and had nothing to do".
#[derive(Default)]
pub(super) struct RenderCheck {
    pub(super) frames: u64,
    /// M168 — the most ARMOUR icons the survival layout emitted in any
    /// frame, and how many of them were FULL. `render_check.py` stages an
    /// iron chestplate (`item replace entity @s armor.chest`), so the
    /// server has to mark the player's own ARMOR attribute dirty and send
    /// it to the player itself — `sendToTrackingPlayersAndSelf`, the path
    /// the initial pairing never takes — for this to move off zero. A
    /// count AND a value: armour draws ten icons whenever it draws any,
    /// so the count alone is "some attribute arrived", and the three
    /// full ones are `6 / 2` of an iron chestplate specifically.
    pub(super) armor_icons_max: usize,
    pub(super) armor_full_max: usize,
    /// M168 — the most EFFECT icons emitted in any frame, and whether one
    /// of them sat on the beneficial row at the first slot. The staged
    /// `effect give @s minecraft:speed infinite 0` — WITHOUT the
    /// `hideParticles` argument: `/effect give` builds its instance with the
    /// five-argument constructor, whose `visible` doubles as `showIcon`
    /// (`MobEffectInstance.java:61`), so hiding the particles hides the
    /// icon the layout gates on. The first staging did, and measured zero.
    pub(super) effect_icons_max: usize,
    pub(super) effect_first_beneficial: bool,
    /// M169 — the jump bar's whole chain, live: the most JumpBar blits in
    /// any frame (2 = background + progress while charging), the highest
    /// `getJumpRidingScale()` seen, and the `START_RIDING_JUMP`s sent.
    /// `render_check.py` summons a tamed, saddled horse, mounts the bot on
    /// it, and the run holds Space over 0.32..0.46 of its length (clear of the
    /// 0.5 inventory), which charges the meter to 1.0 and releases it.
    pub(super) jump_bar_blits_max: usize,
    pub(super) leash_verts_max: usize,
    pub(super) book_frames: u64,
    pub(super) book_pages_seen: usize,
    pub(super) options_slider_sprites_max: usize,
    pub(super) jump_scale_max: f32,
    pub(super) riding_jumps_sent: u64,
    /// `self.baked.is_some()` observed at the top of a frame. The witness whose
    /// absence let the bug live from M3 to M86.
    pub(super) baked_frames: u64,
    /// Frames on which the rain-fog band came from the bake rather than from
    /// the `None` sentinel `[1e9, 1e9 + 1]`. A *value* witness, not a call
    /// count: the sentinel is what the dead branch produced, so a finite band
    /// cannot be faked by merely entering the arm.
    pub(super) fog_band_frames: u64,
    pub(super) gui_item_frames: u64,
    pub(super) hand_frames: u64,
    pub(super) weather_frames: u64,
    /// Frames on which the inventory screen was drawn. The gate opens it
    /// halfway through, so this is roughly half the run — it exists to prove
    /// the screen path (and with it `VelvetTextPass::sync_atlas`) was reached
    /// at all, not to pin a count.
    pub(super) screen_frames: u64,
    /// Frames on which a CONTAINER's screen was drawn — a menu other than the
    /// player's own (M88).
    ///
    /// M87 shipped the container render and could not prove the windowed
    /// client reached it: `--render-check` opens the *inventory*, which is
    /// `menu_layout::PLAYER` and takes the pass's own `inventory.png` rect, so
    /// every container-specific path — the panel blits, the container-sized
    /// origin, the layout's own slot rects — stayed unexercised here. That is
    /// the exact shape of the blind spot M86 was: a path nothing drives.
    pub(super) container_frames: u64,
    /// M94 — the most quads the recipe book drew in any frame **with no
    /// which-of-these overlay open**. Split from the field below by M104 so
    /// the two are comparable; r23's threshold is a floor, so its meaning is
    /// unchanged.
    pub(super) book_quads_max: usize,
    /// M187 — frames on which the crafting table's screen carried the
    /// recipe-book button at vanilla's position, read back out of the
    /// renderer.
    pub(super) book_button_frames: usize,
    /// M192 — frames on which the first-person hand drew the injected
    /// off-hand map WITH its data and its one decoration (r69).
    pub(super) held_map_frames: usize,
    /// M194 — where the injected item frame stands, once injected (r70).
    pub(super) item_frame_pos: Option<[f32; 3]>,
    /// M194 — frames on which the collector built the injected frame's model
    /// and its sword at that position, and no body was drawn for it (r70).
    pub(super) item_frame_frames: usize,
    /// M238 — r105: frames whose draw list carried both staged armour stands
    /// as their model with their flags.
    pub(super) armor_stand_frames: usize,
    /// M240 — r107: frames drawing r94's snowball and its bobber as
    /// camera-facing items.
    pub(super) thrown_frames: (usize, usize),
    /// M241 — r108: frames that drew a fishing line, and the farthest its
    /// hand end sat from the eye.
    pub(super) fishing_line_frames: usize,
    pub(super) fishing_line_hand: f32,
    /// M247 — r109: frames on which every staged block entity
    /// ([`RC_BLOCK_ENTITY_MODELS`]) emitted every quad of its baked model,
    /// and per model `(most emitted, model quads)`.
    pub(super) chest_frames: usize,
    pub(super) be_quads: [(u32, u32); 3],
    /// M250 — r110: frames on which an arrow drew every quad of its model,
    /// and `(most drawn, model quads)`.
    pub(super) arrow_frames: usize,
    pub(super) arrow_quads: (u32, u32),
    /// M252 — r111: frames that drew a minecart as `MinecartModel`.
    pub(super) minecart_frames: usize,
    /// M253 — r112: frames on which both staged carried blocks drew every
    /// quad of their block model, and the most complete in one frame.
    pub(super) carried_block_frames: usize,
    pub(super) carried_blocks_max: usize,
    /// M254 — r113: frames on which a painting drew every quad of its
    /// variant's model.
    pub(super) painting_frames: usize,
    /// M255 — r114: frames whose world text carried the staged text
    /// display's line in its gold.
    pub(super) text_display_frames: usize,
    /// M256 — r115: frames on which the staged item display drew every quad
    /// of its item's model.
    pub(super) item_display_frames: usize,
    /// M257 — r116: frames on which the staged end crystal drew every quad
    /// of its model.
    pub(super) end_crystal_frames: usize,
    /// M258 — r117: frames on which the staged wither skull, llama spit and
    /// shulker bullet all drew every quad of their models.
    pub(super) small_projectile_frames: usize,
    /// M259 — r118: frames on which the staged evoker fangs drew every quad
    /// of their model.
    pub(super) evoker_fangs_frames: usize,
    /// M260 — r119: frames on which the staged block display drew every
    /// quad of its run-time-built model.
    pub(super) block_display_frames: usize,
    /// M261 — r120: frames on which the staged hopper and chest minecarts'
    /// contents both drew every quad.
    pub(super) minecart_contents_frames: usize,
    /// M262 — r121: frames whose world text carried the staged text
    /// display's background plate.
    pub(super) text_display_backdrop_frames: usize,
    /// M264 — r122: frames on which the staged bell drew swinging.
    pub(super) bell_shake_frames: usize,
    /// M265 — r123: frames on which a block display drew at a scale strictly
    /// between its two keyframes.
    pub(super) display_interp_frames: usize,
    /// M266 — r124: distinct ticks on which the staged display stood part-way
    /// through its teleport, and the last such tick.
    pub(super) display_glide_ticks: usize,
    pub(super) display_glide_last: u64,
    pub(super) display_glide_y0: Option<f64>,
    /// M267 — r125: frames on which the staged item display drew at its
    /// brightness override's light.
    pub(super) display_brightness_frames: usize,
    /// M268 — r126: frames whose world text carried the staged text display's
    /// drop shadow.
    pub(super) text_display_shadow_frames: usize,
    /// M269 — r127: frames on which a primed TNT drew swelling.
    pub(super) tnt_swell_frames: usize,
    /// M270 — r128: frames on which the staged posed armour stand drew with
    /// its poses.
    pub(super) stand_pose_frames: usize,
    /// M271 — r129: frames on which the staged invisible named armour stand
    /// drew its name with its body hidden.
    pub(super) hologram_stand_frames: usize,
    /// M272 — r130: frames on which the staged player head drew every quad
    /// in a skin of its own, the staged file's pixels.
    pub(super) skin_head_frames: usize,
    /// M273 — r131: frames on which the staged armour stand drew wearing a
    /// player head in r130's skin.
    pub(super) worn_skin_head_frames: usize,
    /// M274 — r132: frames on which the staged armour stand drew wearing a
    /// carved pumpkin through the head item branch.
    pub(super) worn_item_head_frames: usize,
    /// M275 — r133: frames on which the bot's staged player-head item
    /// resolved to its per-skin icon model.
    pub(super) skinned_icon_frames: usize,
    /// M276 — r134: frames on which r103's enchanting table drew its whole
    /// book, and the range of the book's turn (`transform[0][0]`) over them.
    pub(super) enchant_book_frames: usize,
    pub(super) enchant_book_turn: Option<(f32, f32)>,
    /// M277 — r135: frames on which a TNT draw was lit (white) and unlit.
    pub(super) tnt_lit_frames: usize,
    pub(super) tnt_unlit_frames: usize,
    /// M277 — r136: frames on which the ignited creeper drew swollen, and
    /// those of them with its white flash.
    pub(super) creeper_swollen_frames: usize,
    pub(super) creeper_flash_frames: usize,
    /// M278 — r137: frames on which the glowing armour stand drew with its
    /// outline and the outline pass ran.
    pub(super) outline_frames: usize,
    /// M279 — r138: frames on which the invisible glowing armour stand drew
    /// as its outline alone.
    pub(super) ghost_outline_frames: usize,
    /// M281 — r139: frames on which the burning armour stand drew with its
    /// flames.
    pub(super) burning_frames: usize,
    /// M282 — r140: the lowest vignette brightness a frame drew with.
    pub(super) vignette_low: Option<f32>,
    /// M283 — r141: frames on which the sleep fade drew, and its peak alpha.
    pub(super) sleep_fade_frames: usize,
    pub(super) sleep_fade_peak: i32,
    /// M284 — r142: the last sleeping frame's camera `(yaw, pitch, eye above
    /// the feet)` and the player's own yaw beside it.
    pub(super) sleep_camera: Option<(f32, f32, f64, f32)>,
    /// M284 — r142: frames asleep and how many of them drew a hand; frames
    /// awake that drew one (the control).
    pub(super) asleep_hand_frames: (usize, usize),
    pub(super) awake_hand_frames: usize,
    /// M285 — r143: frames with the charged creeper among the draws, and how
    /// many of them filled the swirl range.
    pub(super) powered_creeper_frames: usize,
    pub(super) swirl_frames: usize,
    /// M286 — r144: frames with the breeze among the draws, and how many of
    /// them filled the wind range.
    pub(super) breeze_frames: usize,
    pub(super) wind_frames: usize,
    /// M287 — r145: frames with the half-health wither drawn powered, and
    /// how many of them filled its swirl sub-range.
    pub(super) powered_wither_frames: usize,
    pub(super) wither_swirl_frames: usize,
    /// M288 — r146: frames with the invulnerable wither drawn in its
    /// invulnerable sheet (resolved in the atlas) at its shrunk scale.
    pub(super) invulnerable_wither_frames: usize,
    /// M289 — r147: frames on which the dropped and the framed player head
    /// drew in their skin.
    pub(super) dropped_skin_head_frames: usize,
    pub(super) framed_skin_head_frames: usize,
    /// M290 — r148: frames on which the armour stand's held head drew in
    /// its skin.
    pub(super) held_skin_head_frames: usize,
    /// M292 — r149: frames of a mount screen whose window posed the mount's
    /// own kind.
    pub(super) mount_preview_frames: usize,
    /// M294 — r150: frames on which the staged horse drew in its iron armour.
    pub(super) horse_armor_frames: usize,
    /// M295 — r151: frames on which a horse drew saddled, and those of them
    /// ridden (reins on).
    pub(super) saddle_frames: (usize, usize),
    /// M296 — r152: frames on which r84's donkey drew with its chests.
    pub(super) chested_donkey_frames: usize,
    /// M297 — r153: frames on which the staged llama drew chested in its
    /// red carpet.
    pub(super) llama_decor_frames: usize,
    /// M298 — r154: frames on which the staged pig and strider drew saddled.
    pub(super) small_saddle_frames: [usize; 2],
    /// M299 — r155: frames on which the staged wolf drew in its armour.
    pub(super) wolf_armor_frames: usize,
    /// M300 — r156: frames on which a camel drew saddled, and those of them
    /// ridden (reins on).
    pub(super) camel_saddle_frames: (usize, usize),
    /// M301 — r157: frames on which r90's nautilus drew saddled.
    pub(super) nautilus_saddle_frames: usize,
    /// M302 — r158: frames on which the staged happy ghast drew harnessed.
    pub(super) harness_frames: usize,
    /// M303 — r159: frames on which the staged camel husk drew saddled.
    pub(super) camel_husk_frames: usize,
    /// M304 — r160: frames on which the staged giant drew as a giant.
    pub(super) giant_frames: usize,
    /// M305 — r161: whether a magma cube drew at size 1 (scale 0.5) and one
    /// at size 4 (scale 2.0).
    pub(super) cube_sizes: (bool, bool),
    /// M306 — r162: frames on which a slime drew and the translucent range
    /// held vertices (its shell).
    pub(super) slime_shell_frames: usize,
    /// M307 — r163: whether a sulfur cube drew as the adult model and one as
    /// the small (baby) model.
    pub(super) sulfur_cubes: (bool, bool),
    /// M308 — r164: frames on which a slim avatar drew in an uploaded skin
    /// (the staged mannequin's default).
    pub(super) mannequin_frames: usize,
    /// M309 — r165: frames on which a wide avatar drew in an uploaded skin
    /// (the staged named mannequin's default).
    pub(super) named_mannequin_frames: usize,
    /// M310 — r166: frames on which an avatar drew with exactly its hat and
    /// jacket hidden.
    pub(super) hidden_layer_frames: usize,
    /// M311 — r167: frames on which a wolf drew its armour's high cracks.
    pub(super) wolf_crack_frames: usize,
    /// M312 — r168: frames on which a harnessed happy ghast holding a leash
    /// drew (its ropes shown).
    pub(super) ghast_rope_frames: usize,
    /// M314 — r169: frames on which a baby happy ghast drew as the ghastling.
    pub(super) ghastling_frames: usize,
    /// M315 — r170: frames on which a baby turtle drew as its own model.
    pub(super) baby_turtle_frames: usize,
    /// M316 — r171: frames on which a polar bear cub drew as its own model.
    pub(super) polar_cub_frames: usize,
    /// M317 — r172: whether a baby zombie and a baby husk drew as
    /// `BabyZombieModel`.
    pub(super) baby_zombies: (bool, bool),
    /// M318 — r173: frames on which a baby villager drew as its own model.
    pub(super) baby_villager_frames: usize,
    /// M319 — r174: frames on which a baby drowned drew as its own model.
    pub(super) baby_drowned_frames: usize,
    /// M320 — r175: whether a baby piglin and a baby zombified piglin drew
    /// as `BabyPiglinModel`.
    pub(super) baby_piglins: (bool, bool),
    /// M321 — r176: whether a calf and a mooshroom calf drew as `BabyCowModel`.
    pub(super) calves: (bool, bool),
    /// M322 — r177: whether a piglet and a chick drew as `BabyPigModel` and
    /// `BabyChickenModel`.
    pub(super) piglet_chick: (bool, bool),
    /// M323 — r178: whether a lamb drew as `BabySheepModel`.
    pub(super) lamb: bool,
    /// M324 — r179: whether a dolphin calf drew as `BabyDolphinModel`.
    pub(super) dolphin_calf: bool,
    /// M325 — r180: whether a baby zombie villager drew as
    /// `BabyZombieVillagerModel`.
    pub(super) baby_zombie_villager: bool,
    /// M326 — r181: whether a baby squid and a baby glow squid drew as
    /// `BabySquidModel`.
    pub(super) baby_squids: (bool, bool),
    /// M327 — r182: whether a hoglet and a baby zoglin drew as
    /// `BabyHoglinModel`.
    pub(super) hoglets: (bool, bool),
    /// M328 — r183: whether a baby strider drew as `BabyStriderModel`.
    pub(super) baby_strider: bool,
    /// M330 — r184: whether a kit drew as `BabyFoxModel`.
    pub(super) kit: bool,
    /// M331 — r185: whether a panda cub drew as `BabyPandaModel`.
    pub(super) panda_cub: bool,
    /// M332 — r186: whether an ocelot kitten drew as `BabyOcelotModel`.
    pub(super) ocelot_kitten: bool,
    /// M333 — r187: whether a baby bee drew as `BabyBeeModel`.
    pub(super) baby_bee: bool,
    /// M334 — r188: whether a donkey foal and a mule foal drew as
    /// `BabyDonkeyModel`.
    pub(super) foals: (bool, bool),
    /// M335 — r189: whether a camel calf drew as `BabyCamelModel`.
    pub(super) camel_calf: bool,
    /// M336 — r190: whether a rabbit kit drew as `BabyRabbitModel`.
    pub(super) rabbit_kit: bool,
    /// M337 — r191: whether a goat kid drew as `BabyGoatModel`.
    pub(super) goat_kid: bool,
    /// M338 — r192: whether a baby llama drew as `BabyLlamaModel`, and on a
    /// coat variant.
    pub(super) baby_llama: (bool, bool),
    /// M339 — r193: whether a horse foal drew as `BabyHorseModel`, and on a
    /// coat variant.
    pub(super) horse_foal: (bool, bool),
    /// M340 — r194: whether a kitten drew as `BabyCatModel`, and on a coat
    /// variant.
    pub(super) kitten: (bool, bool),
    /// M341 — r195: whether a puppy drew as `BabyWolfModel`, and on a coat
    /// variant.
    pub(super) puppy: (bool, bool),
    /// M342 — r196: whether a tame wolf drew with a blue collar.
    pub(super) blue_collar: bool,
    /// M343 — r197: whether a tame cat drew with a blue collar.
    pub(super) cat_collar: bool,
    /// M194 — frames on which the collector read the SERVER's summoned frame
    /// (`render_check.py`: facing 2, rotation 3, a sword) — r71, the
    /// metadata slots calibrated against a real 26.2 server.
    pub(super) server_frame_frames: usize,
    /// M185 — frames on which `collect_leashes` drew the injected horse's
    /// leash to the happy ghast as the four-rope quad leash (r72).
    pub(super) quad_leash_frames: usize,
    /// M206 — frames on which the loaded columns were exactly the set a
    /// 26.2 server tracks for the client's own render distance (staged in
    /// `options.txt` by `render_check.py`, below the server's), and the fog
    /// band was vanilla's for it (r73).
    pub(super) view_distance_frames: usize,
    /// The last frame's reading, for the row's detail: the staged distance,
    /// the client information's, the server's radius, the column count, the
    /// tracking set's size, and the fog band.
    pub(super) view_distance_seen: Option<(Option<i32>, u8, i32, usize, usize, [f32; 2])>,
    /// M179 — r74: the tab the screen opened on and the tab the injected
    /// click hit (root ids), and the frames on which the SERVER's
    /// `select_advancements_tab` had set the clicked one.
    pub(super) adv_click: Option<(String, String)>,
    pub(super) adv_echo_frames: usize,
    /// Why the click never happened, if it did not (fewer than two tabs).
    pub(super) adv_click_skip: Option<String>,
    /// M184 — r75: the staged lectern's position, the pages its reader
    /// showed, the frames on which the server's data echo had moved the page
    /// to 1 after the forward click (menu data AND the reader), and whether
    /// the close left no menu behind.
    pub(super) lectern_pos: Option<(i32, i32, i32)>,
    pub(super) lectern_pages_seen: usize,
    pub(super) lectern_echo_frames: usize,
    pub(super) lectern_closed: bool,
    pub(super) lectern_skip: Option<String>,
    /// M216 — r76: the villager the right-click hit, and the offers the
    /// server's `merchant_offers` decoded to, as `(cost item, cost count,
    /// result item, result count, max uses)`.
    pub(super) trade_villager: Option<i32>,
    pub(super) trade_offers: Vec<(i32, i32, i32, i32, i32)>,
    pub(super) trade_expected: Vec<(i32, i32, i32, i32, i32)>,
    pub(super) trade_skip: Option<String>,
    /// M216 — r77: whether the villager was unhurt before the left-click,
    /// and the frames after it on which the server's `damage_event` had armed
    /// its hurt clock.
    pub(super) attack_unhurt_before: bool,
    pub(super) attack_hurt_frames: usize,
    /// M217 — r78: the leaves' position, whether the press happened in
    /// survival, the client ticks from the press to the STOP, and whether the
    /// server then removed them.
    pub(super) mine_pos: Option<[i32; 3]>,
    /// M224 — the first block the client saw in the leaves' place, and the
    /// client ticks after the press it arrived. Air, or water: r53's flood
    /// spreads around the spawn, and with a neighbour's fluid tick already
    /// due the water fills the gap a tick after the break, so on a slow
    /// frame the air and the water updates land in one frame and the client
    /// never holds air (three runs in ten). Leaves that are not waterlogged
    /// cannot hold flowing water, so water there means they are gone.
    pub(super) mine_changed: Option<(u64, String)>,
    pub(super) mine_survival: bool,
    pub(super) mine_ticks: Option<u64>,
    pub(super) mine_broken: bool,
    pub(super) mine_skip: Option<String>,
    /// What the client believed at the press (elapsed fraction, on ground,
    /// eye in water, game mode flags) — r78's failure detail.
    pub(super) mine_diag: Option<String>,
    /// The main-hand item at the press, and the tick count vanilla's
    /// literals give for it.
    pub(super) mine_held: Option<String>,
    pub(super) mine_expected: Option<u64>,
    /// M218 — r79: the dirt count before Q and after the server's slot
    /// update, and whether an item entity appeared near the bot.
    pub(super) drop_counts: Option<(i32, i32)>,
    pub(super) drop_item_entity: bool,
    /// r80: the off hand after F (item name, count).
    pub(super) swap_offhand: Option<(String, i32)>,
    /// r81: the held item after the pick key on the lectern.
    pub(super) pick_held: Option<String>,
    /// What the pick press targeted (entity, block, eye) — r81's detail.
    pub(super) pick_aim: Option<String>,
    /// r82: the server's sprint flag after START, and after STOP, and how
    /// many sprint commands went out.
    pub(super) sprint_on: bool,
    pub(super) sprint_off: bool,
    pub(super) sprint_sent: u64,
    pub(super) hands_skip: Option<String>,
    /// M219 — r83: frames the in-bed screen's button was drawn with its label.
    pub(super) in_bed_frames: usize,
    /// r83: the bed's foot, whether the server set a sleeping position, the
    /// client's position then, the largest horizontal move while asleep with
    /// W held, the STOP_SLEEPING sends from the button, and whether the
    /// server woke the player and the in-bed screen went away.
    pub(super) bed_pos: Option<[i32; 3]>,
    pub(super) bed_slept: bool,
    /// The server's sleeping position — the HEAD half, whichever was clicked
    /// (`BedBlock.useWithoutItem` moves to it).
    pub(super) bed_head: Option<(i32, i32, i32)>,
    pub(super) bed_client_pos: Option<[f64; 3]>,
    pub(super) bed_drift: f64,
    pub(super) bed_wake_sends: u32,
    pub(super) bed_woke: bool,
    pub(super) bed_screen_closed: bool,
    pub(super) bed_skip: Option<String>,
    /// How many right-clicks on the bed it took (the server's clock echo is
    /// periodic, so the first may land before its night).
    pub(super) bed_clicks: u32,
    /// M220 — r84/r85: per drawn mount-menu frame `(open serial, panel
    /// height, overlay count)`; the donkey; what the first (sneak-click) menu
    /// resolved; whether the bot rode the donkey and E went out; and the
    /// second menu's serial.
    pub(super) mount_panel_frames: Vec<(u64, f32, usize)>,
    pub(super) donkey: Option<i32>,
    pub(super) mount_columns: Option<i32>,
    pub(super) mount_slots: usize,
    pub(super) mount_active: Option<(bool, bool)>,
    pub(super) mount_saddle_item: Option<String>,
    pub(super) mount_chest_item: Option<(String, i32)>,
    pub(super) mount_title: Option<String>,
    pub(super) mount_rode: bool,
    pub(super) mount_e_sent: bool,
    pub(super) mount_second_serial: Option<u64>,
    pub(super) mount_skip: Option<String>,
    pub(super) mount_seen: Option<String>,
    /// M220 — r86: container-panel frames that still carried the player
    /// preview, and player-inventory frames that did (the control).
    pub(super) container_preview_frames: u64,
    /// M221 — r87: the boat, whether the client steered it, how far W moved
    /// it, how far A turned it, the packets sent, the rejections, and the
    /// server's `/data get` position against the client's.
    pub(super) boat: Option<i32>,
    pub(super) boat_steered: bool,
    pub(super) boat_moved: f64,
    pub(super) boat_turned: f32,
    pub(super) boat_moves_sent: u64,
    pub(super) boat_paddles_sent: u64,
    pub(super) boat_rejections: u64,
    pub(super) boat_client_pos: Option<[f64; 3]>,
    pub(super) boat_server_pos: Option<[f64; 3]>,
    pub(super) boat_skip: Option<String>,
    /// M222 — r88: the horse, whether the client steered it, how far W moved
    /// it, its jumps and highest rise, the moves sent, the rejections, and the
    /// server's position against the client's.
    pub(super) horse: Option<i32>,
    pub(super) horse_steered: bool,
    pub(super) horse_moved: f64,
    /// M226: the per-tick distance on the last tick of W.
    pub(super) horse_step: f64,
    pub(super) horse_jumps: u64,
    pub(super) horse_rise: f64,
    pub(super) horse_moves_sent: u64,
    pub(super) horse_rejections: u64,
    pub(super) horse_client_pos: Option<[f64; 3]>,
    pub(super) horse_server_pos: Option<[f64; 3]>,
    pub(super) horse_skip: Option<String>,
    /// M225 — r89 (and r82's sprint state, which needs a vehicle that lets
    /// its rider sprint): the camel, whether the client steered it, its
    /// per-tick distance on the last sprinting tick, its dashes and the
    /// fastest tick after the dash, the jump meter's peak while the dash
    /// cooled down (and the cooldown then), the moves sent, the rejections,
    /// and the server's position against the client's.
    pub(super) camel: Option<i32>,
    pub(super) camel_steered: bool,
    pub(super) camel_sprint_step: f64,
    pub(super) camel_dashes: u64,
    pub(super) camel_dash_step: f64,
    pub(super) camel_cooldown_meter: Option<(f32, i32)>,
    pub(super) camel_moves_sent: u64,
    pub(super) camel_rejections: u64,
    pub(super) camel_client_pos: Option<[f64; 3]>,
    pub(super) camel_server_pos: Option<[f64; 3]>,
    pub(super) camel_skip: Option<String>,
    /// M227 — r90: the nautilus, whether the client steered it, its per-tick
    /// distance on the last tick of W and how far its height drifted (no
    /// gravity in water), its dashes and the fastest tick after one, the
    /// meter's peak and the cooldown while it cooled, the moves sent, the
    /// rejections, and the server's position against the client's.
    pub(super) nautilus: Option<i32>,
    pub(super) nautilus_steered: bool,
    pub(super) nautilus_swim_step: f64,
    pub(super) nautilus_drift: f64,
    pub(super) nautilus_dashes: u64,
    pub(super) nautilus_dash_step: f64,
    pub(super) nautilus_cooldown_meter: Option<(f32, i32)>,
    pub(super) nautilus_moves_sent: u64,
    pub(super) nautilus_rejections: u64,
    pub(super) nautilus_client_pos: Option<[f64; 3]>,
    pub(super) nautilus_server_pos: Option<[f64; 3]>,
    pub(super) nautilus_skip: Option<String>,
    /// M228 — r91: for each ridden vehicle (r87-r90), the local player's
    /// position as the client seated it, and the server's own
    /// (`positionRider` on its side) once the vehicle stood still.
    pub(super) seat_checks: Vec<(&'static str, [f64; 3], Option<[f64; 3]>)>,
    /// M229 — r92-r94, survival right-clicks through the production use key:
    /// food before / after eating bread at the ground and the bread count
    /// before / after; the bucket filled from r87's pool and emptied on dry
    /// ground (and whether the client then saw water there); a snowball and
    /// a fishing bobber appearing, and the bobber reeled back in.
    pub(super) eat_food: Option<(i32, i32)>,
    /// M231 — r96: a tick's walking distance after 25 ticks of W, and after
    /// 25 ticks of W while eating.
    pub(super) walk_steps: (f64, f64),
    /// M236 — the bot's health when the use witnesses began (before the
    /// heal), for a failure's detail.
    pub(super) use_start_health: f32,
    pub(super) eat_count: Option<(i32, i32)>,
    pub(super) bucket_filled: bool,
    pub(super) bucket_emptied: bool,
    pub(super) bucket_water_placed: bool,
    pub(super) snowball_seen: bool,
    pub(super) bobber_seen: bool,
    pub(super) bobber_gone: bool,
    pub(super) use_skip: Option<String>,
    /// M230 — r95: the floor lever and the stone button powered by a
    /// right-click while bread is in hand and the bot is hungry, and the
    /// (food, bread) before and after (nothing may be eaten).
    pub(super) lever_on: bool,
    pub(super) button_on: bool,
    pub(super) click_food: Option<((i32, i32), (i32, i32))>,
    /// M232 — r97: the crafting table opened by a right-click, the result
    /// slot after a log went into the grid, and (planks, logs) in the
    /// inventory after the shift-click; r98: the furnace opened, its (lit
    /// time, cook progress) once ore and fuel were clicked in, and the iron
    /// ingots in the inventory after shift-clicking its result.
    pub(super) craft_opened: bool,
    pub(super) craft_result: Option<(String, i32)>,
    pub(super) craft_inventory: Option<(i32, i32)>,
    pub(super) furnace_opened: bool,
    pub(super) furnace_data: Option<(i16, i16)>,
    pub(super) furnace_taken: i32,
    /// M233 — r99: the frame's FOV (standing, after 40 ticks drawing a bow)
    /// and whether the bow was still being drawn at the second reading.
    pub(super) bow_fov: (f32, f32),
    pub(super) bow_drawing: bool,
    /// M234 — r100: the model the first-person main hand drew standing and
    /// after 40 ticks drawing.
    pub(super) bow_models: (Option<String>, Option<String>),
    /// M235 — r101: health before and after an unblocked hit, after a hit
    /// with the shield up, and whether the off hand was using at that hit.
    pub(super) shield_health: (f32, f32, f32),
    pub(super) shield_up: bool,
    /// M236 — r102: the anvil's drawn cost line (text, linear colour) at 0
    /// levels and at 5, the levels before and after taking the result, and
    /// the swords in the inventory after; r103: the table's three offers,
    /// the levels before and after the first row, and (enchanted, lapis
    /// left) after it.
    pub(super) anvil_labels: (Option<(String, [f32; 3])>, Option<(String, [f32; 3])>),
    pub(super) anvil_xp: (i32, i32),
    pub(super) anvil_swords: i32,
    pub(super) enchant_costs: [i32; 3],
    pub(super) enchant_xp: (i32, i32),
    pub(super) enchant_result: (bool, i32),
    /// M237 — r104: the brewing stand opened, its (brew time, fuel) once
    /// brewing, and slots 0, 3 and 4 then.
    pub(super) brew_opened: bool,
    pub(super) brew_state: Option<(i16, i16)>,
    pub(super) brew_slots: Vec<Option<(String, i32)>>,
    /// M239 — r106: the glide began, a rocket was attached to the bot, the
    /// per-tick horizontal step before the rocket and the fastest after,
    /// and the server corrections before and after the boost.
    pub(super) boost_glided: bool,
    pub(super) boost_rocket: bool,
    pub(super) boost_steps: (f64, f64),
    pub(super) boost_corrections: (u32, u32),
    /// The firework entities seen during the boost, with their attachment.
    pub(super) boost_seen: Vec<(i32, Option<i32>)>,
    pub(super) boost_bot: Option<i32>,
    /// Rockets left in the inventory after the boost, and still gliding.
    pub(super) boost_after: (i32, bool),
    /// The cursor stack after the first press, and where the press was.
    pub(super) enchant_carried: Option<(Option<(String, i32)>, (f64, f64))>,
    /// Frames, and inventory-screen frames, inside the timed run (r16).
    pub(super) timed_frames: u64,
    pub(super) timed_screen_frames: u64,
    pub(super) inventory_preview_frames: u64,
    /// M132 — frames on which the SCOREBOARD SIDEBAR put text on the list.
    ///
    /// The gate injects an objective, three scores and a display-slot
    /// assignment through the production router, so this grades the whole
    /// chain: the three packets decode into `Scoreboard`, `select_objective`
    /// finds the slot, the rows resolve and format, and the layout places
    /// them. A break anywhere drops it to zero. It is a *text* count rather
    /// than a fill count because the two bands are drawn unconditionally once
    /// a sidebar exists, while a row's glyphs need every stage above.
    pub(super) sidebar_frames: u64,
    /// The most text lines the sidebar drew in any frame.
    ///
    /// Separate from the frame count because a sidebar with a title and no
    /// rows still scores `sidebar_frames` — this is what says the *scores*
    /// reached the renderer. Three injected rows with a name and a number
    /// each, plus the title, is seven.
    pub(super) sidebar_text_max: usize,
    /// M151 — frames on which the TAB LIST was drawn in the windowed client.
    ///
    /// The M86 shape a second time: `rewo_gpu::tab_list` had 41 passing tests
    /// and no caller at all, so every other witness in this file is satisfied
    /// by a client whose Tab key does nothing. The gate holds the key down for
    /// the second half of the run, so this must be non-zero and must be
    /// **less** than `frames` — a client that drew the list unconditionally
    /// would score every frame and is exactly as wrong as one that never drew
    /// it.
    pub(super) tab_list_frames: u64,
    /// The most rows the layout PLACED in any frame.
    ///
    /// Separate from the frame count because the bands are drawn as soon as a
    /// view exists: this is what says the listed-player filter, the sort and
    /// the column solve all produced slots. The run injects a set of players,
    /// one of them unlisted, so the count is a claim about the filter and not
    /// merely about arrival.
    pub(super) tab_list_rows_max: usize,
    /// The most text lines the tab list emitted, and the most ping icons.
    ///
    /// Two separate counts because they come from two different lists —
    /// `set_text` and `set_hud_icons` — and a break in either leaves the other
    /// healthy. The icons are the first `HudBlit` anything produces.
    pub(super) tab_list_text_max: usize,
    pub(super) tab_list_icons_max: usize,
    /// M163 — frames on which a mob's nametag carried a RESOLVED translatable.
    ///
    /// The run injects a zombie whose `Entity.DATA_CUSTOM_NAME` is
    /// `{translate: "entity.minecraft.zombie"}`, and this counts the frames on
    /// which the label the renderer was handed reads `Zombie`.
    ///
    /// Every stage has to work: the raw body reaches
    /// `route_set_entity_data`, `MetaKinds::lang` carries the table the app
    /// installed on `PlaySession`, `chat_style::flatten` resolves the key,
    /// `EntityTable::custom_name` stores it and `resolve_labels` hands it to
    /// `EntityDraw::name`. Before M163 the decode called `to_plain_text`, which
    /// has no table.
    /// M166 — what the two blocking configuration tasks asked for, mirrored
    /// off the `PlaySession`.
    ///
    /// Mirrored rather than counted: a counter is satisfied by any client that
    /// reached the arm, including one replying with a zero UUID, whereas the
    /// server stages a **known** pack id and code-of-conduct text and this
    /// carries what came off the wire.
    pub(super) config_tasks: rewo_net::config_tasks::ConfigTaskLog,
    pub(super) nametag_resolved_frames: u64,
    /// The negative half, and the reason a `> 0` counter alone is not enough.
    ///
    /// r26 scored full marks while chat drew `multiplayer.player.joined`, so a
    /// witness that only counts "a name was drawn" cannot see this bug at all.
    /// This counts frames on which the label was the RAW KEY, and must be zero.
    pub(super) nametag_raw_key_frames: u64,
    /// M163 — frames on which a SIGN line carried a RESOLVED translatable.
    ///
    /// The sibling of `nametag_resolved_frames`, and it exists because the two
    /// halves of that feature fail independently. The nametag resolves at
    /// DECODE (`MetaKinds::lang`); a sign resolves in the FRAME LOOP, because
    /// `BlockEntity::sign_text` is called per sign per frame — so r48 proves
    /// nothing at all about this path.
    ///
    /// A review demonstrated the gap by changing both of the app's
    /// `collect_sign_text` call sites to pass no table, with
    /// `blockentityshot` still 179/179: `sg1`/`sg2` hand the collector a table
    /// of their own, so they grade the function and never ask who calls it.
    /// [`collect_session_sign_text`] now makes that particular edit a compile
    /// error, and this counts the frames on which the line the renderer was
    /// handed actually reads `Dirt`.
    pub(super) sign_resolved_frames: u64,
    /// The negative half — frames on which the sign line was the RAW KEY.
    ///
    /// Truncated, and deliberately matched as a PREFIX: `getRenderMessages`
    /// splits each line against the board and keeps fragment 0, so
    /// `block.minecraft.dirt` reaches the renderer as `block.minecraft.di` on
    /// a 90 px standing sign. That truncation is what the bug looked like in
    /// game, and asserting the whole key here would score zero for the wrong
    /// reason.
    pub(super) sign_raw_key_frames: u64,
    /// M164 — the largest `ColumnMesh::carried_fluid_cells` any column uploaded
    /// in the windowed client. See `r53`.
    pub(super) carried_fluid_cells: u32,
    /// Over the draws THIS RUN'S OWN INJECTION produced (`CROWD_IDS`, joined
    /// to the draw list by the position `collect_entities` gave each of them
    /// this frame): the most draws sharing one `EntityModelKind` that reached a
    /// single `set_entities`, and the most distinct non-capsule kinds among
    /// them.
    ///
    /// **The restriction to our own three entity ids is the load-bearing part.**
    /// The first version counted every `EntityModelKind::Zombie` draw in the
    /// frame, which is a claim about the world rather than about this
    /// injection: any other fixture in the same run that happens to spawn a
    /// zombie satisfies `>= 2` on its own, so a *partially* landed injection
    /// (one of the three bodies decoded) would still have passed. Restricting
    /// by id makes the two counts a statement about exactly the three entities
    /// this check created — and the two together pin the multiset `{A, A, B}`,
    /// since `same >= 2` with at most three draws and `distinct >= 2` cannot
    /// both hold otherwise.
    ///
    /// Two counts because they break independently. The first is the crowd —
    /// §0.0's mob-texture report says the symptom needs more than one mob in
    /// the scene, and nothing anywhere drove that in the *windowed* client. The
    /// second is kind RESOLUTION: every serverless gate (including this
    /// milestone's `mobtexshot`) hands `EntityDraw::kind` in directly, so the
    /// live `etypes.name(type_id)` -> `kind_for_entity_name` chain has never
    /// had a witness. A break in the type registry, the `add_entity` decode or
    /// the name match leaves both at their unlucky values rather than erroring.
    ///
    /// The first is a real maximum over kinds, not a count of one hard-coded
    /// kind: the doc used to say "the most draws of ONE mob kind" while the
    /// code counted only `Zombie`, which is the comment-that-outranks-the-code
    /// shape this file keeps recording.
    pub(super) crowd_same_kind_max: usize,
    pub(super) crowd_kinds_max: usize,
    /// M138a — listener transforms that reached the audio device.
    ///
    /// Compared against `frames`, not merely to zero: the interesting claim is
    /// that the push happens per FRAME. A client that moved it beside
    /// `sounds.drive` would still be non-zero here and would score roughly the
    /// tick count instead, which at any real frame rate is an order of
    /// magnitude smaller.
    pub(super) listener_pushes: u64,
    /// M147 — the sound event id `MusicManager` last started, if any.
    ///
    /// **Not gated on the `audio` feature**, and that is the point: music
    /// selection runs against whatever device is attached, so this grades the
    /// attribute layering, the selection and the timers in the DEFAULT build
    /// where the silent device stands in. The device is the only part it cannot
    /// see, and no check in this project can.
    pub(super) music_started: Option<String>,
    /// The forward vector the device last received, for a shape check.
    pub(super) listener_forward: Option<[f32; 3]>,
    /// M162 — where the engine put the wither-spawn sound a global
    /// `level_event` asked for, and where the listener was when it landed.
    ///
    /// **The position rather than the fact.** `live_identifiers` would prove
    /// the sound started and say nothing about where, and the whole of
    /// `globalLevelEvent` is about where: 1023 played at the block instead of
    /// two blocks from the listener is a sound at the right time from the wrong
    /// direction, which every count-shaped witness passes.
    pub(super) global_event_sound: Option<(f64, f64, f64)>,
    pub(super) global_event_eye: Option<[f64; 3]>,
    /// The block CENTRE the injected 1023 named, recorded by the gate from its
    /// own arithmetic so the bearing's direction can be predicted without
    /// asking the code under test where it thought the event was.
    pub(super) global_event_target: Option<[f64; 3]>,
    /// M162 — the explosion sound the `explode` tail carried, if the tail was
    /// walked at all.
    pub(super) explosion_sound: Option<(f64, f64, f64)>,
    /// M162 — `MotionStats::explosion_tails` / `explosion_sounds` at the end.
    pub(super) explosion_tails: u32,
    pub(super) explosion_sounds_queued: u32,
    /// M162 — `SoundStats::queued_delayed`, which only a distance-delayed
    /// `level_event` can move in an Overworld run (the other producer is the
    /// End flash).
    pub(super) queued_delayed: u32,
    /// M111 — frames on which the chat SCROLLBAR was drawn.
    ///
    /// It exists only while the screen is open AND the backlog exceeds the box
    /// (`virtualHeight != chatHeight`), so the gate injects 25 lines to reach
    /// it — otherwise this would be a witness over a path the run cannot enter,
    /// which is worse than no witness at all.
    pub(super) chat_scrollbar_frames: u64,
    /// M110 — frames on which the CHAT SCREEN was open and drew its input bar.
    ///
    /// Separate from `chat_line_frames`, which counts the read-only HUD box
    /// M108 built: that one is non-zero from the first message whether or not
    /// a screen exists, so it cannot see whether `T` opens anything. The gate
    /// force-opens the screen a fifth of the way in, the same way it injects a
    /// container for r19 — a windowed run has no keyboard.
    pub(super) chat_screen_frames: u64,
    /// M174 — frames on which the sign editor's board was actually pushed
    /// into the screen pass (read where the draw reads it, the r20 rule), and
    /// whether a replacement commit sent `sign_update`. The staged injection
    /// opens the editor at 0.55; the 0.90 book-menu injection REPLACES it,
    /// which is exactly vanilla's `setScreen` → `removed()` → commit path.
    pub(super) sign_edit_frames: u64,
    pub(super) sign_update_sent: bool,
    /// M115 — frames on which the SUGGESTION POPUP put fills in the list.
    ///
    /// A strictly narrower claim than r27's: the screen can be open with no
    /// popup at all, which is its state until something is typed. The gate
    /// reaches it through the production chain rather than a fake — a
    /// `custom_chat_completions` packet, then a real keystroke — so a break
    /// anywhere from the decode to the render drops this to zero.
    pub(super) suggestion_popup_frames: u64,
    /// M117 — frames on which the chat field was drawn as COLOURED RUNS
    /// rather than one flat line.
    ///
    /// Narrower than r27 and than r30: it needs a `/`-command in the field
    /// *and* a parse of it, and it is the only witness that can see the
    /// highlighting reach the windowed client at all.
    pub(super) highlighted_command_frames: u64,
    /// M117 — frames on which the USAGE BOX was drawn.
    ///
    /// Mutually exclusive with r29 by construction, so it cannot be satisfied
    /// by the same moment: the popup and the box never coexist.
    pub(super) usage_box_frames: u64,
    /// M134 — frames whose usage box held a parse-error MESSAGE rather than
    /// usage lines. Strictly narrower than `usage_box_frames`: the two name
    /// disjoint moments, because an exception suppresses the entries.
    pub(super) parse_error_frames: u64,
    /// M118 — how many times an `@`-selector was offered by the client's own
    /// `EntitySelectorParser`.
    ///
    /// Narrower than r30: a literal completion needs only the tree, where this
    /// needs `minecraft:entity` to have stopped being `Unknown`.
    pub(super) local_selector_completions: u64,
    /// M119 — how many times a registry id (a block or an item) was offered
    /// by the client's own parser.
    ///
    /// Narrower than r30 and disjoint from r33: it needs `block_state` or
    /// `item_stack` to have stopped being `Unknown`, and it counts a colon,
    /// which no literal or selector-option name carries.
    pub(super) local_resource_completions: u64,
    /// M120 — how many times a COORDINATE default was offered by the
    /// client's own parser. Disjoint from r33 and r34: `~` appears in no
    /// literal, selector or registry id.
    pub(super) local_coordinate_completions: u64,
    /// M124 — how many local answers offered a name from one of the seven
    /// literal tables. `sidebar.team.` prefixes only `scoreboard_slot`'s, so
    /// this is disjoint from r33/r34/r35 by construction.
    pub(super) local_literal_table_completions: u64,
    /// M116 — how many times a `/`-command's completion was answered by the
    /// CLIENT rather than the server.
    ///
    /// The claim r30 makes is the milestone's whole point, and it is a
    /// negative one: M114 sent a packet for every keystroke on a command line
    /// and this counts the ones that no longer leave. A witness on "the popup
    /// opened" cannot see it, because both paths open a popup.
    pub(super) local_command_completions: u64,
    /// M108 — frames on which the chat box put at least one line into the
    /// windowed frame's label list.
    ///
    /// The count comes from the production derivation (`build_text` returns
    /// it) rather than from the gate re-deriving `chat_lines` — a gate that
    /// recomputes the rule it grades agrees with any implementation, which is
    /// M93q's finding. It needs no caller staging: `--render-check` sends its
    /// own chat line, so an unstaged run still exercises the whole path from
    /// `player_chat` through the signature cache to the wrapped line.
    pub(super) chat_line_frames: u64,
    /// M125 — frames on which a drawn chat line was a RESOLVED translatable.
    ///
    /// The scene is the `/give` this gate ALREADY stages for r14, so this adds
    /// no caller requirement — and it is a much stronger claim than the one it
    /// replaces, because the server's success message is a translatable THREE
    /// LEVELS DEEP whose middle argument is a bare integer:
    ///
    /// ```text
    /// commands.give.success.single   "Gave %s %s to %s"
    ///   with[0] = 1                             (a raw IntTag)
    ///   with[1] = chat.square_brackets "[%s]"
    ///               with[0] = item.minecraft.diamond_sword
    ///   with[2] = <the player's display-name component>
    /// ```
    ///
    /// So `Gave 1 [Diamond Sword]` cannot be produced unless the lookup, the
    /// substitution, the recursion into a component argument, and the
    /// heterogeneous-list unwrap all work. **None of those five words appears
    /// anywhere in the raw component**, so the string cannot leak through from
    /// a flattener that resolved nothing.
    ///
    /// The first version of this witness drove the JOIN message instead, on
    /// the stated premise that a server announces a joining player to that
    /// player. **It does not**, and the gate said so by scoring zero:
    /// `PlayerList.placeNewPlayer` broadcasts at line 202 and does
    /// `this.players.add(player)` at line 210, and `broadcastSystemMessage`
    /// iterates `this.players` — so the joiner is not yet in the list it is
    /// announced to. Suspect the witness first.
    pub(super) translated_chat_frames: u64,
    /// M126d — frames on which the drawn chat carried MORE THAN ONE colour.
    ///
    /// The claim r26 and r37 cannot make. Both are satisfied by a chat box
    /// that flattens every message to one white string: r26 counts rows and
    /// r37 reads their characters, and neither can see whether the spans
    /// survived the wrap and reached the renderer as separately-coloured
    /// lines. A zero here with the scene injected means the pipeline is
    /// carrying spans and then throwing them away at the last step, which is
    /// exactly the failure the milestone exists to prevent.
    ///
    /// Counted over the DRAWN lines in `build_text`'s chat range, not over the
    /// chat store — M125's rule, one surface further on.
    pub(super) styled_chat_frames: u64,
    /// M127 — frames whose chat drew a DECORATED line: `<name> text`, from an
    /// injected `disguised_chat` bound to the server's own `minecraft:chat`.
    pub(super) decorated_chat_frames: u64,
    /// M127 — frames where that decoration's OWN style reached the row.
    pub(super) styled_decoration_frames: u64,
    /// M126d — frames on which a drawn chat line carried a non-plain
    /// `TextStyle`.
    ///
    /// Separate from the row above because the two halves travel by different
    /// routes: the colour rides `OwnedTextLine::color`, which existed before
    /// this milestone, while the five flags ride the `style` field it added.
    /// A wiring that dropped `style` on the floor would leave the colour
    /// witness green.
    pub(super) flagged_chat_frames: u64,
    /// M128 — where `chat_lines` DREW the clickable witness message, in
    /// screen pixels, read off the render rather than recomputed.
    ///
    /// That is the whole point of routing the click through this: the hit test
    /// and the draw are two consumers of one geometry, and the failure this
    /// repo keeps producing (M89, M94/M95, M106b, M112) is the two disagreeing
    /// by a row or by a panel's worth of pixels. Feeding the draw's own
    /// numbers back into the hit test is the only version of the witness that
    /// can see it.
    pub(super) chat_link_at: Option<(f32, f32)>,
    /// M128 — the click was fired once and answered `RunCommand` with the
    /// injected command. A count rather than a bool so the report can say how
    /// many frames it held (it is fired once, so 1 is the healthy value and 0
    /// is the failure).
    pub(super) chat_click_ok: u64,
    /// The once-guard for the click above, so the gate sends one command
    /// rather than one per frame.
    pub(super) chat_click_fired: bool,
    /// r67 — the link prompt's progress: 0 not started, 1 opened over chat
    /// (frames are counted while it is up), 2 answered "No", 3 checked.
    pub(super) link_prompt_stage: u8,
    /// r67 — frames on which the prompt was up with its three buttons and
    /// their captions and the URL all drawn, and the chat screen NOT in the
    /// slot (vanilla has one slot: the prompt replaces the chat).
    pub(super) link_prompt_frames: u64,
    /// r67 — the chat field's text when the link was clicked, and the
    /// `chatLinks` / `chatLinksPrompt` values to restore afterwards.
    pub(super) link_prompt_text: String,
    pub(super) link_prompt_options: (bool, bool),
    /// r67 — after "No": the chat screen was back, with the same text, and
    /// no prompt was left behind.
    pub(super) link_prompt_returned: bool,
    /// Frames on which a drawn chat line still carried a raw translation key.
    ///
    /// Not redundant with the row above: that one would stay green if the
    /// outer template resolved and an inner one did not, and this names all
    /// three keys of the same scene, so a break at ANY level turns it red.
    /// It must stay at zero.
    pub(super) unresolved_key_frames: u64,
    /// M105 — frames on which the book drew its `x/y` page counter.
    ///
    /// A LABEL, not a quad, so `book_quads_max` cannot see it: the counter goes
    /// through the text pass. It needs a book of more than one page, which
    /// needs more unlocked recipes than a fresh player has — hence the caller
    /// requirement, the same shape as r14's hotbar staging.
    pub(super) book_page_label_frames: u64,
    /// M104 — and the most it drew on a frame where one WAS open.
    ///
    /// A pair rather than one max, because the claim is a DIFFERENCE: an
    /// overlay adds a nine-sliced panel and a button each. One max over the
    /// whole run could not tell an overlay that drew from one that did not,
    /// since the book alone already clears any absolute threshold.
    pub(super) book_overlay_quads_max: usize,
    /// The panel height the RENDERER was holding while a container was open.
    ///
    /// Read back from `WorldRenderer::container_panel_height`, not from the
    /// open menu's layout — the first cut asked the layout, which answers 168
    /// for a chest whether or not the panel builder returned one, so it could
    /// not tell a working container from one that had silently fallen back to
    /// the player's 166-tall panel. A value witness is only a value witness if
    /// it reads the value the draw used.
    pub(super) container_panel_h: Option<f32>,
    /// The most overlay sprites the renderer's panel carried on any frame
    /// (M92).
    ///
    /// The chest injected at 0.4 has none — a chest's screen paints one sheet
    /// and stops — so this stays 0 unless the second injection at 0.85 (a
    /// brewing stand, with its data slots set) reaches the overlay builder.
    /// `containershot` grades those overlays offscreen; this is the only check
    /// that says the WINDOWED client draws them, which is the gap M88 closed
    /// for the panel itself and M86 for nine features before that.
    pub(super) container_overlays_max: usize,
    /// Frames on which a container was drawn **before** the gate force-opened
    /// the inventory (M89).
    ///
    /// The witness for `open_screen` opening the client's screen. M87 decoded
    /// the packet and drew whatever menu was open, but nothing turned the
    /// screen on, so a chest recorded its menu and showed nothing unless the
    /// player independently pressed E. These frames can only exist if the
    /// packet did it.
    pub(super) container_self_opened_frames: u64,
    /// Passes actually constructed by the end of the run.
    pub(super) gui_items_ready: bool,
    pub(super) hand_ready: bool,
    pub(super) text_ready: bool,
    pub(super) container_ready: bool,
    pub(super) clouds_ready: bool,
    pub(super) weather_ready: bool,
    pub(super) particles_ready: bool,
    pub(super) border_ready: bool,
    pub(super) crumbling_ready: bool,
    /// The ring witnesses, for each of the two passes M86 names.
    ///
    /// `last` is the handle the previous frame bound; `orphans` counts the
    /// frames on which that handle was **no longer among the pass's live
    /// buffers** — i.e. the previous frame's vertex buffer was destroyed while
    /// that frame could still be reading it, which is the VUID stated as a
    /// property. `max_live` is the deepest the ring ever got.
    ///
    /// A first cut of these counted *distinct consecutive handles* instead, on
    /// the theory that a 1-slot ring would keep handing back the same address.
    /// It does not: measured against the 1-slot mutation, that version reported
    /// 3,097 distinct handles and zero repeats over 3,099 frames while
    /// validation logged 11,881 destroy-while-in-use errors. A driver mints a
    /// fresh `VkBuffer` even for an immediate free-and-recreate, so the changed
    /// handle was never evidence of anything.
    pub(super) gui_item_last: u64,
    pub(super) hand_last: u64,
    pub(super) gui_item_orphans: u64,
    pub(super) hand_orphans: u64,
    pub(super) gui_item_max_live: usize,
    pub(super) hand_max_live: usize,
    /// Last-seen rebuild counters, so a legitimate ring reset is not scored as
    /// a use-after-free.
    pub(super) gui_item_generation: u64,
    pub(super) hand_generation: u64,
    /// How many rebuilds happened, reported so a run where the exemption fired
    /// suspiciously often is visible rather than silently forgiven.
    pub(super) gui_item_rebuilds: u64,
    pub(super) hand_rebuilds: u64,
    /// Whether validation was actually on. Asserting "0 errors" is worthless
    /// without it — a run with the layer off reports 0 for free.
    pub(super) validation: bool,
}

/// M272 — r130/r131: whether `t` is `render_check.py`'s synthetic skin,
/// texel (x, y) = (4x, 4y, 0x80, 0xff).
pub(super) fn is_synthetic_skin(t: &rewo_data::held_items::HeldTexture) -> bool {
    t.w == 64
        && t.h == 64
        && [(0usize, 0usize), (8, 8), (40, 12), (63, 63)]
            .iter()
            .all(|&(x, y)| {
                let i = (y * 64 + x) * 4;
                t.rgba.get(i..i + 4) == Some(&[(4 * x) as u8, (4 * y) as u8, 0x80, 0xff][..])
            })
}

impl RenderCheck {
    /// Sample the ringed passes once per frame, after this frame's `set_*`
    /// calls and before the next one.
    pub(super) fn sample_rings(&mut self, wr: &WorldRenderer) {
        // A pass rebuild (`init_gui_items` / `init_hand`, on an atlas repack)
        // legitimately throws the whole ring away — `Pass::destroy` idles
        // first — so the frame after one is exempt, and the comparison
        // restarts from the new ring.
        let g_gen = wr.gui_item_generation();
        let g_rebuilt = g_gen != self.gui_item_generation;
        self.gui_item_rebuilds += u64::from(g_rebuilt);
        self.gui_item_generation = g_gen;
        let g_live = wr.gui_item_live_buffers();
        self.gui_item_max_live = self.gui_item_max_live.max(g_live.len());
        let g = wr.gui_item_vertex_buffer();
        if g != 0 {
            if !g_rebuilt && self.gui_item_last != 0 && !g_live.contains(&self.gui_item_last) {
                self.gui_item_orphans += 1;
            }
            self.gui_item_last = g;
        }
        let h_gen = wr.hand_generation();
        let h_rebuilt = h_gen != self.hand_generation;
        self.hand_rebuilds += u64::from(h_rebuilt);
        self.hand_generation = h_gen;
        let h_live = wr.hand_live_buffers();
        self.hand_max_live = self.hand_max_live.max(h_live.len());
        let h = wr.hand_vertex_buffer();
        if h != 0 {
            if !h_rebuilt && self.hand_last != 0 && !h_live.contains(&self.hand_last) {
                self.hand_orphans += 1;
            }
            self.hand_last = h;
        }
    }

    /// Print one row per witness; `true` if every one passed.
    /// Witnesses `live --render-check` declares (M160).
    ///
    /// **A shared, merge-silent resource, which is why it is declared at all.**
    /// See the seam block at the bottom of [`Self::report`]. Raise it in the
    /// same commit that adds a row, and take the next free id from
    /// `REWO_PLAN.md` §0.0's shared-resource allocation table rather than from
    /// "the highest one I can see" — that is how fifteen specs all chose r48.
    const EXPECTED_RENDER_CHECK_WITNESSES: usize = 197;

    pub(super) fn report(&self) -> bool {
        let vuids = rewo_gpu::validation_error_count();
        let mut rows: Vec<(&str, bool, String)> = Vec::new();
        let mut row = |name: &'static str, ok: bool, detail: String| rows.push((name, ok, detail));

        row(
            "r1 the run rendered frames",
            self.frames >= 60,
            format!("{} frames", self.frames),
        );
        row(
            "r2 the bake survives `resumed`",
            self.baked_frames == self.frames && self.frames > 0,
            format!("{} of {} frames", self.baked_frames, self.frames),
        );
        // Every frame **but the first**. `RainFog` is a stateful ease advanced
        // by `delta_ticks`, and frame 1's `dt` is 0 because there is no previous
        // frame to subtract from — so on that one frame the multiplier is still
        // exactly zero and `rain_fog_band` correctly returns the disabled
        // sentinel. The bound is derived from that, not fitted to the
        // measurement: it is 1, and a second frame of sentinel would fail.
        row(
            "r3 the rain-fog band is the bake's, not the None sentinel",
            self.fog_band_frames + 1 >= self.frames && self.frames > 0,
            format!("{} of {} frames", self.fog_band_frames, self.frames),
        );
        row(
            "r4 the GUI-item pass was built",
            self.gui_items_ready,
            format!("{}", self.gui_items_ready),
        );
        row(
            "r5 the GUI-item branch ran every frame",
            self.gui_item_frames == self.frames && self.frames > 0,
            format!("{} of {} frames", self.gui_item_frames, self.frames),
        );
        row(
            "r6 the hand pass was built",
            self.hand_ready,
            format!("{}", self.hand_ready),
        );
        row(
            "r7 the hand branch ran every frame",
            self.hand_frames == self.frames && self.frames > 0,
            format!("{} of {} frames", self.hand_frames, self.frames),
        );
        row(
            "r8 the weather branch ran every frame",
            self.weather_frames == self.frames && self.frames > 0,
            format!("{} of {} frames", self.weather_frames, self.frames),
        );
        // r9-r13 are **weaker than they look, and were measured to be**.
        //
        // Under the milestone's headline mutation — dropping the bake again, so
        // every branch below goes dead — all five of these still pass, because
        // these passes are constructed in `resumed` from the bake it still had
        // at that moment. They catch a pass that failed to build (a jar missing
        // a texture); they cannot catch a pass that is built and never fed.
        // The rows that die under that mutation are r2, r3, r5, r7, r8, r14,
        // r15 and r16. Do not read a green r9-r13 as "the clouds rendered".
        row(
            "r9 the cloud pass was built",
            self.clouds_ready,
            format!("{}", self.clouds_ready),
        );
        row(
            "r10 the precipitation pass was built",
            self.weather_ready,
            format!("{}", self.weather_ready),
        );
        row(
            "r11 the particle pass was built",
            self.particles_ready,
            format!("{}", self.particles_ready),
        );
        row(
            "r12 the world-border pass was built",
            self.border_ready,
            format!("{}", self.border_ready),
        );
        row(
            "r13 the block-breaking pass was built",
            self.crumbling_ready,
            format!("{}", self.crumbling_ready),
        );
        // The ring witnesses, stated as the property the VUID is about: a
        // buffer a frame bound must still exist on the next frame. `orphans`
        // counts violations directly; `max_live` proves the ring reached its
        // declared depth rather than merely never being caught out.
        //
        // The depth bar is `MAX_FRAMES_IN_FLIGHT + 1`, derived here from the
        // contract rather than read off `buf_ring_slots()` — comparing the ring
        // against its own declared length would be self-calibrating, passing at
        // 4 and at 1 alike. `buf_ring_slots()` appears only in the message, so
        // a disagreement between the two is visible.
        let required = rewo_gpu::MAX_FRAMES_IN_FLIGHT + 1;
        let slots = rewo_gpu::buf_ring_slots();
        row(
            "r14 the GUI-item ring keeps a bound buffer alive",
            self.gui_item_orphans == 0 && self.gui_item_max_live >= required,
            format!(
                "{} orphaned, {} live at peak, need {required}, declared {slots}, {} rebuilds",
                self.gui_item_orphans, self.gui_item_max_live, self.gui_item_rebuilds
            ),
        );
        row(
            "r15 the hand ring keeps a bound buffer alive",
            self.hand_orphans == 0 && self.hand_max_live >= required,
            format!(
                "{} orphaned, {} live at peak, need {required}, declared {slots}, {} rebuilds",
                self.hand_orphans, self.hand_max_live, self.hand_rebuilds
            ),
        );
        // The screen is the only door to `VelvetTextPass::sync_atlas`, this
        // milestone's ninth destroy-in-place. A quarter of the run is a floor
        // well under the half the gate opens for, so it fails on "never
        // reached" rather than on scheduling jitter.
        row(
            "r16 the inventory screen was drawn",
            // M222 — of the TIMED run's frames: the tail's length varies.
            self.timed_screen_frames * 4 >= self.timed_frames && self.timed_frames > 0,
            format!(
                "{} of {} timed frames",
                self.timed_screen_frames, self.timed_frames
            ),
        );
        // M88 — the gap M87 recorded and could not close from a headless gate.
        row(
            "r19 a container screen was drawn in the windowed client",
            self.container_frames > 0,
            format!("{} of {} frames", self.container_frames, self.frames),
        );
        // ...and that it was a CONTAINER's panel, not the player's. A fallback
        // to `PLAYER` would keep r19 green while drawing 176x166 geometry for a
        // 63-slot menu, which is the failure worth naming rather than the one
        // that is merely absent.
        // M94 — and that the recipe book itself was drawn. A shut book carries
        // no quads at all, so this is 0 unless the book's builder ran.
        //
        // The minimum is the panel, the book's OWN tab count and the filter
        // toggle, with no unlocked recipes to fill the grid. Read from
        // `CRAFTING_TABS` rather than written as a literal: M95 corrected that
        // count from four to five, and a literal would have been quietly
        // generous by one from then on.
        let min_quads = 1 + rewo_ui::recipe_book_screen::CRAFTING_TABS.len() + 1;
        row(
            "r23 the recipe book was drawn in the windowed client",
            self.book_quads_max >= min_quads,
            format!(
                "{} quads at peak (panel + {} crafting tabs + filter = {min_quads} with an empty grid)",
                self.book_quads_max,
                rewo_ui::recipe_book_screen::CRAFTING_TABS.len()
            ),
        );
        // M187 — the recipe-book button reached the windowed draw on the
        // crafting table injected at 0.90. The position is vanilla's literal
        // (`CraftingScreen`: `leftPos + 5`, `height / 2 - 49` = 34 below
        // `topPos`), not the builder's constant, so a wrong origin fails here.
        row(
            "r68 the crafting table drew its recipe-book button in the windowed client",
            self.book_button_frames >= 3,
            format!(
                "{} frames carried `recipe_book/button` at panel (5, 34)",
                self.book_button_frames
            ),
        );
        // M192 — the held map, injected into the off-hand at 0.2 with its
        // data (one `player` decoration), drawn by the windowed hand pass
        // until the 0.5 inventory force-open suppresses the hand.
        row(
            "r69 the off-hand map drew with its data and decoration in the windowed client",
            self.held_map_frames >= 3,
            format!(
                "{} frames drew map {HELD_MAP_WITNESS_ID} with data and 1 decoration",
                self.held_map_frames
            ),
        );
        // M194 — the item frame injected at 0.2 (`add_entity` with facing 3
        // in its data, then slot 9 ITEM_STACK and slot 10 INT through the
        // production metadata route) reached the windowed collector as its
        // frame model and its sword at FIXED, and fell through to no capsule.
        row(
            "r70 the injected item frame drew its model and sword in the windowed client",
            self.item_frame_frames >= 3,
            format!(
                "{} frames carried the frame and its sword at {:?}",
                self.item_frame_frames, self.item_frame_pos
            ),
        );
        // M283 — r141: r83's bed: while the bot sleeps the screen fades to
        // `(int)(220 · counter / 100)` alpha (`extractSleepOverlay`); it must
        // draw and reach its 220 cap. No fade drew until M283.
        row(
            "r141 sleeping faded the screen to the sleep overlay",
            self.sleep_fade_frames >= 1 && self.sleep_fade_peak == 220,
            format!(
                "{} frames drew it, peak alpha {}",
                self.sleep_fade_frames, self.sleep_fade_peak
            ),
        );
        // M343 — r197: `render_check.py` summons a tame cat with a blue
        // collar; it must draw with collar 11 (index 23 INT on the cat, after
        // its owner at 19; `mobtexshot m68`).
        row(
            "r197 a tame cat drew wearing its blue collar",
            self.cat_collar,
            format!("tame cat with collar 11 seen: {}", self.cat_collar),
        );
        // M342 — r196: `render_check.py` summons a tame wolf with a blue
        // collar; it must draw with collar 11 (the server's index 21 INT,
        // routed on the wolf, and its tame flag; `mobtexshot m67`).
        row(
            "r196 a tame wolf drew wearing its blue collar",
            self.blue_collar,
            format!("tame wolf with collar 11 seen: {}", self.blue_collar),
        );
        // M341 — r195: `render_check.py` summons an ashen puppy; it must draw
        // as `BabyWolfModel` on a coat variant, the server's registry entry's
        // `baby_assets` (`mobtexshot m66`).
        row(
            "r195 an ashen puppy drew as BabyWolfModel on its coat's baby sheet",
            self.puppy.0 && self.puppy.1,
            format!(
                "puppy seen: {}, on a coat variant: {}",
                self.puppy.0, self.puppy.1
            ),
        );
        // M340 — r194: `render_check.py` summons a black kitten; it must draw
        // as `BabyCatModel` on a coat variant, the server's registry entry's
        // `baby_asset_id` (`mobtexshot m65`).
        row(
            "r194 a black kitten drew as BabyCatModel on its coat's baby sheet",
            self.kitten.0 && self.kitten.1,
            format!(
                "kitten seen: {}, on a coat variant: {}",
                self.kitten.0, self.kitten.1
            ),
        );
        // M339 — r193: `render_check.py` summons a black horse foal; it must
        // draw as `BabyHorseModel` on a coat variant (`mobtexshot m64`).
        row(
            "r193 a black horse foal drew as BabyHorseModel on its coat's baby sheet",
            self.horse_foal.0 && self.horse_foal.1,
            format!(
                "horse foal seen: {}, on a coat variant: {}",
                self.horse_foal.0, self.horse_foal.1
            ),
        );
        // M338 — r192: `render_check.py` summons a gray baby llama; it must
        // draw as `BabyLlamaModel` on a coat variant, not the creamy base
        // (`mobtexshot m63` grades the sheet).
        row(
            "r192 a gray baby llama drew as BabyLlamaModel on its coat's baby sheet",
            self.baby_llama.0 && self.baby_llama.1,
            format!(
                "baby llama seen: {}, on a coat variant: {}",
                self.baby_llama.0, self.baby_llama.1
            ),
        );
        // M337 — r191: `render_check.py` summons a goat kid; it must draw as
        // `BabyGoatModel` (`mobtexshot m62`).
        row(
            "r191 a goat kid drew as BabyGoatModel",
            self.goat_kid,
            format!("goat kid seen: {}", self.goat_kid),
        );
        // M336 — r190: `render_check.py` summons a brown rabbit kit; it must
        // draw as `BabyRabbitModel` (`mobtexshot m61`).
        row(
            "r190 a rabbit kit drew as BabyRabbitModel",
            self.rabbit_kit,
            format!("rabbit kit seen: {}", self.rabbit_kit),
        );
        // M335 — r189: `render_check.py` summons a camel calf; it must draw
        // as `BabyCamelModel` (`mobtexshot m60`).
        row(
            "r189 a camel calf drew as BabyCamelModel",
            self.camel_calf,
            format!("camel calf seen: {}", self.camel_calf),
        );
        // M334 — r188: `render_check.py` summons a donkey foal and a mule
        // foal; both must draw as `BabyDonkeyModel` (`mobtexshot m59`).
        row(
            "r188 a donkey foal and a mule foal drew as BabyDonkeyModel",
            self.foals.0 && self.foals.1,
            format!(
                "donkey foal seen: {}, mule foal seen: {}",
                self.foals.0, self.foals.1
            ),
        );
        // M333 — r187: `render_check.py` summons a baby bee; it must draw as
        // `BabyBeeModel` (`mobtexshot m58`).
        row(
            "r187 a baby bee drew as BabyBeeModel",
            self.baby_bee,
            format!("baby bee seen: {}", self.baby_bee),
        );
        // M332 — r186: `render_check.py` summons an ocelot kitten; it must
        // draw as `BabyOcelotModel` (`mobtexshot m57`).
        row(
            "r186 an ocelot kitten drew as BabyOcelotModel",
            self.ocelot_kitten,
            format!("ocelot kitten seen: {}", self.ocelot_kitten),
        );
        // M331 — r185: `render_check.py` summons a panda cub; it must draw
        // as `BabyPandaModel` (`mobtexshot m56`).
        row(
            "r185 a panda cub drew as BabyPandaModel",
            self.panda_cub,
            format!("panda cub seen: {}", self.panda_cub),
        );
        // M330 — r184: `render_check.py` summons a kit; it must draw as
        // `BabyFoxModel` (`mobtexshot m55`).
        row(
            "r184 a kit drew as BabyFoxModel",
            self.kit,
            format!("kit seen: {}", self.kit),
        );
        // M328 — r183: `render_check.py` summons a baby strider; it must
        // draw as `BabyStriderModel` (`mobtexshot m54`).
        row(
            "r183 a baby strider drew as BabyStriderModel",
            self.baby_strider,
            format!("baby strider seen: {}", self.baby_strider),
        );
        // M327 — r182: `render_check.py` summons a hoglet (immune to
        // zombification) and a baby zoglin; both must draw as
        // `BabyHoglinModel` (`mobtexshot m53`).
        row(
            "r182 a hoglet and a baby zoglin drew as BabyHoglinModel",
            self.hoglets.0 && self.hoglets.1,
            format!(
                "hoglet seen: {}, baby zoglin seen: {}",
                self.hoglets.0, self.hoglets.1
            ),
        );
        // M326 — r181: `render_check.py` summons a baby squid and a baby glow
        // squid; both must draw as `BabySquidModel` (`mobtexshot m52`).
        row(
            "r181 a baby squid and a baby glow squid drew as BabySquidModel",
            self.baby_squids.0 && self.baby_squids.1,
            format!(
                "baby squid seen: {}, baby glow squid seen: {}",
                self.baby_squids.0, self.baby_squids.1
            ),
        );
        // M325 — r180: `render_check.py` summons a helmeted baby zombie
        // villager; it must draw as `BabyZombieVillagerModel` (`mobtexshot
        // m51`).
        row(
            "r180 a baby zombie villager drew as BabyZombieVillagerModel",
            self.baby_zombie_villager,
            format!("baby zombie villager seen: {}", self.baby_zombie_villager),
        );
        // M324 — r179: `render_check.py` summons a dolphin calf; it must draw
        // as `BabyDolphinModel` (`mobtexshot m50`).
        row(
            "r179 a dolphin calf drew as BabyDolphinModel",
            self.dolphin_calf,
            format!("dolphin calf seen: {}", self.dolphin_calf),
        );
        // M323 — r178: `render_check.py` summons a red lamb; it must draw as
        // `BabySheepModel` (`mobtexshot m49` grades the fleece).
        row(
            "r178 a lamb drew as BabySheepModel",
            self.lamb,
            format!("lamb seen: {}", self.lamb),
        );
        // M322 — r177: `render_check.py` summons a piglet and a chick; they
        // must draw as `BabyPigModel` and `BabyChickenModel` (`mobtexshot
        // m48`).
        row(
            "r177 a piglet and a chick drew as their baby models",
            self.piglet_chick.0 && self.piglet_chick.1,
            format!(
                "piglet seen: {}, chick seen: {}",
                self.piglet_chick.0, self.piglet_chick.1
            ),
        );
        // M321 — r176: `render_check.py` summons a calf and a mooshroom calf;
        // both must draw as `BabyCowModel` (`mobtexshot m47`).
        row(
            "r176 a calf and a mooshroom calf drew as BabyCowModel",
            self.calves.0 && self.calves.1,
            format!(
                "calf seen: {}, mooshroom calf seen: {}",
                self.calves.0, self.calves.1
            ),
        );
        // M320 — r175: `render_check.py` summons a baby piglin and a baby
        // zombified piglin; both must draw as `BabyPiglinModel` (`mobtexshot
        // m46`).
        row(
            "r175 a baby piglin and a baby zombified piglin drew as BabyPiglinModel",
            self.baby_piglins.0 && self.baby_piglins.1,
            format!(
                "baby piglin seen: {}, baby zombified piglin seen: {}",
                self.baby_piglins.0, self.baby_piglins.1
            ),
        );
        // M319 — r174: `render_check.py` summons a baby drowned (helmeted
        // against the sun); it must draw as `BabyDrownedModel` (`mobtexshot
        // m45`). Until M319 it was the adult mesh wearing the baby sheet's UV
        // offset.
        row(
            "r174 a baby drowned drew as its own model",
            self.baby_drowned_frames >= 1,
            format!("{} frames drew it", self.baby_drowned_frames),
        );
        // M318 — r173: `render_check.py` summons a baby villager; it must draw
        // as `BabyVillagerModel` (`mobtexshot m44`). Until M318 it was the
        // adult mesh wearing the baby sheet's UV offset.
        row(
            "r173 a baby villager drew as its own model",
            self.baby_villager_frames >= 1,
            format!("{} frames drew it", self.baby_villager_frames),
        );
        // M317 — r172: `render_check.py` summons a baby zombie (helmeted, so
        // the sun does not burn it) and a baby husk; both must draw as
        // `BabyZombieModel`. Until M317 they drew as the adult mesh wearing
        // the baby sheet's UV offset, which scrambled it.
        row(
            "r172 a baby zombie and a baby husk drew as BabyZombieModel",
            self.baby_zombies.0 && self.baby_zombies.1,
            format!(
                "baby zombie seen: {}, baby husk seen: {}",
                self.baby_zombies.0, self.baby_zombies.1
            ),
        );
        // M316 — r171: `render_check.py` summons a polar bear cub; it must
        // draw as `PolarBearBaby` (`mobtexshot m43` checks the sheet and
        // shape). Until M316 it drew as the adult at half size.
        row(
            "r171 a polar bear cub drew as its own model",
            self.polar_cub_frames >= 1,
            format!("{} frames drew it", self.polar_cub_frames),
        );
        // M315 — r170: `render_check.py` summons a baby turtle; it must draw
        // as `TurtleBaby` (`mobtexshot m42` checks its size and sheet).
        // Until M315 it drew as the adult at half size.
        row(
            "r170 a baby turtle drew as its own model",
            self.baby_turtle_frames >= 1,
            format!("{} frames drew it", self.baby_turtle_frames),
        );
        // M314 — r169: `render_check.py` summons a baby happy ghast; its
        // synched baby flag must pick the ghastling's own model
        // (`mobtexshot m41` checks its size and sheet). Until M314 it drew as
        // the adult at half size.
        row(
            "r169 a baby happy ghast drew as the ghastling",
            self.ghastling_frames >= 1,
            format!("{} frames drew it", self.ghastling_frames),
        );
        // M312 — r168: r72's injected happy ghast (the horse leashed to it)
        // wears an injected white harness; it must draw as a leash holder in
        // its harness, which shows `RopesLayer`'s ropes (`mobtexshot m40`
        // checks the pixels). No ropes drew until M312.
        row(
            "r168 a harnessed happy ghast holding a leash drew its ropes",
            self.ghast_rope_frames >= 1,
            format!("{} frames drew it", self.ghast_rope_frames),
        );
        // M311 — r167: `render_check.py` summons a wolf whose armour has 60 of
        // its 64 durability spent; the stack's damage must reach the draw as
        // the high crack sheet (`mobtexshot m39` checks the pixels). No crack
        // drew until M311.
        row(
            "r167 a wolf drew its damaged armour's cracks",
            self.wolf_crack_frames >= 1,
            format!("{} frames drew them", self.wolf_crack_frames),
        );
        // M310 — r166: `render_check.py` summons a mannequin with
        // `hidden_layers:["hat","jacket"]`; its synched customisation mask
        // (index 16, now read for mannequins too) must reach the draw as
        // exactly those two layers hidden. Until M310 every layer drew.
        row(
            "r166 a mannequin drew with its hat and jacket hidden",
            self.hidden_layer_frames >= 1,
            format!("{} frames drew it", self.hidden_layer_frames),
        );
        // M309 — r165: `render_check.py` summons a mannequin whose profile
        // names RewoDummyA (no such account, so nothing can resolve it): its
        // offline UUID's default is index 12, wide Kai, so it must draw as the
        // wide `Player` model in an uploaded skin. Until M309 its profile was
        // not read (the parse stopped at it) and it wore the empty profile's
        // slim Alex.
        row(
            "r165 a named mannequin drew in its profile's default skin (wide Kai)",
            self.named_mannequin_frames >= 1,
            format!("{} frames drew it", self.named_mannequin_frames),
        );
        // M308 — r164: `render_check.py` summons a mannequin. Its empty
        // profile's id is the nil UUID, so `DefaultPlayerSkin.get` gives index
        // 0, slim Alex: it must draw as `PlayerSlim` with a skin uploaded from
        // the jar (it drew as a capsule until M308, and no skinless profile
        // wore anything but wide Steve).
        row(
            "r164 a mannequin drew in its default skin (slim Alex)",
            self.mannequin_frames >= 1,
            format!("{} frames drew it", self.mannequin_frames),
        );
        // M307 — r163: `render_check.py` summons an adult sulfur cube and a
        // baby; they must draw as `SulfurCube` and `SulfurCubeSmall` (a capsule
        // until M307; the baby's model is chosen from the synched baby flag).
        row(
            "r163 an adult and a baby sulfur cube drew as their two models",
            self.sulfur_cubes.0 && self.sulfur_cubes.1,
            format!(
                "adult seen: {}, baby (small model) seen: {}",
                self.sulfur_cubes.0, self.sulfur_cubes.1
            ),
        );
        // M306 — r162: `render_check.py` summons a slime; its shell must reach
        // the translucent range (`mobtexshot m36` checks it blends). Until
        // M306 the shell was opaque and the core undrawn.
        row(
            "r162 a slime drew its translucent shell",
            self.slime_shell_frames >= 1,
            format!("{} frames drew it", self.slime_shell_frames),
        );
        // M305 — r161: `render_check.py` summons magma cubes of size 1
        // (`Size:0`, which the server never sends: the default) and size 4;
        // both must draw at their own scale. Until M305 the size was read at
        // index 16, where 26.2 puts the baby flag, so every cube mob drew at
        // size 2.
        row(
            "r161 magma cubes drew at their own sizes (1 and 4)",
            self.cube_sizes.0 && self.cube_sizes.1,
            format!(
                "size 1 (scale 0.5) seen: {}, size 4 (scale 2.0) seen: {}",
                self.cube_sizes.0, self.cube_sizes.1
            ),
        );
        // M304 — r160: `render_check.py` summons a giant; it must draw as
        // `Giant` (a capsule until M304; `mobtexshot m35` checks its sheet
        // and size).
        row(
            "r160 a giant drew as a giant",
            self.giant_frames >= 1,
            format!("{} frames drew it", self.giant_frames),
        );
        // M303 — r159: `render_check.py` summons a saddled camel husk; it
        // must draw as `CamelHusk` (it was a capsule until M303) with
        // `camel_husk_saddle/saddle` resident (`mobtexshot m34` checks the
        // pixels).
        row(
            "r159 a saddled camel husk drew as a camel husk",
            self.camel_husk_frames >= 1,
            format!("{} frames drew it", self.camel_husk_frames),
        );
        // M302 — r158: `render_check.py` summons a happy ghast in a white
        // harness; the body slot must reach the draw as
        // `happy_ghast_body/white_harness`, resident (`mobtexshot m33` checks
        // the pixels and the goggles). No harness drew until M302.
        row(
            "r158 a happy ghast drew in its harness",
            self.harness_frames >= 1,
            format!("{} frames drew it", self.harness_frames),
        );
        // M301 — r157: r90's nautilus is saddled (its staging); its saddle slot
        // must reach the draw as `nautilus_saddle/saddle`, resident
        // (`mobtexshot m32` checks the pixels, the armour's too). No nautilus
        // saddle drew until M301.
        row(
            "r157 the saddled nautilus drew its saddle",
            self.nautilus_saddle_frames >= 1,
            format!("{} frames drew it", self.nautilus_saddle_frames),
        );
        // M300 — r156: r89's camel is saddled (its staging) and the bot rides
        // it in the tail. Its saddle slot must reach the draw as
        // `camel_saddle/saddle`, resident, with the reins while ridden
        // (`mobtexshot m31` checks the pixels). No camel saddle drew until
        // M300.
        row(
            "r156 a saddled camel drew its saddle, and its reins while ridden",
            self.camel_saddle_frames.0 >= 1 && self.camel_saddle_frames.1 >= 1,
            format!(
                "{} frames drew it saddled, {} of them ridden",
                self.camel_saddle_frames.0, self.camel_saddle_frames.1
            ),
        );
        // M299 — r155: `render_check.py` summons a wolf in wolf armour; the
        // body slot must reach the draw as `wolf_body/armadillo_scute`,
        // resident (`mobtexshot m30` checks the pixels). No wolf armour drew
        // until M299.
        row(
            "r155 a wolf drew in its armour",
            self.wolf_armor_frames >= 1,
            format!("{} frames drew it", self.wolf_armor_frames),
        );
        // M298 — r154: `render_check.py` summons a saddled pig and a saddled
        // strider; each saddle slot must reach the draw as its own layer, the
        // sheet resident (`mobtexshot m29` checks the pixels). Neither saddle
        // drew until M298.
        row(
            "r154 a saddled pig and a saddled strider drew their saddles",
            self.small_saddle_frames.iter().all(|&n| n >= 1),
            format!(
                "{} frames drew the pig's, {} the strider's",
                self.small_saddle_frames[0], self.small_saddle_frames[1]
            ),
        );
        // M297 — r153: `render_check.py` summons a chested llama in a red
        // carpet. `DATA_ID_CHEST` and the body slot must reach the draw, the
        // `llama_body/red` sheet resident (`mobtexshot m28` checks the
        // pixels). No llama carpet drew until M297.
        row(
            "r153 a chested llama drew in its red carpet",
            self.llama_decor_frames >= 1,
            format!("{} frames drew it", self.llama_decor_frames),
        );
        // M296 — r152: r84's donkey is staged `ChestedHorse:1b`; the server's
        // `DATA_ID_CHEST` must reach the draw (`mobtexshot m27` checks the
        // pixels). No donkey showed its chests until M296.
        row(
            "r152 the chested donkey drew with its chests",
            self.chested_donkey_frames >= 1,
            format!("{} frames drew them", self.chested_donkey_frames),
        );
        // M295 — r151: r88's horse wears a saddle (its staging) and the bot
        // rides it in the tail. The saddle slot must reach the draw as the
        // `horse_saddle/saddle` layer, resident, and the reins show while it
        // is ridden (`mobtexshot m26` checks the pixels). No saddle drew until
        // M295.
        row(
            "r151 a saddled horse drew its saddle, and its reins while ridden",
            self.saddle_frames.0 >= 1 && self.saddle_frames.1 >= 1,
            format!(
                "{} frames drew it saddled, {} of them ridden",
                self.saddle_frames.0, self.saddle_frames.1
            ),
        );
        // M294 — r150: `render_check.py` summons a horse wearing iron horse
        // armour. The equipment packet's body slot must reach the draw as the
        // `horse_body/iron` layer, its sheet resident (`mobtexshot m25`
        // checks the pixels). No horse armour drew until M294.
        row(
            "r150 a horse drew in its iron horse armour",
            self.horse_armor_frames >= 1,
            format!("{} frames drew it", self.horse_armor_frames),
        );
        // M292 — r149: r84's donkey screen draws the donkey in its window
        // (`AbstractMountInventoryScreen` poses the mount there), read back
        // as the preview's model kind against the mount's entity type. The
        // window stayed empty until M292; r86 still holds that no container
        // screen draws the player.
        row(
            "r149 the donkey's screen drew the donkey in its window",
            self.mount_preview_frames >= 1,
            format!("{} frames posed the mount", self.mount_preview_frames),
        );
        // M290 — r148: `render_check.py` summons an armour stand holding a
        // player head with r130's skin in its main hand. Its held item must
        // draw in the per-skin model; a held head drew Steve until M290.
        row(
            "r148 an armour stand's held player head drew in its own skin",
            self.held_skin_head_frames >= 1,
            format!("{} frames drew it", self.held_skin_head_frames),
        );
        // M289 — r147: `render_check.py` drops a player head item and frames
        // another, both with r130's skin in their profile. Each must draw in a
        // per-skin item model that samples the synthetic skin (the model
        // M275's icons use); both drew Steve until M289.
        row(
            "r147 a dropped and a framed player head drew in their own skin",
            self.dropped_skin_head_frames >= 1 && self.framed_skin_head_frames >= 1,
            format!(
                "{} frames drew the dropped head skinned, {} the framed one",
                self.dropped_skin_head_frames, self.framed_skin_head_frames
            ),
        );
        // M288 — r146: `render_check.py` summons a NoAI wither with `Invul`
        // 220 (NoAI stops `customServerAiStep`, so it never counts down or
        // explodes). The client reads `DATA_ID_INV` and draws it in
        // `wither_invulnerable.png` at `(2 - 219.x / 220 · 0.5) / 2` of its
        // size. Both were missing until M288.
        row(
            "r146 an invulnerable wither drew shrunk in its invulnerable sheet",
            self.invulnerable_wither_frames >= 1,
            format!("{} frames drew it", self.invulnerable_wither_frames),
        );
        // M287 — r145: `render_check.py` summons a NoAI wither at 100 of its
        // 300 health. The client's `isPowered` (health <= max / 2, the max
        // from the attribute) must hold and its armour swirl draw on the
        // wither's own sheet. No wither swirl drew until M287.
        row(
            "r145 a wither at half health drew its armour swirl",
            self.powered_wither_frames >= 1 && self.wither_swirl_frames >= 1,
            format!(
                "{} frames drew it powered, {} with swirl geometry",
                self.powered_wither_frames, self.wither_swirl_frames
            ),
        );
        // M286 — r144: `render_check.py` summons a NoAI breeze. Its wind
        // funnel must draw as its own translucent layer (`mobtexshot m22`
        // checks the blend); it was opaque body geometry until M286.
        row(
            "r144 a breeze drew its wind as its own layer",
            self.breeze_frames >= 1 && self.wind_frames >= 1,
            format!(
                "{} frames drew the breeze, {} with wind geometry",
                self.breeze_frames, self.wind_frames
            ),
        );
        // M285 — r143: `render_check.py` summons a charged creeper
        // (`powered:1b`). The server's `DATA_IS_POWERED` must reach the draw
        // and the pass must fill its swirl range (`mobtexshot m21` checks the
        // pixels). No swirl drew until M285.
        row(
            "r143 a charged creeper drew its energy swirl",
            self.powered_creeper_frames >= 1 && self.swirl_frames >= 1,
            format!(
                "{} frames drew it powered, {} with swirl geometry",
                self.powered_creeper_frames, self.swirl_frames
            ),
        );
        // M284 — r142: r83's bed (`red_bed[facing=west]`, staged in
        // render_check.py) lays the camera down: yaw `toYRot(west) - 180 =
        // -90`, pitch 0, the eye `0.2 + 0.15` above the feet, and no hand while
        // asleep. The player's own yaw must differ, or the yaw proves nothing.
        // The camera stood at the player's eye with the hand drawn until M284.
        let cam_ok = self.sleep_camera.is_some_and(|(yaw, pitch, dy, own)| {
            let wrap = |a: f32| (a + 540.0).rem_euclid(360.0) - 180.0;
            wrap(yaw + 90.0).abs() < 1e-3
                && pitch == 0.0
                && (dy - 0.35).abs() < 1e-6
                && wrap(own - yaw).abs() > 1.0
        });
        row(
            "r142 sleeping laid the camera in the bed and hid the hand",
            cam_ok
                && self.asleep_hand_frames.0 >= 1
                && self.asleep_hand_frames.1 == 0
                && self.awake_hand_frames >= 1,
            format!(
                "camera (yaw, pitch, eye dy, own yaw) {:?}; asleep frames {} with a hand {}; awake hand frames {}",
                self.sleep_camera,
                self.asleep_hand_frames.0,
                self.asleep_hand_frames.1,
                self.awake_hand_frames
            ),
        );
        // M282 — r140: the HUD vignette is drawn every frame (the option is
        // on by default) and follows the light: it starts at 1 and, in the
        // render check's daylight, must fall toward 0 a hundredth of the gap a
        // tick (`mobshot` checks its pixels). There was no vignette until M282.
        let low = self.vignette_low.unwrap_or(1.0);
        row(
            "r140 the HUD vignette drew and followed the daylight down",
            self.vignette_low.is_some() && low < 0.8,
            format!("lowest brightness drawn {low:.3} (started at 1)"),
        );
        // M281 — r139: an armour stand set burning (`Fire` 32767): the server
        // sets shared flag 0, and the client must draw its flames
        // (`mobtexshot m20` checks their pixels). Nothing burned visibly until
        // M281.
        row(
            "r139 a burning armour stand drew its flames",
            self.burning_frames >= 1,
            format!("{} frames drew them", self.burning_frames),
        );
        // M279 — r138: an invisible armour stand with the `Glowing` tag: its
        // body hidden, its outline drawn (`mobshot` checks that only the rim
        // shows). Invisible glowing entities drew nothing until M279.
        row(
            "r138 an invisible glowing armour stand drew as its outline alone",
            self.ghost_outline_frames >= 1,
            format!("{} frames drew it", self.ghost_outline_frames),
        );
        // M278 — r137: `render_check.py` summons an armour stand with the
        // `Glowing` tag; the client must read shared flag 6, give the draw
        // its outline colour (white: it has no team) and run the outline
        // pass (`mobshot` checks the outline's pixels). Nothing glowed until
        // M278.
        row(
            "r137 a glowing armour stand drew its outline",
            self.outline_frames >= 1,
            format!("{} frames drew it", self.outline_frames),
        );
        // M277 — r135: lit TNT flashes white for five ticks in every ten
        // (`TntRenderer.isLit`): r112's and r127's TNT must reach the draws
        // both white and plain. It never flashed until M277 (`hurtshot c6`
        // checks the white mix's pixels).
        row(
            "r135 primed TNT flashed white",
            self.tnt_lit_frames >= 1 && self.tnt_unlit_frames >= 1,
            format!(
                "{} lit and {} unlit TNT draws",
                self.tnt_lit_frames, self.tnt_unlit_frames
            ),
        );
        // M277 — r136: two seconds in, the render check ignites a staged
        // NoAI creeper (`Fuse` 32767, so the server never blows it up). The
        // client counts its swell (`swellDir` 1) and draws it swollen
        // (`CreeperRenderer.scale`) and flashing on its odd tenths. Creepers
        // never swelled or flashed until M277.
        row(
            "r136 an ignited creeper swelled and flashed white",
            self.creeper_swollen_frames >= 1 && self.creeper_flash_frames >= 1,
            format!(
                "{} frames swollen, {} of them white",
                self.creeper_swollen_frames, self.creeper_flash_frames
            ),
        );
        // M276 — r134: r103's enchanting table draws its book, every quad,
        // posed by the client's own clock: alone, vanilla's book keeps turning
        // (`tRot += 0.02` a tick), so its pose must change across frames. No
        // book was drawn until M276 (the opening is the tick's unit test and
        // the pose's lectern cross-check).
        let turn = self.enchant_book_turn.map_or(0.0, |(lo, hi)| hi - lo);
        row(
            "r134 an enchanting table drew its book, turning",
            self.enchant_book_frames >= 1 && turn > 0.01,
            format!(
                "{} frames drew it; turn range {turn:.3}",
                self.enchant_book_frames
            ),
        );
        // M275 — r133: `render_check.py` puts a player head ITEM whose
        // profile names r130's skin in the bot's inventory slot 26 (menu slot
        // 35). Its icon must resolve, through the production
        // `skinned_icon_model`, to a per-skin item model that the GUI pass
        // holds and that samples the synthetic skin; every head icon showed
        // Steve until M275.
        row(
            "r133 a player head item's icon wore its own skin",
            self.skinned_icon_frames >= 1,
            format!("{} frames resolved it", self.skinned_icon_frames),
        );
        // M274 — r132: `render_check.py` summons an armour stand wearing a
        // carved pumpkin, which has no equipment asset: it must draw through
        // `CustomHeadLayer`'s item branch (`mobtexshot m19` checks the
        // pixels). It drew nothing until M274.
        row(
            "r132 an armour stand drew wearing a carved pumpkin",
            self.worn_item_head_frames >= 1,
            format!("{} frames drew it", self.worn_item_head_frames),
        );
        // M273 — r131: `render_check.py` summons an armour stand wearing a
        // player head whose `minecraft:profile` component names r130's skin.
        // The stand must draw wearing it, in that skin; a worn skull drew
        // nothing until M273 (`mobtexshot m18` checks the worn pixels).
        row(
            "r131 an armour stand drew wearing a player head in its own skin",
            self.worn_skin_head_frames >= 1,
            format!("{} frames drew it", self.worn_skin_head_frames),
        );
        // M272 — r130: `render_check.py` places a player head whose profile
        // carries a `textures` property (served from a local file, a
        // synthetic skin). The head must draw every quad in a model of its
        // own whose texture holds that file's texels; every head wore
        // Steve until M272.
        row(
            "r130 a player head drew in its own skin",
            self.skin_head_frames >= 1,
            format!("{} frames drew it", self.skin_head_frames),
        );
        // M271 — r129: `render_check.py` summons the classic server hologram,
        // an invisible armour stand with a visible custom name. Its name must
        // draw and its body must not (`mobshot` checks the hidden body draws
        // no pixel); the whole stand drew under the name until M271.
        row(
            "r129 an invisible named armour stand drew its name, not its body",
            self.hologram_stand_frames >= 1,
            format!("{} frames drew it so", self.hologram_stand_frames),
        );
        // M270 — r128: `render_check.py` summons an armour stand posed with
        // its head at (45, 0, 0) and right arm at (-90, 0, 0); both poses
        // must reach its draw. Every stand stood at the default pose until
        // M270.
        row(
            "r128 a posed armour stand drew with its poses",
            self.stand_pose_frames >= 1,
            format!("{} frames drew it posed", self.stand_pose_frames),
        );
        // M269 — r127: `render_check.py` primes a TNT with a 60-tick fuse high
        // in the air. In its last ten ticks it swells (`getSwellAmount`); it
        // drew at rest until M269. (r112's TNT has a fuse of 32767 and never
        // swells.)
        row(
            "r127 a primed TNT drew swelling before it blew",
            self.tnt_swell_frames >= 1,
            format!("{} frames drew it swelling", self.tnt_swell_frames),
        );
        // M268 — r126: r114's text display has the shadow flag; its line's
        // quarter-colour copy must reach the world-text list behind it.
        row(
            "r126 a text display drew its drop shadow",
            self.text_display_shadow_frames >= 1,
            format!("{} frames drew it", self.text_display_shadow_frames),
        );
        // M267 — r125: r115's item display is summoned with
        // `brightness:{sky:0,block:0}`; it must draw at the lightmap's (0, 0)
        // light, which differs from the daylight where it hangs.
        row(
            "r125 a display drew at its brightness override's light",
            self.display_brightness_frames >= 1,
            format!(
                "{} frames drew it at the override",
                self.display_brightness_frames
            ),
        );
        // M266 — r124: three seconds in, the render check teleports r123's
        // display (summoned with `teleport_duration:20`) up a block. Vanilla
        // glides it over 20 ticks; Rewo's 3-tick lerp spent 2 ticks part-way,
        // and a display without a duration snaps. At least 10 distinct ticks
        // part-way.
        row(
            "r124 a display with a teleport duration glided to its new position",
            self.display_glide_ticks >= 10,
            format!("{} ticks part-way", self.display_glide_ticks),
        );
        // M265 — r123: two seconds in, the render check merges a 40-tick
        // scale-up (1 to 3) into a staged stone block display. Vanilla slides
        // between the two; Rewo jumped until M265. Some frame must draw it
        // strictly between.
        row(
            "r123 a block display's transformation interpolated between keyframes",
            self.display_interp_frames >= 1,
            format!("{} frames drew it part-way", self.display_interp_frames),
        );
        // M264 — r122: `render_check.py` powers the r109 bell with a redstone
        // block, which rings it (`BellBlock.attemptToRing`, a block event
        // with its facing). The ring must reach the bell's draw as a swing
        // away from rest; the bell hung still until M264.
        row(
            "r122 a rung bell drew swinging",
            self.bell_shake_frames >= 1,
            format!("{} frames drew it swinging", self.bell_shake_frames),
        );
        // M262 — r121: r114's text display has vanilla's default background
        // (`0x40000000`); its plate must reach the world-text list behind the
        // glyphs, 25% black. It drew no plate until M262.
        row(
            "r121 a text display drew its background plate",
            self.text_display_backdrop_frames >= 1,
            format!("{} frames drew it", self.text_display_backdrop_frames),
        );
        // M261 — r120: `render_check.py` summons a hopper minecart and a chest
        // minecart. Their contents (a hopper, a chest) were never drawn;
        // both must draw every quad, the hopper through a model built at run
        // time and the chest through its block-entity model at rest.
        row(
            "r120 hopper and chest minecarts drew what they carry, the hopper in its own textures",
            self.minecart_contents_frames >= 1,
            format!("{} frames drew both", self.minecart_contents_frames),
        );
        // M260 — r119: `render_check.py` summons a block display of an
        // east-facing oak stair, a state the bake never makes an entity model
        // for. The client builds it from the terrain quads at run time and
        // hands the GPU the new model and layers; it must draw every quad.
        row(
            "r119 a block display drew its state's run-time model, in its own texture",
            self.block_display_frames >= 1,
            format!("{} frames drew it", self.block_display_frames),
        );
        // M259 — r118: `render_check.py` summons evoker fangs with a 60-tick
        // warmup. They draw nothing until the server's entity event 4 starts
        // the bite, then about a second of rising jaws before they vanish;
        // they were a grey capsule the whole time until M259.
        row(
            "r118 evoker fangs drew their model once they bit",
            self.evoker_fangs_frames >= 1,
            format!("{} frames drew them", self.evoker_fangs_frames),
        );
        // M258 — r117: `render_check.py` summons a wither skull, a llama spit
        // and a shulker bullet, held still. Until M258 each was a grey
        // capsule; each must draw every quad of its model in one frame.
        row(
            "r117 a wither skull, llama spit and shulker bullet drew their models",
            self.small_projectile_frames >= 1,
            format!("{} frames drew all three", self.small_projectile_frames),
        );
        // M257 — r116: `render_check.py` summons an (invulnerable) end
        // crystal. Until M257 it was a grey capsule; it must draw every quad
        // of `rewo:be/end_crystal`, whose 128x64 sheet is halved to fit the
        // entity atlas's large band.
        row(
            "r116 an end crystal drew its model",
            self.end_crystal_frames >= 1,
            format!("{} frames drew it", self.end_crystal_frames),
        );
        // M256 — r115: `render_check.py` summons an item display holding an
        // amethyst shard. Its stack arrives as metadata index 23 (an
        // ITEM_STACK); the shard's model must draw every quad through the
        // block-entity emitter — not a capsule, not nothing.
        row(
            "r115 an item display drew its item's model",
            self.item_display_frames >= 1,
            format!("{} frames drew it", self.item_display_frames),
        );
        // M255 — r114: `render_check.py` summons a text display reading
        // `RewoHolo` in gold. Its text arrives as metadata index 23 (a
        // component), and must reach the world-text draw list as that line in
        // that colour — not a capsule, not white.
        row(
            "r114 a text display drew its gold line as world text",
            self.text_display_frames >= 1,
            format!("{} frames drew it", self.text_display_frames),
        );
        // M254 — r113: `render_check.py` hangs a kebab painting on a stone
        // wall. Its variant arrives as a raw `painting_variant` id (the
        // registry synced at configuration), its facing as spawn data; it
        // must draw its variant's model where it was a capsule.
        row(
            "r113 a hung painting drew its variant's model",
            self.painting_frames >= 1,
            format!("{} frames drew it completely", self.painting_frames),
        );
        // M253 — r112: `render_check.py` summons a floating falling block
        // (gravel) and primed TNT with a long fuse, both NoGravity. Each must
        // draw its block model (every quad) where it was a capsule.
        row(
            "r112 a falling block and primed TNT drew their block models",
            self.carried_block_frames >= 1,
            format!(
                "{} frames drew both completely (at most {} complete in a frame)",
                self.carried_block_frames, self.carried_blocks_max
            ),
        );
        // M252 — r111: `render_check.py` summons a minecart; the live wire
        // name must reach `MinecartModel` (mobshot grades its faces), not the
        // capsule every minecart was until M252.
        row(
            "r111 a summoned minecart drew as its model, not a capsule",
            self.minecart_frames >= 1,
            format!("{} frames drew it", self.minecart_frames),
        );
        // M250 — r110: the bow r99 draws is released at stage 51 with arrows
        // in the inventory, so the server shoots a real arrow. It must draw
        // as `ArrowModel` (every quad), not the capsule it was until M250.
        row(
            "r110 a shot arrow drew every quad of the arrow model in the windowed client",
            self.arrow_frames >= 1,
            format!(
                "{} frames drew it; (most drawn, model quads) {:?}",
                self.arrow_frames, self.arrow_quads
            ),
        );
        // M247 — r109: `render_check.py` stages a chest by the anvil. Its
        // draw must emit all of its model's quads: a block-entity sheet that
        // cannot page into the entity atlas emits none (every block entity
        // until M246's large-item band).
        row(
            "r109 a placed chest, bell and lectern book drew every quad of their models in the windowed client",
            self.chest_frames >= 1,
            format!(
                "{} frames drew them all; (drawn, model) per {:?}: {:?}",
                self.chest_frames, RC_BLOCK_ENTITY_MODELS, self.be_quads
            ),
        );
        // M241 — r108: r94's cast rod draws its line, and the line's far end
        // is the local first-person hand: vanilla's near-plane point scaled
        // by 960 / fov sits under a block from the eye.
        row(
            "r108 a cast fishing rod drew its line to the first-person hand",
            self.fishing_line_frames >= 1
                && self.fishing_line_hand > 0.0
                && self.fishing_line_hand < 1.0,
            format!(
                "{} frames drew a line, its hand end at most {:.3} blocks from the eye",
                self.fishing_line_frames, self.fishing_line_hand
            ),
        );
        // M240 — r107: r94's thrown snowball and its cast bobber reach the
        // windowed draw list as `ThrownItemRenderer` / `FishingHookRenderer`
        // draw them: the item (or the hook sprite) facing the camera, not
        // the capsule every non-living entity without a model used to get.
        row(
            "r107 a thrown snowball and a fishing bobber drew as camera-facing items, not capsules",
            self.thrown_frames.0 >= 1 && self.thrown_frames.1 >= 1,
            format!(
                "{} frames drew the snowball, {} the bobber",
                self.thrown_frames.0, self.thrown_frames.1
            ),
        );
        // M238 — r105: two armour stands `render_check.py` summons, drawn as
        // the armour stand model rather than the capsule, with their client
        // flags decoded from index 15 (kind-gated away from `Mob`'s flags):
        // a plain one (no arms, a base plate) and one with `ShowArms`,
        // `NoBasePlate` and an iron helmet on the armour layer.
        row(
            "r105 two armour stands drew as the model, one plain and one with arms, no base plate and a helmet",
            self.armor_stand_frames >= 3,
            format!(
                "{} frames carried both stands with their flags",
                self.armor_stand_frames
            ),
        );
        // M194 — the frame `render_check.py` summons on the real server:
        // facing north, rotation 3, a diamond sword. Seen with exactly those
        // values, slots 8 / 9 / 10 are the ones 26.2 sends.
        row(
            "r71 the server's item frame decoded its facing, rotation and item",
            self.server_frame_frames >= 3,
            format!(
                "{} frames saw a frame with facing 2, rotation 3, a diamond sword",
                self.server_frame_frames
            ),
        );
        // M185 — the quad leash: a horse leashed to a happy ghast, injected
        // at 0.28 beside r60's cow, reached the windowed collector's quad
        // branch (`supportQuadLeashAsHolder` on the ghast, the horse line's
        // shoulder points).
        row(
            "r72 a horse leashed to a happy ghast drew the four-rope quad leash",
            self.quad_leash_frames >= 3,
            format!(
                "{} frames drew {:?} as a quad leash",
                self.quad_leash_frames, QUAD_LEASH_EIDS
            ),
        );
        // M206 — options.txt's render distance reached the server: the
        // columns it sent are exactly `ChunkTrackingView`'s set for the
        // staged 6 (not for its own 8, which is larger), and the fog band is
        // `FogRenderer.setupFog`'s for 6.
        row(
            "r73 options.txt's render distance set the columns the server sent and the fog",
            self.view_distance_frames >= 3,
            format!(
                "{} frames matched; last (staged, sent, server radius, columns, \
                 tracking set, fog) = {:?}",
                self.view_distance_frames, self.view_distance_seen
            ),
        );
        // M179 — the advancement tab click's send, graded by the real
        // server: `opened_tab` for the tab the click hit came back as
        // `select_advancements_tab` with the same id
        // (`PlayerAdvancements.setSelectedTab` sends only on a change, and the
        // screen had opened on the OTHER tab, so the echo is the click's).
        row(
            "r74 an advancement tab click reached the server, which selected that tab",
            self.adv_echo_frames >= 3
                && self
                    .adv_click
                    .as_ref()
                    .is_some_and(|(opened, hit)| opened != hit),
            format!(
                "opened on / clicked {:?}; {} frames with the server's selection on the \
                 clicked tab{}",
                self.adv_click,
                self.adv_echo_frames,
                self.adv_click_skip
                    .as_ref()
                    .map(|s| format!(" (not clicked: {s})"))
                    .unwrap_or_default()
            ),
        );
        // M184 — the lectern against a real server: `render_check.py` places
        // one holding a two-page book; the production right-click
        // (`use_item_on`) opened its menu, the reader showed both pages, the
        // forward arrow's `container_button_click(2)` came back as
        // container-data page 1 (the client never turns a lectern locally),
        // and `close_book`'s `container_close` left no menu open.
        row(
            "r75 a lectern opened, the server turned its page on the forward click, and it closed",
            self.lectern_pages_seen == 2 && self.lectern_echo_frames >= 3 && self.lectern_closed,
            format!(
                "lectern at {:?}: reader showed {} page(s), {} frames on page 1 after the \
                 forward click, closed cleanly: {}{}",
                self.lectern_pos,
                self.lectern_pages_seen,
                self.lectern_echo_frames,
                self.lectern_closed,
                self.lectern_skip
                    .as_ref()
                    .map(|s| format!(" (stopped: {s})"))
                    .unwrap_or_default()
            ),
        );
        // M216 — the right-click on an entity, end to end: the bot faces the
        // librarian `render_check.py` summoned, `entity_click` sends 26.2's
        // `interact` (hand, hit location as LpVec3, shift), the server opens
        // the trade screen, and `merchant_offers` decodes to exactly the two
        // trades the summon wrote.
        row(
            "r76 a right-click on a villager opened its trades, decoded as summoned",
            self.trade_villager.is_some()
                && !self.trade_expected.is_empty()
                && self.trade_offers == self.trade_expected,
            format!(
                "villager {:?}; offers (cost, count, result, count, max uses) {:?} against \
                 {:?}{}",
                self.trade_villager,
                self.trade_offers,
                self.trade_expected,
                self.trade_skip
                    .as_ref()
                    .map(|s| format!(" (stopped: {s})"))
                    .unwrap_or_default()
            ),
        );
        // M216 — and the left-click: `entity_click` sends 26.2's
        // `ServerboundAttackPacket`, and the server's `damage_event` for the
        // same villager arms its hurt clock (it was unhurt before).
        row(
            "r77 a left-click on the villager was an attack the server answered with damage",
            self.attack_unhurt_before && self.attack_hurt_frames >= 3,
            format!(
                "villager {:?} unhurt before the click: {}; {} frames with its hurt clock armed after",
                self.trade_villager, self.attack_unhurt_before, self.attack_hurt_frames
            ),
        );
        // M217 — survival block breaking against the real server: in
        // survival, the press on `render_check.py`'s oak leaves (destroy time
        // 0.2, no tool needed: `1 / 0.2 / 30` a tick by hand, summed in f32 as
        // vanilla sums it) STARTs, the held key accumulates progress, STOP goes
        // on the tick the sum reaches 1, and the server turns the leaves to air
        // — which it does only for a STOP that arrives late enough
        // (`ServerPlayerGameMode.handleBlockBreakAction` checks the progress).
        row(
            "r78 a survival press broke the leaves: STOP on the vanilla tick, and the server agreed",
            self.mine_survival && self.mine_expected.is_some() && self.mine_ticks == self.mine_expected && self.mine_broken,
            format!(
                "leaves at {:?}, held {:?}: pressed in survival {}, STOP {:?} ticks after the press (vanilla {:?}), \
                 the leaves were gone (air, or the flood's water) {} [{}]{}",
                self.mine_pos,
                self.mine_held,
                self.mine_survival,
                self.mine_ticks,
                self.mine_expected,
                self.mine_broken,
                self.mine_diag.as_deref().unwrap_or("-"),
                self.mine_skip
                    .as_ref()
                    .map(|s| format!(" (stopped: {s})"))
                    .unwrap_or_default()
            ),
        );
        // M218 — the hand keys and the sprint state, each graded by what the
        // real server did with it (run while the bot sits on r72's injected
        // horse, which nothing moves).
        let skip = self
            .hands_skip
            .as_ref()
            .map(|s| format!(" (stopped: {s})"))
            .unwrap_or_default();
        row(
            "r79 Q dropped one item: the predicted stack lost one and the server spawned it",
            self.drop_counts.is_some_and(|(b, a)| a == b - 1) && self.drop_item_entity,
            format!(
                "dirt count before / after {:?}, an item entity near the bot {}{skip}",
                self.drop_counts, self.drop_item_entity
            ),
        );
        row(
            "r80 F swapped the hands: the server moved the stack, at the predicted count, to the off hand",
            self.swap_offhand.as_ref().is_some_and(|(n, c)| {
                n == "minecraft:dirt" && self.drop_counts.is_some_and(|(_, a)| *c == a)
            }),
            format!("off hand after F {:?}{skip}", self.swap_offhand),
        );
        row(
            "r81 the pick key on the lectern put a lectern in the hand (creative)",
            self.pick_held.as_deref() == Some("minecraft:lectern"),
            format!(
                "held after the pick {:?}, aimed at {}{skip}",
                self.pick_held,
                self.pick_aim.as_deref().unwrap_or("-")
            ),
        );
        row(
            "r82 the sprint state reached the server: its flag echo set then cleared",
            self.sprint_on && self.sprint_off && self.sprint_sent >= 2,
            format!(
                "server sprint flag after START {}, cleared after STOP {}, {} sprint command(s) sent, \
                 sprinting on r89's camel{}",
                self.sprint_on,
                self.sprint_off,
                self.sprint_sent,
                self.camel_skip.as_ref().map(|s| format!(" (stopped: {s})")).unwrap_or_default()
            ),
        );
        // M219 — the bed, against the real server: a right-click on the bed
        // at night sleeps (the server's SLEEPING_POS_ID moves the client onto
        // the bed and the in-bed chat opens into the empty slot), W moves
        // nothing while asleep, a click on Leave Bed sends STOP_SLEEPING, and
        // the server's cleared position closes the screen (`Gui.tick`).
        let at_bed = match (self.bed_head, self.bed_client_pos) {
            (Some((x, _, z)), Some(p)) => {
                (p[0] - (x as f64 + 0.5)).abs() < 1e-6 && (p[2] - (z as f64 + 0.5)).abs() < 1e-6
            }
            _ => false,
        };
        row(
            "r83 sleeping opened the in-bed chat; Leave Bed woke the player and closed it",
            self.bed_slept
                && at_bed
                && self.bed_drift < 1e-6
                && self.in_bed_frames > 0
                && self.bed_wake_sends >= 1
                && self.bed_woke
                && self.bed_screen_closed,
            format!(
                "bed {:?} (head {:?}): slept {}, client at {:?} (on the bed {at_bed}), drift with W held {:.4}, \
                 in-bed frames {}, wake-ups sent {}, woke {}, screen closed {}, bed clicks {}{}",
                self.bed_pos,
                self.bed_head,
                self.bed_slept,
                self.bed_client_pos,
                self.bed_drift,
                self.in_bed_frames,
                self.bed_wake_sends,
                self.bed_woke,
                self.bed_screen_closed,
                self.bed_clicks,
                self.bed_skip
                    .as_ref()
                    .map(|s| format!(" (stopped: {s})"))
                    .unwrap_or_default()
            ),
        );
        // M220 — the donkey's inventory against the real server, in the
        // run's tail. r84: a sneak-right-click (`isSecondaryUseActive` on a
        // tamed horse opens `openCustomInventoryScreen`) brought a
        // `mount_screen_open` the client turned into the donkey's menu: five
        // chest columns (2 + 15 + 36 slots), the saddle slot active and the
        // body slot not (a donkey is not in `can_wear_horse_armor`), the
        // summoned saddle in slot 0 and the summoned apples at chest index 7 —
        // row 1, column 2, menu slot 9 — and the panel drawn 166 tall with its
        // two overlays (the strip and the saddle slot's background).
        let mskip = self
            .mount_skip
            .as_ref()
            .map(|s| format!(" (stopped: {s})"))
            .unwrap_or_default();
        let first = self.mount_panel_frames.first().map(|f| f.0);
        let first_frames: Vec<&(u64, f32, usize)> = self
            .mount_panel_frames
            .iter()
            .filter(|f| Some(f.0) == first)
            .collect();
        let drawn =
            !first_frames.is_empty() && first_frames.iter().all(|f| f.1 == 166.0 && f.2 == 2);
        row(
            "r84 a sneak-right-click opened a real donkey's inventory: its layout, slots and panel",
            self.mount_columns == Some(5)
                && self.mount_slots == 53
                && self.mount_active == Some((true, false))
                && self.mount_saddle_item.as_deref() == Some("minecraft:saddle")
                && self.mount_chest_item == Some(("minecraft:apple".to_string(), 3))
                && drawn,
            format!(
                "donkey {:?} {:?}: columns {:?}, {} slots, (saddle, body) active {:?}, slot 0 {:?}, \
                 slot 9 {:?}, {} drawn frame(s) all 166 tall with 2 overlays {drawn}{mskip}",
                self.donkey,
                self.mount_title,
                self.mount_columns,
                self.mount_slots,
                self.mount_active,
                self.mount_saddle_item,
                self.mount_chest_item,
                first_frames.len(),
            ),
        );
        // r85: then a plain right-click rode it (`doPlayerRide`, the server's
        // `set_passengers`), and the inventory key while riding sent
        // `OPEN_INVENTORY` instead of opening the player's inventory — and the
        // server answered with a second, distinct mount menu that drew.
        let second_drawn = self.mount_second_serial.is_some_and(|s| {
            self.mount_panel_frames
                .iter()
                .any(|f| f.0 == s && f.1 == 166.0)
        });
        row(
            "r85 riding the donkey, the inventory key asked the server, which opened its inventory",
            self.mount_rode && self.mount_e_sent && second_drawn && self.mount_second_serial != first,
            format!(
                "rode {}, OPEN_INVENTORY sent {}, second menu {:?} (first {first:?}) drawn {second_drawn}{mskip}",
                self.mount_rode, self.mount_e_sent, self.mount_second_serial
            ),
        );
        // M221 — r87: the bot rides a real boat on a staged pool and steers
        // it with the production keys: the client simulates it (`AbstractBoat
        // .tick`), W moves it, A turns it, every tick sends `move_vehicle` and
        // `paddle_boat`, the server never rejects a move (`move_vehicle` back),
        // and once it has drifted to a stop the server's own position for it
        // (`/data get entity`) is the client's.
        let agree = match (self.boat_client_pos, self.boat_server_pos) {
            (Some(c), Some(s)) => (0..3).all(|i| (c[i] - s[i]).abs() < 0.1),
            _ => false,
        };
        row(
            "r87 the bot steered a real boat: it moved and turned, the server accepted every move and agrees where it is",
            self.boat_steered
                && self.boat_moved > 2.0
                && self.boat_turned < -5.0
                && self.boat_moves_sent >= 20
                && self.boat_paddles_sent >= 20
                && self.boat_rejections == 0
                && agree,
            format!(
                "boat {:?}: steered {}, moved {:.2} with W, turned {:.1} with A, {} move_vehicle and {} paddle_boat sent, \
                 {} rejection(s), client {:?} server {:?} (agree {agree}){}",
                self.boat,
                self.boat_steered,
                self.boat_moved,
                self.boat_turned,
                self.boat_moves_sent,
                self.boat_paddles_sent,
                self.boat_rejections,
                self.boat_client_pos,
                self.boat_server_pos,
                self.boat_skip.as_ref().map(|s| format!(" (stopped: {s})")).unwrap_or_default()
            ),
        );
        // M222 — r88: the bot rides a summoned, tamed, saddled horse (fixed
        // speed and jump strength) through the production right-click, holds
        // W, then holds and releases the jump key: the client simulates the
        // horse (`tickRidden` / `travel`) and fires `executeRidersJump` from
        // the meter's release; no move is rejected and the server's own
        // position for the horse, once it stands, is the client's.
        // M226: and W's last tick moved it vanilla's `0.98 · 0.3 / (1 - 0.6
        // · 0.91)` (the rider's modified `zza`; the raw key gave 0.661).
        let horse_step = 0.98f32 as f64 * 0.3f32 as f64 / (1.0 - (0.6f32 * 0.91f32) as f64);
        let hagree = match (self.horse_client_pos, self.horse_server_pos) {
            (Some(c), Some(s)) => (0..3).all(|i| (c[i] - s[i]).abs() < 0.1),
            _ => false,
        };
        row(
            "r88 the bot rode and jumped a real horse: the server accepted every move and agrees where it is",
            self.horse_steered
                && self.horse_moved > 2.0
                && (self.horse_step - horse_step).abs() < 1e-3
                && self.horse_jumps >= 1
                && self.horse_rise > 0.5
                && self.horse_moves_sent >= 20
                && self.horse_rejections == 0
                && hagree,
            format!(
                "horse {:?}: steered {}, moved {:.2} with W, last step {:.4} (vanilla {horse_step:.4}), \
                 {} jump(s) rising {:.2}, {} move_vehicle sent, {} rejection(s), client {:?} server {:?} \
                 (agree {hagree}){}",
                self.horse,
                self.horse_steered,
                self.horse_moved,
                self.horse_step,
                self.horse_jumps,
                self.horse_rise,
                self.horse_moves_sent,
                self.horse_rejections,
                self.horse_client_pos,
                self.horse_server_pos,
                self.horse_skip.as_ref().map(|s| format!(" (stopped: {s})")).unwrap_or_default()
            ),
        );
        // M225 — r89: the bot rides a summoned, saddled camel (vanilla's
        // speed 0.09 and jump 0.42) through the production right-click, holds
        // W with sprint (`getRiddenSpeed` adds 0.1, and the rider's `zza` is
        // the modified 0.98 — M226: a tick's distance settles at `0.98 · 0.19
        // / (1 - 0.6 · 0.91)`, 0.410, where the raw key would give 0.4185
        // and a walk 0.194), then
        // releases the jump meter: `Camel.executeRidersJump`'s dash, whose
        // cooldown keeps the meter empty while it runs. No move is rejected,
        // and the server's position once it stands is the client's.
        let cagree = match (self.camel_client_pos, self.camel_server_pos) {
            (Some(c), Some(s)) => (0..3).all(|i| (c[i] - s[i]).abs() < 0.1),
            _ => false,
        };
        let sprint_step =
            0.98f32 as f64 * (0.09f32 + 0.1) as f64 / (1.0 - (0.6f32 * 0.91f32) as f64);
        let cooled = self
            .camel_cooldown_meter
            .is_some_and(|(peak, cd)| peak == 0.0 && cd > 0);
        row(
            "r89 the bot sprinted and dashed a real camel: the cooldown held the meter, the server accepted every move and agrees where it is",
            self.camel_steered
                && (self.camel_sprint_step - sprint_step).abs() < 1e-3
                && self.camel_dashes == 1
                && self.camel_dash_step > 0.7
                && cooled
                && self.camel_moves_sent >= 20
                && self.camel_rejections == 0
                && cagree,
            format!(
                "camel {:?}: steered {}, sprinting step {:.4} (vanilla {sprint_step:.4}), {} dash(es), \
                 fastest step after it {:.3}, meter peak / cooldown while cooling {:?}, {} move_vehicle sent, \
                 {} rejection(s), client {:?} server {:?} (agree {cagree}){}",
                self.camel,
                self.camel_steered,
                self.camel_sprint_step,
                self.camel_dashes,
                self.camel_dash_step,
                self.camel_cooldown_meter,
                self.camel_moves_sent,
                self.camel_rejections,
                self.camel_client_pos,
                self.camel_server_pos,
                self.camel_skip.as_ref().map(|s| format!(" (stopped: {s})")).unwrap_or_default()
            ),
        );
        // M227 — r90: the bot rides a summoned, tamed, saddled nautilus
        // (vanilla's speed 1.0) in a three-deep channel through the
        // production right-click, holds W level for 70 ticks
        // (`AbstractNautilus.travelInWater`: `moveRelative(0.0325)`, move,
        // ×0.9, no gravity: a tick's distance settles at `0.0325 / (1 - 0.9)`
        // and its height does not move), then releases the jump meter: the
        // dash along the rider's look (1.2 in water), whose 40-tick cooldown
        // keeps the meter empty. No move is rejected, and the server's
        // position once it is still is the client's.
        let nagree = match (self.nautilus_client_pos, self.nautilus_server_pos) {
            (Some(c), Some(s)) => (0..3).all(|i| (c[i] - s[i]).abs() < 0.1),
            _ => false,
        };
        let swim_step = 0.0325f32 as f64 / (1.0 - 0.9);
        let ncooled = self
            .nautilus_cooldown_meter
            .is_some_and(|(peak, cd)| peak == 0.0 && cd > 0);
        row(
            "r90 the bot swam and dashed a real nautilus: no gravity in water, the cooldown held the meter, the server agrees where it is",
            self.nautilus_steered
                && (self.nautilus_swim_step - swim_step).abs() < 1e-3
                && self.nautilus_drift < 1e-6
                && self.nautilus_dashes == 1
                && self.nautilus_dash_step > 1.0
                && ncooled
                && self.nautilus_moves_sent >= 20
                && self.nautilus_rejections == 0
                && nagree,
            format!(
                "nautilus {:?}: steered {}, swimming step {:.4} (vanilla {swim_step:.4}), height drift {:.2e}, \
                 {} dash(es), fastest step after it {:.3}, meter peak / cooldown while cooling {:?}, \
                 {} move_vehicle sent, {} rejection(s), client {:?} server {:?} (agree {nagree}){}",
                self.nautilus,
                self.nautilus_steered,
                self.nautilus_swim_step,
                self.nautilus_drift,
                self.nautilus_dashes,
                self.nautilus_dash_step,
                self.nautilus_cooldown_meter,
                self.nautilus_moves_sent,
                self.nautilus_rejections,
                self.nautilus_client_pos,
                self.nautilus_server_pos,
                self.nautilus_skip.as_ref().map(|s| format!(" (stopped: {s})")).unwrap_or_default()
            ),
        );
        // M228 — r91: the local rider sits where the server seats it. The
        // server places a passenger with its own `positionRider` (the
        // vehicle's PASSENGER point, less the player's VEHICLE point) every
        // tick, so the bot's live position there (read through a marker, see
        // `seat_ask`), asked while each vehicle stands still, is vanilla's
        // seat; the client's must match.
        let seats_ok = self.seat_checks.len() == 4
            && self
                .seat_checks
                .iter()
                .all(|(_, c, s)| s.is_some_and(|s| (0..3).all(|i| (c[i] - s[i]).abs() < 0.05)));
        row(
            "r91 the local rider sat where the server seats it, on the boat, the horse, the camel and the nautilus",
            seats_ok,
            format!("(vehicle, client seat, server seat): {:?}", self.seat_checks),
        );
        // M229 — r92: eating while looking at the ground. Vanilla sends
        // `use_item_on` for the grass, the grass passes (no override), and
        // `startUseItem` falls through to `use_item`: the server's food goes
        // up by the bread's 5 (capped) and one bread is gone. Before M229 the
        // client sent `use_item_on` alone and nothing was eaten.
        let ate = self.eat_food.is_some_and(|(a, b)| b > a)
            && self.eat_count.is_some_and(|(a, b)| b == a - 1);
        let uskip = self
            .use_skip
            .as_ref()
            .map(|s| format!(" (stopped: {s})"))
            .unwrap_or_default();
        row(
            "r92 a survival right-click on the ground with bread ate it (use_item_on passed, use_item followed)",
            ate,
            format!(
                "food before / after {:?}, bread before / after {:?}{uskip}",
                self.eat_food, self.eat_count
            ),
        );
        // r93: a bucket. A ray at the pool passes through the water (the
        // crosshair ignores fluids) to the pool's floor, which passes, so
        // `use_item` lets the server's own fluid raycast fill the bucket;
        // emptied on dry grass, it places water.
        row(
            "r93 a bucket filled from water and emptied onto the ground (survival)",
            self.bucket_filled && self.bucket_emptied && self.bucket_water_placed,
            format!(
                "filled {}, emptied {}, water seen where it was poured {}{uskip}",
                self.bucket_filled, self.bucket_emptied, self.bucket_water_placed
            ),
        );
        // r94: the instant uses that a zero use-duration used to block: a
        // snowball is thrown, a fishing rod casts its bobber and reels it in.
        row(
            "r94 a snowball was thrown and a fishing rod cast and reeled in (survival)",
            self.snowball_seen && self.bobber_seen && self.bobber_gone,
            format!(
                "snowball entity {}, bobber cast {}, reeled in {}{uskip}",
                self.snowball_seen, self.bobber_seen, self.bobber_gone
            ),
        );
        // M239 — r106: the firework boost, in survival with an elytra. The
        // server spawns the rocket attached to the bot (index 9); the push
        // (`look * 0.1 + (look * 1.5 - v) * 0.5` a tick, converging on
        // 1.7 blocks a tick before the glide's drag) is the CLIENT's, so
        // without it a rocket does nothing. Graded on the bot's own speed
        // and on the server accepting the flight (no correction).
        let (before, after) = self.boost_steps;
        row(
            "r106 a firework rocket boosted the elytra glide (attached rocket, faster than 1.2 blocks a tick, no correction)",
            self.boost_glided
                && self.boost_rocket
                && before < 0.8
                && after > 1.2
                && self.boost_corrections.0 == self.boost_corrections.1,
            format!(
                "glided {}, rocket attached {}, step before {before:.3}, fastest after {after:.3}, corrections {:?}, rockets seen (id, attached to) {:?} (bot {:?}), (rockets left, still gliding) {:?}{}",
                self.boost_glided,
                self.boost_rocket,
                self.boost_corrections,
                self.boost_seen,
                self.boost_bot,
                self.boost_after,
                self.use_skip
                    .as_ref()
                    .map(|s| format!(" (stopped: {s})"))
                    .unwrap_or_default()
            ),
        );
        // M237 — r104: a real brewing stand, in survival: a water bottle
        // clicked into a bottle slot, nether wart and blaze powder
        // shift-clicked (wart to the ingredient slot; powder, being fuel
        // first, to the fuel slot), and the stand brews: data 0 (brew time,
        // counting down from 400) and 1 (fuel, 20 less the brew's one) are up.
        row(
            "r104 a brewing stand took a water bottle, nether wart and blaze powder and started brewing",
            self.brew_opened
                && self.brew_state.is_some_and(|(t, f)| t > 0 && f > 0)
                && self.brew_slots.first().cloned().flatten().is_some_and(|(n, _)| n == "minecraft:potion")
                && self.brew_slots.get(1).cloned().flatten().is_some_and(|(n, _)| n == "minecraft:nether_wart"),
            format!(
                "opened {}, (brew time, fuel) {:?}, slots 0/3/4 {:?}{}",
                self.brew_opened,
                self.brew_state,
                self.brew_slots,
                self.use_skip
                    .as_ref()
                    .map(|s| format!(" (stopped: {s})"))
                    .unwrap_or_default()
            ),
        );
        // M236 — r102: the anvil's cost line, drawn as `AnvilScreen.
        // extractLabels` does: a rename costs one level, so at 0 levels the
        // line is red (`-40864`, 0xFF6060: `mayPickup` refuses) and at 5 it
        // is green (`-8323296`, 0x80FF20); taking the result spends the level
        // and puts the sword back in the inventory.
        let colour = |rgb: u32| super::hud::srgb_bytes_to_linear(rgb);
        let line = |l: &Option<(String, [f32; 3])>, rgb: u32| {
            l.as_ref()
                .is_some_and(|(t, c)| t == "Enchantment Cost: 1" && *c == colour(rgb))
        };
        row(
            "r102 an anvil rename showed 'Enchantment Cost: 1' red at 0 levels, green at 5, and took the level",
            line(&self.anvil_labels.0, 0xFF6060)
                && line(&self.anvil_labels.1, 0x80FF20)
                && self.anvil_xp == (5, 4)
                && self.anvil_swords == 1,
            format!(
                "labels {:?} then {:?}, levels {:?}, swords after {}{}",
                self.anvil_labels.0,
                self.anvil_labels.1,
                self.anvil_xp,
                self.anvil_swords,
                self.use_skip
                    .as_ref()
                    .map(|s| format!(" (stopped: {s})"))
                    .unwrap_or_default()
            ),
        );
        // r103: the enchanting table's offers arrive and the first row's
        // press (`EnchantmentScreen.mouseClicked` -> `container_button_click`)
        // enchants the pickaxe for one lapis and one level.
        row(
            "r103 an enchanting table offered enchantments and its first row enchanted the pickaxe for one lapis and one level",
            self.enchant_costs[0] > 0
                && self.enchant_result == (true, 2)
                && self.enchant_xp.0 - self.enchant_xp.1 == 1,
            format!(
                "offers {:?}, levels {:?}, (enchanted, lapis left) {:?}, carried after the first press {:?}{}",
                self.enchant_costs,
                self.enchant_xp,
                self.enchant_result,
                self.enchant_carried,
                self.use_skip
                    .as_ref()
                    .map(|s| format!(" (stopped: {s})"))
                    .unwrap_or_default()
            ),
        );
        // M235 — r101: a right-click with a shield in the off hand and an
        // empty main hand raises it (the main hand passes, the off hand's
        // `use` starts), and the server blocks a hit from in front: the
        // unblocked control lost health, the blocked hit none (natural
        // regeneration can only raise it).
        let (h0, h1, h2) = self.shield_health;
        row(
            "r101 a right-click raised the off-hand shield and the server blocked a hit from in front",
            self.shield_up && h0 - h1 >= 3.0 && h2 >= h1,
            format!(
                "health {h0} -> {h1} unblocked, -> {h2} with the shield up (off hand using {}){}",
                self.shield_up,
                self.use_skip
                    .as_ref()
                    .map(|s| format!(" (stopped: {s})"))
                    .unwrap_or_default()
            ),
        );
        // M234 — r100: the bow's item model follows the draw. Standing it is
        // `item/bow`; after 40 ticks (past the jar's 0.9 threshold at scale
        // 0.05, 18 ticks) the hand draws `item/bow_pulling_2`, the name the
        // hand pass looked its quads up by.
        row(
            "r100 a fully drawn bow's first-person model is bow_pulling_2 (the item model's use_duration)",
            self.bow_drawing
                && self.bow_models.0.as_deref() == Some("minecraft:bow")
                && self.bow_models.1.as_deref() == Some("minecraft:bow#minecraft:item/bow_pulling_2"),
            format!(
                "standing {:?}, drawing {:?}",
                self.bow_models.0, self.bow_models.1
            ),
        );
        // M233 — r99: vanilla's dynamic FOV, read from the FOV the frame was
        // projected with: a drawn bow multiplies the modifier by
        // `1 - min(t / 20, 1)² · 0.15`, so after 40 ticks the ratio to the
        // standing FOV is 0.85 whatever the speed term (the bot carries
        // Speed I, so both readings are above 70).
        let (still, drawn) = self.bow_fov;
        let bow_ratio = if still > 0.0 { drawn / still } else { 0.0 };
        row(
            "r99 drawing a bow narrowed the frame's FOV to 0.85 of the standing FOV (vanilla's getFieldOfViewModifier)",
            self.bow_drawing && (bow_ratio - 0.85).abs() < 0.002,
            format!(
                "standing {still:.3} degrees, drawing {drawn:.3}, ratio {bow_ratio:.4} (vanilla 0.85), still drawing {}",
                self.bow_drawing
            ),
        );
        // M232 — r97: a real crafting table, in survival: a right-click opens
        // it (the server's `open_screen`), a log clicked from the hotbar into
        // the grid makes the server put four planks in the result slot, and a
        // shift-click on the result moves them to the inventory (the log is
        // gone). The clicks go through `screen_mouse_press`, the cursor parked
        // on each slot.
        row(
            "r97 a crafting table opened, a log in the grid made four planks, and a shift-click took them",
            self.craft_opened
                && self.craft_result == Some(("minecraft:oak_planks".to_string(), 4))
                && self.craft_inventory == Some((4, 0)),
            format!(
                "opened {}, result {:?}, (planks, logs) in the inventory {:?}",
                self.craft_opened, self.craft_result, self.craft_inventory
            ),
        );
        // r98: a real furnace: ore and coal clicked into its input and fuel
        // slots light it and start cooking (container data 0 and 2 rise), and
        // the ingot staged in its result is shift-clicked into the inventory.
        row(
            "r98 a furnace took ore and fuel, lit and cooked, and its result was shift-clicked out",
            self.furnace_opened
                && self
                    .furnace_data
                    .is_some_and(|(lit, cook)| lit > 0 && cook > 0)
                && self.furnace_taken == 1,
            format!(
                "opened {}, (lit time, cook progress) {:?}, iron ingots taken {}",
                self.furnace_opened, self.furnace_data, self.furnace_taken
            ),
        );
        // M231 — r96: using an item slows the walk to vanilla's
        // `itemUseSpeedMultiplier` (`UseEffects.DEFAULT`, 0.2): both terminal
        // steps scale with the input, so the ratio is the multiplier whatever
        // the movement speed (the bot carries Speed I).
        let (walk, eat) = self.walk_steps;
        let ratio = if walk > 0.0 { eat / walk } else { 0.0 };
        row(
            "r96 walking while eating went at a fifth of the walking speed (vanilla's use slowdown)",
            walk > 0.1 && (ratio - 0.2).abs() < 0.01,
            format!(
                "walking step {walk:.4}, while eating {eat:.4}, ratio {ratio:.4} (vanilla 0.2), health at the start {}{}",
                self.use_start_health,
                self.use_skip
                    .as_ref()
                    .map(|s| format!(" (stopped: {s})"))
                    .unwrap_or_default()
            ),
        );
        // M230 — r95: the crosshair clips against OUTLINE shapes, so a floor
        // lever and a stone button (no collision box) can be targeted, and
        // their `useWithoutItem` takes the click: with bread in hand and food
        // below 20, each powers and nothing is eaten. Before M230 the ray
        // passed through both to the grass, which passes, and the bot ate.
        let kept = self.click_food.is_some_and(|(a, b)| a == b);
        row(
            "r95 a lever and a button were targeted and pressed while holding food, and nothing was eaten",
            self.lever_on && self.button_on && kept,
            format!(
                "lever powered {}, button powered {}, (food, bread) before / after {:?}{}",
                self.lever_on,
                self.button_on,
                self.click_food,
                self.use_skip
                    .as_ref()
                    .map(|s| format!(" (stopped: {s})"))
                    .unwrap_or_default()
            ),
        );
        // r86 — the player preview belongs to the player's inventory only:
        // no container frame (chests, furnaces, the donkey...) carries it,
        // and the inventory's own frames do (so the reading is live).
        row(
            "r86 the player preview drew in the inventory and in no container screen",
            self.container_frames > 0
                && self.container_preview_frames == 0
                && self.inventory_preview_frames > 0,
            format!(
                "{} of {} container frame(s) carried the player preview; {} inventory frame(s) did",
                self.container_preview_frames, self.container_frames, self.inventory_preview_frames
            ),
        );
        // M104 — and that the which-of-these overlay reached the windowed
        // draw. Nothing here can right-click a recipe cell (that needs the
        // cursor over a specific cell AND a server that has sent a
        // multi-recipe group), so the `Open` is injected through the same
        // `open_overlay` the click path calls — M17's rule, that injection is
        // the deterministic proof where a live encounter depends on timing
        // nothing here controls. What that leaves to the unit tests is the
        // click ROUTING; what it proves is the render path, which is the only
        // thing this gate can see and M86's whole reason for existing.
        //
        // The floor is derived from the claim rather than from the emitter: an
        // overlay is at least one panel quad plus one button each, so three
        // buttons must add at least four. Reading `overlay_chrome`'s own
        // length here would make it self-calibrating.
        row(
            "r24 the which-of-these overlay was drawn in the windowed client",
            self.book_overlay_quads_max >= self.book_quads_max + 4,
            format!(
                "{} quads at peak with an overlay open against {} without — at least a panel quad and one per button",
                self.book_overlay_quads_max, self.book_quads_max
            ),
        );
        // M105 — the counter reaches the windowed frame's label list. It is a
        // caller requirement like r14's hotbar: a fresh player's book is one
        // page and draws no counter at all, so `recipe give @s *` has to be
        // staged. Failing closed on an unstaged run is the gate refusing to
        // certify a path it never saw.
        row(
            "r25 the recipe book drew its page counter",
            self.book_page_label_frames > 0,
            format!(
                "{} of {} frames — needs a multi-page book, i.e. REWO_PRECMD with `recipe give @s *`",
                self.book_page_label_frames, self.frames
            ),
        );
        // M108 — the chat box reached the windowed frame. Unlike r14 and r25
        // this needs no caller staging: the run sends its own chat line, and
        // the server echoing it back is what drives `player_chat` through the
        // signature cache, the trust level, the wrap and the geometry. A zero
        // here means the whole chain is dead in the windowed client, which is
        // the failure M86 existed to catch.
        //
        // **It is structurally blind to the fade**, and the near-total count is
        // the tell rather than a worry: `RENDER_CHECK_SECONDS` is 8, i.e. 160
        // ticks, and `AlphaCalculator.timeBased` holds full alpha until 180, so
        // no message this run receives can fade before it ends. The fade is
        // graded by unit tests on both sides of the seam
        // (`the_fade_and_the_text_opacity_both_reach_the_line` here,
        // `a_message_holds_full_alpha_for_one_hundred_and_eighty_ticks` in
        // `rewo_ui::chat`). Lengthening the run to reach the fade would turn
        // a gate into a soak for one property two tests already pin.
        row(
            "r26 the chat box drew a line in the windowed client",
            self.chat_line_frames > 0,
            format!(
                "{} of {} frames carried at least one wrapped chat line                  (near-total is CORRECT: the run is 8 s = 160 ticks and the                  fade starts at 180, so nothing can fade inside it)",
                self.chat_line_frames, self.frames
            ),
        );
        // M125 — and that a line was a RESOLVED translatable, which r26 cannot
        // see: before M125 the chat box drew `multiplayer.player.joined` and
        // scored a full r26.
        row(
            "r37 a chat line resolved a nested translatable component",
            self.translated_chat_frames > 0 && self.unresolved_key_frames == 0,
            format!(
                "{} of {} frames drew \"Gave 1 [Diamond Sword]\"; {} drew a raw key                  (must be 0). Three nesting levels and a bare-integer argument,                  off the `/give` r14 already stages — a count of 0 with the                  sword staged means the resolution is dead.",
                self.translated_chat_frames, self.frames, self.unresolved_key_frames
            ),
        );
        // M126d — that the spans survived to the renderer. r26 and r37 are
        // both satisfied by a chat box that flattens everything to one white
        // string, so neither can see this.
        row(
            "r38 a drawn chat line carried more than one colour",
            self.styled_chat_frames > 0,
            format!(
                "{} of {} frames drew ONE ROW in 2+ distinct colours (from one                  injected section-sign-coded system message: the codes must                  survive `parse_component`, the wrap's part list, and                  `chat_lines`' per-span emit. Across the whole box would be a                  weaker claim a flattening client also satisfies)",
                self.styled_chat_frames, self.frames
            ),
        );
        row(
            "r39 a drawn chat line carried a non-plain style flag",
            self.flagged_chat_frames > 0,
            format!(
                "{} of {} frames drew italic/underline/strikethrough (the flags                  ride `TextStyle`, a different field from the colour, so a                  wiring that dropped them would leave r38 green)",
                self.flagged_chat_frames, self.frames
            ),
        );
        // M127 — the decoration. r26, r37, r38 and r39 are all satisfied by a
        // client that renders a chat line's own content and never decorates
        // it, which is what Rewo did from M1 to M126: a player line read `hi`
        // where vanilla reads `<Steve> hi`.
        //
        // This grades the WHOLE chain and nothing else can: the server's
        // configuration `registry_data` has to have carried
        // `minecraft:chat_type` and been parsed (an unsynced registry makes
        // `chat_type_id` answer `None` and the injection is skipped, scoring
        // zero); the bound's `holder` VarInt has to resolve back to that entry;
        // the decoration has to build `Component.translatable("chat.type.text",
        // [name, content])`; M125's resolution has to find `<%s> %s` and
        // substitute both arguments; and M126's spans have to reach the glyphs.
        row(
            "r40 a drawn chat line was DECORATED by its chat type",
            self.decorated_chat_frames > 0,
            format!(
                "{} of {} frames drew \"<RewoDecoWitness> decorated\" — the                  server's own `minecraft:chat` entry, looked up by name rather                  than assumed, so an unsynced registry scores 0 rather than                  silently using index 0",
                self.decorated_chat_frames, self.frames
            ),
        );
        // M127 — and that the decoration's own `Style` reached the row, which
        // r40 cannot see: `chat.type.text` carries `Style.EMPTY`, so a
        // decoration that dropped `withStyle` entirely would leave r40 green.
        // Five of the seven vanilla types are unstyled and the two `/msg` ones
        // are `withColor(GRAY).withItalic(true)` — which is the whole reason
        // M126's pipeline had to land before this milestone.
        row(
            "r41 the decoration's own style reached the row",
            self.styled_decoration_frames > 0,
            format!(
                "{} of {} frames drew the `/msg` decoration's row entirely in                  gray AND italic. The arguments are bare literals with                  `Style.EMPTY`, so they INHERIT it — which is the composition                  rule (`getStyle().applyTo(parentStyle)`) and not a coincidence",
                self.styled_decoration_frames, self.frames
            ),
        );
        // M128 — a click on chat text, driven through the production path:
        // `chat_mouse_pressed` -> `clickable_style_at` -> `ChatScreen
        // ::mouse_clicked` -> `handle_component_clicked` -> `send_command`.
        // The click POINT is read off the drawn line rather than recomputed,
        // so a hit test that disagreed with the renderer about where a row
        // sits turns this red while every unit test stays green.
        row(
            "r42 a click on a chat link ran its command",
            self.chat_click_ok > 0,
            format!(
                "{} clicks answered RunCommand at the drawn line's own                  position (0 means the injected `click_event` did not survive                  the wire, the wrap, the store or the hit test — or that the                  hit test and the draw disagree about where a row is)",
                self.chat_click_ok
            ),
        );
        // `Screen.clickUrlAction` — a clicked link must reach a CONFIRMATION,
        // not the browser. Driven through the production entry
        // (`chat_click_url`) with the stock option values, then answered "No"
        // through the production press path (`press_widget`). Three claims in
        // one row, because they are one round trip: the prompt drew (three
        // buttons, their captions, the URL), the chat left the slot while it
        // was up, and "No" gave the chat back with what had been typed.
        // Nothing is opened: the witness never presses "Yes".
        row(
            "r67 a clicked chat link raised the confirm prompt and No returned to the chat",
            self.link_prompt_frames >= 3 && self.link_prompt_returned,
            format!(
                "{} frames drew the prompt over an empty chat slot; the chat                  came back with its text: {}",
                self.link_prompt_frames, self.link_prompt_returned
            ),
        );
        // M110 — the chat SCREEN, as distinct from r26's read-only box. A zero
        // means `T` reaches nothing in the windowed client, which is the
        // failure M86 existed to catch and which r26 structurally cannot see.
        row(
            "r27 the chat screen opened and drew its input bar",
            self.chat_screen_frames > 0,
            format!(
                "{} of {} frames had the screen up with its input bar in the                  backdrop list",
                self.chat_screen_frames, self.frames
            ),
        );
        // M111 — and the scrollbar within it, which needs more chat than the
        // box holds and so is a strictly narrower claim than r27's.
        row(
            "r28 the chat scrollbar was drawn",
            self.chat_scrollbar_frames > 0,
            format!(
                "{} of {} frames drew the bar's two rects (needs a backlog past                  the focused box's 20 rows, which the run injects)",
                self.chat_scrollbar_frames, self.frames
            ),
        );
        // M115 — and the popup within it, which needs a completion word AND a
        // keystroke, so it is narrower again than r28's.
        row(
            "r29 the suggestion popup was drawn",
            self.suggestion_popup_frames > 0,
            format!(
                "{} of {} frames drew the popup's row fills (needs a                  custom_chat_completions word and a keystroke, both of which the run                  drives through the production path)",
                self.suggestion_popup_frames, self.frames
            ),
        );
        // M117 — the syntax highlighting reached the frame at all.
        row(
            "r31 the command line was drawn as coloured runs",
            self.highlighted_command_frames > 0,
            format!(
                "{} of {} frames drew the field from a parse rather than as one                  flat line",
                self.highlighted_command_frames, self.frames
            ),
        );
        // M117 — the usage box, which `extractRenderState` draws only when
        // the popup is absent, so this and r29 name disjoint moments.
        row(
            "r32 the usage box was drawn under the field",
            self.usage_box_frames > 0,
            format!(
                "{} of {} frames drew a usage line (needs an ARGUMENT expected                  at the cursor and no popup over it)",
                self.usage_box_frames, self.frames
            ),
        );
        // M134 — and a parse-error MESSAGE in that same box, which is a
        // strictly narrower claim than r32's and names a disjoint moment:
        // `if (commandUsage.isEmpty())` means an exception suppresses the
        // usage entries, so no frame can score both. The staged input is a
        // command that does not exist, whose `getExceptions()` is EMPTY —
        // it reaches the line through `Commands.getParseException`, the
        // branch M117 omitted outright.
        row(
            "r43 a parse error was drawn white under the command field",
            self.parse_error_frames > 0,
            format!(
                "{} of {} frames drew a line opening \"Unknown command\" in the                  usage box, in WHITE. Grey is `USAGE_FORMAT`'s and a brigadier                  message carries no style at all, so `extractUsage`'s `-1`                  stands — the colour is half the claim because the box drew                  every line white from M117 until this milestone",
                self.parse_error_frames, self.frames
            ),
        );
        // M118 — and a SELECTOR among them, which needs the entity argument
        // type rather than just the tree.
        row(
            "r33 an entity selector was offered locally",
            self.local_selector_completions > 0,
            format!(
                "{} completions containing an @-selector (needs                  minecraft:entity to parse, which it did not before M118)",
                self.local_selector_completions
            ),
        );
        // M119 — and a registry id among them, which needs the block/item
        // argument types rather than the tree or the selector.
        row(
            "r34 a registry id was offered locally",
            self.local_resource_completions > 0,
            format!(
                "{} completions containing a namespaced id (needs block_state or                  item_stack to parse, which they did not before M119)",
                self.local_resource_completions
            ),
        );
        // M120 — and a coordinate among them.
        row(
            "r35 a coordinate default was offered locally",
            self.local_coordinate_completions > 0,
            format!(
                "{} completions containing a `~` (needs the coordinate family to                  parse, which it did not before M120)",
                self.local_coordinate_completions
            ),
        );
        // M124 — and a name from one of the seven tables that live outside
        // their own argument class.
        row(
            "r36 a literal table was offered locally",
            self.local_literal_table_completions > 0,
            format!(
                "{} completions offering a `sidebar.team.*` slot (needs                  scoreboard_slot's table, which read as a bare word before M124)",
                self.local_literal_table_completions
            ),
        );
        // M116 — the dispatcher answered a command locally, i.e. WITHOUT a
        // round trip. Both paths open a popup, so r29 cannot see this.
        row(
            "r30 a command completion was answered locally",
            self.local_command_completions > 0,
            format!(
                "{} completions answered by the client's own dispatcher (the run                  types `/` then a letter, which reaches only literals)",
                self.local_command_completions
            ),
        );
        row(
            "r21 open_screen opened the client's screen by itself",
            self.container_self_opened_frames > 0,
            format!(
                "{} frames drawn before the gate force-opened the inventory",
                self.container_self_opened_frames
            ),
        );
        row(
            "r20 the container's panel was its own, not the player's 166",
            self.container_panel_h
                .is_some_and(|h| (h - 166.0).abs() > 0.5),
            format!(
                "panel height {:?} (player's is 166)",
                self.container_panel_h
            ),
        );
        row(
            "r22 the windowed client drew a container's overlays",
            self.container_overlays_max > 0,
            format!(
                "{} overlay sprites at peak (a brewing stand draws fuel + arrow + bubbles;                  the chest injected earlier draws none)",
                self.container_overlays_max
            ),
        );
        // M132 — the scoreboard sidebar. Nothing before this could see it:
        // Rewo decoded the four scoreboard packets from M65 and drew nothing,
        // so every other witness here is satisfied by a client with no sidebar
        // at all.
        //
        // The gate injects an objective, three scores and a display-slot
        // assignment as raw bodies through the production router (M17's rule),
        // which makes the whole chain the claim: the packets have to decode,
        // `select_objective` has to find the slot, the rows have to resolve
        // their names and formats, and the layout has to place them. A hidden
        // holder is injected alongside, so a client that skipped
        // `isHidden` scores EIGHT lines rather than seven and fails on the
        // count as well as on the ordering.
        row(
            "r44 the scoreboard sidebar was drawn in the windowed client",
            self.sidebar_frames > 0 && self.sidebar_text_max == 7,
            format!(
                "{} of {} frames drew a sidebar, at most {} text lines (must be                  exactly 7: a title, then three visible rows of name + score.                  A fourth `#hidden` holder is injected too, so 9 means                  `isHidden` was skipped and 1 means only the title resolved)",
                self.sidebar_frames, self.frames, self.sidebar_text_max
            ),
        );
        // M138a — the listener seam. `AudioDevice` had four methods and none of
        // them was a listener, so every sound was positioned in absolute world
        // coordinates against ears at the origin facing -Z.
        //
        // **Compared against the frame count, not against zero.** The push
        // belongs on the render path, because vanilla's
        // `SoundEngine.updateSource(camera)` is called with the camera the frame
        // is about to use, while `SoundEngine.tick` is `Minecraft.tick`'s. Moved
        // beside `sounds.drive` this would still be non-zero and would score
        // about the tick count — an order of magnitude below the frames — so a
        // `> 0` threshold cannot tell the two apart. This is also the only check
        // that reaches the call site at all: it is a composition root in a binary
        // crate, and deleting it survives the whole unit suite.
        row(
            "r45 the listener reached the audio device, once per frame",
            self.listener_pushes == self.frames
                && self
                    .listener_forward
                    .is_some_and(|f| (f[0] * f[0] + f[1] * f[1] + f[2] * f[2] - 1.0).abs() < 1e-3),
            format!(
                "{} pushes over {} frames (must be equal -- per tick would be                  roughly the tick count), last forward {:?} (must be a unit                  vector, which a default-constructed or zeroed transform is not)",
                self.listener_pushes, self.frames, self.listener_forward
            ),
        );
        row(
            "r46 the client selected and started a music track",
            self.music_started.is_some(),
            format!(
                "started {:?} (M147: the Overworld's BackgroundMusic lives on the                  DIMENSION TYPE and not on plains, so with an EMPTY base this                  is None and no music plays anywhere in the Overworld -- which                  is what M146 shipped. The run is 8 s = 160 ticks against a                  100-tick STARTING_DELAY, so a client that never decremented                  scores None too.)",
                self.music_started
            ),
        );
        // M151 — the tab list. The M86 shape a second time: `rewo_gpu::tab_list`
        // shipped with 41 passing tests and no caller, so every other witness
        // above is satisfied by a client whose Tab key does nothing at all.
        //
        // Four claims, and each fails differently:
        //
        // * `> 0` — the key, the resolver, the layout and the emitters were all
        //   reached in the WINDOWED client, which is the only thing no
        //   serverless gate can see.
        // * `< frames` — and it is a HOLD. The key goes down at half the run,
        //   so a client that drew the list unconditionally scores every frame
        //   and is exactly as wrong as one that never drew it.
        // * `rows == 3` — the local player plus the two the run injects as
        //   `listed: true`. Two more are injected as `listed: false`, so a
        //   client that ignored `getListedOnlinePlayers()` scores FIVE. That is
        //   the whole reason `listed` had to stop being decoded-and-dropped.
        // * `icons == rows` — one ping sprite per row, through `set_hud_icons`,
        //   which is a different list from the text and breaks independently.
        row(
            "r47 the tab list was drawn in the windowed client, while its key was held",
            self.tab_list_frames > 0
                && self.tab_list_frames < self.frames
                && self.tab_list_rows_max == 3
                && self.tab_list_icons_max == 3
                && self.tab_list_text_max == 3,
            format!(
                "{} of {} frames drew it (must be neither 0 nor all — the key \
                 goes down at half the run), at most {} rows / {} ping icons / \
                 {} text lines. Three is the local player plus the two injected \
                 `listed: true` players; FIVE means the two `listed: false` ones \
                 were shown, and one means the injection never landed.",
                self.tab_list_frames,
                self.frames,
                self.tab_list_rows_max,
                self.tab_list_icons_max,
                self.tab_list_text_max
            ),
        );
        // ── M162: the two packet tails that carry a sound ──────────────────
        //
        // All three call sites live inside `PlaySession`, which owns a socket
        // and cannot be built in a unit test (M71) — so `soundshot` grades the
        // arithmetic exactly and would stay green with `queue_explosion_sound`
        // deleted and `camera_eye` hardwired to `None`. This is the only check
        // that can see them, which is r45's stated reason for existing.
        let bearing = match (
            self.global_event_sound,
            self.global_event_eye,
            self.global_event_target,
        ) {
            (Some(s), Some(eye), Some(t)) => {
                let to_sound = [s.0 - eye[0], s.1 - eye[1], s.2 - eye[2]];
                let len = (to_sound[0] * to_sound[0]
                    + to_sound[1] * to_sound[1]
                    + to_sound[2] * to_sound[2])
                    .sqrt();
                let to_event = [t[0] - eye[0], t[1] - eye[1], t[2] - eye[2]];
                let dot = to_sound[0] * to_event[0]
                    + to_sound[1] * to_event[1]
                    + to_sound[2] * to_event[2];
                let to_block =
                    ((s.0 - t[0]).powi(2) + (s.1 - t[1]).powi(2) + (s.2 - t[2]).powi(2)).sqrt();
                Some((len, dot, to_block))
            }
            _ => None,
        };
        row(
            "r48 a global level event placed its sound against the live camera",
            // 2.0 blocks from the EYE the gate recorded independently — so a
            // `camera_eye` reading feet rather than the eye misses by 1.62 and
            // fails here. The dot's SIGN is what kills the reversed
            // `Vec3.subtract`, which a distance-only check passes at 180
            // degrees. And the block is 58 blocks away, so "the sound is not at
            // the block" separates this from the block-placed path.
            bearing.is_some_and(|(len, dot, to_block)| {
                (len - 2.0).abs() < 1e-6 && dot > 0.0 && to_block > 50.0
            }),
            format!(
                "eye {:?} -> sound {:?} (target {:?}): {:?} = (blocks from eye, dot with the \
                 bearing, blocks from the block). `None` here means the camera never \
                 resolved, which is what a client that hands `route_level_event_sound` a \
                 hardcoded `None` scores.",
                self.global_event_eye, self.global_event_sound, self.global_event_target, bearing
            ),
        );
        row(
            "r49 an explode packet's TAIL was walked and its sound started",
            self.explosion_tails > 0
                && self.explosion_sounds_queued > 0
                && self.explosion_sound.is_some(),
            format!(
                "{} tails walked, {} sounds queued, engine position {:?}. M68 consumed only \
                 the physics prefix and threw the sound away; a client that still does \
                 scores 0/0/None here with every other witness green.",
                self.explosion_tails, self.explosion_sounds_queued, self.explosion_sound
            ),
        );
        row(
            "r50 a far level event was QUEUED rather than played",
            self.queued_delayed > 0,
            format!(
                "{} delayed. The injected 3012 is 200 blocks out, past the strict \
                 `distanceToSqr > 100.0` gate, so it must reach `playDelayed`. The only \
                 other producer in this run would be an End flash, and this is the \
                 Overworld.",
                self.queued_delayed
            ),
        );
        // M163 — the wire-time flattens. Two halves, and the second is what
        // makes this a witness rather than a count: r26 scored full marks for
        // three milestones while chat drew `multiplayer.player.joined`, so
        // "a name was drawn" is satisfied by the bug.
        row(
            "r51 a mob nametag drew a RESOLVED translatable, and never its raw key",
            self.nametag_resolved_frames > 0 && self.nametag_raw_key_frames == 0,
            format!(
                "{} of {} frames carried `Zombie`, {} carried \
                 `entity.minecraft.zombie`. The injected zombie's \
                 `DATA_CUSTOM_NAME` is `{{translate: \"entity.minecraft.zombie\"}}`; \
                 before M163 `metadata::parse` flattened it with \
                 `Nbt::to_plain_text`, which has no language table, so the \
                 second number was the whole of it. Zero for BOTH means the \
                 injection never landed",
                self.nametag_resolved_frames, self.frames, self.nametag_raw_key_frames
            ),
        );
        row(
            "r52 a sign line drew a RESOLVED translatable, and never its raw key",
            self.sign_resolved_frames > 0 && self.sign_raw_key_frames == 0,
            format!(
                "{} of {} frames carried `Dirt`, {} carried the truncated key. \
                 The injected sign's front text is \
                 `{{translate: \"block.minecraft.dirt\"}}`; the app resolves it \
                 in the FRAME LOOP rather than at decode, which is why r51 \
                 cannot stand in for this. Zero for BOTH means the injection \
                 never landed or `collect_session_sign_text` was not called",
                self.sign_resolved_frames, self.frames, self.sign_raw_key_frames
            ),
        );
        // M164 — waterlogged water in the WINDOWED client.
        //
        // This does not grade a pixel, and says so: `meshshot`'s waterlogged
        // control grades the geometry, and the water rides the translucent pass
        // the client has drawn since M3. What no serverless gate can see is the
        // wiring — `MeshTables.fluid` is a new field, and a `live_cmd` that
        // failed to fill it would mesh no carried water anywhere while every
        // other witness here, every unit test and every `*shot` stayed green.
        // That is the M86 shape exactly, so the claim is "the production chain
        // from the bake through the mesh pool reached a real server's block".
        //
        // The counter behind it is **emission-gated** (`mesh_column`:
        // `f.carried && fv.len() > fluid_verts_before`), which it was not when
        // M164 shipped: it incremented beside the `emit_fluid` CALL, so it
        // proved the table reached the mesher rather than that any water was
        // meshed, and this row's own wording over-claimed by exactly that gap.
        //
        // The run places the block itself rather than making this a fourth
        // caller requirement (M108's precedent): `/setblock` needs op, which
        // `tools/render_check.py` already grants for r25.
        row(
            "r53 a waterlogged block from the server meshed its water",
            self.carried_fluid_cells > 0,
            format!(
                "{} carried-fluid cells in the largest column (0 means the bake, \
                 the MeshTables field, the mesh pool or the setblock did not \
                 reach each other -- and renders EXACTLY as it did before M164)",
                self.carried_fluid_cells
            ),
        );
        // r48 — the crowd, and the kind resolution under it.
        //
        // Both counts are taken over THIS CHECK'S OWN three injected entities
        // (`CROWD_IDS`), never over the frame at large. That is what stops the
        // threshold being satisfied by somebody else's mob: with at most three
        // draws in scope, `same >= 2` means both injected zombies decoded and
        // resolved to one kind, and `distinct >= 2` means the villager decoded
        // and resolved to a different one — together the multiset `{A, A, B}`,
        // which no partially landed injection can produce.
        //
        // It stays `>=` rather than `==` because the numbers are running
        // maxima over frames and a third distinct kind is unreachable from
        // three entities of two types; the restriction, not the comparison, is
        // what makes it a claim about the client rather than about the world.
        row(
            "r54 two mobs of one type and one of another reached ONE draw list, with distinct resolved kinds",
            self.crowd_same_kind_max >= 2 && self.crowd_kinds_max >= 2,
            format!(
                "over the {} injected entities: {} draws of one kind at peak, {} \
                 distinct non-capsule kinds (0 same-kind means the injected \
                 `add_entity` bodies never decoded or never reached the draw \
                 list; 1 distinct kind means `kind_for_entity_name` collapsed \
                 them, which is the wrong-model half of the reported bug)",
                CROWD_IDS.len(),
                self.crowd_same_kind_max,
                self.crowd_kinds_max
            ),
        );
        // ── M166: the two BLOCKING configuration tasks ──
        //
        // These two are unlike every other row here, because before M166 the
        // failure was not a wrong pixel — it was **no run at all**. A server
        // with `resource-pack=` or `enable-code-of-conduct=true` set left the
        // client in `run_configuration` forever, so the whole gate scored
        // 0 witnesses and the window never opened.
        //
        // Both claims are cross-checks against something the SERVER was
        // configured with, passed in by `tools/render_check.py` through the
        // environment. That matters: a counter (`we replied twice`) is
        // satisfied by a client that replied with a zero id and an empty
        // string, which is exactly what the malformed-body path produces.
        // Comparing against the staged values means the wire must have carried
        // them.
        //
        // They fail CLOSED when unstaged, on §5's rule — a witness that skips
        // itself when its precondition is absent is green on every machine
        // where the precondition is what matters.
        let want_pack = crate::knobs::var("REWO_RC_PACK_ID").ok();
        row(
            "r55 both pushed resource packs were decoded and answered TERMINALLY",
            match (&want_pack, self.config_tasks.pack_replies.as_slice()) {
                (Some(want), [(cfg_id, cfg_act), (play_id, play_act)]) => {
                    use rewo_net::config_tasks::PackAction;
                    format!("{cfg_id:032x}") == want.to_ascii_lowercase()
                        && *cfg_act == PackAction::FailedDownload
                        && *play_id == PLAY_PACK_ID
                        && *play_act == PackAction::InvalidUrl
                }
                _ => false,
            },
            format!(
                "staged id {:?}, replies {:?}. TWO are required, in order: the                  configuration task's (the server's own `resource-pack-id`, so                  agreement proves the UUID came off the wire rather than out of                  a default) answered FAILED_DOWNLOAD, then the injected                  PLAY-state push answered INVALID_URL because its url is                  `ftp:`. One reply means the play arm is dead -- nothing else                  in this repo reaches it, since a vanilla server pushes during                  CONFIGURATION. Empty means the run was not staged, or -- the                  bug this milestone fixes -- the client never got past                  configuration at all, in which case every row above is also                  zero.",
                want_pack,
                self.config_tasks
                    .pack_replies
                    .iter()
                    .map(|(id, a)| format!("{id:032x}:{a:?}"))
                    .collect::<Vec<_>>()
            ),
        );
        let want_coc = crate::knobs::var("REWO_RC_COC").ok();
        row(
            "r56 the server's code of conduct was decoded and accepted",
            match (&want_coc, self.config_tasks.codes_of_conduct.as_slice()) {
                (Some(want), [got]) => got == want,
                _ => false,
            },
            format!(
                "staged {:?}, accepted {:?}. This task is queued AHEAD of the                  resource pack (`addOptionalTasks`), so on a server with both                  it is the one that hangs first and r55 can never be reached                  without it.",
                want_coc, self.config_tasks.codes_of_conduct
            ),
        );
        // M168 — the survival HUD's two live inputs that only a server can
        // supply. Both are COUNT-plus-VALUE: ten armour icons is "an ARMOR
        // attribute reached the local store" and three full ones is "it was
        // the staged iron chestplate's 6"; one effect icon at the first
        // beneficial slot is "`update_mob_effect` for YOU was kept, its
        // showIcon bit read, and the table called speed beneficial". The
        // server sends the player's own attributes only through
        // `sendToTrackingPlayersAndSelf` on a dirty set, never in the initial
        // pairing, so a green r57 is also the proof that path exists.
        row(
            "r57 the staged iron chestplate reached the armour row as three full icons of ten",
            self.armor_icons_max == 10 && self.armor_full_max == 3,
            format!(
                "max armour icons {} (10 when any armour is drawn), max full {} \
                 (iron chestplate = 6 armour = 3 full). Zero means the player's own \
                 `update_attributes` never arrived or never reached `local_attributes`",
                self.armor_icons_max, self.armor_full_max
            ),
        );
        row(
            "r58 the staged speed effect drew one icon at the first beneficial slot",
            self.effect_icons_max == 1 && self.effect_first_beneficial,
            format!(
                "max effect icons {}, first beneficial slot seen {}. `effect give ... \n                 infinite 0` with particles ON: the `hideParticles` argument also clears \n                 showIcon (MobEffectInstance.java:61); the icon sits at (guiWidth - 25 + 3, 4)",
                self.effect_icons_max, self.effect_first_beneficial
            ),
        );
        row(
            "r59 the staged saddled horse put the jump bar up and the release sent START_RIDING_JUMP",
            self.jump_bar_blits_max >= 2 && self.jump_scale_max >= 0.9 && self.riding_jumps_sent >= 1,
            format!(
                "max jump-bar blits {} (2 = background + progress), max scale {:.2} \
                 (a 16-tick hold peaks at 1.0 and decays toward 0.8), START_RIDING_JUMPs \
                 sent {}. Needs: the saddle off equipment slot 7, the bot as the horse's \
                 first passenger, the ramp in the mounted tick, and the packet",
                self.jump_bar_blits_max, self.jump_scale_max, self.riding_jumps_sent
            ),
        );
        row(
            "r60 the staged leashed cow drew a rope through collect_leashes",
            self.leash_verts_max >= 3,
            format!(
                "max leash ribbon verts {} (a ribbon is 98 triangles = 294 verts). \
                 Needs: the cow + fence-knot in the table, the set_entity_link \
                 holder, and collect_leashes building the ribbon into the pass",
                self.leash_verts_max
            ),
        );
        row(
            "r61 the injected written book opened the reader with its pages",
            self.book_frames > 0 && self.book_pages_seen == 2,
            format!(
                "book-screen frames {} (needs set_player_inventory recording the \
                 SlotText, open_book resolving hand 0 off the app's hotbar slot, \
                 and the screen opening), pages {} (want 2 — the resolve read the \
                 written_book_content off the wire)",
                self.book_frames, self.book_pages_seen
            ),
        );
        row(
            "r62 the sound options page drew its eleven sliders in the windowed client",
            self.options_slider_sprites_max == 22,
            format!(
                "max slider sprites {} (11 sliders x track+handle = 22; fewer means a                  lowering was dropped, more a stray widget). The page was opened and                  closed through the production paths — Sound, root, pause, game",
                self.options_slider_sprites_max
            ),
        );
        row(
            "r17 validation was enabled",
            self.validation,
            format!("{}", self.validation),
        );
        row(
            "r63 the injected open_sign_editor opened the editor in the windowed client",
            self.sign_edit_frames > 0,
            format!(
                "{} editor frames (needs the staged oak_sign BE, the production decode \
                 opening the screen, and the board reaching the screen pass)",
                self.sign_edit_frames
            ),
        );
        row(
            "r64 the editor's replacement committed sign_update",
            self.sign_update_sent,
            format!(
                "sent {} (vanilla's setScreen calls removed() on the old screen, which \
                 IS the commit — the 0.90 book-menu injection replaces the editor)",
                self.sign_update_sent
            ),
        );
        // The windowed client's OWN renderer has the passes that draw text and
        // container screens. Every other text/container row counts data
        // (lines built, panel heights) that is stored whether or not a pass
        // exists to draw it — which is how the windowed client shipped with
        // neither pass while those rows stayed green.
        row(
            "r65 the windowed renderer built the vanilla-font text pass",
            self.text_ready,
            format!("{}", self.text_ready),
        );
        row(
            "r66 the windowed renderer built the container-screen pass",
            self.container_ready,
            format!("{}", self.container_ready),
        );
        row(
            "r18 the session was validation-clean",
            vuids == 0,
            format!("{vuids} errors"),
        );

        // ── M160: the witness namespace is a SHARED, MERGE-SILENT resource ──
        //
        // Until this block existed, `report` ended `pass == rows.len()` with no
        // declared count and no uniqueness check — so two branches could each
        // add an `r48`, git would merge both cleanly, and the gate would print
        // `48/48` while grading one of them twice and the other never.
        //
        // **That is not hypothetical.** M127-M134's integration lost r42 to
        // exactly this (three branches minting the same id, §0.0), and a
        // 20-agent survey in 2026-08-17 found **fifteen** independent specs all
        // claiming `r48`. A conflict git can see is a good outcome; this one it
        // cannot.
        //
        // So: ids are parsed out of the names, and the run fails closed on a
        // duplicate, a gap, or a count that has moved without the constant
        // moving with it. All three are things a merge produces and no pixel
        // can show.
        let seam = witness_seam_faults(
            &rows.iter().map(|(n, _, _)| *n).collect::<Vec<_>>(),
            Self::EXPECTED_RENDER_CHECK_WITNESSES,
        );
        for fault in &seam {
            println!("[rendercheck] SEAM FAIL {fault}");
        }
        let seam_ok = seam.is_empty();
        let mut pass = 0usize;
        for (name, ok, detail) in &rows {
            println!(
                "[rendercheck] {} {name} ({detail})",
                if *ok { "PASS" } else { "FAIL" }
            );
            if *ok {
                pass += 1;
            }
        }
        println!("[rendercheck] {pass}/{} witnesses", rows.len());
        pass == rows.len() && seam_ok
    }
}

/// Faults in `live --render-check`'s witness namespace (M160).
///
/// **The namespace is a shared, MERGE-SILENT resource, and that is the whole
/// reason this exists.** Until M160, `report` ended `pass == rows.len()` with no
/// declared count and no uniqueness check — so two branches could each add an
/// `r48`, git would merge both cleanly, and the gate would print `48/48` while
/// grading one of them twice and the other never.
///
/// That is not hypothetical. The M127-M134 integration lost r42 to exactly this
/// (`REWO_PLAN.md` §0.0), and a 20-agent survey on 2026-08-17 found **fifteen**
/// independent specs all claiming `r48` — because every one of them picked "the
/// highest id I can see plus one" from the same pre-merge tree.
///
/// Three faults, each of which a merge produces and no pixel can show:
///
/// * a **duplicate** id — two milestones minted the same number;
/// * a **count** that has moved without [`RenderCheck::EXPECTED_RENDER_CHECK_WITNESSES`]
///   moving with it, which is what catches a row silently lost to a conflict
///   resolution;
/// * a **gap** in the sequence — a deleted row whose id was never reclaimed,
///   which is how the *next* milestone picks a number that looks free and is not.
///
/// Free-standing rather than a method on `RenderCheck` because `report` needs a
/// live session and a server, so anything inside it is untestable by
/// construction — M97's finding, applied for the fifth time in this repo.
pub(crate) fn witness_seam_faults(names: &[&str], expected: usize) -> Vec<String> {
    let ids: Vec<usize> = names
        .iter()
        .filter_map(|name| {
            name.strip_prefix('r')?
                .split_whitespace()
                .next()?
                .parse::<usize>()
                .ok()
        })
        .collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    let mut faults = Vec::new();

    let mut dupes: Vec<usize> = Vec::new();
    for w in sorted.windows(2) {
        if w[0] == w[1] && !dupes.contains(&w[0]) {
            dupes.push(w[0]);
        }
    }
    if !dupes.is_empty() {
        faults.push(format!(
            "duplicate witness id(s) {dupes:?} — two milestones minted the same \
             rNN and the merge was silent"
        ));
    }
    if ids.len() != expected {
        faults.push(format!(
            "witness count {} != declared {expected} — bump \
             EXPECTED_RENDER_CHECK_WITNESSES in the same commit that adds a row, \
             and take the next free id from REWO_PLAN §0.0's shared-resource table",
            ids.len()
        ));
    }
    sorted.dedup();
    if let Some(gap) = (1..=sorted.len()).find(|n| sorted.get(n - 1) != Some(n)) {
        faults.push(format!(
            "witness ids are not contiguous from r1 — first gap at r{gap}"
        ));
    }
    faults
}

impl LiveApp {
    /// M229 — r92-r94 in the tail, after r90-r91: survival right-clicks
    /// through `use_key_press` / `use_down`, each graded by what the server
    /// sent back (food, slot contents, block updates, entities).
    pub(super) fn use_witnesses(&mut self, now: u64, elapsed: f32, limit: f32) {
        let seats_done = self
            .check
            .as_ref()
            .is_some_and(|c| c.seat_checks.iter().all(|e| e.2.is_some()));
        if self.use_stage > 0
            && self.use_stage < 100
            && elapsed >= limit + RENDER_CHECK_TAIL_SECONDS - 0.2
        {
            let at = self.session.as_ref().map(|s| {
                (
                    [s.player.x, s.player.y, s.player.z],
                    s.player.yaw,
                    s.local_vehicle(),
                )
            });
            let origin = self.precmd_origin;
            if let Some(c) = self.check.as_mut() {
                c.use_skip.get_or_insert_with(|| {
                    format!(
                        "timed out in stage {} (bot (pos, yaw, vehicle) {at:?}, spawn origin {origin:?})",
                        self.use_stage
                    )
                });
            }
            self.use_down = false;
            self.use_stage = 100;
        }
        let held = |app: &Self| {
            app.session.as_ref().and_then(|s| {
                let h = s.inventory.held()?;
                Some((app.items.name(h.item_id)?.to_string(), h.count))
            })
        };
        let aim = |app: &mut Self, target: [f64; 3]| {
            if let Some(s) = app.session.as_mut() {
                let eye = [s.player.x, s.player.eye_y(), s.player.z];
                let d = [target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]];
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                s.player.yaw = (-d[0]).atan2(d[2]).to_degrees() as f32;
                s.player.pitch = (-d[1] / len).asin().to_degrees() as f32;
            }
        };
        let cmd = |app: &mut Self, c: &str| {
            if let Some(s) = app.session.as_mut() {
                let _ = s.send_command(c);
            }
        };
        // The open menu's layout name, a menu slot's (item, count), the
        // selected hotbar index, an item's total in the player inventory, and
        // a press on a menu slot through `screen_mouse_press` with the cursor
        // parked on it (M232).
        // The shown menu's name once its content has arrived: `open_screen`
        // puts the screen up empty and `container_set_content` follows, so a
        // click before it lands on empty slots (r98 failed that way once).
        let shown = |app: &Self| {
            app.session
                .as_ref()
                .filter(|s| app.screen.inventory_open() && s.shown_menu().content_updates() > 0)
                .map(|s| s.shown_menu().layout().name)
        };
        let menu_slot = |app: &Self, i: usize| {
            app.session.as_ref().and_then(|s| {
                let st = s.shown_menu().menu_slot(i)?;
                Some((app.items.name(st.item_id)?.to_string(), st.count))
            })
        };
        let selected = |app: &Self| {
            app.session
                .as_ref()
                .map_or(0, |s| s.inventory.selected() as usize)
        };
        let inventory_count = |app: &Self, item: &str| {
            app.session.as_ref().map_or(0, |s| {
                (0..s.inventory.slot_count())
                    .filter_map(|i| s.inventory.menu_slot(i))
                    .filter(|st| app.items.name(st.item_id) == Some(item))
                    .map(|st| st.count)
                    .sum::<i32>()
            })
        };
        let press = |app: &mut Self, slot: usize, shift: bool| {
            let Some(ext) = app.state.as_ref().map(|s| s.window.inner_size()) else {
                return;
            };
            let rect = app.session.as_ref().and_then(|s| {
                super::slots::menu_slot_rects(
                    s.shown_menu(),
                    ext.width as f32,
                    ext.height as f32,
                    super::recipe_book::book_visible(s),
                )
                .get(slot)
                .copied()
            });
            if let Some((x, y, size)) = rect {
                app.screen.mouse = ((x + size * 0.5) as f64, (y + size * 0.5) as f64);
                app.shift = shift;
                app.screen_mouse_press(winit::event::MouseButton::Left);
                app.screen_mouse_release();
                app.shift = false;
            }
        };
        // M236 — a press and release on enchanting row `i`'s centre
        // (`enchant_row_rect`: 108 x 19 at (60, 14 + 19 i)).
        let press_row = |app: &mut Self, i: usize| {
            let Some(ext) = app.state.as_ref().map(|s| s.window.inner_size()) else {
                return;
            };
            let layout = &rewo_world::menu_layout::REGISTRY[13];
            let (left, top, scale) = rewo_gpu::container::gui_origin_for(
                ext.width as f32,
                ext.height as f32,
                layout.image_w as f32,
                layout.image_h as f32,
            );
            let r = rewo_ui::menu_screen::enchant_row_rect(i);
            app.screen.mouse = (
                (left + (r.dx as f32 + r.w as f32 * 0.5) * scale) as f64,
                (top + (r.dy as f32 + r.h as f32 * 0.5) * scale) as f64,
            );
            app.screen_mouse_press(winit::event::MouseButton::Left);
            app.screen_mouse_release();
        };
        let xp = |app: &Self| app.session.as_ref().map_or(-1, |s| s.hud.experience.level);
        // What the crosshair is on and where the bot stands, for a timeout's
        // reason (M236).
        let looking_at = |app: &Self| {
            app.session.as_ref().map_or_else(String::new, |s| {
                let hit = s
                    .target_block(
                        super::eye_f64(s),
                        super::look_dir(s.player.yaw, s.player.pitch),
                        super::REACH,
                    )
                    .map(|h| {
                        let st = s.world.block_state_at(h.block[0], h.block[1], h.block[2]);
                        (
                            h.block,
                            app.blocks.block_name(st).unwrap_or("?").to_string(),
                        )
                    });
                format!(
                    "hit {hit:?}, at ({:.2}, {:.2}, {:.2}) yaw {:.1} pitch {:.1}",
                    s.player.x, s.player.y, s.player.z, s.player.yaw, s.player.pitch
                )
            })
        };
        let Some(o) = self.precmd_origin else {
            return;
        };
        // A dry spot two blocks east of r87's pool (`~5..9, ~-1, ~4..18`).
        let spot = [o[0].floor() + 11.5, o[1], o[2].floor() + 6.5];
        let grass = [spot[0] - 1.0, o[1] - 0.5, spot[2]];
        let pool = [o[0].floor() + 8.5, o[1] - 0.6, spot[2]];
        // r95's floor lever and stone button (`render_check.py` stages them).
        let lever = [o[0].floor() + 12.5, o[1] + 0.1, o[2].floor() + 4.5];
        // r97's crafting table and r98's furnace (`render_check.py`).
        let table = [o[0].floor() + 14.5, o[1] + 0.5, o[2].floor() + 8.5];
        let furnace = [o[0].floor() + 13.5, o[1] + 0.5, o[2].floor() + 9.5];
        // r102's anvil and r103's enchanting table (`render_check.py`).
        let anvil = [o[0].floor() + 12.5, o[1] + 0.5, o[2].floor() + 9.5];
        let enchanting = [o[0].floor() + 10.5, o[1] + 0.5, o[2].floor() + 9.5];
        // r104's brewing stand (`render_check.py`).
        let brewing = [o[0].floor() + 13.5, o[1] + 0.5, o[2].floor() + 5.5];
        let button = [o[0].floor() + 10.5, o[1] + 0.05, o[2].floor() + 4.5];
        let stage_before = self.use_stage;
        match self.use_stage {
            0 if self.nautilus_stage >= 20 && seats_done => {
                self.use_tick0 = now;
                self.use_stage = 1;
            }
            1 if now >= self.use_tick0 + 5
                && self
                    .session
                    .as_ref()
                    .is_some_and(|s| s.local_vehicle().is_none()) =>
            {
                cmd(self, "gamemode survival");
                // r87's boat is done with and floats in the pool a few blocks
                // from the spot: a later right-click with the boat under the
                // crosshair mounted it instead of using the item, and the bot
                // spent the rest of the tail paddling (M236; two runs).
                cmd(self, "kill @e[type=minecraft:oak_boat]");
                // And any slime: a flat world spawns them, the use witnesses
                // run in survival, and one killed the bot mid-tail (the
                // harness also stages `gamerule spawn_monsters false`).
                // Shrunk to size 1 first: a killed slime larger than that
                // SPLITS (`AbstractCubeMob.remove`), and the medium children
                // the kill itself made hit the bot through the rest of the
                // tail (M273's second run: slimes 53-55, r101-r106 failed).
                // NBT `Size` is the size minus one; a tiny slime neither
                // splits nor deals damage (`isDealsDamage`).
                cmd(
                    self,
                    "execute as @e[type=minecraft:slime] run data merge entity @s {Size:0}",
                );
                cmd(self, "kill @e[type=minecraft:slime]");
                // And full health: the earlier tail can leave the bot hurt (one
                // run reached r101 at 3.1 health, drowned under the nautilus,
                // and r101's hits killed it). Instant health leaves the food
                // r92 eats against alone.
                let health = self.session.as_ref().map_or(-1.0, |s| s.health);
                if let Some(c) = self.check.as_mut() {
                    c.use_start_health = health;
                }
                cmd(self, "effect give @s minecraft:instant_health 1 4 true");
                cmd(self, &format!("tp @s {} {} {}", spot[0], spot[1], spot[2]));
                cmd(
                    self,
                    "item replace entity @s weapon.mainhand with minecraft:bread 4",
                );
                cmd(self, "effect give @s minecraft:hunger 2 255 true");
                self.use_tick0 = now;
                self.use_stage = 2;
            }
            // Hungry, the effect gone, the bread in hand.
            2 if now >= self.use_tick0 + 50 => {
                cmd(self, "effect clear @s minecraft:hunger");
                let food = self.session.as_ref().map_or(20, |s| s.food);
                let bread = held(self).filter(|(n, _)| n == "minecraft:bread");
                if food < 20 && bread.is_some() {
                    if let Some(c) = self.check.as_mut() {
                        c.eat_food = Some((food, food));
                        c.eat_count = bread.map(|(_, n)| (n, n));
                    }
                    // r96 first: walk east (clear ground) looking at the
                    // grass ahead, so the eat below starts mid-walk.
                    if let Some(s) = self.session.as_mut() {
                        s.player.yaw = -90.0;
                        s.player.pitch = 60.0;
                    }
                    self.keys.w = true;
                    self.walk_mark = None;
                    self.use_tick0 = now;
                    self.use_stage = 17;
                } else if now >= self.use_tick0 + 120 {
                    if let Some(c) = self.check.as_mut() {
                        c.use_skip = Some(format!("not hungry ({food}) or no bread ({bread:?})"));
                    }
                    self.use_stage = 100;
                }
            }
            // r96: the walking step on the 25th tick, then eat while walking
            // and take the step again on the 25th tick of that.
            17 | 18 => {
                let pos = self
                    .session
                    .as_ref()
                    .map(|s| [s.player.x, s.player.y, s.player.z]);
                if now >= self.use_tick0 + 24 && self.walk_mark.is_none() {
                    self.walk_mark = pos.map(|p| (p, now));
                }
                // Per tick: a frame that runs two client ticks (a catch-up)
                // puts two steps between the marks, and one run read a
                // walking "step" of exactly twice the real one (M236).
                let step = match (pos, self.walk_mark) {
                    (Some(p), Some((q, t))) if now > t => Some(
                        ((p[0] - q[0]).powi(2) + (p[2] - q[2]).powi(2)).sqrt() / (now - t) as f64,
                    ),
                    _ => None,
                };
                if let Some(st) = step {
                    if let Some(c) = self.check.as_mut() {
                        if self.use_stage == 17 {
                            c.walk_steps.0 = st;
                        } else {
                            c.walk_steps.1 = st;
                        }
                    }
                    if self.use_stage == 17 {
                        self.use_key_press();
                        self.walk_mark = None;
                        self.use_tick0 = now;
                        self.use_stage = 18;
                    } else {
                        self.keys.w = false;
                        self.use_stage = 3;
                    }
                }
            }
            // Held past the 32-tick eat, released before a second bite ends;
            // then back to the spot for r95.
            3 if now >= self.use_tick0 + 36 => {
                self.use_down = false;
                cmd(self, &format!("tp @s {} {} {}", spot[0], spot[1], spot[2]));
                self.use_tick0 = now;
                self.use_stage = 4;
            }
            4 if now >= self.use_tick0 + 10 => {
                let food = self.session.as_ref().map_or(0, |s| s.food);
                let count = held(self).map_or(0, |(_, n)| n);
                if let Some(c) = self.check.as_mut() {
                    c.eat_food = c.eat_food.map(|(a, _)| (a, food));
                    c.eat_count = c.eat_count.map(|(a, _)| (a, count));
                    c.click_food = Some(((food, count), (food, count)));
                }
                // r95 next, before r93's water can reach the lever.
                aim(self, lever);
                self.use_key_press();
                self.use_down = false;
                self.use_tick0 = now;
                self.use_stage = 15;
            }
            15 | 16 => {
                let (target, key) = if self.use_stage == 15 {
                    (lever, "lever")
                } else {
                    (button, "button")
                };
                let on = self.session.as_ref().is_some_and(|s| {
                    let st = s.world.block_state_at(
                        target[0].floor() as i32,
                        target[1].floor() as i32,
                        target[2].floor() as i32,
                    );
                    self.blocks.property(st, "powered") == Some("true")
                });
                if on || now >= self.use_tick0 + 20 {
                    if let Some(c) = self.check.as_mut() {
                        if key == "lever" {
                            c.lever_on = on;
                        } else {
                            c.button_on = on;
                        }
                    }
                    if self.use_stage == 15 {
                        aim(self, button);
                        self.use_key_press();
                        self.use_down = false;
                        self.use_tick0 = now;
                        self.use_stage = 16;
                    } else {
                        let food = self.session.as_ref().map_or(0, |s| s.food);
                        let count = held(self).map_or(0, |(_, n)| n);
                        if let Some(c) = self.check.as_mut() {
                            c.click_food = c.click_food.map(|(a, _)| (a, (food, count)));
                        }
                        cmd(
                            self,
                            "item replace entity @s weapon.mainhand with minecraft:bucket",
                        );
                        self.use_tick0 = now;
                        self.use_stage = 5;
                    }
                }
            }
            5 if now >= self.use_tick0 + 6 => {
                aim(self, pool);
                self.use_key_press();
                self.use_down = false;
                self.use_tick0 = now;
                self.use_stage = 6;
            }
            6 => {
                let filled = held(self).is_some_and(|(n, _)| n == "minecraft:water_bucket");
                if filled {
                    if let Some(c) = self.check.as_mut() {
                        c.bucket_filled = true;
                    }
                    aim(self, grass);
                    self.use_key_press();
                    self.use_down = false;
                    self.use_tick0 = now;
                    self.use_stage = 7;
                } else if now >= self.use_tick0 + 30 {
                    if let Some(c) = self.check.as_mut() {
                        c.use_skip = Some("the bucket never filled".into());
                    }
                    self.use_stage = 8;
                    self.use_tick0 = now;
                }
            }
            7 => {
                let emptied = held(self).is_some_and(|(n, _)| n == "minecraft:bucket");
                // The water goes on top of the grass that was clicked.
                let water = self.session.as_ref().is_some_and(|s| {
                    let (x, y, z) = (
                        grass[0].floor() as i32,
                        o[1].floor() as i32,
                        grass[2].floor() as i32,
                    );
                    self.blocks.block_name(s.world.block_state_at(x, y, z))
                        == Some("minecraft:water")
                });
                if emptied && water {
                    if let Some(c) = self.check.as_mut() {
                        c.bucket_emptied = true;
                        c.bucket_water_placed = true;
                    }
                    self.use_stage = 8;
                    self.use_tick0 = now;
                } else if now >= self.use_tick0 + 30 {
                    if let Some(c) = self.check.as_mut() {
                        c.bucket_emptied = emptied;
                        c.bucket_water_placed = water;
                    }
                    self.use_stage = 8;
                    self.use_tick0 = now;
                }
            }
            8 if now >= self.use_tick0 + 4 => {
                cmd(
                    self,
                    "item replace entity @s weapon.mainhand with minecraft:snowball 4",
                );
                self.use_tick0 = now;
                self.use_stage = 9;
            }
            9 if now >= self.use_tick0 + 6 => {
                // Level, away from the pool.
                aim(self, [spot[0] + 8.0, o[1] + 1.6, spot[2]]);
                self.use_key_press();
                self.use_down = false;
                self.use_tick0 = now;
                self.use_stage = 10;
            }
            10 => {
                let seen = self.session.as_ref().is_some_and(|s| {
                    let t = s
                        .entity_types
                        .as_deref()
                        .and_then(|t| t.id_of("minecraft:snowball"));
                    s.world.entities.iter().any(|(_, e)| Some(e.type_id) == t)
                });
                if seen || now >= self.use_tick0 + 30 {
                    if let Some(c) = self.check.as_mut() {
                        c.snowball_seen = seen;
                    }
                    cmd(
                        self,
                        "item replace entity @s weapon.mainhand with minecraft:fishing_rod",
                    );
                    self.use_tick0 = now;
                    self.use_stage = 11;
                }
            }
            11 if now >= self.use_tick0 + 8 => {
                aim(self, [spot[0] + 8.0, o[1] + 0.5, spot[2]]);
                self.use_key_press();
                self.use_down = false;
                self.use_tick0 = now;
                self.use_stage = 12;
            }
            12 | 13 => {
                let bobber = self.session.as_ref().is_some_and(|s| {
                    let t = s
                        .entity_types
                        .as_deref()
                        .and_then(|t| t.id_of("minecraft:fishing_bobber"));
                    s.world.entities.iter().any(|(_, e)| Some(e.type_id) == t)
                });
                if self.use_stage == 12 {
                    if bobber && now >= self.use_tick0 + 10 {
                        if let Some(c) = self.check.as_mut() {
                            c.bobber_seen = true;
                        }
                        // Reel in: the same key again.
                        self.use_key_press();
                        self.use_down = false;
                        self.use_tick0 = now;
                        self.use_stage = 13;
                    } else if now >= self.use_tick0 + 40 {
                        self.use_stage = 14;
                    }
                } else if !bobber || now >= self.use_tick0 + 30 {
                    if let Some(c) = self.check.as_mut() {
                        c.bobber_gone = !bobber;
                    }
                    self.use_stage = 14;
                }
            }
            // r97 — the crafting table, for real: a log into the grid, the
            // planks shift-clicked out.
            14 => {
                cmd(
                    self,
                    "item replace entity @s weapon.mainhand with minecraft:oak_log 1",
                );
                self.use_tick0 = now;
                self.use_stage = 30;
            }
            30 if now >= self.use_tick0 + 6 => {
                aim(self, table);
                self.use_key_press();
                self.use_down = false;
                self.use_tick0 = now;
                self.use_stage = 31;
            }
            31 => {
                if shown(self) == Some("crafting") {
                    if let Some(c) = self.check.as_mut() {
                        c.craft_opened = true;
                    }
                    // Hotbar -> cursor -> the grid's first cell.
                    let hot = 37 + selected(self);
                    press(self, hot, false);
                    press(self, 1, false);
                    self.use_tick0 = now;
                    self.use_stage = 32;
                } else if now >= self.use_tick0 + 40 {
                    let at = looking_at(self);
                    if let Some(c) = self.check.as_mut() {
                        c.use_skip.get_or_insert_with(|| {
                            format!("the crafting table never opened ({at})")
                        });
                    }
                    self.use_stage = 40;
                    self.use_tick0 = now;
                }
            }
            32 => {
                let result = menu_slot(self, 0);
                if result
                    .as_ref()
                    .is_some_and(|(n, _)| n == "minecraft:oak_planks")
                {
                    if let Some(c) = self.check.as_mut() {
                        c.craft_result = result.clone();
                    }
                    press(self, 0, true);
                    self.use_tick0 = now;
                    self.use_stage = 33;
                } else if now >= self.use_tick0 + 40 {
                    if let Some(c) = self.check.as_mut() {
                        c.craft_result = result;
                    }
                    self.use_stage = 33;
                    self.use_tick0 = now;
                }
            }
            // The player inventory is its own menu here, which the server
            // settles once the container closes (`transferState`, then
            // `broadcastChanges`): count after the close.
            33 if now >= self.use_tick0 + 10 => {
                self.close_container_screen();
                self.use_tick0 = now;
                self.use_stage = 34;
            }
            34 if now >= self.use_tick0 + 10 => {
                let planks = inventory_count(self, "minecraft:oak_planks");
                let logs = inventory_count(self, "minecraft:oak_log");
                if let Some(c) = self.check.as_mut() {
                    c.craft_inventory = Some((planks, logs));
                }
                // Slot 0 selected BEFORE the main hand is filled, so the ore
                // and the coal sit in hotbar 0 and 1 (furnace menu 30, 31).
                if let Some(s) = self.session.as_mut() {
                    let _ = s.select_hotbar(0);
                }
                cmd(
                    self,
                    "item replace entity @s weapon.mainhand with minecraft:raw_iron 1",
                );
                cmd(
                    self,
                    "item replace entity @s hotbar.1 with minecraft:coal 1",
                );
                self.use_tick0 = now;
                self.use_stage = 40;
            }
            // r98 — the furnace: ore and fuel placed by clicks, the furnace
            // lights and cooks (its container data), and the ingot staged in
            // its result slot shift-clicked out.
            40 if now >= self.use_tick0 + 8 && !self.screen.inventory_open() => {
                aim(self, furnace);
                self.use_key_press();
                self.use_down = false;
                self.use_tick0 = now;
                self.use_stage = 41;
            }
            41 => {
                if shown(self) == Some("furnace") {
                    if let Some(c) = self.check.as_mut() {
                        c.furnace_opened = true;
                    }
                    press(self, 30, false);
                    press(self, 0, false);
                    press(self, 31, false);
                    press(self, 1, false);
                    self.use_tick0 = now;
                    self.use_stage = 42;
                } else if now >= self.use_tick0 + 40 {
                    let at = looking_at(self);
                    if let Some(c) = self.check.as_mut() {
                        c.use_skip
                            .get_or_insert_with(|| format!("the furnace never opened ({at})"));
                    }
                    self.use_stage = 60;
                }
            }
            42 => {
                // `AbstractFurnaceMenu` data: 0 litTime, 2 cookingProgress.
                let (lit, cook) = self
                    .session
                    .as_ref()
                    .and_then(|s| s.menus.open())
                    .map_or((0, 0), |m| (m.data(0), m.data(2)));
                if lit > 0 && cook > 0 || now >= self.use_tick0 + 60 {
                    if let Some(c) = self.check.as_mut() {
                        c.furnace_data = Some((lit, cook));
                    }
                    press(self, 2, true);
                    self.use_tick0 = now;
                    self.use_stage = 43;
                }
            }
            43 if now >= self.use_tick0 + 10 => {
                self.close_container_screen();
                self.use_tick0 = now;
                self.use_stage = 44;
            }
            44 if now >= self.use_tick0 + 10 => {
                let ingots = inventory_count(self, "minecraft:iron_ingot");
                if let Some(c) = self.check.as_mut() {
                    c.furnace_taken = ingots;
                }
                // r99 next: a bow and arrows, looking level, standing still.
                cmd(
                    self,
                    "item replace entity @s weapon.mainhand with minecraft:bow 1",
                );
                cmd(self, "give @s minecraft:arrow 4");
                if let Some(s) = self.session.as_mut() {
                    s.player.pitch = 0.0;
                }
                self.use_tick0 = now;
                self.use_stage = 50;
            }
            // r99 — the bow's FOV: the frame's FOV standing still, then after
            // 40 ticks of drawing (the narrowing is full at 20 and the eased
            // modifier settles within a few more).
            50 if now >= self.use_tick0 + 30 => {
                let model = self.hand.as_ref().and_then(|h| h.last_main_model.clone());
                if let Some(c) = self.check.as_mut() {
                    c.bow_fov.0 = self.last_frame_fov;
                    c.bow_models.0 = model;
                }
                self.use_key_press();
                self.use_tick0 = now;
                self.use_stage = 51;
            }
            51 if now >= self.use_tick0 + 40 => {
                let drawing = self
                    .session
                    .as_ref()
                    .is_some_and(|s| s.local_use_state().using);
                let model = self.hand.as_ref().and_then(|h| h.last_main_model.clone());
                if let Some(c) = self.check.as_mut() {
                    c.bow_fov.1 = self.last_frame_fov;
                    c.bow_drawing = drawing;
                    c.bow_models.1 = model;
                }
                // Release: the server shoots r110's arrow on the release
                // packet, which leaves in the NEXT client tick. A command
                // sent now could empty the hand first (commands run off the
                // server's task queue, ahead of the player's tick), so the
                // next staging waits in stage 98.
                self.use_down = false;
                self.use_tick0 = now;
                self.use_stage = 98;
            }
            98 if now >= self.use_tick0 + 5 => {
                // r101 next: a shield in the off hand, nothing in the main
                // hand, facing south (+Z) and level.
                cmd(
                    self,
                    "item replace entity @s weapon.mainhand with minecraft:air",
                );
                cmd(
                    self,
                    "item replace entity @s weapon.offhand with minecraft:shield",
                );
                if let Some(s) = self.session.as_mut() {
                    s.player.yaw = 0.0;
                    s.player.pitch = 0.0;
                }
                self.use_tick0 = now;
                self.use_stage = 70;
            }
            // r101 — shield blocking, graded by the server's own damage: a
            // hit from in front while NOT blocking costs health (the
            // control), the same hit while the right-click holds the shield
            // up costs none. `/damage .. at <pos>` gives the source a
            // position, which `applyItemBlocking` measures the angle to.
            70 if now >= self.use_tick0 + 20 => {
                let front = self.session.as_ref().map(|s| {
                    format!(
                        "{:.2} {:.2} {:.2}",
                        s.player.x,
                        s.player.y + 1.0,
                        s.player.z + 3.0
                    )
                });
                let health = self.session.as_ref().map_or(0.0, |s| s.health);
                if let Some(c) = self.check.as_mut() {
                    c.shield_health.0 = health;
                }
                if let Some(f) = front {
                    cmd(self, &format!("damage @s 4 minecraft:mob_attack at {f}"));
                }
                self.use_tick0 = now;
                self.use_stage = 71;
            }
            // Past the 20-tick hurt invulnerability before blocking.
            71 if now >= self.use_tick0 + 30 => {
                let health = self.session.as_ref().map_or(0.0, |s| s.health);
                if let Some(c) = self.check.as_mut() {
                    c.shield_health.1 = health;
                }
                self.use_key_press();
                self.use_tick0 = now;
                self.use_stage = 72;
            }
            // Past the shield's `block_delay_seconds` (0.25 s, 5 ticks).
            72 if now >= self.use_tick0 + 12 => {
                let blocking = self.session.as_ref().is_some_and(|s| {
                    let u = s.local_use_state();
                    u.using && u.hand == rewo_world::entities::InteractionHand::OffHand
                });
                let front = self.session.as_ref().map(|s| {
                    format!(
                        "{:.2} {:.2} {:.2}",
                        s.player.x,
                        s.player.y + 1.0,
                        s.player.z + 3.0
                    )
                });
                if let Some(c) = self.check.as_mut() {
                    c.shield_up = blocking;
                }
                if let Some(f) = front {
                    cmd(self, &format!("damage @s 4 minecraft:mob_attack at {f}"));
                }
                self.use_tick0 = now;
                self.use_stage = 73;
            }
            73 if now >= self.use_tick0 + 15 => {
                let health = self.session.as_ref().map_or(0.0, |s| s.health);
                if let Some(c) = self.check.as_mut() {
                    c.shield_health.2 = health;
                }
                self.use_down = false;
                // r102 next: an iron sword in hotbar 0 (the selected slot),
                // no levels.
                cmd(
                    self,
                    "item replace entity @s weapon.offhand with minecraft:air",
                );
                cmd(
                    self,
                    "item replace entity @s hotbar.0 with minecraft:iron_sword 1",
                );
                cmd(self, "xp set @s 0 levels");
                // Back to the spot: r101's unblocked hit knocked the bot back.
                cmd(self, &format!("tp @s {} {} {}", spot[0], spot[1], spot[2]));
                if let Some(s) = self.session.as_mut() {
                    let _ = s.select_hotbar(0);
                }
                self.use_tick0 = now;
                self.use_stage = 80;
            }
            // r102 — the anvil, for real: the sword clicked into the first
            // input, a letter typed into the name field (a rename costs one
            // level), the cost line read off the drawn labels at 0 levels and
            // at 5, then the result shift-clicked out.
            // Only once the teleport has landed: aiming from where the
            // knockback left the bot missed the anvil in one run.
            80 if now >= self.use_tick0 + 10
                && self.session.as_ref().is_some_and(|s| {
                    (s.player.x - spot[0]).abs() < 0.2 && (s.player.z - spot[2]).abs() < 0.2
                }) =>
            {
                aim(self, anvil);
                self.use_key_press();
                self.use_down = false;
                self.use_tick0 = now;
                self.use_stage = 81;
            }
            81 => {
                if shown(self) == Some("anvil") {
                    // Hotbar 0 is menu slot 3 + 27 = 30.
                    press(self, 30, false);
                    press(self, 0, false);
                    self.use_tick0 = now;
                    self.use_stage = 82;
                } else if now >= self.use_tick0 + 40 {
                    let at = looking_at(self);
                    if let Some(c) = self.check.as_mut() {
                        c.use_skip
                            .get_or_insert_with(|| format!("the anvil never opened ({at})"));
                    }
                    self.use_stage = 90;
                    self.use_tick0 = now;
                }
            }
            82 if now >= self.use_tick0 + 5 => {
                if let Some(s) = self.session.as_mut() {
                    super::menus::anvil_char(
                        s,
                        &mut self.screen,
                        &self.items,
                        self.baked.as_ref(),
                        'X',
                    );
                }
                self.use_tick0 = now;
                self.use_stage = 83;
            }
            83 => {
                let ready = self.session.as_ref().is_some_and(|s| {
                    s.menus
                        .open()
                        .is_some_and(|m| m.data(0) > 0 && m.menu.menu_slot(2).is_some())
                });
                if ready && now >= self.use_tick0 + 5 || now >= self.use_tick0 + 40 {
                    let label = self.last_anvil_label.clone();
                    if let Some(c) = self.check.as_mut() {
                        c.anvil_labels.0 = label;
                    }
                    cmd(self, "xp set @s 5 levels");
                    self.use_tick0 = now;
                    self.use_stage = 84;
                }
            }
            84 if now >= self.use_tick0 + 10 => {
                let label = self.last_anvil_label.clone();
                let before = xp(self);
                if let Some(c) = self.check.as_mut() {
                    c.anvil_labels.1 = label;
                    c.anvil_xp.0 = before;
                }
                press(self, 2, true);
                self.use_tick0 = now;
                self.use_stage = 85;
            }
            85 if now >= self.use_tick0 + 10 => {
                self.close_container_screen();
                self.use_tick0 = now;
                self.use_stage = 86;
            }
            86 if now >= self.use_tick0 + 10 => {
                let after = xp(self);
                let swords = inventory_count(self, "minecraft:iron_sword");
                if let Some(c) = self.check.as_mut() {
                    c.anvil_xp.1 = after;
                    c.anvil_swords = swords;
                }
                // r103 next: a pickaxe and lapis in hotbar 0 and 1, levels
                // to spend.
                cmd(self, "clear @s minecraft:iron_sword");
                cmd(
                    self,
                    "item replace entity @s hotbar.0 with minecraft:iron_pickaxe 1",
                );
                cmd(
                    self,
                    "item replace entity @s hotbar.1 with minecraft:lapis_lazuli 3",
                );
                cmd(self, "xp set @s 30 levels");
                self.use_tick0 = now;
                self.use_stage = 90;
            }
            // r103 — the enchanting table: pickaxe and lapis clicked in, the
            // server's three offers arrive (data 0..2), the first row is
            // pressed, and the pickaxe comes back enchanted for one lapis and
            // one level.
            90 if now >= self.use_tick0 + 10 && !self.screen.inventory_open() => {
                aim(self, enchanting);
                self.use_key_press();
                self.use_down = false;
                self.use_tick0 = now;
                self.use_stage = 91;
            }
            91 => {
                if shown(self) == Some("enchantment") {
                    // Hotbar 0 and 1 are menu slots 2 + 27 = 29 and 30.
                    press(self, 29, false);
                    let carried = self.session.as_ref().and_then(|s| {
                        let c = s.shown_menu().carried()?;
                        Some((self.items.name(c.item_id)?.to_string(), c.count))
                    });
                    let mouse = self.screen.mouse;
                    press(self, 0, false);
                    press(self, 30, false);
                    press(self, 1, false);
                    if let Some(c) = self.check.as_mut() {
                        c.enchant_carried = Some((carried, mouse));
                    }
                    self.use_tick0 = now;
                    self.use_stage = 92;
                } else if now >= self.use_tick0 + 40 {
                    let at = looking_at(self);
                    if let Some(c) = self.check.as_mut() {
                        c.use_skip.get_or_insert_with(|| {
                            format!("the enchanting table never opened ({at})")
                        });
                    }
                    self.use_stage = 60;
                }
            }
            92 => {
                let costs = self
                    .session
                    .as_ref()
                    .and_then(|s| s.menus.open())
                    .map_or([0; 3], |m| m.enchant_costs());
                // The three data slots arrive one packet each: wait for all
                // of them (rows 1 and 2 may stay 0 with no bookshelves, so
                // not forever).
                if costs.iter().all(|&c| c > 0)
                    || (costs[0] > 0 && now >= self.use_tick0 + 15)
                    || now >= self.use_tick0 + 40
                {
                    if costs[0] <= 0 {
                        let slots: Vec<_> =
                            (0..2).chain(29..31).map(|i| menu_slot(self, i)).collect();
                        let at = looking_at(self);
                        if let Some(c) = self.check.as_mut() {
                            c.use_skip.get_or_insert_with(|| {
                                format!("no offers: menu slots 0, 1, 29, 30 {slots:?} ({at})")
                            });
                        }
                    }
                    let before = xp(self);
                    if let Some(c) = self.check.as_mut() {
                        c.enchant_costs = costs;
                        c.enchant_xp.0 = before;
                    }
                    press_row(self, 0);
                    self.use_tick0 = now;
                    self.use_stage = 93;
                }
            }
            93 if now >= self.use_tick0 + 10 => {
                let enchanted = self.session.as_ref().is_some_and(|s| {
                    s.menus
                        .open()
                        .and_then(|m| m.menu.menu_slot(0))
                        .is_some_and(|st| st.enchanted)
                });
                let lapis = self
                    .session
                    .as_ref()
                    .and_then(|s| s.menus.open())
                    .map_or(-1, |m| m.enchant_lapis());
                let after = xp(self);
                if let Some(c) = self.check.as_mut() {
                    c.enchant_result = (enchanted, lapis);
                    c.enchant_xp.1 = after;
                }
                self.close_container_screen();
                // r104 next: a water bottle, nether wart and blaze powder.
                cmd(self, "clear @s minecraft:iron_pickaxe");
                cmd(
                    self,
                    "item replace entity @s hotbar.0 with minecraft:potion[minecraft:potion_contents={potion:\"minecraft:water\"}] 1",
                );
                cmd(
                    self,
                    "item replace entity @s hotbar.1 with minecraft:nether_wart 1",
                );
                cmd(
                    self,
                    "item replace entity @s hotbar.2 with minecraft:blaze_powder 1",
                );
                self.use_tick0 = now;
                self.use_stage = 95;
            }
            // r104 — the brewing stand: the bottle clicked into slot 0, the
            // wart and the powder SHIFT-clicked (the wart to the ingredient
            // slot, the powder to the fuel slot, `quickMoveStack`'s order),
            // and the stand starts brewing (data 0 brew time, 1 fuel).
            95 if now >= self.use_tick0 + 10 && !self.screen.inventory_open() => {
                aim(self, brewing);
                self.use_key_press();
                self.use_down = false;
                self.use_tick0 = now;
                self.use_stage = 96;
            }
            96 => {
                if shown(self) == Some("brewing_stand") {
                    if let Some(c) = self.check.as_mut() {
                        c.brew_opened = true;
                    }
                    // Hotbar 0, 1, 2 are menu slots 5 + 27 = 32, 33, 34.
                    press(self, 32, false);
                    press(self, 0, false);
                    press(self, 33, true);
                    press(self, 34, true);
                    self.use_tick0 = now;
                    self.use_stage = 97;
                } else if now >= self.use_tick0 + 40 {
                    let at = looking_at(self);
                    if let Some(c) = self.check.as_mut() {
                        c.use_skip.get_or_insert_with(|| {
                            format!("the brewing stand never opened ({at})")
                        });
                    }
                    self.use_stage = 60;
                }
            }
            97 => {
                let state = self.session.as_ref().and_then(|s| {
                    let m = s.menus.open()?;
                    Some((m.data(0), m.data(1)))
                });
                let slots: Vec<_> = [0, 3, 4].iter().map(|&i| menu_slot(self, i)).collect();
                if state.is_some_and(|(t, f)| t > 0 && f > 0) || now >= self.use_tick0 + 40 {
                    if let Some(c) = self.check.as_mut() {
                        c.brew_state = state;
                        c.brew_slots = slots;
                    }
                    self.close_container_screen();
                    self.use_tick0 = now;
                    self.use_stage = 78;
                }
            }
            // r106 next: an elytra, three rockets, thirty blocks up, looking
            // level east over the open flat world. A few ticks AFTER the
            // close: the server handles `container_close` in the player's
            // tick but runs a chat command off its task queue, so a command
            // sent right behind the close can still find the stand open and
            // send its slot changes to the stand's container id, which the
            // client (as vanilla's) drops once it has closed, and the server
            // then believes they arrived. One run lost the rockets that way.
            78 if now >= self.use_tick0 + 4 => {
                cmd(
                    self,
                    "item replace entity @s armor.chest with minecraft:elytra",
                );
                cmd(
                    self,
                    "item replace entity @s weapon.mainhand with minecraft:firework_rocket 3",
                );
                cmd(
                    self,
                    &format!("tp @s {} {} {}", spot[0], spot[1] + 30.0, spot[2]),
                );
                self.use_tick0 = now;
                self.use_stage = 74;
            }
            // r106 — the firework boost: take off (a jump press while
            // falling with an elytra), right-click a rocket, and the rocket
            // the server attaches to the bot pushes it each tick (a
            // client-side push in vanilla: the server only replays the
            // client's moves).
            74 if now >= self.use_tick0 + 8 && !self.screen.inventory_open() => {
                if let Some(s) = self.session.as_mut() {
                    s.player.yaw = -90.0;
                    s.player.pitch = 0.0;
                }
                self.keys.jump = true;
                self.use_tick0 = now;
                self.use_stage = 75;
            }
            75 if now > self.use_tick0 => {
                self.keys.jump = false;
                let gliding = self
                    .session
                    .as_ref()
                    .and_then(|s| s.local_player_view())
                    .is_some_and(|v| v.fall_flying);
                if gliding {
                    self.walk_mark = None;
                    let corrections = self.session.as_ref().map_or(0, |s| s.corrections);
                    if let Some(c) = self.check.as_mut() {
                        c.boost_glided = true;
                        c.boost_corrections.0 = corrections;
                    }
                    self.use_tick0 = now;
                    self.use_stage = 76;
                } else if now >= self.use_tick0 + 20 {
                    // Try the take-off once more: the press may have landed
                    // before the fall began.
                    self.keys.jump = true;
                    self.use_tick0 = now;
                }
            }
            // Two ticks of plain gliding (the step before the push), then the
            // rocket.
            76 => {
                let pos = self
                    .session
                    .as_ref()
                    .map(|s| [s.player.x, s.player.y, s.player.z]);
                if let (Some(p), Some((q, t))) = (pos, self.walk_mark) {
                    if now > t && now >= self.use_tick0 + 3 {
                        let step = ((p[0] - q[0]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
                            / (now - t) as f64;
                        if let Some(c) = self.check.as_mut() {
                            c.boost_steps.0 = step;
                        }
                        self.use_key_press();
                        self.use_down = false;
                        self.walk_mark = Some((p, now));
                        self.use_tick0 = now;
                        self.use_stage = 77;
                    }
                } else if now > self.use_tick0 {
                    self.walk_mark = pos.map(|p| (p, now));
                }
            }
            // Twenty ticks of the boost: the fastest per-tick horizontal step,
            // and whether a rocket was ever attached to the bot.
            77 => {
                let pos = self
                    .session
                    .as_ref()
                    .map(|s| [s.player.x, s.player.y, s.player.z]);
                let rockets = self.session.as_ref().map_or(0, |s| s.attached_rockets());
                let seen: Vec<(i32, Option<i32>)> = self.session.as_ref().map_or(Vec::new(), |s| {
                    let rocket = s.firework_type_id;
                    s.world
                        .entities
                        .iter()
                        .filter(|(_, e)| Some(e.type_id) == rocket)
                        .map(|(id, _)| (id, s.world.entities.attached_to(id)))
                        .collect()
                });
                let bot = self.session.as_ref().and_then(|s| s.player_id);
                if !seen.is_empty() {
                    if let Some(c) = self.check.as_mut() {
                        c.boost_seen = seen.clone();
                        c.boost_bot = bot;
                    }
                }
                if let (Some(p), Some((q, t))) = (pos, self.walk_mark) {
                    if now > t {
                        let step = ((p[0] - q[0]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
                            / (now - t) as f64;
                        if let Some(c) = self.check.as_mut() {
                            c.boost_steps.1 = c.boost_steps.1.max(step);
                            c.boost_rocket |= rockets > 0;
                        }
                        self.walk_mark = Some((p, now));
                    }
                }
                if now >= self.use_tick0 + 20 {
                    let corrections = self.session.as_ref().map_or(0, |s| s.corrections);
                    let left = inventory_count(self, "minecraft:firework_rocket");
                    let flying = self
                        .session
                        .as_ref()
                        .and_then(|s| s.local_player_view())
                        .is_some_and(|v| v.fall_flying);
                    if let Some(c) = self.check.as_mut() {
                        c.boost_corrections.1 = corrections;
                        c.boost_after = (left, flying);
                    }
                    cmd(self, "gamemode creative");
                    cmd(
                        self,
                        "item replace entity @s armor.chest with minecraft:air",
                    );
                    cmd(self, &format!("tp @s {} {} {}", spot[0], spot[1], spot[2]));
                    self.use_stage = 60;
                }
            }
            60 => {
                cmd(self, "gamemode creative");
                self.use_stage = 100;
            }
            _ => {}
        }
        if self.use_stage != stage_before {
            if let Some(s) = self.session.as_ref() {
                log::info!(
                    "render-check: use stage {stage_before} -> {} at tick {now}: pos ({:.2}, {:.2}, {:.2}) yaw {:.1} health {} vehicle {:?}",
                    self.use_stage,
                    s.player.x,
                    s.player.y,
                    s.player.z,
                    s.player.yaw,
                    s.health,
                    s.local_vehicle()
                );
            }
        }
    }

    /// M228 — r91: ask the server where it seats the bot, right after a
    /// vehicle's own position was asked, and note where the client put it.
    ///
    /// NOT `data get entity @s Pos`: `Entity.saveWithoutId` stores a
    /// passenger's `Pos` as `(vehicle.x, own y, vehicle.z)`, so it can never
    /// show a horizontal seat offset (the first cut of r91 read the camel's
    /// 0.5 forward driver seat as a client error that way). A marker
    /// summoned `at @s` stands at the bot's live `position()`, and a marker
    /// rides nothing, so its own `Pos` is honest.
    pub(super) fn seat_ask(&mut self, what: &'static str) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let _ = s
            .send_command("execute at @s run summon minecraft:marker ~ ~ ~ {Tags:[\"rewo_seat\"]}");
        let _ = s.send_command(
            "data get entity @e[type=minecraft:marker,tag=rewo_seat,limit=1,sort=nearest] Pos",
        );
        let _ = s.send_command("kill @e[type=minecraft:marker,tag=rewo_seat]");
        let client = [s.player.x, s.player.y, s.player.z];
        if let Some(c) = self.check.as_mut() {
            c.seat_checks.push((what, client, None));
        }
    }

    /// M228 — r91: the oldest unanswered seat question takes the newest
    /// "Marker has the following entity data" line it has not seen yet.
    pub(super) fn seat_resolve(&mut self) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let Some(c) = self.check.as_mut() else {
            return;
        };
        let Some(idx) = c.seat_checks.iter().position(|e| e.2.is_none()) else {
            return;
        };
        let reply = s
            .chat_log
            .iter()
            .rev()
            .find(|l| l.contains("Marker has the following entity data"))
            .cloned();
        let Some(line) = reply else {
            return;
        };
        if self.seat_last_line.as_ref() == Some(&line) {
            return;
        }
        let nums: Vec<f64> = line
            .split(['[', ']', ','])
            .filter_map(|t| t.trim().trim_end_matches('d').parse::<f64>().ok())
            .collect();
        if nums.len() == 3 {
            c.seat_checks[idx].2 = Some([nums[0], nums[1], nums[2]]);
            self.seat_last_line = Some(line);
        }
    }
}

impl LiveApp {
    /// Whether the render check is still inside its timed run (M222), not
    /// the tail.
    pub(super) fn in_timed_run(&self) -> bool {
        let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
        self.started.elapsed().as_secs_f32() < limit
    }
}

impl LiveApp {
    /// M86's gate drives the inventory open for the second half of its run.
    ///
    /// The screen is one of the paths the bake fix re-enables, and it is the
    /// one carrying the ninth instance of this milestone's bug —
    /// `VelvetTextPass::sync_atlas` destroying its glyph image and rewriting
    /// its descriptor set in place, whose own comment said "the caller is
    /// expected to have idled" of a caller that does not. Nothing reaches it
    /// except an open inventory, so a check that never opens one would grade
    /// that fix not at all. The mouse is parked over the panel so a tooltip
    /// lays out glyphs and the cache actually goes dirty.
    pub(super) fn render_check_inject(&mut self) {
        if let Some(c) = self.check.as_ref() {
            // Half of *this* run, not half of the default: `--render-check
            // --run-seconds 4` would otherwise never reach the trigger and
            // `r16` would fail with the screen having never opened. Caught by
            // exactly that.
            let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
            let half = c.frames > 0 && self.started.elapsed().as_secs_f32() >= limit * 0.5;
            // M89 note: the guard is `!screen_forced_open`, not
            // `!inventory_open()`. Since M89 the injected container opens the
            // screen at 0.4, so an `inventory_open()` guard skips this whole
            // branch — INCLUDING the cursor park below, which is the only
            // thing that lays out a tooltip and therefore the only door to
            // `VelvetTextPass::sync_atlas`. `r16` would have stayed green
            // while no longer proving anything it was written for, which is
            // the same vacuity `p3` exists to catch in `containershot`.
            if half && !self.screen_forced_open {
                self.screen_forced_open = true;
                if !self.screen.inventory_open() {
                    self.set_screen_open(true);
                }
                // **After** the open, not before: `grab_for_screen` parks the
                // cursor in the middle of the window, which would overwrite
                // this. Menu slot 36 is hotbar slot 0, which the spawn handler
                // fills with a stack of dirt on the creative test server.
                //
                // The slot choice is not cosmetic. A tooltip is the only thing
                // in this client that lays out Velvet glyphs, an *empty* slot
                // produces none, and without one the glyph cache never goes
                // dirty and `VelvetTextPass::sync_atlas` never runs at all. Two
                // earlier cuts of this — a plausible-looking fraction of the
                // window, then the right slot set before the open — both left
                // the M3 mutation (deleting that path's `wait_idle`) alive
                // through the entire gate.
                if let Some(s) = self.state.as_ref() {
                    let e = s.window.inner_size();
                    let r = screen_slot_rects(e.width as f32, e.height as f32)
                        [rewo_world::inventory::HOTBAR_MENU_START];
                    self.screen.mouse = ((r.0 + r.2 * 0.5) as f64, (r.1 + r.2 * 0.5) as f64);
                }
            }
            // M88 — six-tenths through, open a CONTAINER over the inventory.
            //
            // Injected as a raw `open_screen` body through the production
            // router rather than staged by interacting with a real chest,
            // which is M17's precedent and its reasoning: raw-packet injection
            // into the production dispatcher is the deterministic proof, where
            // a live encounter depends on the server's own timing and on the
            // client aiming at the right block. What is being graded here is
            // the *render*, and this drives the whole chain that feeds it —
            // decode, layout resolution, `Menus::apply_open_screen`, and the
            // frame loop's choice of which menu to draw.
            //
            // A generic_9x3 chest: menu id 2, 63 slots, and a 168-tall panel,
            // so its geometry is distinguishable from the player's 46-slot
            // 166-tall one by more than rounding.
            // Four-tenths through — BEFORE the gate force-opens the inventory
            // at half (M89). The ordering is the witness: if the screen is up
            // between 0.4 and 0.5 it can only be because `open_screen` opened
            // it, which is the behaviour M87 was missing and M89 added. With
            // the injection after the forced open, the container would render
            // either way and `r21` could not tell.
            // M94 — open the recipe book, which no server does unprompted: a
            // fresh player's `RecipeBookSettings` are all shut, so without this
            // the windowed client never reaches the book's draw and `r23` would
            // be measuring nothing. Injected EARLY so it is live while the
            // inventory (a CRAFTING book menu) is on screen for `r16`.
            //
            // Eight booleans, four `(open, filtering)` pairs in
            // `RecipeBookSettings`' positional order — crafting first.
            if !self.book_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.92 {
                    if let Some(session) = self.session.as_mut() {
                        let id = session.ids.cb_play_recipe_book_settings;
                        let body = [1u8, 0, 0, 0, 0, 0, 0, 0];
                        session.apply_recipe_book(id, &body);
                        if session.recipe_book_settings.crafting.open {
                            self.book_injected = true;
                        }
                    }
                }
            }
            // M110 — force-open the chat screen a fifth of the way in. A
            // windowed run has no keyboard, so without this `T` reaches
            // nothing the gate can see and r27 measures a path no test drives
            // — the M86 shape. It goes in EARLY and stays open: the screen is
            // closed by nothing here, so every later frame carries its bar and
            // the count is unambiguous.
            // M132 — an objective, four scores and a display-slot assignment,
            // as raw bodies through the production router (M17's rule). No
            // server sets a sidebar up unprompted, so without this r42 would
            // be a witness over a path the gate cannot reach.
            //
            // Injected EARLY and never cleared, so the sidebar is live for
            // most of the run and its frame count is unambiguous. The fourth
            // holder is named `#hidden`, which `PlayerScoreEntry.isHidden`
            // filters out: a client that skipped that filter draws nine text
            // lines rather than seven and fails r42 on the count.
            if !self.sidebar_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.1 {
                    if let Some(session) = self.session.as_mut() {
                        let obj = "rewo_sidebar";
                        let mc_string = |b: &mut Vec<u8>, s: &str| {
                            rewo_proto::varint::write_varint(b, s.len() as i32);
                            b.extend_from_slice(s.as_bytes());
                        };
                        let nbt_string = |b: &mut Vec<u8>, s: &str| {
                            b.push(8); // TAG_String
                            b.extend_from_slice(&(s.len() as u16).to_be_bytes());
                            b.extend_from_slice(s.as_bytes());
                        };
                        // `set_objective`, METHOD_ADD.
                        let mut body: Vec<u8> = Vec::new();
                        mc_string(&mut body, obj);
                        body.push(0); // ADD
                        nbt_string(&mut body, "Rewo Sidebar");
                        rewo_proto::varint::write_varint(&mut body, 0); // INTEGER
                        body.push(0); // no number format
                        session.inject_packet(session.ids.cb_play_set_objective, &body);

                        // Four holders. The scores are deliberately out of
                        // order on the wire, so a client that drew them in
                        // arrival order rather than by value would place them
                        // differently — which the sort test pins and this
                        // keeps honest end to end.
                        for (owner, value) in [
                            ("RewoAlpha", 3),
                            ("#hidden", 999),
                            ("RewoGamma", 9),
                            ("RewoBeta", 6),
                        ] {
                            let mut b: Vec<u8> = Vec::new();
                            mc_string(&mut b, owner);
                            mc_string(&mut b, obj);
                            rewo_proto::varint::write_varint(&mut b, value);
                            b.push(0); // no display override
                            b.push(0); // no number format
                            session.inject_packet(session.ids.cb_play_set_score, &b);
                        }

                        // `set_display_objective` — slot 1 is SIDEBAR. Sent
                        // last, because it is what makes the panel appear.
                        let mut b: Vec<u8> = Vec::new();
                        rewo_proto::varint::write_varint(
                            &mut b,
                            rewo_net::scoreboard::DisplaySlot::Sidebar.id(),
                        );
                        mc_string(&mut b, obj);
                        session.inject_packet(session.ids.cb_play_set_display_objective, &b);
                    }
                    self.sidebar_injected = true;
                }
            }
            // M151 — four players onto the tab list, and hold Tab.
            //
            // Both halves are needed and neither exists in a windowed run: a
            // fresh test server has exactly one player (this client), and
            // there is no keyboard, so without this r47 would be a witness
            // over a path the gate cannot reach — worse than no witness.
            //
            // **Two of the four are `listed: false`**, which is what makes the
            // row count a claim about `getListedOnlinePlayers()` rather than
            // about arrival: the list must come to THREE (the local player
            // plus the two listed ones), where a client that ignored the flag
            // scores five.
            //
            // The key goes down at half, not at the start, so
            // `tab_list_frames` must be strictly less than `frames` — a client
            // that drew the list unconditionally is exactly as wrong as one
            // that never drew it, and a `> 0` threshold cannot tell them
            // apart.
            {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                let elapsed = self.started.elapsed().as_secs_f32();
                if !self.tab_list_injected && elapsed >= limit * 0.45 {
                    if let Some(session) = self.session.as_mut() {
                        // ADD_PLAYER (0) | UPDATE_GAME_MODE (2) |
                        // UPDATE_LISTED (3) | UPDATE_LATENCY (4).
                        let mask = (1u8 << 0) | (1u8 << 2) | (1u8 << 3) | (1u8 << 4);
                        let mut body: Vec<u8> = vec![mask];
                        rewo_proto::varint::write_varint(&mut body, 4);
                        // `(name, listed, gamemode, latency)`. The spectator is
                        // named so that it would sort FIRST alphabetically and
                        // must not: `PLAYER_COMPARATOR` puts spectators after
                        // everyone else, one key ahead of the name.
                        for (i, (name, listed, mode, ms)) in [
                            ("RewoZulu", true, 0i32, 10i32),
                            ("RewoAlpha", true, 3, 800),
                            ("RewoHidden", false, 0, 50),
                            ("RewoGhost", false, 3, 50),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            // A uuid far from any real profile's, so a
                            // collision with the local player is impossible.
                            let uuid = 0x5157_0000_0000_0000_0000_0000_0000_0000u128 + i as u128;
                            body.extend_from_slice(&uuid.to_be_bytes());
                            rewo_proto::varint::write_varint(&mut body, name.len() as i32);
                            body.extend_from_slice(name.as_bytes());
                            rewo_proto::varint::write_varint(&mut body, 0); // no properties
                            rewo_proto::varint::write_varint(&mut body, mode);
                            body.push(u8::from(listed));
                            rewo_proto::varint::write_varint(&mut body, ms);
                        }
                        session.inject_packet(session.ids.cb_play_player_info_update, &body);
                    }
                    // M163 — a zombie whose custom name is a TRANSLATABLE, so
                    // r48 can see whether the windowed client's decode reached
                    // a language table. Injected as raw bodies through the
                    // production router (M17's rule): a live encounter would
                    // depend on a server spawning a mob and naming it, which is
                    // neither deterministic nor stageable without a third
                    // caller requirement (deliberately not added -- §0.0).
                    if let Some(session) = self.session.as_mut() {
                        if let Some(zombie) = self.etypes.id_of("minecraft:zombie") {
                            // `add_entity`: id, uuid, type, xyz, LpVec3 zero
                            // sentinel, then pitch / yaw / head-yaw bytes.
                            let mut add: Vec<u8> = Vec::new();
                            rewo_proto::varint::write_varint(&mut add, NAMETAG_WITNESS_ID);
                            add.extend_from_slice(
                                &0x5257_0000_0000_0000_0000_0000_0000_0001u128.to_be_bytes(),
                            );
                            rewo_proto::varint::write_varint(&mut add, zombie);
                            // Beside the PLAYER, not at a fixed point: the
                            // label is gated on `name_tag_distance`, so a
                            // zombie at the origin scores zero for a reason
                            // that has nothing to do with the flatten. The
                            // first cut did exactly that and r48 read 0/0.
                            let p = &session.player;
                            for v in [p.x + 2.0, p.y, p.z] {
                                add.extend_from_slice(&v.to_be_bytes());
                            }
                            add.push(0); // LpVec3's zero sentinel
                            add.extend_from_slice(&[0, 0, 0]); // pitch, yaw, head yaw
                            session.inject_packet(session.ids.cb_play_add_entity, &add);
                            // `set_entity_data`: index 2, OPTIONAL_COMPONENT
                            // (serializer 6), present, `{translate: "..."}`.
                            let key = "entity.minecraft.zombie";
                            let mut meta: Vec<u8> = Vec::new();
                            rewo_proto::varint::write_varint(&mut meta, NAMETAG_WITNESS_ID);
                            meta.extend_from_slice(&[2, 6, 1]);
                            meta.extend_from_slice(&[10, 8]);
                            meta.extend_from_slice(&9u16.to_be_bytes());
                            meta.extend_from_slice(b"translate");
                            meta.extend_from_slice(&(key.len() as u16).to_be_bytes());
                            meta.extend_from_slice(key.as_bytes());
                            meta.push(0); // TAG_End
                                          // …and `DATA_CUSTOM_NAME_VISIBLE` (index 3,
                                          // BOOLEAN) in the same body, because
                                          // `Entity.shouldShowName()` IS
                                          // `isCustomNameVisible()` for everything but a
                                          // player — a named mob nobody has told to show its
                                          // tag draws none, and `resolve_labels` returns
                                          // `None` rather than the string.
                            meta.extend_from_slice(&[3, 8, 1]);
                            meta.push(0xFF); // metadata terminator
                            session.inject_packet(session.ids.cb_play_set_entity_data, &meta);
                        }
                    }
                    // M163 — a SIGN whose front text is a translatable, for
                    // r49. Written straight into the world rather than
                    // injected as two packets, and the reason is a decode rule
                    // rather than convenience: `World::set_block_entity_data`
                    // keeps vanilla's "a position with no block entity is
                    // ignored" (`lib.rs:218-221`), so a `block_entity_data`
                    // packet cannot CREATE one — a block entity arrives only
                    // in a chunk payload, and staging a real sign would mean a
                    // third caller requirement plus an SNBT form nothing here
                    // can check.
                    //
                    // What r49 grades is downstream of that anyway: the claim
                    // is that the windowed frame loop hands the language table
                    // to the sign collector, not that a block entity can be
                    // conjured mid-session.
                    if let Some(session) = self.session.as_mut() {
                        if let Some(state) = self.blocks.default_state("minecraft:oak_sign") {
                            let (dx, dy, dz) = SIGN_WITNESS_OFFSET;
                            let (x, y, z) = (
                                session.player.x.floor() as i32 + dx,
                                session.player.y.floor() as i32 + dy,
                                session.player.z.floor() as i32 + dz,
                            );
                            session.world.set_block(x, y, z, state);
                            let face = rewo_proto::nbt::Nbt::Compound(vec![(
                                "messages".to_string(),
                                rewo_proto::nbt::Nbt::List(vec![
                                    rewo_proto::nbt::Nbt::Compound(vec![(
                                        "translate".to_string(),
                                        rewo_proto::nbt::Nbt::String(SIGN_WITNESS_KEY.to_string()),
                                    )]),
                                    rewo_proto::nbt::Nbt::String(String::new()),
                                    rewo_proto::nbt::Nbt::String(String::new()),
                                    rewo_proto::nbt::Nbt::String(String::new()),
                                ]),
                            )]);
                            session.world.block_entities.insert(
                                rewo_world::block_entities::BlockEntityPos { x, y, z },
                                rewo_world::block_entities::BlockEntity {
                                    // The type id is not read by the sign path
                                    // — `collect_sign_text` keys off the BLOCK
                                    // STATE through `SignStates::get` — so this
                                    // is a marker rather than a claim about the
                                    // block-entity registry.
                                    type_id: -1,
                                    data: rewo_proto::nbt::Nbt::Compound(vec![(
                                        "front_text".to_string(),
                                        face,
                                    )]),
                                },
                            );
                            log::info!("render-check: sign witness at ({x},{y},{z})");
                            self.sign_witness_pos = Some((x, y, z));
                        }
                    }
                    self.tab_list_injected = true;
                }
                // r48 — TWO mobs of one type plus one of another, injected
                // through the production `add_entity` route (M17/M88's rule:
                // injection is the deterministic proof where a live encounter
                // depends on the server's spawn timing).
                //
                // This is the only check anywhere that drives
                // `etypes.name(type_id)` -> `kind_for_entity_name`: every
                // serverless gate, this milestone's `mobtexshot` included,
                // hands `kind` straight to `EntityDraw`. It is also the only
                // one that puts more than one mob in the WINDOWED client's
                // draw list, which is the axis §0.0's "a mob renders with
                // another mob's texture when more than one is in the scene"
                // report is defined by.
                //
                // The ids are fixed and named in `CROWD_IDS` because the census
                // below joins on them: without that, the count is a claim about
                // whatever the world contains rather than about this injection.
                // r75 — M184's lectern, between the options pages (closed by
                // 0.16) and r74 (0.24). Stage 1: right-click the staged
                // lectern through `use_item_on`, the production interaction.
                // Stage 2: once the reader shows page 0 of both pages, click
                // the forward arrow's centre through `lectern_arrow_click`.
                // Stage 3: after the echo, close through `close_book`.
                if self.lectern_stage == 0
                    && elapsed >= limit * 0.17
                    && !self.screen.inventory_open()
                {
                    let found = self.session.as_ref().and_then(|s| {
                        let (px, py, pz) = (
                            s.player.x.floor() as i32,
                            s.player.y.floor() as i32,
                            s.player.z.floor() as i32,
                        );
                        // By block state, not block entity: a lectern has no
                        // client update packet, so a `setblock` after the chunk
                        // loaded reaches the client as a bare block update.
                        let mut hit = None;
                        'scan: for dy in -3..=3 {
                            for dx in -6..=6 {
                                for dz in -6..=6 {
                                    let (x, y, z) = (px + dx, py + dy, pz + dz);
                                    let st = s.world.block_state_at(x, y, z);
                                    if self.blocks.block_name(st) == Some("minecraft:lectern") {
                                        hit = Some((x, y, z));
                                        break 'scan;
                                    }
                                }
                            }
                        }
                        hit
                    });
                    match found {
                        Some((x, y, z)) => {
                            if let Some(s) = self.session.as_mut() {
                                let _ = s.use_item_on(x, y, z, 1);
                            }
                            if let Some(c) = self.check.as_mut() {
                                c.lectern_pos = Some((x, y, z));
                            }
                            self.lectern_stage = 1;
                        }
                        None if elapsed >= limit * 0.2 => {
                            if let Some(c) = self.check.as_mut() {
                                c.lectern_skip =
                                    Some("no lectern block entity near the bot".into());
                            }
                            self.lectern_stage = 3;
                        }
                        None => {}
                    }
                }
                if self.lectern_stage == 1 && self.lectern_shown.is_some() {
                    let pages = self.lectern_pages.as_ref().map_or(0, |p| p.len());
                    let page = self.book.as_ref().map(|b| b.current_page());
                    if pages > 0 && page == Some(0) {
                        if let Some(c) = self.check.as_mut() {
                            c.lectern_pages_seen = pages;
                        }
                        let gw = self.gui_size().0;
                        let (x, y, w, h) =
                            rewo_ui::book_view_screen::BookViewScreen::forward_rect(gw);
                        self.lectern_arrow_click(
                            f64::from(x) + f64::from(w) / 2.0,
                            f64::from(y) + f64::from(h) / 2.0,
                            gw,
                        );
                        self.lectern_stage = 2;
                    }
                }
                if self.lectern_stage == 2 {
                    let menu_page = self
                        .session
                        .as_ref()
                        .and_then(|s| super::screens::lectern_menu_state(&s.menus))
                        .map(|(_, _, page)| page);
                    let reader_page = self.book.as_ref().map(|b| b.current_page());
                    if menu_page == Some(1) && reader_page == Some(1) {
                        if let Some(c) = self.check.as_mut() {
                            c.lectern_echo_frames += 1;
                        }
                    }
                    let echoed = self
                        .check
                        .as_ref()
                        .is_some_and(|c| c.lectern_echo_frames >= 3);
                    if echoed || elapsed >= limit * 0.23 {
                        self.close_book();
                        self.lectern_stage = 3;
                    }
                }
                if self.lectern_stage == 3 {
                    if let (Some(c), Some(s)) = (self.check.as_mut(), self.session.as_ref()) {
                        if c.lectern_pos.is_some()
                            && !c.lectern_closed
                            && super::screens::lectern_menu_state(&s.menus).is_none()
                            && self.lectern_shown.is_none()
                        {
                            c.lectern_closed = true;
                        }
                    }
                }
                // r78 — M217's survival mining, between the lectern (closed
                // by 0.23 at the latest) and r76 (0.25), before the horse
                // mounts the bot at 0.28 (riding takes `onGround` away and
                // divides the speed by five). The bot is op: it asks for
                // survival itself, mines through the production press and
                // `mining_tick`, then asks for creative again.
                if self.mine_stage == 0
                    && self.lectern_stage == 3
                    && elapsed >= limit * 0.19
                    && !self.screen.inventory_open()
                {
                    if let Some(s) = self.session.as_mut() {
                        let _ = s.send_command("gamemode survival");
                    }
                    self.mine_stage = 1;
                }
                if self.mine_stage == 1 {
                    let survival = self
                        .session
                        .as_ref()
                        .is_some_and(|s| !s.abilities.instabuild);
                    if survival {
                        let carpet = self.session.as_ref().and_then(|s| {
                            let (px, py, pz) = (
                                s.player.x.floor() as i32,
                                s.player.y.floor() as i32,
                                s.player.z.floor() as i32,
                            );
                            let mut hit = None;
                            'scan: for dy in -2..=2 {
                                for dx in -4..=4 {
                                    for dz in -4..=4 {
                                        let (x, y, z) = (px + dx, py + dy, pz + dz);
                                        let st = s.world.block_state_at(x, y, z);
                                        if self.blocks.block_name(st)
                                            == Some("minecraft:oak_leaves")
                                        {
                                            hit = Some([x, y, z]);
                                            break 'scan;
                                        }
                                    }
                                }
                            }
                            hit
                        });
                        match carpet {
                            Some(c) => {
                                if let Some(s) = self.session.as_mut() {
                                    let eye = [s.player.x, s.player.eye_y(), s.player.z];
                                    let d = [
                                        c[0] as f64 + 0.5 - eye[0],
                                        c[1] as f64 + 0.5 - eye[1],
                                        c[2] as f64 + 0.5 - eye[2],
                                    ];
                                    let len =
                                        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                                    s.player.yaw = (-d[0]).atan2(d[2]).to_degrees() as f32;
                                    s.player.pitch = (-d[1] / len).asin().to_degrees() as f32;
                                }
                                self.attack_held = true;
                                self.mining_press();
                                // The block the machine actually started on
                                // (the crosshair's raycast), which need not be
                                // the scanned leaves: r78's failure detail.
                                let mined = self.mining.target();
                                self.mine_pressed_tick =
                                    self.session.as_ref().map_or(0, |s| s.ticks);
                                // The press's own tick has run: the next
                                // `mining_tick` is the first continue.
                                self.mining_tick_at = self.mine_pressed_tick;
                                let held = self
                                    .session
                                    .as_ref()
                                    .and_then(|s| s.inventory.held())
                                    .and_then(|st| self.items.name(st.item_id))
                                    .map(str::to_string);
                                // Vanilla's literals: a sword's `sword_efficient`
                                // rule (leaves are in the tag) mines at 1.5, a
                                // bare hand or a non-tool item at 1.
                                let speed = match held.as_deref() {
                                    Some(n) if n.ends_with("_sword") => 1.5,
                                    _ => 1.0,
                                };
                                let waterlogged = self.session.as_ref().is_some_and(|s| {
                                    let st = s.world.block_state_at(c[0], c[1], c[2]) as usize;
                                    s.water_states.get(st).copied().unwrap_or(false)
                                });
                                let diag = self.session.as_ref().map(|s| {
                                    format!(
                                        "pressed at {:.3} of the run, mining {:?}, waterlogged {}, on ground {}, eye in water {}, pos ({:.2}, {:.2}, {:.2})",
                                        elapsed / limit,
                                        mined,
                                        waterlogged,
                                        s.player.on_ground,
                                        s.local_player_view().is_some_and(|v| v.underwater),
                                        s.player.x,
                                        s.player.y,
                                        s.player.z
                                    )
                                });
                                if let Some(ch) = self.check.as_mut() {
                                    ch.mine_diag = diag;
                                    ch.mine_pos = Some(c);
                                    ch.mine_survival = true;
                                    ch.mine_expected = Some(leaves_ticks(speed));
                                    ch.mine_held = held;
                                }
                                self.mine_stage = 2;
                            }
                            None => {
                                let what = self.session.as_ref().map(|s| {
                                    let (px, py, pz) = (
                                        s.player.x.floor() as i32,
                                        s.player.y.floor() as i32,
                                        s.player.z.floor() as i32,
                                    );
                                    let n = self.blocks.block_name(s.world.block_state_at(
                                        px - 2,
                                        py,
                                        pz,
                                    ));
                                    format!("bot at ({px},{py},{pz}), block at x-2: {n:?}")
                                });
                                if let Some(ch) = self.check.as_mut() {
                                    ch.mine_skip =
                                        Some(format!("no oak leaves near the bot; {what:?}"));
                                }
                                self.mine_stage = 3;
                            }
                        }
                    } else if elapsed >= limit * 0.24 {
                        if let Some(ch) = self.check.as_mut() {
                            ch.mine_skip = Some("the server never confirmed survival".into());
                        }
                        self.mine_stage = 3;
                    }
                }
                if self.mine_stage == 2 && !self.mining.destroying() {
                    // The machine finished (STOP sent) or never started.
                    let now = self.session.as_ref().map_or(0, |s| s.ticks);
                    if let Some(ch) = self.check.as_mut() {
                        ch.mine_ticks = Some(now.saturating_sub(self.mine_pressed_tick));
                    }
                    self.attack_held = false;
                    self.mine_stage = 3;
                }
                if self.mine_stage == 2 || self.mine_stage == 3 {
                    let seen = self
                        .check
                        .as_ref()
                        .and_then(|c| c.mine_pos)
                        .zip(self.session.as_ref())
                        .map(|([x, y, z], s)| {
                            (
                                s.ticks.saturating_sub(self.mine_pressed_tick),
                                self.blocks
                                    .block_name(s.world.block_state_at(x, y, z))
                                    .unwrap_or("?")
                                    .to_string(),
                            )
                        });
                    if let (Some((t, name)), Some(ch)) = (seen, self.check.as_mut()) {
                        if ch.mine_changed.is_none() && name != "minecraft:oak_leaves" {
                            ch.mine_changed = Some((t, name));
                        }
                    }
                }
                if self.mine_stage == 3 {
                    let broken = self
                        .check
                        .as_ref()
                        .and_then(|c| c.mine_changed.as_ref())
                        .is_some_and(|(_, n)| n == "minecraft:air" || n == "minecraft:water");
                    // A STOP the server judges early (`progress >= 0.7` over
                    // ITS elapsed ticks) is finished by its own tick a few
                    // ticks later, so the wait runs past r76's camera move
                    // (which cannot matter: the key is released).
                    if broken || elapsed >= limit * 0.28 {
                        let pos = self.check.as_ref().and_then(|c| c.mine_pos);
                        let left = pos.zip(self.session.as_ref()).and_then(|([x, y, z], s)| {
                            self.blocks.block_name(s.world.block_state_at(x, y, z))
                        });
                        if let Some(ch) = self.check.as_mut() {
                            ch.mine_broken = broken;
                            let first = ch.mine_changed.clone();
                            if let Some(d) = ch.mine_diag.as_mut() {
                                d.push_str(&format!(
                                    ", first change {first:?} (ticks after the press), left {left:?}"
                                ));
                                log::info!("render-check: r78 broken {broken} [{d}]");
                            }
                        }
                        self.attack_held = false;
                        if let Some(s) = self.session.as_mut() {
                            let _ = s.send_command("gamemode creative");
                            // Does the SERVER still have the leaves? (Never
                            // broken, or broken without the client seeing it.)
                            if let (false, Some([x, y, z])) = (broken, pos) {
                                let _ = s.send_command(&format!(
                                    "execute if block {x} {y} {z} minecraft:oak_leaves"
                                ));
                            }
                        }
                        self.mine_probe_tick = self.session.as_ref().map_or(0, |s| s.ticks);
                        self.mine_stage = if broken { 6 } else { 5 };
                    }
                }
                if self.mine_stage == 5 {
                    let now = self.session.as_ref().map_or(0, |s| s.ticks);
                    let reply = self.session.as_ref().and_then(|s| {
                        s.chat_log
                            .iter()
                            .rev()
                            .find(|l| l.starts_with("Test passed") || l.starts_with("Test failed"))
                            .cloned()
                    });
                    if reply.is_some() || now >= self.mine_probe_tick + 40 {
                        if let Some(d) = self.check.as_mut().and_then(|c| c.mine_diag.as_mut()) {
                            d.push_str(&format!(", server has the leaves: {reply:?}"));
                        }
                        self.mine_stage = 6;
                    }
                }
                // r83 — M219's bed, in the TAIL (after the timed run, once
                // the tail has cleared the stage — see r84 below): a sleeping
                // player is immobile and every timed slot has a screen or a
                // held key in it. Night is needed (`BedRule.WHEN_DARK`);
                // waking teleports the bot beside the bed, so it is sent back
                // to where it stood before the donkey's turn.
                if self.bed_stage > 0 && self.bed_stage < 20 {
                    self.bed_stage_frames += 1;
                    if elapsed >= limit + RENDER_CHECK_TAIL_SECONDS - 0.2 {
                        if let Some(c) = self.check.as_mut() {
                            c.bed_skip.get_or_insert_with(|| {
                                format!("timed out in stage {}", self.bed_stage)
                            });
                        }
                        self.keys.w = false;
                        self.bed_stage = 6;
                    }
                }
                let tail_ready = self.mount_stage == 1
                    && self
                        .session
                        .as_ref()
                        .is_some_and(|s| s.ticks >= self.mount_tick0 + 5);
                if self.bed_stage == 0
                    && tail_ready
                    && self.chat_screen.is_none()
                    && !self.screen.any_open()
                {
                    if let Some(s) = self.session.as_mut() {
                        self.bed_origin = Some((
                            [s.player.x, s.player.y, s.player.z],
                            s.day_ticks.unwrap_or(0),
                        ));
                        self.bed_tick0 = s.ticks;
                        let _ = s.send_command("time set 18000");
                    }
                    self.bed_stage = 1;
                    self.bed_stage_frames = 0;
                }
                if self.bed_stage == 1 {
                    let night = self
                        .session
                        .as_ref()
                        .and_then(|s| s.day_ticks)
                        .is_some_and(|t| (13_000..23_000).contains(&t.rem_euclid(24_000)));
                    let bed = self.session.as_ref().and_then(|s| {
                        let (px, py, pz) = (
                            s.player.x.floor() as i32,
                            s.player.y.floor() as i32,
                            s.player.z.floor() as i32,
                        );
                        let mut hit = None;
                        'scan: for dy in -2..=2 {
                            for dx in -4..=4 {
                                for dz in -4..=4 {
                                    let (x, y, z) = (px + dx, py + dy, pz + dz);
                                    let st = s.world.block_state_at(x, y, z);
                                    if self.blocks.block_name(st) == Some("minecraft:red_bed") {
                                        hit = Some([x, y, z]);
                                        break 'scan;
                                    }
                                }
                            }
                        }
                        hit
                    });
                    // The server syncs its clock only every 20 ticks, so the
                    // echo of `time set` may come after the night has begun
                    // server-side: click once night is seen or three ticks
                    // after the command, and again every four ticks until the
                    // server says the bot sleeps (a refused click answers
                    // "You can only sleep at night" and changes nothing).
                    let now = self.session.as_ref().map_or(0, |s| s.ticks);
                    if self.session.as_ref().is_some_and(|s| s.is_sleeping()) {
                        self.bed_stage = 2;
                        self.bed_stage_frames = 0;
                    } else if let Some([x, y, z]) = bed {
                        let due = (night || now >= self.bed_tick0 + 3)
                            && self.check.as_ref().is_none_or(|c| {
                                c.bed_pos.is_none() || now >= self.bed_click_tick + 4
                            });
                        if due {
                            if let Some(s) = self.session.as_mut() {
                                // M288 — r142's control: face south first, so
                                // the bed camera's -90 (the west bed's) cannot
                                // coincide with wherever an earlier tail stage
                                // left the bot looking (it once left it at -90
                                // exactly, and the row failed closed).
                                s.player.yaw = 0.0;
                                let _ = s.use_item_on(x, y, z, 1);
                            }
                            if let Some(c) = self.check.as_mut() {
                                c.bed_pos = Some([x, y, z]);
                                c.bed_clicks += 1;
                            }
                            self.bed_click_tick = now;
                        }
                    }
                }
                if self.bed_stage == 2 {
                    let sleeping = self.session.as_ref().and_then(|s| {
                        s.sleeping_pos()
                            .map(|b| (b, [s.player.x, s.player.y, s.player.z]))
                    });
                    let shown = self.check.as_ref().is_some_and(|c| c.in_bed_frames >= 2);
                    if let (Some((head, p)), true) = (sleeping, shown) {
                        if let Some(c) = self.check.as_mut() {
                            c.bed_slept = true;
                            c.bed_head = Some(head);
                            c.bed_client_pos = Some(p);
                        }
                        // `isImmobile`: hold W for a few ticks; nothing moves.
                        self.keys.w = true;
                        self.bed_stage = 3;
                        self.bed_stage_frames = 0;
                    }
                }
                if self.bed_stage == 3 {
                    let drift = self
                        .session
                        .as_ref()
                        .zip(self.check.as_ref().and_then(|c| c.bed_client_pos))
                        .map_or(0.0, |(s, p)| {
                            ((s.player.x - p[0]).powi(2) + (s.player.z - p[2]).powi(2)).sqrt()
                        });
                    if let Some(c) = self.check.as_mut() {
                        c.bed_drift = c.bed_drift.max(drift);
                    }
                    let ticks_in = self.session.as_ref().map_or(0, |s| s.ticks);
                    if self.bed_stage_frames == 1 {
                        self.bed_click_tick = ticks_in;
                    }
                    if ticks_in >= self.bed_click_tick + 4 {
                        self.keys.w = false;
                        // The Leave Bed button, through the production click
                        // path: the cursor on its centre, then the press.
                        let (gw, gh) = self.gui_size();
                        let (x, y, w, h) = rewo_ui::chat_screen::leave_bed_bounds(gw, gh);
                        let px = self
                            .state
                            .as_ref()
                            .map(|st| {
                                let e = st.window.inner_size();
                                gui_px(e.width, e.height) as f64
                            })
                            .unwrap_or(1.0);
                        self.screen.mouse = (
                            (f64::from(x) + f64::from(w) / 2.0) * px,
                            (f64::from(y) + f64::from(h) / 2.0) * px,
                        );
                        let before = self.wake_ups_sent;
                        self.in_bed_click();
                        if let Some(c) = self.check.as_mut() {
                            c.bed_wake_sends = self.wake_ups_sent - before;
                        }
                        self.bed_stage = 4;
                        self.bed_stage_frames = 0;
                    }
                }
                if self.bed_stage == 4 {
                    let awake = self.session.as_ref().is_some_and(|s| !s.is_sleeping());
                    if awake && self.chat_screen.is_none() {
                        if let Some(c) = self.check.as_mut() {
                            c.bed_woke = true;
                            c.bed_screen_closed = true;
                        }
                        self.bed_stage = 5;
                    }
                }
                if self.bed_stage == 5 || self.bed_stage == 6 {
                    if let (Some(([x, y, z], day)), Some(s)) =
                        (self.bed_origin, self.session.as_mut())
                    {
                        let _ = s.send_command(&format!("tp @s {x} {y} {z}"));
                        let _ = s.send_command(&format!("time set {}", day.rem_euclid(24_000)));
                        // The donkey's turn waits for the teleport.
                        self.mount_tick0 = s.ticks;
                    }
                    self.bed_stage = 20;
                }
                // M265 — r123: two seconds in, scale the staged stone display
                // 1 -> 3 over 40 ticks, starting at once.
                if !self.rc_interp_sent && elapsed >= 2.0 {
                    if let Some(s) = self.session.as_mut() {
                        let _ = s.send_command(
                            "data merge entity @e[type=minecraft:block_display,tag=rewo_interp,limit=1] \
                             {transformation:{left_rotation:[0f,0f,0f,1f],right_rotation:[0f,0f,0f,1f],\
                             translation:[0f,0f,0f],scale:[3f,3f,3f]},interpolation_duration:40,\
                             start_interpolation:0}",
                        );
                    }
                    // M277 — r136: and ignite the staged creeper.
                    if let Some(s) = self.session.as_mut() {
                        let _ = s.send_command(
                            "data merge entity @e[type=minecraft:creeper,tag=rewo_creeper,limit=1] \
                             {ignited:1b}",
                        );
                    }
                    self.rc_interp_sent = true;
                }
                // M266 — r124: a second later, teleport it up a block; its
                // `teleport_duration` of 20 makes the move a glide.
                if !self.rc_glide_sent && elapsed >= 3.0 {
                    if let Some(s) = self.session.as_mut() {
                        let _ = s.send_command(
                            "execute as @e[type=minecraft:block_display,tag=rewo_interp,limit=1] \
                             at @s run tp @s ~ ~1 ~",
                        );
                    }
                    self.rc_glide_sent = true;
                }
                // `REWO_RC_CAPTURE_AT=<fraction>`: one F2 capture at that point
                // of the timed run, into the staged game directory, for a
                // person to look at (`render_check.py --keep`).
                if !self.rc_captured_at {
                    if let Some(f) = crate::knobs::var("REWO_RC_CAPTURE_AT")
                        .ok()
                        .and_then(|v| v.parse::<f32>().ok())
                    {
                        if elapsed >= limit * f {
                            self.capture_pending = true;
                            self.rc_captured_at = true;
                        }
                    }
                }
                // r84/r85 — M220's donkey, in the TAIL: after the timed run
                // (`elapsed >= limit`), when no other stage can hold the
                // screen; the run ends when this reaches 20 (or the tail cap).
                if self.mount_stage > 0 && self.mount_stage < 20 {
                    self.mount_stage_frames += 1;
                    if elapsed >= limit + RENDER_CHECK_TAIL_SECONDS - 0.2 {
                        if let Some(c) = self.check.as_mut() {
                            let seen = c.mount_seen.clone().unwrap_or_default();
                            c.mount_skip.get_or_insert_with(|| {
                                format!("timed out in stage {} ({seen})", self.mount_stage)
                            });
                        }
                        self.keys.sneak = false;
                        self.mount_stage = 20;
                    }
                }
                let now = self.session.as_ref().map_or(0, |s| s.ticks);
                let aim_donkey = |app: &mut Self| -> bool {
                    let Some(id) = app.check.as_ref().and_then(|c| c.donkey) else {
                        return false;
                    };
                    let Some(s) = app.session.as_mut() else {
                        return false;
                    };
                    let Some(e) = s.world.entities.get(id) else {
                        return false;
                    };
                    let (ex, ey, ez) = (e.x, e.y + 0.8, e.z);
                    let eye = [s.player.x, s.player.eye_y(), s.player.z];
                    let d = [ex - eye[0], ey - eye[1], ez - eye[2]];
                    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                    s.player.yaw = (-d[0]).atan2(d[2]).to_degrees() as f32;
                    s.player.pitch = (-d[1] / len).asin().to_degrees() as f32;
                    true
                };
                if self.mount_stage == 0 && elapsed >= limit {
                    // Clear the stage: every screen closed through its own
                    // production path, the keys released, and r72's
                    // client-side horse dismounted and removed (through the
                    // router, the way it was added and seated).
                    if self.screen.inventory_open() {
                        self.close_container_screen();
                    }
                    if self.chat_screen.is_some() {
                        self.close_chat_screen();
                    }
                    if !matches!(self.view, ScreenView::None) || self.screen.any_open() {
                        self.close_view_screen();
                    }
                    self.keys.w = false;
                    self.keys.sprint = false;
                    self.keys.jump = false;
                    self.keys.tab_list = false;
                    if let Some(session) = self.session.as_mut() {
                        let mut pass: Vec<u8> = Vec::new();
                        rewo_proto::varint::write_varint(&mut pass, 0x00A9_0000);
                        rewo_proto::varint::write_varint(&mut pass, 0);
                        session.inject_packet(session.ids.cb_play_set_passengers, &pass);
                        // ...and take the horse away: it stands where the bot
                        // does, so the bot's eye is inside its box and every
                        // pick would hit it first.
                        let mut gone: Vec<u8> = Vec::new();
                        rewo_proto::varint::write_varint(&mut gone, 1);
                        rewo_proto::varint::write_varint(&mut gone, 0x00A9_0000);
                        session.inject_packet(session.ids.cb_play_remove_entities, &gone);
                        // r71's item frame hangs in the bot's head block and
                        // took the pick on an earlier cut of this stage; it
                        // has done its job, so the server removes it.
                        let _ = session.send_command("kill @e[type=minecraft:item_frame]");
                    }
                    let donkey = self.session.as_ref().and_then(|s| {
                        let want = s.entity_types.as_deref()?.id_of("minecraft:donkey")?;
                        let (px, pz) = (s.player.x, s.player.z);
                        s.world
                            .entities
                            .iter()
                            .filter(|(_, e)| {
                                e.type_id == want
                                    && (e.x - px).abs() < 8.0
                                    && (e.z - pz).abs() < 8.0
                            })
                            .map(|(id, _)| id)
                            .next()
                    });
                    match donkey {
                        Some(id) => {
                            if let Some(c) = self.check.as_mut() {
                                c.donkey = Some(id);
                            }
                            self.mount_tick0 = now;
                            self.mount_stage = 1;
                        }
                        None => {
                            if let Some(c) = self.check.as_mut() {
                                c.mount_skip = Some("no donkey near the bot".into());
                            }
                            self.mount_stage = 20;
                        }
                    }
                }
                if self.mount_stage == 1
                    && self.bed_stage >= 20
                    && now >= self.mount_tick0 + 5
                    && !self.screen.any_open()
                    && self.chat_screen.is_none()
                {
                    // Sneak + right-click: the production entity click, which
                    // reads the shift key into the interact packet.
                    if aim_donkey(self) {
                        self.keys.sneak = true;
                        // Only a click that the production pick resolves to
                        // the donkey counts (a frame or a mob in the way would
                        // take the interaction instead).
                        let on_donkey = self.session.as_ref().is_some_and(|s| {
                            super::entities::frame_crosshair_hit(s, &self.etypes, 1.0).map(|h| h.id)
                                == self.check.as_ref().and_then(|c| c.donkey)
                        });
                        if on_donkey && self.entity_click(winit::event::MouseButton::Right) {
                            self.mount_stage = 2;
                            self.mount_stage_frames = 0;
                        } else if !on_donkey {
                            let why = self.session.as_ref().map(|s| {
                                let hit = super::entities::frame_crosshair_hit(s, &self.etypes, 1.0);
                                let d = self.check.as_ref().and_then(|c| c.donkey).and_then(|id| s.world.entities.get(id));
                                format!(
                                    "the crosshair is not on the donkey: hit {:?}, eye ({:.2}, {:.2}, {:.2}) yaw {:.1} pitch {:.1}, donkey at {:?}, vehicle {:?}",
                                    hit.map(|h| (h.id, s.world.entities.get(h.id).and_then(|e| self.etypes.name(e.type_id)))),
                                    s.player.x,
                                    s.player.eye_y(),
                                    s.player.z,
                                    s.player.yaw,
                                    s.player.pitch,
                                    d.map(|e| (e.x, e.y, e.z)),
                                    s.local_vehicle()
                                )
                            });
                            if let Some(c) = self.check.as_mut() {
                                c.mount_skip = why;
                            }
                        }
                    }
                }
                if self.mount_stage == 2 {
                    let open = self.session.as_ref().and_then(|s| {
                        let m = s.menus.shown_open()?;
                        let mount = m.mount?;
                        let name = |slot: usize| {
                            m.menu.menu_slot(slot).and_then(|st| {
                                self.items
                                    .name(st.item_id)
                                    .map(|n| (n.to_string(), st.count))
                            })
                        };
                        Some((
                            m.open_serial,
                            mount,
                            m.layout.slot_count(),
                            m.title.clone(),
                            name(0),
                            name(9),
                            m.menu.content_updates(),
                        ))
                    });
                    let drawn = |serial: u64, c: &RenderCheck| {
                        c.mount_panel_frames
                            .iter()
                            .filter(|f| f.0 == serial)
                            .count()
                    };
                    let seen = format!(
                        "stage 2 saw: inventory screen {}, shown menu {:?}, panel frames {}",
                        self.screen.inventory_open(),
                        open.as_ref().map(|o| (o.0, o.2, o.6)),
                        self.check
                            .as_ref()
                            .map_or(0, |c| c.mount_panel_frames.len())
                    );
                    if let Some(c) = self.check.as_mut() {
                        c.mount_seen = Some(seen);
                    }
                    if let Some((serial, mount, slots, title, saddle, chest, contents)) = open {
                        let frames = self.check.as_ref().map_or(0, |c| drawn(serial, c));
                        // The contents arrive in `container_set_content` just
                        // after the open; wait for them and a few frames.
                        // `REWO_RC_CAPTURE_MOUNT`: one F2 capture of the
                        // donkey screen into the staged game directory, for a
                        // person to look at (`render_check.py --keep`), with
                        // the screen held long enough for it to land.
                        let capture = crate::knobs::var("REWO_RC_CAPTURE_MOUNT").is_ok();
                        if capture && frames >= 3 && contents > 0 && !self.mount_captured {
                            self.capture_pending = true;
                            self.mount_captured = true;
                        }
                        if frames >= if capture { 30 } else { 3 } && contents > 0 {
                            if let Some(c) = self.check.as_mut() {
                                c.mount_columns = Some(mount.columns);
                                c.mount_slots = slots;
                                c.mount_active = Some((mount.saddle_active, mount.body_active));
                                c.mount_saddle_item = saddle.map(|s| s.0);
                                c.mount_chest_item = chest;
                                c.mount_title = Some(title);
                            }
                            self.mount_serial = serial;
                            self.close_container_screen();
                            self.keys.sneak = false;
                            self.mount_tick0 = now;
                            self.mount_stage = 3;
                        }
                    }
                }
                if self.mount_stage == 3
                    && now >= self.mount_tick0 + 3
                    && self
                        .session
                        .as_ref()
                        .is_some_and(|s| s.menus.open().is_none())
                {
                    // A plain right-click rides the tamed, saddled donkey.
                    if aim_donkey(self) && {
                        let on = self.session.as_ref().is_some_and(|s| {
                            super::entities::frame_crosshair_hit(s, &self.etypes, 1.0).map(|h| h.id)
                                == self.check.as_ref().and_then(|c| c.donkey)
                        });
                        on && self.entity_click(winit::event::MouseButton::Right)
                    } {
                        self.mount_stage = 4;
                        self.mount_stage_frames = 0;
                    }
                }
                if self.mount_stage == 4 {
                    let donkey = self.check.as_ref().and_then(|c| c.donkey);
                    let riding = self
                        .session
                        .as_ref()
                        .is_some_and(|s| s.local_vehicle() == donkey && donkey.is_some());
                    if riding
                        && self
                            .session
                            .as_ref()
                            .is_some_and(|s| s.server_controlled_inventory())
                    {
                        if let Some(c) = self.check.as_mut() {
                            c.mount_rode = true;
                        }
                        // The inventory key, through its production handler.
                        self.inventory_key();
                        if let Some(c) = self.check.as_mut() {
                            c.mount_e_sent = !self.screen.inventory_open();
                        }
                        self.mount_stage = 5;
                    }
                }
                if self.mount_stage == 5 {
                    let serial = self
                        .session
                        .as_ref()
                        .and_then(|s| s.menus.shown_open())
                        .filter(|m| m.mount.is_some() && m.open_serial != self.mount_serial)
                        .map(|m| m.open_serial);
                    if let Some(serial) = serial {
                        let frames = self.check.as_ref().map_or(0, |c| {
                            c.mount_panel_frames
                                .iter()
                                .filter(|f| f.0 == serial)
                                .count()
                        });
                        if frames >= 3 {
                            if let Some(c) = self.check.as_mut() {
                                c.mount_second_serial = Some(serial);
                            }
                            self.close_container_screen();
                            if let Some(s) = self.session.as_mut() {
                                let _ = s.send_command("ride @s dismount");
                            }
                            self.mount_tick0 = now;
                            self.mount_stage = 6;
                        }
                    }
                }
                // r86's control: off the donkey, the same key opens the
                // player's own inventory, whose frames carry the preview.
                if self.mount_stage == 6
                    && now >= self.mount_tick0 + 4
                    && self
                        .session
                        .as_ref()
                        .is_some_and(|s| s.local_vehicle().is_none())
                    && !self.screen.inventory_open()
                {
                    self.inventory_key();
                    self.mount_stage_frames = 0;
                    self.mount_stage = 7;
                }
                if self.mount_stage == 7 && self.mount_stage_frames >= 30 {
                    // E again closes it (`onClose`).
                    self.inventory_key();
                    self.mount_stage = 20;
                    self.boat_tick0 = now;
                }
                // r87 — M221's boat, last in the tail: ride it, hold W, then
                // A, let it drift to a stop, ask the server where it is.
                if self.boat_stage > 0
                    && self.boat_stage < 20
                    && elapsed >= limit + RENDER_CHECK_TAIL_SECONDS - 0.2
                {
                    if let Some(c) = self.check.as_mut() {
                        c.boat_skip.get_or_insert_with(|| {
                            format!("timed out in stage {}", self.boat_stage)
                        });
                    }
                    self.keys.w = false;
                    self.keys.a = false;
                    self.boat_stage = 20;
                }
                if self.boat_stage == 0
                    && self.mount_stage >= 20
                    && now >= self.boat_tick0 + 5
                    && !self.screen.any_open()
                    && self
                        .session
                        .as_ref()
                        .is_some_and(|s| s.local_vehicle().is_none())
                {
                    let boat = self.session.as_ref().and_then(|s| {
                        let want = s.entity_types.as_deref()?.id_of("minecraft:oak_boat")?;
                        let (px, pz) = (s.player.x, s.player.z);
                        s.world
                            .entities
                            .iter()
                            .filter(|(_, e)| {
                                e.type_id == want
                                    && (e.x - px).abs() < 12.0
                                    && (e.z - pz).abs() < 12.0
                            })
                            .map(|(id, e)| (id, e.x, e.y, e.z))
                            .next()
                    });
                    match boat {
                        Some((id, bx, by, bz)) => {
                            if let Some(c) = self.check.as_mut() {
                                c.boat = Some(id);
                            }
                            // Walk-free: put the bot beside the boat on the
                            // bank and face it.
                            if let Some(s) = self.session.as_mut() {
                                let _ = s.send_command(&format!(
                                    "tp @s {} {} {}",
                                    bx - 2.5,
                                    by.ceil(),
                                    bz
                                ));
                            }
                            self.boat_tick0 = now;
                            self.boat_stage = 1;
                        }
                        None => {
                            if let Some(c) = self.check.as_mut() {
                                c.boat_skip = Some("no oak boat near the bot".into());
                            }
                            self.boat_stage = 20;
                        }
                    }
                }
                if self.boat_stage == 1 && now >= self.boat_tick0 + 8 {
                    let id = self.check.as_ref().and_then(|c| c.boat);
                    let aimed = id.zip(self.session.as_mut()).is_some_and(|(id, s)| {
                        let Some(e) = s.world.entities.get(id) else {
                            return false;
                        };
                        let (ex, ey, ez) = (e.x, e.y + 0.3, e.z);
                        let eye = [s.player.x, s.player.eye_y(), s.player.z];
                        let d = [ex - eye[0], ey - eye[1], ez - eye[2]];
                        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                        s.player.yaw = (-d[0]).atan2(d[2]).to_degrees() as f32;
                        s.player.pitch = (-d[1] / len).asin().to_degrees() as f32;
                        true
                    });
                    let on_boat = self.session.as_ref().is_some_and(|s| {
                        super::entities::frame_crosshair_hit(s, &self.etypes, 1.0).map(|h| h.id)
                            == id
                    });
                    if aimed && on_boat && self.entity_click(winit::event::MouseButton::Right) {
                        self.boat_tick0 = now;
                        self.boat_stage = 2;
                    } else if now >= self.boat_tick0 + 40 {
                        if let Some(c) = self.check.as_mut() {
                            c.boat_skip = Some("the crosshair never reached the boat".into());
                        }
                        self.boat_stage = 20;
                    }
                }
                if self.boat_stage == 2 {
                    let steering = self
                        .session
                        .as_ref()
                        .and_then(|s| s.steered_boat)
                        .map(|b| [b.state.x, b.state.z]);
                    if let Some(start) = steering {
                        if let Some(c) = self.check.as_mut() {
                            c.boat_steered = true;
                        }
                        // Face along the boat so W is "forward" for both.
                        if let Some(s) = self.session.as_mut() {
                            if let Some(b) = s.steered_boat {
                                s.player.yaw = b.state.yaw;
                            }
                        }
                        self.keys.w = true;
                        self.boat_start = Some(start);
                        self.boat_tick0 = now;
                        self.boat_stage = 3;
                    }
                }
                if self.boat_stage == 3 && now >= self.boat_tick0 + 20 {
                    self.keys.w = false;
                    let (pos, yaw) = self
                        .session
                        .as_ref()
                        .and_then(|s| s.steered_boat)
                        .map_or(([0.0; 2], 0.0), |b| ([b.state.x, b.state.z], b.state.yaw));
                    if let (Some(c), Some(st)) = (self.check.as_mut(), self.boat_start) {
                        c.boat_moved = ((pos[0] - st[0]).powi(2) + (pos[1] - st[1]).powi(2)).sqrt();
                    }
                    self.boat_yaw0 = yaw;
                    self.keys.a = true;
                    self.boat_tick0 = now;
                    self.boat_stage = 4;
                }
                if self.boat_stage == 4 && now >= self.boat_tick0 + 10 {
                    self.keys.a = false;
                    let yaw = self
                        .session
                        .as_ref()
                        .and_then(|s| s.steered_boat)
                        .map_or(0.0, |b| b.state.yaw);
                    if let Some(c) = self.check.as_mut() {
                        c.boat_turned = yaw - self.boat_yaw0;
                    }
                    self.boat_tick0 = now;
                    self.boat_stage = 5;
                }
                if self.boat_stage == 5 {
                    // Drift to a stop (friction 0.9 a tick), then ask.
                    let still = self
                        .session
                        .as_ref()
                        .and_then(|s| s.steered_boat)
                        .is_some_and(|b| {
                            b.state.vx.abs() < 0.002
                                && b.state.vz.abs() < 0.002
                                && b.state.delta_rotation.abs() < 0.05
                        });
                    if still || now >= self.boat_tick0 + 60 {
                        if let Some(s) = self.session.as_mut() {
                            let _ = s.send_command("data get entity @e[type=minecraft:oak_boat,limit=1,sort=nearest] Pos");
                        }
                        self.seat_ask("boat");
                        let (pos, moves, paddles, rejections) = self
                            .session
                            .as_ref()
                            .map(|s| {
                                (
                                    s.steered_boat.map(|b| [b.state.x, b.state.y, b.state.z]),
                                    s.vehicle_moves_sent,
                                    s.paddles_sent,
                                    s.vehicle_rejections,
                                )
                            })
                            .unwrap_or((None, 0, 0, 0));
                        if let Some(c) = self.check.as_mut() {
                            c.boat_client_pos = pos;
                            c.boat_moves_sent = moves;
                            c.boat_paddles_sent = paddles;
                            c.boat_rejections = rejections;
                        }
                        self.boat_tick0 = now;
                        self.boat_stage = 6;
                    }
                }
                if self.boat_stage == 6 {
                    // "Oak Boat has the following entity data: [x d, y d, z d]".
                    let reply = self.session.as_ref().and_then(|s| {
                        s.chat_log
                            .iter()
                            .rev()
                            // "Oak Boat has ...": not the bot's own reply
                            // (r91), which can arrive in the same frame.
                            .find(|l| l.contains("Boat has the following entity data"))
                            .cloned()
                    });
                    if let Some(line) = reply {
                        let nums: Vec<f64> = line
                            .split(['[', ']', ','])
                            .filter_map(|t| t.trim().trim_end_matches('d').parse::<f64>().ok())
                            .collect();
                        if let (3, Some(c)) = (nums.len(), self.check.as_mut()) {
                            c.boat_server_pos = Some([nums[0], nums[1], nums[2]]);
                        }
                        if let Some(s) = self.session.as_mut() {
                            let _ = s.send_command("ride @s dismount");
                        }
                        self.boat_stage = 20;
                        self.horse_tick0 = now;
                    } else if now >= self.boat_tick0 + 40 {
                        if let Some(c) = self.check.as_mut() {
                            c.boat_skip = Some("no /data reply".into());
                        }
                        self.boat_stage = 20;
                        self.horse_tick0 = now;
                    }
                }
                // r88 — M222's horse, after the boat.
                if self.horse_stage > 0
                    && self.horse_stage < 20
                    && elapsed >= limit + RENDER_CHECK_TAIL_SECONDS - 0.2
                {
                    if let Some(c) = self.check.as_mut() {
                        c.horse_skip.get_or_insert_with(|| {
                            format!("timed out in stage {}", self.horse_stage)
                        });
                    }
                    self.keys.w = false;
                    self.keys.jump = false;
                    self.horse_stage = 20;
                }
                if self.horse_stage == 0
                    && self.boat_stage >= 20
                    && now >= self.horse_tick0 + 5
                    && self
                        .session
                        .as_ref()
                        .is_some_and(|s| s.local_vehicle().is_none())
                {
                    let horse = self.session.as_ref().and_then(|s| {
                        let want = s.entity_types.as_deref()?.id_of("minecraft:horse")?;
                        s.world
                            .entities
                            .iter()
                            // The summoned one: not r72's or the crowd's
                            // injected horses, and saddled.
                            .filter(|(id, e)| {
                                e.type_id == want
                                    && *id != 0x00A9_0000
                                    && !CROWD_IDS.contains(id)
                                    && s.world.entities.saddled(*id)
                            })
                            .map(|(id, e)| (id, e.x, e.y, e.z))
                            .next()
                    });
                    match horse {
                        Some((id, hx, hy, hz)) => {
                            if let Some(c) = self.check.as_mut() {
                                c.horse = Some(id);
                            }
                            if let Some(s) = self.session.as_mut() {
                                let _ =
                                    s.send_command(&format!("tp @s {} {} {}", hx + 2.0, hy, hz));
                            }
                            self.horse_tick0 = now;
                            self.horse_stage = 1;
                        }
                        None => {
                            if let Some(c) = self.check.as_mut() {
                                c.horse_skip = Some("no summoned horse".into());
                            }
                            self.horse_stage = 20;
                        }
                    }
                }
                if self.horse_stage == 1 && now >= self.horse_tick0 + 8 {
                    let id = self.check.as_ref().and_then(|c| c.horse);
                    let aimed = id.zip(self.session.as_mut()).is_some_and(|(id, s)| {
                        let Some(e) = s.world.entities.get(id) else {
                            return false;
                        };
                        let (ex, ey, ez) = (e.x, e.y + 1.0, e.z);
                        let eye = [s.player.x, s.player.eye_y(), s.player.z];
                        let d = [ex - eye[0], ey - eye[1], ez - eye[2]];
                        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                        s.player.yaw = (-d[0]).atan2(d[2]).to_degrees() as f32;
                        s.player.pitch = (-d[1] / len).asin().to_degrees() as f32;
                        true
                    });
                    let on_horse = self.session.as_ref().is_some_and(|s| {
                        super::entities::frame_crosshair_hit(s, &self.etypes, 1.0).map(|h| h.id)
                            == id
                    });
                    if aimed && on_horse && self.entity_click(winit::event::MouseButton::Right) {
                        self.horse_tick0 = now;
                        self.horse_stage = 2;
                    } else if now >= self.horse_tick0 + 40 {
                        if let Some(c) = self.check.as_mut() {
                            c.horse_skip = Some("the crosshair never reached the horse".into());
                        }
                        self.horse_stage = 20;
                    }
                }
                if self.horse_stage == 2 {
                    let steering = self
                        .session
                        .as_ref()
                        .and_then(|s| s.steered_horse)
                        .map(|h| [h.state.x, h.state.y, h.state.z]);
                    if let Some(start) = steering {
                        if let Some(c) = self.check.as_mut() {
                            c.horse_steered = true;
                        }
                        if let Some(s) = self.session.as_mut() {
                            // West, away from the spawn's flood.
                            s.player.yaw = 90.0;
                            s.player.pitch = 0.0;
                            self.horse_counts0 = (s.vehicle_moves_sent, s.vehicle_rejections);
                            self.horse_jumps0 = s.ridden_jumps;
                        }
                        self.keys.w = true;
                        self.horse_start = Some(start);
                        self.horse_tick0 = now;
                        self.horse_stage = 3;
                    } else if now >= self.horse_tick0 + 40 {
                        if let Some(c) = self.check.as_mut() {
                            c.horse_skip = Some("never steered the horse".into());
                        }
                        self.horse_stage = 20;
                    }
                }
                // One reading per client tick of the steered mount: the
                // distance since the last (r88 and r89).
                let mount_now = self
                    .session
                    .as_ref()
                    .and_then(|s| s.steered_horse)
                    .map(|h| (h.id, [h.state.x, h.state.y, h.state.z]));
                let mount_step = match (mount_now, self.mount_last) {
                    (Some((id, p)), Some((lid, q, t))) if id == lid && t != now => {
                        Some(((p[0] - q[0]).powi(2) + (p[2] - q[2]).powi(2)).sqrt())
                    }
                    _ => None,
                };
                if let Some((id, p)) = mount_now {
                    if self.mount_last.is_none_or(|(_, _, t)| t != now) {
                        self.mount_last = Some((id, p, now));
                    }
                }
                if self.horse_stage == 3 && now >= self.horse_tick0 + 20 {
                    if let (Some(c), Some(st)) = (self.check.as_mut(), mount_step) {
                        c.horse_step = st;
                    }
                    self.keys.w = false;
                    let pos = self
                        .session
                        .as_ref()
                        .and_then(|s| s.steered_horse)
                        .map(|h| [h.state.x, h.state.y, h.state.z]);
                    if let (Some(c), Some(p), Some(st)) =
                        (self.check.as_mut(), pos, self.horse_start)
                    {
                        c.horse_moved = ((p[0] - st[0]).powi(2) + (p[2] - st[2]).powi(2)).sqrt();
                    }
                    // Hold the jump key for the riding meter, then release.
                    self.keys.jump = true;
                    self.horse_tick0 = now;
                    self.horse_stage = 4;
                }
                if self.horse_stage == 4 {
                    if now >= self.horse_tick0 + 12 {
                        self.keys.jump = false;
                    }
                    let (y, ground_y) = self
                        .session
                        .as_ref()
                        .and_then(|s| s.steered_horse)
                        .map_or((0.0, 0.0), |h| {
                            (h.state.y, self.horse_start.map_or(h.state.y, |st| st[1]))
                        });
                    if let Some(c) = self.check.as_mut() {
                        c.horse_rise = c.horse_rise.max(y - ground_y);
                    }
                    let (jumps, still) = self
                        .session
                        .as_ref()
                        .map(|s| {
                            (
                                s.ridden_jumps,
                                s.steered_horse.is_some_and(|h| {
                                    h.state.on_ground
                                        && h.state.vx.abs() < 0.003
                                        && h.state.vz.abs() < 0.003
                                }),
                            )
                        })
                        .unwrap_or((0, false));
                    // Ten ticks standing after the landing, so the server has
                    // the final position before it is asked.
                    if !still {
                        self.horse_still_since = now;
                    }
                    let settled = still && now >= self.horse_still_since + 10;
                    if (jumps > self.horse_jumps0 && settled && now >= self.horse_tick0 + 25)
                        || now >= self.horse_tick0 + 100
                    {
                        if let Some(s) = self.session.as_mut() {
                            let _ = s.send_command(
                                "data get entity @e[type=minecraft:horse,limit=1,sort=nearest] Pos",
                            );
                        }
                        self.seat_ask("horse");
                        let snap = self.session.as_ref().map(|s| {
                            (
                                s.steered_horse.map(|h| [h.state.x, h.state.y, h.state.z]),
                                s.vehicle_moves_sent,
                                s.vehicle_rejections,
                                s.ridden_jumps,
                            )
                        });
                        if let (Some((pos, moves, rej, jumps)), Some(c)) =
                            (snap, self.check.as_mut())
                        {
                            c.horse_client_pos = pos;
                            c.horse_moves_sent = moves - self.horse_counts0.0;
                            c.horse_rejections = rej - self.horse_counts0.1;
                            c.horse_jumps = jumps - self.horse_jumps0;
                        }
                        self.horse_tick0 = now;
                        self.horse_stage = 5;
                    }
                }
                if self.horse_stage == 5 {
                    let reply = self.session.as_ref().and_then(|s| {
                        s.chat_log
                            .iter()
                            .rev()
                            .find(|l| l.contains("Horse has the following entity data"))
                            .cloned()
                    });
                    if let Some(line) = reply {
                        let nums: Vec<f64> = line
                            .split(['[', ']', ','])
                            .filter_map(|t| t.trim().trim_end_matches('d').parse::<f64>().ok())
                            .collect();
                        if let (3, Some(c)) = (nums.len(), self.check.as_mut()) {
                            c.horse_server_pos = Some([nums[0], nums[1], nums[2]]);
                        }
                        if let Some(s) = self.session.as_mut() {
                            let _ = s.send_command("ride @s dismount");
                        }
                        self.horse_stage = 20;
                    } else if now >= self.horse_tick0 + 40 {
                        if let Some(c) = self.check.as_mut() {
                            c.horse_skip = Some("no /data reply".into());
                        }
                        self.horse_stage = 20;
                    }
                }
                // r89 — M225's camel, after the horse; r82's sprint state
                // rides along.
                if self.camel_stage > 0
                    && self.camel_stage < 20
                    && elapsed >= limit + RENDER_CHECK_TAIL_SECONDS - 0.2
                {
                    if let Some(c) = self.check.as_mut() {
                        c.camel_skip.get_or_insert_with(|| {
                            format!("timed out in stage {}", self.camel_stage)
                        });
                    }
                    self.keys.w = false;
                    self.keys.sprint = false;
                    self.keys.jump = false;
                    self.camel_stage = 20;
                }
                if self.camel_stage == 0 && self.horse_stage >= 20 {
                    self.camel_tick0 = now;
                    self.camel_stage = 1;
                }
                if self.camel_stage == 1
                    && now >= self.camel_tick0 + 5
                    && self
                        .session
                        .as_ref()
                        .is_some_and(|s| s.local_vehicle().is_none())
                {
                    let camel = self.session.as_ref().and_then(|s| {
                        let want = s.entity_types.as_deref()?.id_of("minecraft:camel")?;
                        s.world
                            .entities
                            .iter()
                            .filter(|(id, e)| {
                                e.type_id == want
                                    && !CROWD_IDS.contains(id)
                                    && s.world.entities.saddled(*id)
                            })
                            .map(|(id, e)| (id, e.x, e.y, e.z))
                            .next()
                    });
                    match camel {
                        Some((id, cx, cy, cz)) => {
                            if let Some(c) = self.check.as_mut() {
                                c.camel = Some(id);
                            }
                            if let Some(s) = self.session.as_mut() {
                                let _ =
                                    s.send_command(&format!("tp @s {} {} {}", cx + 2.0, cy, cz));
                            }
                            self.camel_tick0 = now;
                            self.camel_stage = 2;
                        }
                        None => {
                            if let Some(c) = self.check.as_mut() {
                                c.camel_skip = Some("no summoned camel".into());
                            }
                            self.camel_stage = 20;
                        }
                    }
                }
                if self.camel_stage == 2 && now >= self.camel_tick0 + 8 {
                    let id = self.check.as_ref().and_then(|c| c.camel);
                    let aimed = id.zip(self.session.as_mut()).is_some_and(|(id, s)| {
                        let Some(e) = s.world.entities.get(id) else {
                            return false;
                        };
                        let (ex, ey, ez) = (e.x, e.y + 1.4, e.z);
                        let eye = [s.player.x, s.player.eye_y(), s.player.z];
                        let d = [ex - eye[0], ey - eye[1], ez - eye[2]];
                        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                        s.player.yaw = (-d[0]).atan2(d[2]).to_degrees() as f32;
                        s.player.pitch = (-d[1] / len).asin().to_degrees() as f32;
                        true
                    });
                    let on_camel = self.session.as_ref().is_some_and(|s| {
                        super::entities::frame_crosshair_hit(s, &self.etypes, 1.0).map(|h| h.id)
                            == id
                    });
                    if aimed && on_camel && self.entity_click(winit::event::MouseButton::Right) {
                        self.camel_tick0 = now;
                        self.camel_stage = 3;
                    } else if now >= self.camel_tick0 + 40 {
                        if let Some(c) = self.check.as_mut() {
                            c.camel_skip = Some("the crosshair never reached the camel".into());
                        }
                        self.camel_stage = 20;
                    }
                }
                let camel_pos = self
                    .session
                    .as_ref()
                    .and_then(|s| s.steered_horse)
                    .filter(|h| Some(h.id) == self.check.as_ref().and_then(|c| c.camel))
                    .map(|h| ([h.state.x, h.state.y, h.state.z], h.state.on_ground));
                if self.camel_stage == 3 {
                    if camel_pos.is_some() {
                        if let Some(c) = self.check.as_mut() {
                            c.camel_steered = true;
                        }
                        if let Some(s) = self.session.as_mut() {
                            // West, along a lane clear of the horse's.
                            s.player.yaw = 90.0;
                            s.player.pitch = 0.0;
                            self.camel_counts0 = (s.vehicle_moves_sent, s.vehicle_rejections);
                            self.camel_jumps0 = s.ridden_jumps;
                            self.camel_sprints0 = s.sprint_commands_sent;
                        }
                        self.keys.w = true;
                        self.keys.sprint = true;
                        self.camel_tick0 = now;
                        self.camel_stage = 4;
                    } else if now >= self.camel_tick0 + 40 {
                        if let Some(c) = self.check.as_mut() {
                            c.camel_skip = Some("never steered the camel".into());
                        }
                        self.camel_stage = 20;
                    }
                }
                // The steered mount's per-tick distance (computed above, by
                // the horse's stages), when it is the camel.
                let step = mount_step.filter(|_| camel_pos.is_some());
                let sprint_flag = self
                    .session
                    .as_ref()
                    .is_some_and(|s| s.local_shared_flags() & (1 << 3) != 0);
                if self.camel_stage == 4 {
                    if sprint_flag {
                        if let Some(c) = self.check.as_mut() {
                            c.sprint_on = true;
                        }
                    }
                    if now >= self.camel_tick0 + 25 {
                        if let (Some(c), Some(st)) = (self.check.as_mut(), step) {
                            c.camel_sprint_step = st;
                        }
                        self.keys.w = false;
                        self.keys.sprint = false;
                        // Hold the jump key for the riding meter, then release.
                        self.keys.jump = true;
                        self.camel_tick0 = now;
                        self.camel_stage = 5;
                    }
                }
                if self.camel_stage == 5 {
                    if now >= self.camel_tick0 + 12 {
                        self.keys.jump = false;
                    }
                    let dashes = self
                        .session
                        .as_ref()
                        .map_or(0, |s| s.ridden_jumps - self.camel_jumps0);
                    if let (Some(c), Some(st), true) = (self.check.as_mut(), step, dashes > 0) {
                        c.camel_dash_step = c.camel_dash_step.max(st);
                    }
                    let landed = camel_pos.is_some_and(|(_, g)| g);
                    if dashes > 0 && landed && now >= self.camel_tick0 + 20 {
                        // Inside the 55-tick cooldown: hold the key again.
                        self.keys.jump = true;
                        self.camel_meter_peak = 0.0;
                        self.camel_tick0 = now;
                        self.camel_stage = 6;
                    } else if now >= self.camel_tick0 + 80 {
                        if let Some(c) = self.check.as_mut() {
                            c.camel_skip = Some("the dash never landed".into());
                        }
                        self.keys.jump = false;
                        self.camel_stage = 20;
                    }
                }
                if self.camel_stage == 6 {
                    let scale = self.session.as_ref().map_or(0.0, |s| s.jump_riding_scale());
                    self.camel_meter_peak = self.camel_meter_peak.max(scale);
                    if now >= self.camel_tick0 + 6 {
                        let cd = self
                            .session
                            .as_ref()
                            .zip(self.check.as_ref().and_then(|c| c.camel))
                            .map_or(0, |(s, id)| s.world.entities.dash_cooldown(id));
                        let peak = self.camel_meter_peak;
                        if let Some(c) = self.check.as_mut() {
                            c.camel_cooldown_meter = Some((peak, cd));
                        }
                        self.keys.jump = false;
                        self.camel_tick0 = now;
                        self.camel_still_since = now;
                        self.camel_stage = 7;
                    }
                }
                if self.camel_stage == 7 {
                    if !sprint_flag {
                        if let Some(c) = self.check.as_mut() {
                            c.sprint_off = true;
                        }
                    }
                    if step.is_some_and(|st| st > 1e-4) {
                        self.camel_still_since = now;
                    }
                    let settled =
                        camel_pos.is_some_and(|(_, g)| g) && now >= self.camel_still_since + 10;
                    if settled || now >= self.camel_tick0 + 100 {
                        if let Some(s) = self.session.as_mut() {
                            let _ = s.send_command(
                                "data get entity @e[type=minecraft:camel,limit=1,sort=nearest] Pos",
                            );
                        }
                        self.seat_ask("camel");
                        let snap = self.session.as_ref().map(|s| {
                            (
                                s.steered_horse.map(|h| [h.state.x, h.state.y, h.state.z]),
                                s.vehicle_moves_sent,
                                s.vehicle_rejections,
                                s.ridden_jumps,
                                s.sprint_commands_sent,
                            )
                        });
                        if let (Some((pos, moves, rej, jumps, sprints)), Some(c)) =
                            (snap, self.check.as_mut())
                        {
                            c.camel_client_pos = pos;
                            c.camel_moves_sent = moves - self.camel_counts0.0;
                            c.camel_rejections = rej - self.camel_counts0.1;
                            c.camel_dashes = jumps - self.camel_jumps0;
                            c.sprint_sent = sprints - self.camel_sprints0;
                        }
                        self.camel_tick0 = now;
                        self.camel_stage = 8;
                    }
                }
                if self.camel_stage == 8 {
                    let reply = self.session.as_ref().and_then(|s| {
                        s.chat_log
                            .iter()
                            .rev()
                            .find(|l| l.contains("Camel has the following entity data"))
                            .cloned()
                    });
                    if let Some(line) = reply {
                        let nums: Vec<f64> = line
                            .split(['[', ']', ','])
                            .filter_map(|t| t.trim().trim_end_matches('d').parse::<f64>().ok())
                            .collect();
                        if let (3, Some(c)) = (nums.len(), self.check.as_mut()) {
                            c.camel_server_pos = Some([nums[0], nums[1], nums[2]]);
                        }
                        if let Some(s) = self.session.as_mut() {
                            let _ = s.send_command("ride @s dismount");
                        }
                        self.camel_stage = 20;
                    } else if now >= self.camel_tick0 + 40 {
                        if let Some(c) = self.check.as_mut() {
                            c.camel_skip = Some("no /data reply".into());
                        }
                        self.camel_stage = 20;
                    }
                }
                // r90 — M227's nautilus, after the camel.
                if self.nautilus_stage > 0
                    && self.nautilus_stage < 20
                    && elapsed >= limit + RENDER_CHECK_TAIL_SECONDS - 0.2
                {
                    if let Some(c) = self.check.as_mut() {
                        c.nautilus_skip.get_or_insert_with(|| {
                            format!("timed out in stage {}", self.nautilus_stage)
                        });
                    }
                    self.keys.w = false;
                    self.keys.jump = false;
                    self.nautilus_stage = 20;
                }
                if self.nautilus_stage == 0 && self.camel_stage >= 20 {
                    self.nautilus_tick0 = now;
                    self.nautilus_stage = 1;
                }
                if self.nautilus_stage == 1
                    && now >= self.nautilus_tick0 + 5
                    && self
                        .session
                        .as_ref()
                        .is_some_and(|s| s.local_vehicle().is_none())
                {
                    let found = self.session.as_ref().and_then(|s| {
                        let want = s.entity_types.as_deref()?.id_of("minecraft:nautilus")?;
                        s.world
                            .entities
                            .iter()
                            .filter(|(id, e)| {
                                e.type_id == want
                                    && !CROWD_IDS.contains(id)
                                    && s.world.entities.saddled(*id)
                            })
                            .map(|(id, e)| (id, e.x, e.y, e.z))
                            .next()
                    });
                    match found {
                        Some((id, nx, ny, nz)) => {
                            if let Some(c) = self.check.as_mut() {
                                c.nautilus = Some(id);
                            }
                            if let Some(s) = self.session.as_mut() {
                                let _ =
                                    s.send_command(&format!("tp @s {} {} {}", nx + 2.0, ny, nz));
                            }
                            self.nautilus_tick0 = now;
                            self.nautilus_stage = 2;
                        }
                        None => {
                            // What the client does see: any nautilus and its
                            // saddle, and the server's replies to the staging.
                            let seen = self.session.as_ref().map(|s| {
                                let want = s
                                    .entity_types
                                    .as_deref()
                                    .and_then(|t| t.id_of("minecraft:nautilus"));
                                let found: Vec<(i32, bool)> = s
                                    .world
                                    .entities
                                    .iter()
                                    .filter(|(_, e)| Some(e.type_id) == want)
                                    .map(|(id, _)| (id, s.world.entities.saddled(id)))
                                    .collect();
                                // The replies from the channel's fill on.
                                let from = s
                                    .chat_log
                                    .iter()
                                    .rposition(|l| l.contains("filled 387"))
                                    .unwrap_or(0);
                                let replies: Vec<String> =
                                    s.chat_log.iter().skip(from).take(4).cloned().collect();
                                format!("nautili {found:?}, replies {replies:?}")
                            });
                            if let Some(c) = self.check.as_mut() {
                                c.nautilus_skip = Some(format!(
                                    "no summoned nautilus; {}",
                                    seen.unwrap_or_default()
                                ));
                            }
                            self.nautilus_stage = 20;
                        }
                    }
                }
                if self.nautilus_stage == 2 && now >= self.nautilus_tick0 + 8 {
                    let id = self.check.as_ref().and_then(|c| c.nautilus);
                    let aimed = id.zip(self.session.as_mut()).is_some_and(|(id, s)| {
                        let Some(e) = s.world.entities.get(id) else {
                            return false;
                        };
                        let (ex, ey, ez) = (e.x, e.y + 0.45, e.z);
                        let eye = [s.player.x, s.player.eye_y(), s.player.z];
                        let d = [ex - eye[0], ey - eye[1], ez - eye[2]];
                        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                        s.player.yaw = (-d[0]).atan2(d[2]).to_degrees() as f32;
                        s.player.pitch = (-d[1] / len).asin().to_degrees() as f32;
                        true
                    });
                    let on_it = self.session.as_ref().is_some_and(|s| {
                        super::entities::frame_crosshair_hit(s, &self.etypes, 1.0).map(|h| h.id)
                            == id
                    });
                    if aimed && on_it && self.entity_click(winit::event::MouseButton::Right) {
                        self.nautilus_tick0 = now;
                        self.nautilus_stage = 3;
                    } else if now >= self.nautilus_tick0 + 40 {
                        if let Some(c) = self.check.as_mut() {
                            c.nautilus_skip =
                                Some("the crosshair never reached the nautilus".into());
                        }
                        self.nautilus_stage = 20;
                    }
                }
                let naut_pos = self
                    .session
                    .as_ref()
                    .and_then(|s| s.steered_horse)
                    .filter(|h| Some(h.id) == self.check.as_ref().and_then(|c| c.nautilus))
                    .map(|h| [h.state.x, h.state.y, h.state.z]);
                let naut_step = mount_step.filter(|_| naut_pos.is_some());
                if self.nautilus_stage == 3 {
                    if let Some(p) = naut_pos {
                        if let Some(c) = self.check.as_mut() {
                            c.nautilus_steered = true;
                        }
                        if let Some(s) = self.session.as_mut() {
                            // West along the channel, level.
                            s.player.yaw = 90.0;
                            s.player.pitch = 0.0;
                            self.nautilus_counts0 = (s.vehicle_moves_sent, s.vehicle_rejections);
                            self.nautilus_jumps0 = s.ridden_jumps;
                        }
                        self.nautilus_y0 = p[1];
                        self.keys.w = true;
                        self.nautilus_tick0 = now;
                        self.nautilus_stage = 4;
                    } else if now >= self.nautilus_tick0 + 40 {
                        if let Some(c) = self.check.as_mut() {
                            c.nautilus_skip = Some("never steered the nautilus".into());
                        }
                        self.nautilus_stage = 20;
                    }
                }
                if self.nautilus_stage == 4 {
                    if let (Some(c), Some(p)) = (self.check.as_mut(), naut_pos) {
                        c.nautilus_drift = c.nautilus_drift.max((p[1] - self.nautilus_y0).abs());
                    }
                    if now >= self.nautilus_tick0 + 70 {
                        if let (Some(c), Some(st)) = (self.check.as_mut(), naut_step) {
                            c.nautilus_swim_step = st;
                        }
                        self.keys.w = false;
                        // Hold the jump key for the riding meter, then release.
                        self.keys.jump = true;
                        self.nautilus_tick0 = now;
                        self.nautilus_stage = 5;
                    }
                }
                if self.nautilus_stage == 5 {
                    if now >= self.nautilus_tick0 + 12 {
                        self.keys.jump = false;
                    }
                    let dashes = self
                        .session
                        .as_ref()
                        .map_or(0, |s| s.ridden_jumps - self.nautilus_jumps0);
                    if let (Some(c), Some(st), true) = (self.check.as_mut(), naut_step, dashes > 0)
                    {
                        c.nautilus_dash_step = c.nautilus_dash_step.max(st);
                    }
                    if dashes > 0 && now >= self.nautilus_tick0 + 20 {
                        // Inside the 40-tick cooldown: hold the key again.
                        self.keys.jump = true;
                        self.nautilus_meter_peak = 0.0;
                        self.nautilus_tick0 = now;
                        self.nautilus_stage = 6;
                    } else if now >= self.nautilus_tick0 + 60 {
                        if let Some(c) = self.check.as_mut() {
                            c.nautilus_skip = Some("the dash never fired".into());
                        }
                        self.keys.jump = false;
                        self.nautilus_stage = 20;
                    }
                }
                if self.nautilus_stage == 6 {
                    let scale = self.session.as_ref().map_or(0.0, |s| s.jump_riding_scale());
                    self.nautilus_meter_peak = self.nautilus_meter_peak.max(scale);
                    if now >= self.nautilus_tick0 + 6 {
                        let cd = self
                            .session
                            .as_ref()
                            .zip(self.check.as_ref().and_then(|c| c.nautilus))
                            .map_or(0, |(s, id)| s.world.entities.dash_cooldown(id));
                        let peak = self.nautilus_meter_peak;
                        if let Some(c) = self.check.as_mut() {
                            c.nautilus_cooldown_meter = Some((peak, cd));
                        }
                        self.keys.jump = false;
                        self.nautilus_tick0 = now;
                        self.nautilus_still_since = now;
                        self.nautilus_stage = 7;
                    }
                }
                if self.nautilus_stage == 7 {
                    if naut_step.is_some_and(|st| st > 1e-4) {
                        self.nautilus_still_since = now;
                    }
                    let settled = now >= self.nautilus_still_since + 10;
                    if settled || now >= self.nautilus_tick0 + 100 {
                        if let Some(s) = self.session.as_mut() {
                            let _ = s.send_command(
                                "data get entity @e[type=minecraft:nautilus,limit=1,sort=nearest] Pos",
                            );
                        }
                        self.seat_ask("nautilus");
                        let snap = self.session.as_ref().map(|s| {
                            (
                                s.steered_horse.map(|h| [h.state.x, h.state.y, h.state.z]),
                                s.vehicle_moves_sent,
                                s.vehicle_rejections,
                                s.ridden_jumps,
                            )
                        });
                        if let (Some((pos, moves, rej, jumps)), Some(c)) =
                            (snap, self.check.as_mut())
                        {
                            c.nautilus_client_pos = pos;
                            c.nautilus_moves_sent = moves - self.nautilus_counts0.0;
                            c.nautilus_rejections = rej - self.nautilus_counts0.1;
                            c.nautilus_dashes = jumps - self.nautilus_jumps0;
                        }
                        self.nautilus_tick0 = now;
                        self.nautilus_stage = 8;
                    }
                }
                if self.nautilus_stage == 8 {
                    let reply = self.session.as_ref().and_then(|s| {
                        s.chat_log
                            .iter()
                            .rev()
                            .find(|l| l.contains("Nautilus has the following entity data"))
                            .cloned()
                    });
                    if let Some(line) = reply {
                        let nums: Vec<f64> = line
                            .split(['[', ']', ','])
                            .filter_map(|t| t.trim().trim_end_matches('d').parse::<f64>().ok())
                            .collect();
                        if let (3, Some(c)) = (nums.len(), self.check.as_mut()) {
                            c.nautilus_server_pos = Some([nums[0], nums[1], nums[2]]);
                        }
                        if let Some(s) = self.session.as_mut() {
                            let _ = s.send_command("ride @s dismount");
                        }
                        self.nautilus_stage = 20;
                    } else if now >= self.nautilus_tick0 + 40 {
                        if let Some(c) = self.check.as_mut() {
                            c.nautilus_skip = Some("no /data reply".into());
                        }
                        self.nautilus_stage = 20;
                    }
                }
                self.seat_resolve();
                self.use_witnesses(now, elapsed, limit);
                // r79-r82 — M218's hand keys and the sprint state, from 0.29
                // (the bot rides r72's client-side horse, so W moves nothing
                // and no screen is up until 0.46). One stage per server echo.
                if self.hands_stage > 0 && self.hands_stage < 20 {
                    self.hands_stage_frames += 1;
                }
                let hand_items = self.items.clone();
                let hand_name =
                    |s: &rewo_net::play::PlaySession, off: bool| -> Option<(String, i32)> {
                        let slot = if off {
                            s.inventory.offhand()
                        } else {
                            s.inventory.held()
                        }?;
                        Some((hand_items.name(slot.item_id)?.to_string(), slot.count))
                    };
                if self.hands_stage == 0 && elapsed >= limit * 0.29 && !self.screen.inventory_open()
                {
                    // Select the dirt (the digit-key path).
                    let dirt_slot = self.session.as_ref().and_then(|s| {
                        (0..9).find(|&i| {
                            s.inventory
                                .hotbar(i)
                                .and_then(|st| self.items.name(st.item_id))
                                == Some("minecraft:dirt")
                        })
                    });
                    match dirt_slot {
                        Some(n) => {
                            if let Some(s) = self.session.as_mut() {
                                let _ = s.select_hotbar(n as u8);
                            }
                            self.hands_stage = 1;
                            self.hands_stage_frames = 0;
                        }
                        None => {
                            if let Some(c) = self.check.as_mut() {
                                c.hands_skip = Some("no dirt in the hotbar".into());
                            }
                            self.hands_stage = 20;
                        }
                    }
                }
                let dirt_in_hand = self
                    .session
                    .as_ref()
                    .and_then(|s| hand_name(s, false))
                    .is_some_and(|(n, _)| n == "minecraft:dirt");
                if self.hands_stage == 1 && (dirt_in_hand || self.hands_stage_frames > 600) {
                    let before = self.session.as_ref().and_then(|s| hand_name(s, false));
                    if let (Some((n, count)), Some(c)) = (before, self.check.as_mut()) {
                        if n == "minecraft:dirt" {
                            c.drop_counts = Some((count, count));
                        }
                    }
                    self.drop_key();
                    self.hands_stage = 2;
                    self.hands_stage_frames = 0;
                }
                if self.hands_stage == 2 {
                    let now = self
                        .session
                        .as_ref()
                        .and_then(|s| hand_name(s, false))
                        .map(|h| h.1);
                    let item_type = self.etypes.id_of("minecraft:item");
                    let near_item = self.session.as_ref().is_some_and(|s| {
                        let (px, py, pz) = (s.player.x, s.player.y, s.player.z);
                        s.world.entities.iter().any(|(_, e)| {
                            Some(e.type_id) == item_type
                                && (e.x - px).abs() < 4.0
                                && (e.y - py).abs() < 4.0
                                && (e.z - pz).abs() < 4.0
                        })
                    });
                    if let Some(c) = self.check.as_mut() {
                        if let (Some((b, _)), Some(a)) = (c.drop_counts, now) {
                            c.drop_counts = Some((b, a));
                        }
                        c.drop_item_entity |= near_item;
                    }
                    let done = self.check.as_ref().is_some_and(|c| {
                        c.drop_item_entity && c.drop_counts.is_some_and(|(b, a)| a == b - 1)
                    });
                    if done || self.hands_stage_frames > 600 {
                        self.swap_hands_key();
                        self.hands_stage = 3;
                        self.hands_stage_frames = 0;
                    }
                }
                if self.hands_stage == 3 {
                    let off = self.session.as_ref().and_then(|s| hand_name(s, true));
                    if off.as_ref().is_some_and(|(n, _)| n == "minecraft:dirt")
                        || self.hands_stage_frames > 600
                    {
                        if let Some(c) = self.check.as_mut() {
                            c.swap_offhand = off;
                        }
                        // And back, so the dirt is where later rows expect it.
                        self.swap_hands_key();
                        self.hands_stage = 4;
                        self.hands_stage_frames = 0;
                    }
                }
                if self.hands_stage == 4 {
                    let back = self
                        .session
                        .as_ref()
                        .and_then(|s| hand_name(s, false))
                        .is_some_and(|(n, _)| n == "minecraft:dirt");
                    if back || self.hands_stage_frames > 600 {
                        // Face the lectern r75 used and press the pick key.
                        let target = self.check.as_ref().and_then(|c| c.lectern_pos);
                        if let (Some((x, y, z)), Some(s)) = (target, self.session.as_mut()) {
                            let eye = [s.player.x, s.player.eye_y(), s.player.z];
                            let d = [
                                x as f64 + 0.5 - eye[0],
                                y as f64 + 0.5 - eye[1],
                                z as f64 + 0.5 - eye[2],
                            ];
                            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                            s.player.yaw = (-d[0]).atan2(d[2]).to_degrees() as f32;
                            s.player.pitch = (-d[1] / len).asin().to_degrees() as f32;
                        }
                        // What the press will target (r81's failure detail).
                        let aim = self.session.as_ref().map(|s| {
                            let ent = super::entities::frame_crosshair_hit(s, &self.etypes, 1.0)
                                .map(|h| (h.id, s.world.entities.get(h.id).map(|e| e.type_id)));
                            let block = s
                                .target_block(
                                    [s.player.x, s.player.eye_y(), s.player.z],
                                    super::look_dir(s.player.yaw, s.player.pitch),
                                    5.0,
                                )
                                .map(|h| h.block);
                            format!(
                                "entity {ent:?}, block {block:?}, eye ({:.2}, {:.2}, {:.2})",
                                s.player.x,
                                s.player.eye_y(),
                                s.player.z
                            )
                        });
                        if let Some(c) = self.check.as_mut() {
                            c.pick_aim = aim;
                        }
                        self.pick_key();
                        self.hands_stage = 5;
                        self.hands_stage_frames = 0;
                    }
                }
                if self.hands_stage == 5 {
                    let held = self
                        .session
                        .as_ref()
                        .and_then(|s| hand_name(s, false))
                        .map(|h| h.0);
                    if held.as_deref() == Some("minecraft:lectern") || self.hands_stage_frames > 600
                    {
                        if let Some(c) = self.check.as_mut() {
                            c.pick_held = held;
                        }
                        // M225: r82's sprint state moved to r89's camel. A
                        // rider sprints only on a vehicle that lets it
                        // (`vehicleCanSprint`), and the bot sits on r72's
                        // injected horse here.
                        // Back to the sword slot for the rest of the run.
                        if let Some(s) = self.session.as_mut() {
                            let _ = s.select_hotbar(0);
                        }
                        self.hands_stage = 20;
                    }
                }
                // r76 — M216's entity right-click, after r74 closes (0.24)
                // and before the horse (0.28+). Face the summoned librarian,
                // click through the production `entity_click`, read the
                // decoded offers, close through `close_container_screen`.
                // ...and only once r78 has handed creative back: a survival
                // player's entity reach is 3.0 and the villager stands ~3.1
                // away, so a click while r78 still waits for its block misses
                // (the flake this guard closes).
                let creative_again = self.mine_stage >= 4
                    && self
                        .session
                        .as_ref()
                        .is_some_and(|s| s.abilities.instabuild);
                if self.trade_stage == 0
                    && elapsed >= limit * 0.25
                    && creative_again
                    && !self.screen.inventory_open()
                {
                    let villager = self.session.as_ref().and_then(|s| {
                        let tid = self.etypes.id_of("minecraft:villager")?;
                        let (px, py, pz) = (s.player.x, s.player.y, s.player.z);
                        s.world
                            .entities
                            .iter()
                            .filter(|(id, e)| {
                                e.type_id == tid
                                    && !CROWD_IDS.contains(id)
                                    && (e.x - px).abs() < 6.0
                                    && (e.y - py).abs() < 3.0
                                    && (e.z - pz).abs() < 6.0
                            })
                            .map(|(id, e)| (id, e.x, e.y, e.z))
                            .next()
                    });
                    if let Some((id, vx, vy, vz)) = villager {
                        if let Some(s) = self.session.as_mut() {
                            let eye = [s.player.x, s.player.eye_y(), s.player.z];
                            let d = [vx - eye[0], vy + 1.0 - eye[1], vz - eye[2]];
                            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                            s.player.yaw = (-d[0]).atan2(d[2]).to_degrees() as f32;
                            s.player.pitch = (-d[1] / len).asin().to_degrees() as f32;
                        }
                        let clicked = self.entity_click(winit::event::MouseButton::Right);
                        let want = |n: &str| self.items.id(n).unwrap_or(-1);
                        let expected = vec![
                            (want("minecraft:emerald"), 5, want("minecraft:book"), 1, 12),
                            (
                                want("minecraft:paper"),
                                24,
                                want("minecraft:emerald"),
                                1,
                                16,
                            ),
                        ];
                        if let Some(c) = self.check.as_mut() {
                            c.trade_villager = clicked.then_some(id);
                            c.trade_expected = expected;
                            if !clicked {
                                c.trade_skip =
                                    Some("the crosshair pick did not hit the villager".into());
                            }
                        }
                        self.trade_stage = if clicked { 1 } else { 2 };
                    } else if elapsed >= limit * 0.27 {
                        if let Some(c) = self.check.as_mut() {
                            c.trade_skip = Some("no villager near the bot".into());
                        }
                        self.trade_stage = 2;
                    }
                }
                if self.trade_stage == 1 {
                    let offers = self
                        .session
                        .as_ref()
                        .and_then(|s| s.merchant.as_ref())
                        .map(|m| {
                            m.offers
                                .iter()
                                .map(|o| {
                                    let (ri, rc) = match &o.result {
                                        rewo_net::item_stack::WireSlot::Stack(st) => {
                                            (st.item_id, st.count)
                                        }
                                        rewo_net::item_stack::WireSlot::Empty => (-1, 0),
                                    };
                                    (o.cost_a.item_id, o.cost_a.count, ri, rc, o.max_uses)
                                })
                                .collect::<Vec<_>>()
                        });
                    if let Some(offers) = offers {
                        if let Some(c) = self.check.as_mut() {
                            c.trade_offers = offers;
                        }
                        self.close_container_screen();
                        self.trade_stage = 3;
                    } else if elapsed >= limit * 0.31 {
                        if let Some(c) = self.check.as_mut() {
                            c.trade_skip = Some("no merchant_offers by 0.31".into());
                        }
                        self.trade_stage = 2;
                    }
                }
                // r77 — the attack, once the trade screen is gone.
                if self.trade_stage == 3 && !self.screen.inventory_open() {
                    let target = self.check.as_ref().and_then(|c| c.trade_villager);
                    if let (Some(id), Some(s)) = (target, self.session.as_mut()) {
                        if let Some(e) = s.world.entities.get(id) {
                            let (vx, vy, vz) = (e.x, e.y, e.z);
                            let eye = [s.player.x, s.player.eye_y(), s.player.z];
                            let d = [vx - eye[0], vy + 1.0 - eye[1], vz - eye[2]];
                            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                            s.player.yaw = (-d[0]).atan2(d[2]).to_degrees() as f32;
                            s.player.pitch = (-d[1] / len).asin().to_degrees() as f32;
                        }
                        let unhurt = s.world.entities.hurt_state(id).hurt_time == 0;
                        if let Some(c) = self.check.as_mut() {
                            c.attack_unhurt_before = unhurt;
                        }
                        self.entity_click(winit::event::MouseButton::Left);
                    }
                    self.trade_stage = 4;
                }
                if self.trade_stage == 4 {
                    let target = self.check.as_ref().and_then(|c| c.trade_villager);
                    let hurt = target
                        .zip(self.session.as_ref())
                        .is_some_and(|(id, s)| s.world.entities.hurt_state(id).hurt_time > 0);
                    if let (true, Some(c)) = (hurt, self.check.as_mut()) {
                        c.attack_hurt_frames += 1;
                    }
                }
                // r74 — M179's tab click. `render_check.py` grants two root
                // advancements, so the screen has two tabs: open it (the
                // production `open_advancements`, which sends `opened_tab`
                // for its selected tab), click the OTHER tab's cell through
                // `advancements_tab_click` (hit-test, select, send), close it.
                // Retried each frame until the server's update has delivered
                // both tabs; the echo is counted below.
                if !self.adv_click_injected
                    && elapsed >= limit * 0.24
                    && !self.screen.inventory_open()
                {
                    let tabs = self.session.as_ref().map(|s| {
                        let b = self
                            .baked
                            .as_ref()
                            .and_then(|b| b.font.as_ref())
                            .map(|f| f.advance);
                        b.map(|adv| {
                            crate::advancements_view::AdvancementsView::build(
                                &s.advancements,
                                &self.lang,
                                &adv,
                            )
                            .screen
                            .tabs
                            .len()
                        })
                    });
                    if tabs.flatten().unwrap_or(0) >= 2 {
                        self.open_advancements();
                        let (gw, gh) = self.gui_size();
                        let pick = self.advancements.as_ref().and_then(|v| {
                            let opened = v.selected_root_id()?.to_string();
                            let (j, t) = v
                                .screen
                                .tabs
                                .iter()
                                .enumerate()
                                .find(|(i, _)| v.root_id_at(*i) != Some(opened.as_str()))?;
                            let (xo, yo) = rewo_ui::advancements_screen::window_origin(gw, gh);
                            let (x, y, w, h) = t.kind.rect_at(t.index);
                            let hit = v.root_id_at(j)?.to_string();
                            Some((
                                opened,
                                hit,
                                f64::from(xo + x) + f64::from(w) / 2.0,
                                f64::from(yo + y) + f64::from(h) / 2.0,
                            ))
                        });
                        if let Some((opened, hit, mx, my)) = pick {
                            self.advancements_tab_click(mx, my);
                            if let Some(c) = self.check.as_mut() {
                                c.adv_click = Some((opened, hit));
                            }
                        }
                        self.close_advancements();
                        self.adv_click_injected = true;
                    } else if elapsed >= limit * 0.6 {
                        if let Some(c) = self.check.as_mut() {
                            c.adv_click_skip = Some(format!(
                                "the client held {} advancement tab(s) by 0.6",
                                tabs.flatten().unwrap_or(0)
                            ));
                        }
                        self.adv_click_injected = true;
                    }
                }
                if let (Some(c), Some(s)) = (self.check.as_mut(), self.session.as_ref()) {
                    if c.adv_click
                        .as_ref()
                        .is_some_and(|(_, hit)| s.advancements.selected_tab() == Some(hit.as_str()))
                    {
                        c.adv_echo_frames += 1;
                    }
                }
                if !self.crowd_injected && elapsed >= limit * 0.3 {
                    if let Some(session) = self.session.as_mut() {
                        let types = session.entity_types.clone();
                        let p = &session.player;
                        let eye = [p.x, p.y, p.z];
                        if let Some(types) = types {
                            let spawn = |eid: i32, tid: i32, dx: f64| -> Vec<u8> {
                                let mut b: Vec<u8> = Vec::new();
                                rewo_proto::varint::write_varint(&mut b, eid);
                                // A uuid far from any real profile's.
                                let uuid =
                                    0xC0D0_0000_0000_0000_0000_0000_0000_0000u128 + eid as u128;
                                b.extend_from_slice(&uuid.to_be_bytes());
                                rewo_proto::varint::write_varint(&mut b, tid);
                                b.extend_from_slice(&(eye[0] + dx).to_be_bytes());
                                b.extend_from_slice(&eye[1].to_be_bytes());
                                b.extend_from_slice(&(eye[2] - 4.0).to_be_bytes());
                                // `LpVec3`'s one-byte zero sentinel: no motion.
                                b.push(0);
                                b.extend_from_slice(&[0, 0, 0]); // pitch, yaw, head yaw
                                b
                            };
                            let pid = session.ids.cb_play_add_entity;
                            for (i, (name, dx)) in [
                                ("minecraft:zombie", -1.5f64),
                                ("minecraft:zombie", 0.0),
                                ("minecraft:villager", 1.5),
                            ]
                            .into_iter()
                            .enumerate()
                            {
                                if let Some(tid) = types.id_of(name) {
                                    let body = spawn(CROWD_IDS[i], tid, dx);
                                    session.inject_packet(pid, &body);
                                }
                            }
                        }
                    }
                    self.crowd_injected = true;
                }
                // M173 — walk the options pages through the PRODUCTION open
                // and close paths (r62): the Sound page at 0.06 (sliders
                // sampled while it is up), Done-equivalent closes at 0.12
                // (to the root), 0.14 (to the pause screen) and 0.16 (back
                // to the game), so nothing later meets a leftover screen or
                // a stale `self.view`.
                // r83 (M219) owns the slot until the bed round trip is done;
                // the Sound page waits for it, but never past 0.11.
                if self.options_staged == 0 && elapsed >= limit * 0.06 {
                    self.options_staged = 1;
                    self.open_options_screen(rewo_ui::options_screen::OptionsPage::Sound);
                }
                if self.options_staged == 1 && elapsed >= limit * 0.12 {
                    self.options_staged = 2;
                    self.close_options(); // Sound -> Root, saving options.txt
                }
                if self.options_staged == 2 && elapsed >= limit * 0.14 {
                    self.options_staged = 3;
                    self.close_options(); // Root -> the pause screen
                }
                if self.options_staged == 3 && elapsed >= limit * 0.16 {
                    self.options_staged = 4;
                    self.close_view_screen(); // pause -> the game
                }
                // M169 — mount the bot on a saddled horse by INJECTION, the way
                // r48's crowd and r19's chest are staged (M17: injection is the
                // deterministic proof where a live encounter depends on the
                // server). The `/ride` command does not survive the render check
                // — the server never sent `set_passengers` naming the bot, so
                // `mounts` stayed empty and `jumpable_vehicle()` was always None.
                // Three packets through the production router: add the horse,
                // give it a saddle (slot 7), seat the bot on it. Before the
                // 0.32 hold so the meter has a vehicle to charge against.
                if !self.jump_injected && elapsed >= limit * 0.28 {
                    if let Some(session) = self.session.as_mut() {
                        let types = session.entity_types.clone();
                        if let (Some(types), Some(me)) = (types, session.player_id) {
                            if let Some(horse) = types.id_of("minecraft:horse") {
                                const HORSE_EID: i32 = 0x00A9_0000;
                                let p = &session.player;
                                let mut add: Vec<u8> = Vec::new();
                                rewo_proto::varint::write_varint(&mut add, HORSE_EID);
                                let uuid = 0xC0D0_0000_0000_0000_0000_0000_00A9_0000u128;
                                add.extend_from_slice(&uuid.to_be_bytes());
                                rewo_proto::varint::write_varint(&mut add, horse);
                                add.extend_from_slice(&p.x.to_be_bytes());
                                add.extend_from_slice(&p.y.to_be_bytes());
                                add.extend_from_slice(&p.z.to_be_bytes());
                                add.push(0); // LpVec3 zero sentinel — no motion
                                add.extend_from_slice(&[0, 0, 0]); // pitch, yaw, head yaw
                                session.inject_packet(session.ids.cb_play_add_entity, &add);
                                // set_equipment: the saddle in slot 7. A minimal
                                // present stack (count 1, an arbitrary valid id,
                                // an empty patch); `saddled` only tests non-empty.
                                let mut eq: Vec<u8> = Vec::new();
                                rewo_proto::varint::write_varint(&mut eq, HORSE_EID);
                                eq.push(7); // SADDLE ordinal, top bit clear = last
                                rewo_proto::varint::write_varint(&mut eq, 1); // count
                                rewo_proto::varint::write_varint(&mut eq, 1); // item id (any)
                                rewo_proto::varint::write_varint(&mut eq, 0); // added
                                rewo_proto::varint::write_varint(&mut eq, 0); // removed
                                session.inject_packet(session.ids.cb_play_set_equipment, &eq);
                                // set_passengers: seat the bot on the horse.
                                let mut pass: Vec<u8> = Vec::new();
                                rewo_proto::varint::write_varint(&mut pass, HORSE_EID);
                                rewo_proto::varint::write_varint(&mut pass, 1);
                                rewo_proto::varint::write_varint(&mut pass, me);
                                session.inject_packet(session.ids.cb_play_set_passengers, &pass);
                                // M222 — the client now steers a saddled horse
                                // its player rides, and this one exists only
                                // here: a movement speed of 0 keeps r82's held
                                // W from walking it (and the bot) away.
                                let speed = session.attribute_registry.as_deref().and_then(|r| {
                                    r.id_of("movement_speed")
                                        .or_else(|| r.id_of("minecraft:movement_speed"))
                                });
                                if let Some(attr) = speed {
                                    let mut at: Vec<u8> = Vec::new();
                                    rewo_proto::varint::write_varint(&mut at, HORSE_EID);
                                    rewo_proto::varint::write_varint(&mut at, 1);
                                    rewo_proto::varint::write_varint(&mut at, attr);
                                    at.extend_from_slice(&0.0f64.to_be_bytes());
                                    rewo_proto::varint::write_varint(&mut at, 0);
                                    session
                                        .inject_packet(session.ids.cb_play_update_attributes, &at);
                                }
                            }
                        }
                    }
                    self.jump_injected = true;
                }
                // M170 — a leashed cow tied to a fence knot, injected so
                // `collect_leashes` builds a ribbon and the pass draws it.
                // The leash decode (`set_entity_link`) is graded by
                // `rideshot`; this proves the render path only a live frame
                // reaches. Same injection technique as the horse above.
                if !self.leash_injected && elapsed >= limit * 0.28 {
                    let harness_item = self.items.id("minecraft:white_harness");
                    if let Some(session) = self.session.as_mut() {
                        let types = session.entity_types.clone();
                        if let Some(types) = types {
                            if let (Some(cow), Some(knot)) = (
                                types.id_of("minecraft:cow"),
                                types.id_of("minecraft:leash_knot"),
                            ) {
                                const COW_EID: i32 = 0x00C0_0000;
                                const KNOT_EID: i32 = 0x00C0_0001;
                                let p = &session.player;
                                let quad_at = [p.x - 3.0, p.y, p.z + 2.0];
                                let add = |eid: i32, tid: i32, x: f64, y: f64, z: f64| {
                                    let mut b: Vec<u8> = Vec::new();
                                    rewo_proto::varint::write_varint(&mut b, eid);
                                    b.extend_from_slice(&((eid as u128) << 8).to_be_bytes());
                                    rewo_proto::varint::write_varint(&mut b, tid);
                                    b.extend_from_slice(&x.to_be_bytes());
                                    b.extend_from_slice(&y.to_be_bytes());
                                    b.extend_from_slice(&z.to_be_bytes());
                                    b.push(0);
                                    b.extend_from_slice(&[0, 0, 0]);
                                    b
                                };
                                let cow_body = add(COW_EID, cow, p.x + 2.0, p.y, p.z);
                                let knot_body = add(KNOT_EID, knot, p.x + 3.0, p.y + 1.0, p.z);
                                session.inject_packet(session.ids.cb_play_add_entity, &cow_body);
                                session.inject_packet(session.ids.cb_play_add_entity, &knot_body);
                                // set_entity_link: source cow, dest knot —
                                // two fixed BE i32s.
                                let mut link: Vec<u8> = Vec::new();
                                link.extend_from_slice(&COW_EID.to_be_bytes());
                                link.extend_from_slice(&KNOT_EID.to_be_bytes());
                                session.inject_packet(session.ids.cb_play_set_entity_link, &link);
                                // M185 / r72 — a horse leashed to a happy
                                // ghast hovering above it: the quad branch.
                                if let (Some(horse), Some(ghast)) = (
                                    types.id_of("minecraft:horse"),
                                    types.id_of("minecraft:happy_ghast"),
                                ) {
                                    let (h, g) = QUAD_LEASH_EIDS;
                                    let hb = add(h, horse, quad_at[0], quad_at[1], quad_at[2]);
                                    let gb =
                                        add(g, ghast, quad_at[0], quad_at[1] + 3.0, quad_at[2]);
                                    session.inject_packet(session.ids.cb_play_add_entity, &hb);
                                    session.inject_packet(session.ids.cb_play_add_entity, &gb);
                                    let mut link: Vec<u8> = Vec::new();
                                    link.extend_from_slice(&h.to_be_bytes());
                                    link.extend_from_slice(&g.to_be_bytes());
                                    session
                                        .inject_packet(session.ids.cb_play_set_entity_link, &link);
                                    // M312 / r168 — and a white harness on the
                                    // ghast (slot 6, BODY), so the leash it
                                    // holds shows its ropes.
                                    if let Some(harness) = harness_item {
                                        let mut eq: Vec<u8> = Vec::new();
                                        rewo_proto::varint::write_varint(&mut eq, g);
                                        eq.push(6); // BODY ordinal, last
                                        rewo_proto::varint::write_varint(&mut eq, 1);
                                        rewo_proto::varint::write_varint(&mut eq, harness);
                                        rewo_proto::varint::write_varint(&mut eq, 0);
                                        rewo_proto::varint::write_varint(&mut eq, 0);
                                        session
                                            .inject_packet(session.ids.cb_play_set_equipment, &eq);
                                    }
                                }
                            }
                        }
                    }
                    self.leash_injected = true;
                }
                // M192 / M194 — r69 and r70, both at 0.2 through the
                // production router: a filled map with its data into the
                // OFF-hand (inventory slot 45, so the main-hand witnesses keep
                // their sword; the map is drawn one-handed), and an item frame
                // holding a sword, beside the bot.
                if !self.frames_injected && elapsed >= limit * 0.2 {
                    let items = self.items.clone();
                    let mut frame_pos = None;
                    if let Some(session) = self.session.as_mut() {
                        let write = rewo_proto::varint::write_varint;
                        // map_item_data: id, scale 0, unlocked, one `player`
                        // decoration at the centre, the whole 128 x 128 patch.
                        let mut data: Vec<u8> = Vec::new();
                        write(&mut data, HELD_MAP_WITNESS_ID);
                        data.push(0); // scale
                        data.push(0); // locked
                        data.push(1); // decorations present
                        write(&mut data, 1);
                        write(&mut data, 0); // `player`
                        data.extend_from_slice(&[0, 0, 0, 0]); // x, y, rot, no name
                        data.extend_from_slice(&[128, 128, 0, 0]); // w, h, x0, y0
                        write(&mut data, 128 * 128);
                        data.extend(std::iter::repeat_n(4u8 << 2 | 2, 128 * 128));
                        session.inject_packet(session.ids.cb_play_map_item_data, &data);
                        // container_set_slot: window 0, slot 45 (the off-hand),
                        // a filled map whose patch carries `minecraft:map_id`.
                        let map_item = items.id("minecraft:filled_map");
                        let map_id_type = session
                            .component_names
                            .as_ref()
                            .and_then(|c| c.ids().get("minecraft:map_id").copied());
                        if let (Some(item), Some(ty)) = (map_item, map_id_type) {
                            let mut slot: Vec<u8> = Vec::new();
                            write(&mut slot, 0); // container
                            write(&mut slot, 0); // state id
                            slot.extend_from_slice(&45i16.to_be_bytes());
                            write(&mut slot, 1); // count
                            write(&mut slot, item);
                            write(&mut slot, 1); // added
                            write(&mut slot, 0); // removed
                            write(&mut slot, ty);
                            write(&mut slot, HELD_MAP_WITNESS_ID);
                            session.inject_packet(session.ids.cb_play_container_set_slot, &slot);
                        }
                        // The frame: `add_entity` with facing 3 (south) in
                        // its data, then the sword (slot 9) turned 3 (slot 10).
                        let types = session.entity_types.clone();
                        let frame_type =
                            types.as_ref().and_then(|t| t.id_of("minecraft:item_frame"));
                        let sword = items.id("minecraft:diamond_sword");
                        if let (Some(tid), Some(sword)) = (frame_type, sword) {
                            let p = &session.player;
                            // M228: 1.5 south of the bot, not level with it:
                            // a rider seated by `positionRider` (the bot is on
                            // r72's horse from 0.28) looks down at r81's lectern
                            // (x + 2) through x + 1.5, y + 1 on its own z.
                            let (x, y, z) = (p.x + 1.5, p.y + 1.0, p.z + 1.5);
                            let mut add: Vec<u8> = Vec::new();
                            write(&mut add, FRAME_WITNESS_EID);
                            add.extend_from_slice(
                                &0xC0D0_0000_0000_0000_0000_0000_00F7_0000u128.to_be_bytes(),
                            );
                            write(&mut add, tid);
                            add.extend_from_slice(&x.to_be_bytes());
                            add.extend_from_slice(&y.to_be_bytes());
                            add.extend_from_slice(&z.to_be_bytes());
                            add.push(0); // LpVec3 zero sentinel
                            add.extend_from_slice(&[0, 0, 0]); // pitch, yaw, head yaw
                            write(&mut add, 3); // data: the facing
                            session.inject_packet(session.ids.cb_play_add_entity, &add);
                            let mut meta: Vec<u8> = Vec::new();
                            write(&mut meta, FRAME_WITNESS_EID);
                            meta.push(9);
                            write(&mut meta, 7); // ITEM_STACK
                            write(&mut meta, 1); // count
                            write(&mut meta, sword);
                            write(&mut meta, 0); // added
                            write(&mut meta, 0); // removed
                            meta.push(10);
                            write(&mut meta, 1); // INT
                            write(&mut meta, 3);
                            meta.push(0xFF);
                            session.inject_packet(session.ids.cb_play_set_entity_data, &meta);
                            frame_pos = Some([x as f32, y as f32, z as f32]);
                        }
                    }
                    if let Some(c) = self.check.as_mut() {
                        c.item_frame_pos = frame_pos;
                    }
                    self.frames_injected = true;
                }
                // M172 — a written book into hotbar slot 0 + an `open_book`,
                // both through the production router. The window is
                // 0.465..0.5: after the jump hold (ends 0.46), before the
                // 0.5 inventory force-open — which then REPLACES the reader
                // (`Screens` is one slot) and exercises the stale-book sync,
                // so every later screen-dependent witness (r22-r25 at
                // 0.90-0.95) keeps its stage. The first cut injected at 0.93
                // and r24's overlay lost its inventory.
                if !self.book_view_injected && elapsed >= limit * 0.465 {
                    if let Some(session) = self.session.as_mut() {
                        let book_item = self.items.id("minecraft:written_book");
                        let comp = session.swing_data.as_ref().map(|d| d.components);
                        if let (Some(item), Some(comp)) = (book_item, comp) {
                            let pstr = |b: &mut Vec<u8>, t: &str| {
                                rewo_proto::varint::write_varint(b, t.len() as i32);
                                b.extend_from_slice(t.as_bytes());
                            };
                            let snbt = |b: &mut Vec<u8>, t: &str| {
                                b.push(8); // TAG_String
                                b.extend_from_slice(&(t.len() as u16).to_be_bytes());
                                b.extend_from_slice(t.as_bytes());
                            };
                            // written_book_content: Filterable<title>, author,
                            // generation, pages, resolved.
                            let mut value: Vec<u8> = Vec::new();
                            pstr(&mut value, "Gate Book");
                            value.push(0);
                            pstr(&mut value, "r61");
                            rewo_proto::varint::write_varint(&mut value, 0);
                            rewo_proto::varint::write_varint(&mut value, 2);
                            snbt(&mut value, "the first page");
                            value.push(0);
                            snbt(&mut value, "the second page");
                            value.push(0);
                            value.push(0);
                            // set_player_inventory: VarInt INVENTORY slot 0
                            // (hotbar 0 = menu 36 — the app's hotbar_slot is 0,
                            // which is the slot hand 0 resolves), then the stack.
                            let mut body: Vec<u8> = Vec::new();
                            rewo_proto::varint::write_varint(&mut body, 0);
                            rewo_proto::varint::write_varint(&mut body, 1); // count
                            rewo_proto::varint::write_varint(&mut body, item);
                            rewo_proto::varint::write_varint(&mut body, 1); // added
                            rewo_proto::varint::write_varint(&mut body, 0); // removed
                            rewo_proto::varint::write_varint(&mut body, comp.written_book_content);
                            body.extend_from_slice(&value);
                            session.inject_packet(session.ids.cb_play_set_player_inventory, &body);
                            // open_book: one enum ordinal — hand 0.
                            session.inject_packet(session.ids.cb_play_open_book, &[0]);
                        }
                    }
                    self.book_view_injected = true;
                }
                // The hold. Assigned every frame rather than latched, so it is
                // the same "read the key state fresh" the real gate does.
                self.keys.tab_list = elapsed >= limit * 0.5;
                // M169 — hold Space for a tenth of the run while mounted on
                // the staged horse: the meter climbs for ~16 ticks and the
                // release sends `START_RIDING_JUMP`. A level, like Tab.
                // Only in the timed run: the tail (r88) drives the key itself.
                if elapsed < limit {
                    self.keys.jump = elapsed >= limit * 0.32 && elapsed < limit * 0.46;
                }
            }
            // M162 — the two packet tails that carry a sound. Both call sites
            // live inside `PlaySession`, which owns a socket and cannot be
            // built in a unit test (M71), so this is the only check that can
            // reach them: `soundshot` grades the arithmetic and would stay
            // green with `queue_explosion_sound` deleted and `camera_eye`
            // returning `None` forever.
            //
            // Raw bodies through the production dispatcher (M17's rule), and
            // late enough in the run that `spawned` is true — before the
            // server's first `player_position` the camera is legitimately
            // absent and 1023 is correctly silent.
            // M166 — the PLAY-state `resource_pack_push`, injected because
            // NOTHING ELSE REACHES IT. A vanilla server pushes its pack during
            // configuration, so the play arm is only taken by `/resourcepack`
            // or a plugin, and without this the whole arm could be deleted with
            // every gate green — the M45/M158 shape, caught by re-reading this
            // milestone's own diff rather than by any check.
            //
            // Raw body through the production dispatcher (M17's rule), with a
            // DIFFERENT UUID from the configuration one so r55's list has to
            // grow rather than merely be non-empty.
            if !self.play_pack_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.55 {
                    if let Some(session) = self.session.as_mut() {
                        let mut b = PLAY_PACK_ID.to_be_bytes().to_vec();
                        // An `ftp:` URL, so this one must come back INVALID_URL
                        // and the two replies differ in their ACTION as well as
                        // their id. A second FAILED_DOWNLOAD would be satisfied
                        // by a client that ignored the url entirely.
                        for t in ["ftp://example.invalid/mid-session.zip", ""] {
                            rewo_proto::varint::write_varint(&mut b, t.len() as i32);
                            b.extend_from_slice(t.as_bytes());
                        }
                        b.push(0); // required = false
                        b.push(0); // no prompt
                        let pid = session.ids.cb_play_resource_pack_push;
                        session.inject_packet(pid, &b);
                        // Inside the `if let`, not after it: a run that reached
                        // the deadline before the session existed would
                        // otherwise mark itself injected and never inject.
                        self.play_pack_injected = true;
                    }
                }
            }
            if !self.sound_tails_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.3 {
                    let explode_holder = self
                        .sounds
                        .registry
                        .id_of("minecraft:entity.generic.explode")
                        .map(|id| id + 1);
                    let mut recorded: Option<([f64; 3], [f64; 3])> = None;
                    if let (Some(session), Some(holder)) = (self.session.as_mut(), explode_holder) {
                        let eye = [session.player.x, session.player.eye_y(), session.player.z];
                        let block = |dx: i64, dy: i64, dz: i64| {
                            let x = eye[0].floor() as i64 + dx;
                            let y = eye[1].floor() as i64 + dy;
                            let z = eye[2].floor() as i64 + dz;
                            let packed =
                                ((x & 0x3FF_FFFF) << 38) | ((z & 0x3FF_FFFF) << 12) | (y & 0xFFF);
                            (x, y, z, packed)
                        };
                        let level_event = |kind: i32, packed: i64, global: bool| {
                            let mut b = kind.to_be_bytes().to_vec();
                            b.extend_from_slice(&packed.to_be_bytes());
                            b.extend_from_slice(&0i32.to_be_bytes());
                            b.push(u8::from(global));
                            b
                        };

                        // 1023, a wither spawning 40 blocks north-east and 12
                        // up: far enough that "the sound is 2 blocks from the
                        // listener" cannot be confused with "the sound is at
                        // the block".
                        let (bx, by, bz, packed) = block(40, 12, 40);
                        recorded = Some((eye, [bx as f64 + 0.5, by as f64 + 0.5, bz as f64 + 0.5]));
                        session.inject_packet(
                            session.ids.cb_play_level_event.unwrap_or(-1),
                            &level_event(1023, packed, true),
                        );

                        // 3012, a trial spawner 200 blocks away: past the
                        // strict `distanceToSqr > 100.0` gate, so it must be
                        // QUEUED rather than played.
                        let (_, _, _, far) = block(200, 0, 0);
                        session.inject_packet(
                            session.ids.cb_play_level_event.unwrap_or(-1),
                            &level_event(3012, far, false),
                        );

                        // An `explode` whose TAIL carries the sound. The
                        // particle is named rather than numbered, through the
                        // session's own report-backed registry — a hardcoded id
                        // would fail closed for the wrong reason if the
                        // registry were ever renumbered.
                        if let Some(emitter) = session
                            .particle_types()
                            .id_of("minecraft:explosion_emitter")
                        {
                            let poof = session.particle_types().id_of("minecraft:poof");
                            let smoke = session.particle_types().id_of("minecraft:smoke");
                            if let (Some(poof), Some(smoke)) = (poof, smoke) {
                                let mut b = Vec::new();
                                for v in [eye[0], eye[1] - 1.0, eye[2]] {
                                    b.extend_from_slice(&v.to_be_bytes());
                                }
                                b.extend_from_slice(&4.0f32.to_be_bytes());
                                b.extend_from_slice(&37i32.to_be_bytes()); // fixed i32
                                b.push(0); // no playerKnockback
                                rewo_proto::varint::write_varint(&mut b, emitter);
                                rewo_proto::varint::write_varint(&mut b, holder);
                                rewo_proto::varint::write_varint(&mut b, 2);
                                for id in [poof, smoke] {
                                    rewo_proto::varint::write_varint(&mut b, id);
                                    b.extend_from_slice(&0.5f32.to_be_bytes());
                                    b.extend_from_slice(&1.0f32.to_be_bytes());
                                    rewo_proto::varint::write_varint(&mut b, 1);
                                }
                                session.inject_packet(session.ids.cb_play_explode, &b);
                            }
                        }
                    }
                    if let (Some(c), Some((eye, target))) = (self.check.as_mut(), recorded) {
                        c.global_event_eye = Some(eye);
                        c.global_event_target = Some(target);
                    }
                    self.sound_tails_injected = true;
                }
            }
            if !self.chat_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.2 {
                    // M111 — 25 lines, because the scrollbar's guard is
                    // `virtualHeight != chatHeight`: it does not exist until
                    // there is more chat than the focused box's twenty rows,
                    // and a run's own join messages come to about six. Without
                    // these r28 would be a witness over a path the gate cannot
                    // reach, which is worse than no witness.
                    //
                    // Injected as raw `system_chat` bodies through the
                    // production router (M17's rule: injection is the
                    // deterministic proof where a live trigger depends on
                    // timing nothing here controls).
                    if let Some(session) = self.session.as_mut() {
                        let id = session.ids.cb_play_system_chat;
                        for i in 0..25u8 {
                            let text = format!("scrollbar filler {i}");
                            let mut body: Vec<u8> = vec![8];
                            body.extend_from_slice(&(text.len() as u16).to_be_bytes());
                            body.extend_from_slice(text.as_bytes());
                            body.push(0); // overlay = false
                            if let Some(pid) = id {
                                session.inject_packet(pid, &body);
                            }
                        }
                    }
                    // M126d — one message carrying legacy codes, so the
                    // drawn chat has more than one colour and at least one
                    // non-plain flag. Injected as a raw `system_chat` body
                    // through the production router (M17's rule) rather than
                    // sent as chat, because a server owns what it echoes back
                    // and `§` in a player message is not something the gate
                    // can rely on surviving the round trip.
                    //
                    // Every stage has to work for this to score: the NBT
                    // string reaches `parse_component`, `push_legacy` resolves
                    // the codes into five spans, `FlatComponents` carries them
                    // through the wrap as separate parts, and `chat_lines`
                    // emits one text line each. A flatten anywhere in that
                    // chain drops both counters to zero.
                    if let Some(session) = self.session.as_mut() {
                        if let Some(pid) = session.ids.cb_play_system_chat {
                            let text = concat!(
                                "\u{00a7}crewored \u{00a7}9blue ",
                                "\u{00a7}oital \u{00a7}nunder \u{00a7}mstrike"
                            );
                            let mut body: Vec<u8> = vec![8];
                            body.extend_from_slice(&(text.len() as u16).to_be_bytes());
                            body.extend_from_slice(text.as_bytes());
                            body.push(0); // overlay = false
                            session.inject_packet(pid, &body);
                        }
                    }
                    // M127 — two `disguised_chat` bodies, bound to the
                    // SERVER's own chat types by name. `handleDisguisedChat`
                    // is the shortest path to a decorated line: a component,
                    // a bound, and no signature machinery. Injected as raw
                    // bodies through the production router (M17's rule).
                    //
                    // The ids are looked up rather than assumed, because the
                    // registry is datapack-driven and the index is whatever
                    // the server's packs say. That also makes the lookup part
                    // of the claim: a client that never parsed
                    // `registry_data` for this registry answers `None` here
                    // and injects nothing, so r40 and r41 read zero instead of
                    // quietly decorating with entry 0.
                    if let Some(session) = self.session.as_mut() {
                        let pid = session.ids.cb_play_disguised_chat;
                        let plain = session.chat_type_id("minecraft:chat");
                        let whisper = session.chat_type_id("minecraft:msg_command_incoming");
                        let body = |content: &str, name: &str, id: i32| {
                            let mut b: Vec<u8> = vec![8];
                            b.extend_from_slice(&(content.len() as u16).to_be_bytes());
                            b.extend_from_slice(content.as_bytes());
                            // `ByteBufCodecs.holder` — id + 1, 0 meaning inline.
                            rewo_proto::varint::write_varint(&mut b, id + 1);
                            b.push(8);
                            b.extend_from_slice(&(name.len() as u16).to_be_bytes());
                            b.extend_from_slice(name.as_bytes());
                            b.push(0); // no targetName
                            b
                        };
                        if let Some(id) = plain {
                            let b = body("decorated", "RewoDecoWitness", id);
                            session.inject_packet(pid, &b);
                        }
                        if let Some(id) = whisper {
                            let b = body("whispered", "RewoStyleWitness", id);
                            session.inject_packet(pid, &b);
                        }
                    }
                    // M128 — one message carrying a `click_event`, so the
                    // gate can click it. A COMPOUND rather than a bare string,
                    // because a `click_event` has nowhere to live on a string
                    // component; assembled as network NBT bytes by hand, which
                    // is what `system_chat`'s content is.
                    if let Some(session) = self.session.as_mut() {
                        if let Some(pid) = session.ids.cb_play_system_chat {
                            let body = click_witness_body();
                            session.inject_packet(pid, &body);
                        }
                    }
                    // M115 — a completion word, then a keystroke, so r29
                    // measures the whole production chain rather than a
                    // hand-built `Suggestions`: the packet reaches
                    // `SuggestionProviderState`, `tab_words()` unions it with
                    // the online players, `on_edited` matches the typed prefix
                    // against it, and `auto_show` opens the list. A break
                    // anywhere in that drops the count to zero.
                    //
                    // The word is deliberately nothing a server would send, so
                    // it cannot be confused with a real player's name, and it
                    // begins with the character typed below.
                    if let Some(session) = self.session.as_mut() {
                        if let Some(pid) = Some(session.ids.cb_play_custom_chat_completions) {
                            let words = ["rewopopupwitness", "rewopopupsecond"];
                            let mut body: Vec<u8> = Vec::new();
                            body.push(2); // Action.SET
                            body.push(words.len() as u8);
                            for w in words {
                                body.push(w.len() as u8);
                                body.extend_from_slice(w.as_bytes());
                            }
                            session.inject_packet(pid, &body);
                        }
                    }
                    self.open_chat_screen(rewo_ui::chat_screen::ChatMethod::Message);
                    // `onEdited` is what turns suggestions on and asks for
                    // them; nothing else in a windowed run types.
                    self.chat_char('r');
                    self.chat_injected = true;
                }
            }
            // M128 — click the link, once, at the position the renderer
            // reported for it. Between the chat screen opening (0.2) and the
            // command screen replacing it (0.55), so the popup under the input
            // bar cannot be what answers.
            if self.chat_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                let due = self.started.elapsed().as_secs_f32() >= limit * 0.4;
                let at = self
                    .check
                    .as_ref()
                    .and_then(|c| (!c.chat_click_fired).then_some(c.chat_link_at).flatten());
                if let (true, Some((lx, ly))) = (due, at) {
                    if let Some(c) = self.check.as_mut() {
                        c.chat_click_fired = true;
                    }
                    // Four pixels down and one right of the line's origin —
                    // inside the first glyph's `[x, x + advance) x [y, y + 9)`
                    // box at any GUI scale, because both are screen pixels and
                    // the box is at least that big.
                    self.screen.mouse = ((lx + 1.0) as f64, (ly + 4.0) as f64);
                    // …and close the suggestion popup first, because
                    // `ChatScreen::mouse_clicked` gives it precedence over the
                    // link (`if self.suggestions.mouse_clicked(..) { return
                    // Handled }`) and `SuggestionsList` claims ANY click inside
                    // its rect. That is vanilla's ordering, not a bug.
                    //
                    // The comment above used to argue the popup "cannot be what
                    // answers" because the click sits between the screen opening
                    // and the command screen replacing it. That held on M128's
                    // own branch and stopped holding when M127c's decoration
                    // witnesses joined the same injection block: the clickable
                    // row is the NEWEST message, so it draws nearest the input
                    // bar — which is exactly where the popup is. The click
                    // answered `Handled`, the popup's answer, not the link's.
                    //
                    // Closing it does not weaken the claim. r42 asks whether a
                    // click on a chat link runs its command; the popup is a
                    // different widget, and r29 already witnesses that it draws.
                    if let Some(cs) = self.chat_screen.as_mut() {
                        cs.suggestions.hide();
                    }
                    let outcome = self.chat_mouse_pressed(0);
                    if outcome
                        == rewo_ui::chat_screen::ChatClick::RunCommand(CLICK_WITNESS_COMMAND.into())
                    {
                        if let Some(c) = self.check.as_mut() {
                            c.chat_click_ok += 1;
                        }
                    } else {
                        log::warn!("rendercheck: chat click answered {outcome:?}");
                    }
                }
            }
            // r67 — the link prompt, between r42's click (0.4) and the command
            // screen (0.55). Stage 0 clicks, stage 1 waits for frames, stage 2
            // answers "No" and stage 3 grades the return.
            if self.chat_injected && !self.command_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                let elapsed = self.started.elapsed().as_secs_f32();
                let (stage, frames) = self
                    .check
                    .as_ref()
                    .map_or((3, 0), |c| (c.link_prompt_stage, c.link_prompt_frames));
                match stage {
                    0 if elapsed >= limit * 0.45 && self.chat_screen.is_some() => {
                        let text = self
                            .chat_screen
                            .as_ref()
                            .map(|s| s.input.value())
                            .unwrap_or_default();
                        // The stock values, whatever this machine's
                        // options.txt says; restored at stage 2.
                        let saved = (self.options.chat_links, self.options.chat_links_prompt);
                        self.options.chat_links = true;
                        self.options.chat_links_prompt = true;
                        self.chat_click_url("https://example.com/rewo-link-witness");
                        if let Some(c) = self.check.as_mut() {
                            c.link_prompt_text = text;
                            c.link_prompt_options = saved;
                            c.link_prompt_stage = 1;
                        }
                    }
                    1 if frames >= 3 => {
                        use rewo_ui::confirm_link_screen::NO;
                        self.press_widget(rewo_ui::screen::ScreenKind::ConfirmLink, NO);
                        if let Some(c) = self.check.as_mut() {
                            self.options.chat_links = c.link_prompt_options.0;
                            self.options.chat_links_prompt = c.link_prompt_options.1;
                            c.link_prompt_stage = 2;
                        }
                    }
                    2 => {
                        let back = self.chat_screen.as_ref().map(|s| s.input.value());
                        let ok = !self.screen.any_open()
                            && matches!(self.view, ScreenView::None)
                            && self.link_return_chat.is_none();
                        if let Some(c) = self.check.as_mut() {
                            c.link_prompt_returned =
                                ok && back.as_deref() == Some(&c.link_prompt_text);
                            c.link_prompt_stage = 3;
                        }
                    }
                    _ => {}
                }
            }
            // M116 — later, and as its own screen, so r29's frames stay
            // unambiguously the message popup's. Reopening with `Command`
            // seeds the field with `/`; one letter then reaches the top-level
            // literals, which the dispatcher answers without a packet.
            if self.chat_injected && !self.command_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.55 {
                    self.close_chat_screen();
                    self.open_chat_screen(rewo_ui::chat_screen::ChatMethod::Command);
                    self.chat_char('g');
                    // …and then on to `/give `, which is where an ARGUMENT is
                    // expected: the integer child suggests nothing, so no
                    // popup opens and the USAGE box takes its place. r30 has
                    // already counted the literal completion `/g` produced;
                    // this reaches r31 and r32.
                    // …then on to `/give @s `, which is two arguments deep.
                    // M118 changed what `/give ` alone shows: its first
                    // argument is `minecraft:entity`, so a SELECTOR POPUP
                    // opens there and the usage box is suppressed by the
                    // mutual exclusion — which is what vanilla does too. The
                    // box needs an argument that suggests nothing, and the
                    // item after the targets is one.
                    // …then on to `/give @s dirt `, which is three arguments
                    // deep. **r32's precondition recedes by one word for every
                    // argument type transcribed**: M118 made `/give ` open a
                    // selector popup and M119 made `/give @s ` open an item
                    // one, each time suppressing the usage box by the mutual
                    // exclusion. The count after the item is a plain integer
                    // and suggests nothing, which is what the box needs.
                    for ch in "ive @s dirt ".chars() {
                        self.chat_char(ch);
                    }
                    self.command_injected = true;
                }
            }
            // M120 — and a coordinate. Injected LAST, at 0.8, so r32 has
            // already banked its frames against `/give @s dirt `: a
            // coordinate popup suppresses the box, which is the receding
            // precondition M118 and M119 both hit.
            if self.command_injected && !self.coords_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.8 {
                    self.close_chat_screen();
                    // **Drop the draft first.** `close_chat_screen` saves one,
                    // and `ChatMethod::Command` restores a COMMAND draft — so
                    // reopening handed the field back `/give @s dirt ` and the
                    // typing below appended to it. That is M110's
                    // `isDraftRestorable` working exactly as documented, and
                    // the gate read as "the coordinate family offers nothing"
                    // when what it had actually typed was
                    // `/give @s dirt setblock `.
                    self.chat_draft = None;
                    self.open_chat_screen(rewo_ui::chat_screen::ChatMethod::Command);
                    for ch in "setblock ".chars() {
                        self.chat_char(ch);
                    }
                    self.coords_injected = true;
                }
            }
            // M124 — a literal table, injected LAST at 0.9 for the reason
            // every one of these has been: whichever popup is open last
            // suppresses the usage box, and r32 banks its frames earlier.
            if self.coords_injected && !self.literal_table_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.9 {
                    self.close_chat_screen();
                    // The draft drop M120 needed, for M110's reason.
                    self.chat_draft = None;
                    self.open_chat_screen(rewo_ui::chat_screen::ChatMethod::Command);
                    for ch in "scoreboard objectives setdisplay ".chars() {
                        self.chat_char(ch);
                    }
                    self.literal_table_injected = true;
                }
            }
            // M134 — a command that does not exist, which is the shortest
            // route to `updateUsageInfo`'s THIRD branch and therefore to an
            // exception message. Injected after every other typing stage
            // because it is the one that must not have a popup over it: no
            // literal matches, so no completion is offered, so
            // `extractRenderState` falls through to the usage box. Every
            // earlier stage exists to open a popup, and a popup would hide
            // this.
            //
            // Nothing is recorded in `getExceptions()` for this input — the
            // root's children are all literals, none matched, and
            // `getRelevantNodes` therefore returned the empty argument set —
            // so this reaches the line through `Commands.getParseException`
            // rather than through the exception loop. That is the branch M117
            // omitted entirely.
            if self.literal_table_injected && !self.bad_command_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.95 {
                    self.close_chat_screen();
                    // The draft drop M120 needed, for M110's reason.
                    self.chat_draft = None;
                    self.open_chat_screen(rewo_ui::chat_screen::ChatMethod::Command);
                    for ch in "rewonosuchcommand".chars() {
                        self.chat_char(ch);
                    }
                    self.bad_command_injected = true;
                }
            }
            if !self.container_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.4 {
                    if let Some(session) = self.session.as_mut() {
                        // VarInt container id, VarInt menu type (RAW, not a
                        // holder), then an NBT string title.
                        let mut body: Vec<u8> = vec![7, 2, 8];
                        let title = b"Chest";
                        body.extend_from_slice(&(title.len() as u16).to_be_bytes());
                        body.extend_from_slice(title);
                        let id = session.ids.cb_play_open_screen;
                        let opened =
                            rewo_net::route_menu(id, &body, &session.ids, &mut session.menus)
                                && session.menus.open().is_some();
                        if opened {
                            self.container_injected = true;
                        }
                    }
                }
            }
            // M174 — open the staged oak_sign's editor through the PRODUCTION
            // decode (`inject_packet` -> the play dispatch), at 0.80: after
            // the container's r19/r20 have latched, late enough that the
            // inventory (force-opened at 0.5) keeps the >25%-of-frames
            // window r16 requires — the first cut injected at 0.55 and
            // starved it to 21% — and before the 0.90 book-menu injection
            // whose replacement exercises r64's commit.
            if !self.sign_editor_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.80 {
                    let witness = self.sign_witness_pos;
                    if let Some(session) = self.session.as_mut() {
                        // Where the witness was staged (M222: the bot can
                        // have moved since — its client-side horse is now
                        // steered, and r72's riding jump nudges it).
                        let (dx, dy, dz) = SIGN_WITNESS_OFFSET;
                        let (x, y, z) = witness.unwrap_or((
                            session.player.x.floor() as i32 + dx,
                            session.player.y.floor() as i32 + dy,
                            session.player.z.floor() as i32 + dz,
                        ));
                        // Packed BlockPos: x<<38 | z<<12 | y — the packing
                        // `sign_update_body` writes and `position()` reads.
                        let packed: u64 = (((x as i64 & 0x3ff_ffff) as u64) << 38)
                            | (((z as i64 & 0x3ff_ffff) as u64) << 12)
                            | ((y as i64 & 0xfff) as u64);
                        let mut body = Vec::new();
                        body.extend_from_slice(&packed.to_be_bytes());
                        body.push(1); // isFrontText
                        session.inject_packet(session.ids.cb_play_open_sign_editor, &body);
                        self.sign_editor_injected = true;
                    }
                }
            }
            // M94 — last of all, a CRAFTING TABLE, because the book only draws
            // while a book menu is on screen and neither of the two injections
            // below is one: `book_type_of` answers for the player's own
            // inventory, `crafting`, and the three furnaces, and nothing else.
            // The chest opened at 0.4 holds the screen for the rest of the run,
            // so without this the windowed client never reaches the book's
            // builder however long it runs — the same shape of gap as M92's
            // overlay injection, one screen over.
            //
            // Last (0.90) so `r20` and `r22` have already latched: a crafting
            // table's panel is 176x166, the player's own size, and it draws no
            // overlays.
            if !self.book_menu_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.90 {
                    if let Some(session) = self.session.as_mut() {
                        // Menu type 12. NOT 13 - that is `enchantment`,
                        // which this first got wrong, and which opened a
                        // perfectly valid screen with no book.
                        let mut body: Vec<u8> = vec![11, 12, 8];
                        let title = b"Crafting";
                        body.extend_from_slice(&(title.len() as u16).to_be_bytes());
                        body.extend_from_slice(title);
                        let open_id = session.ids.cb_play_open_screen;
                        if rewo_net::route_menu(open_id, &body, &session.ids, &mut session.menus) {
                            self.book_menu_injected = true;
                        }
                    }
                }
            }
            // M104 — and then the which-of-these overlay, through the SAME
            // `open_overlay` a right-click calls.
            //
            // Injected rather than clicked because a click needs the cursor
            // over a particular cell AND a server that has sent a multi-recipe
            // group, neither of which this gate controls — M17's rule. It must
            // come after the crafting table (0.90), because `set_screen_open`
            // clears the overlay: a snapshot of a page must not survive the
            // screen it was taken on.
            if !self.book_overlay_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.95 && self.book_menu_injected {
                    // Only `filtering` and `furnace_family` are read out of
                    // the view, and both are false for a crafting table's
                    // unfiltered book — the rest is named so the fixture reads
                    // as a whole `BookView` rather than a partial one.
                    let view = rewo_ui::recipe_book_screen::BookView {
                        tabs: rewo_ui::recipe_book_screen::CRAFTING_TABS.len(),
                        selected_tab: 0,
                        page: 0,
                        total_pages: 1,
                        shown: 1,
                        filtering: false,
                        furnace_family: false,
                    };
                    let collection = (0..3)
                        .map(|i| rewo_ui::recipe_overlay::Button {
                            recipe: i,
                            craftable: i == 0,
                            slots: Vec::new(),
                        })
                        .collect();
                    self.screen.book_overlay = Some(open_overlay(collection, 0, view));
                    self.book_overlay_injected = true;
                }
            }
            // M92 — near the end, replace it with a BREWING STAND and give it
            // data, so the frame loop has to reach the overlay builder.
            //
            // A chest has no overlays at all, so the injection above cannot
            // exercise this path however long it runs. `containershot` grades
            // the overlays offscreen; without this, nothing says the windowed
            // client ever draws one — the gap M88 closed for the panel and M86
            // for nine features before that.
            //
            // Late (0.85) and after `r20` has latched its height, because a
            // brewing stand's panel is 166 tall — the same as the player's —
            // and so cannot serve `r20`'s discrimination.
            if !self.brewing_injected {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.85 {
                    if let Some(session) = self.session.as_mut() {
                        let mut body: Vec<u8> = vec![9, 11, 8];
                        let title = b"Brewing Stand";
                        body.extend_from_slice(&(title.len() as u16).to_be_bytes());
                        body.extend_from_slice(title);
                        let open_id = session.ids.cb_play_open_screen;
                        if rewo_net::route_menu(open_id, &body, &session.ids, &mut session.menus) {
                            // ...and its data: 200 ticks left of a brew, 20
                            // charges of fuel. Both packets through the
                            // production router (M17's precedent), so the
                            // decode, the id gate and the data-slot write are
                            // all the shipped ones.
                            let data_id = session.ids.cb_play_container_set_data;
                            for (slot, value) in [(0i16, 200i16), (1, 20)] {
                                // VarInt container id, then two BE i16s —
                                // fixed-width shorts among the var-ints (M87).
                                let mut d = vec![9u8];
                                d.extend_from_slice(&slot.to_be_bytes());
                                d.extend_from_slice(&value.to_be_bytes());
                                rewo_net::route_menu(data_id, &d, &session.ids, &mut session.menus);
                            }
                            self.brewing_injected = true;
                        }
                    }
                }
            }
            // Three-quarters through, turn on advanced tooltips (F3+H), which
            // adds the item's id as a second line.
            //
            // This is what makes the Velvet fix *gradeable*. The rebuild the
            // screen-open triggers happens on the first frame the pass exists,
            // before it has ever drawn — so destroying its image then is legal
            // and the M3 mutation survived it. New glyphs arriving while the
            // pass is already drawing every frame is the case the `wait_idle`
            // is actually for, and a second tooltip line is the cheapest way to
            // produce it.
            if half && self.screen.inventory_open() && !self.advanced_tooltips {
                let limit = self.run_seconds.unwrap_or(RENDER_CHECK_SECONDS);
                if self.started.elapsed().as_secs_f32() >= limit * 0.75 {
                    self.advanced_tooltips = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod witness_seam_tests {
    use super::witness_seam_faults;

    fn names(ids: &[usize]) -> Vec<String> {
        ids.iter().map(|i| format!("r{i} something")).collect()
    }
    fn faults(ids: &[usize], expected: usize) -> Vec<String> {
        let owned = names(ids);
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        witness_seam_faults(&refs, expected)
    }

    /// The healthy case, so every assertion below is a difference rather than a
    /// constant.
    #[test]
    fn a_contiguous_unique_set_of_the_declared_size_is_clean() {
        assert!(faults(&[1, 2, 3, 4], 4).is_empty());
    }

    /// **The M127-M134 failure, and the one fifteen specs were about to
    /// reproduce.** Two rows carrying `r48` merge cleanly and the count still
    /// matches, so the duplicate is the ONLY signal.
    #[test]
    fn two_milestones_minting_the_same_id_is_a_fault() {
        let f = faults(&[1, 2, 3, 3], 4);
        assert!(
            f.iter().any(|s| s.contains("duplicate")),
            "a duplicate id is reported: {f:?}"
        );
    }

    /// …and it is reported EVEN THOUGH the other two checks are silent, which
    /// is what makes the duplicate check load-bearing rather than redundant.
    ///
    /// **This assertion was wrong before the code was.** Its first version
    /// claimed two faults — "the duplicate AND the gap at r4" — and there is no
    /// gap: the count check sees four ids against a declared four, and the
    /// contiguity check runs on the DEDUPLICATED set, which is `[1, 2, 3]` and
    /// perfectly contiguous. That is precisely the point. A merge that mints
    /// `r48` twice leaves a namespace whose size and shape both look right, so
    /// the duplicate is the only signal there is.
    #[test]
    fn a_duplicate_is_the_only_signal_when_the_count_and_shape_still_look_right() {
        let f = faults(&[1, 2, 3, 3], 4);
        assert_eq!(f.len(), 1, "exactly one fault: {f:?}");
        assert!(f[0].contains("duplicate"), "{f:?}");
    }

    /// A row lost to a conflict resolution: unique, contiguous, wrong count.
    #[test]
    fn a_row_silently_dropped_moves_the_count() {
        let f = faults(&[1, 2, 3], 4);
        assert!(
            f.iter()
                .any(|s| s.contains("witness count 3 != declared 4")),
            "{f:?}"
        );
    }

    /// A deleted row whose id was never reclaimed — the fault that stops the
    /// NEXT milestone choosing a number that looks free.
    #[test]
    fn an_unreclaimed_id_leaves_a_gap() {
        let f = faults(&[1, 2, 4], 3);
        assert!(f.iter().any(|s| s.contains("first gap at r3")), "{f:?}");
    }

    /// A non-`rNN` row is not a witness id and must not be counted as one —
    /// otherwise the count check fires on rows the namespace does not own.
    #[test]
    fn a_row_without_an_id_is_not_counted() {
        let owned = ["r1 a".to_string(), "not-a-witness".to_string()];
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        assert!(witness_seam_faults(&refs, 1).is_empty());
    }
}

/// The columns a 26.2 server keeps sent to a player whose chunk-cache centre
/// is `center` at view distance `vd` (r73): `ChunkTrackingView.Positioned.
/// contains(x, z)`, i.e. `isWithinDistance(.., includeNeighbors = true)`:
///
/// ```text
/// int bufferRange = includeNeighbors ? 2 : 1;
/// long deltaX = Math.max(0, Math.abs(chunkX - centerX) - bufferRange);
/// long deltaZ = Math.max(0, Math.abs(chunkZ - centerZ) - bufferRange);
/// return deltaX * deltaX + deltaZ * deltaZ < viewDistance * viewDistance;
/// ```
///
/// `vd` is `ChunkMap.getPlayerViewDistance`: `clamp(requested, 2,
/// serverViewDistance)`.
pub(super) fn server_tracked_columns(
    center: (i32, i32),
    vd: i32,
) -> std::collections::HashSet<(i32, i32)> {
    let r = vd + 2;
    let mut out = std::collections::HashSet::new();
    for dx in -r..=r {
        for dz in -r..=r {
            let bx = i64::from((dx.abs() - 2).max(0));
            let bz = i64::from((dz.abs() - 2).max(0));
            if bx * bx + bz * bz < i64::from(vd) * i64::from(vd) {
                out.insert((center.0 + dx, center.1 + dz));
            }
        }
    }
    out
}

#[cfg(test)]
mod tracking_tests {
    use super::server_tracked_columns;

    /// The sizes, against the 2026-10-18 normal-world load test's
    /// measurement: a client asking for 10 from a server at 16 held exactly
    /// 473 columns, which is the set at 10.
    #[test]
    fn the_tracking_set_matches_the_measured_column_count() {
        assert_eq!(server_tracked_columns((0, 0), 10).len(), 473);
        assert_eq!(server_tracked_columns((0, 0), 6).len(), 213);
        assert_eq!(server_tracked_columns((0, 0), 8).len(), 329);
        let s = server_tracked_columns((3, -2), 6);
        // Along an axis `|dx| - 2 < vd`: the outermost ring is vd + 1.
        assert!(s.contains(&(3 + 7, -2)) && !s.contains(&(3 + 8, -2)));
        assert!(s.contains(&(3 + 6, -2 + 6)) && !s.contains(&(3 + 7, -2 + 7)));
    }
}

/// r78's expected tick count, from vanilla's literals and its float
/// arithmetic: oak leaves `strength(0.2F)`, no tool needed (modifier 30) —
/// `speed / 0.2F / 30` added per tick until the sum reaches 1.
pub(super) fn leaves_ticks(speed: f32) -> u64 {
    let per_tick = speed / 0.2f32 / 30.0;
    let (mut sum, mut n) = (0.0f32, 0u64);
    while sum < 1.0 {
        sum += per_tick;
        n += 1;
    }
    n
}
