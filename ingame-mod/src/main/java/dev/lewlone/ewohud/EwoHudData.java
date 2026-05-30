package dev.lewlone.ewohud;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.Collection;

import net.minecraft.client.Minecraft;
import net.minecraft.client.Options;
import net.minecraft.client.multiplayer.ClientPacketListener;
import net.minecraft.client.multiplayer.PlayerInfo;
import net.minecraft.client.multiplayer.ServerData;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.world.effect.MobEffect;
import net.minecraft.world.effect.MobEffectInstance;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;

import dev.lewlone.ewohud.pvp.EwoHitRange;
import dev.lewlone.ewohud.pvp.EwoJumpReset;
import dev.lewlone.ewohud.pvp.EwoPvpModule;

/**
 * The JVM&rarr;Rust HUD data block.
 *
 * <p>A direct {@link ByteBuffer} the mod allocates once and hands to Rust (via
 * {@code nativeInit}); the Rust side resolves its address and never marshals
 * across JNI again. {@link #capture} fills it once per frame on the render
 * thread, just before {@code nativeRender}.
 *
 * <p>The field offsets here are mirrored byte-for-byte in
 * {@code crates/ewo-jni/src/hud.rs}. {@link #SCHEMA_VERSION} guards the two
 * sides against drifting — bump it on both sides whenever the layout changes.
 */
public final class EwoHudData {
    private EwoHudData() {}

    /** Layout version — checked on the Rust side; bump on any layout change. */
    public static final int SCHEMA_VERSION = 10;
    /** Fixed buffer size — generous headroom for the whole E3 widget set. */
    public static final int CAPACITY = 4096;

    /** Max potion effects carried per frame. */
    public static final int MAX_POTIONS = 8;
    private static final int POTION_REC = 44;      // bytes per potion record
    private static final int POTION_NAME_CAP = 28; // name data bytes per record
    private static final int TARGET_NAME_CAP = 44; // target-name data bytes
    private static final int SERVER_CAP = 48;      // server-address data bytes
    private static final int PLAYER_NAME_CAP = 24; // player-name data bytes

    // Field offsets — keep in sync with hud.rs `mod off`.
    private static final int OFF_VERSION = 0;
    private static final int OFF_FLAGS = 4;
    private static final int OFF_FPS = 8;
    private static final int OFF_PING = 12;
    private static final int OFF_KEYS = 16;
    private static final int OFF_X = 24;
    private static final int OFF_Y = 32;
    private static final int OFF_Z = 40;
    private static final int OFF_ARMOR = 48;        // 4 × { i32 present, f32 durability }
    private static final int OFF_POTION_COUNT = 80;
    private static final int OFF_POTIONS = 84;      // MAX_POTIONS × POTION_REC
    private static final int OFF_TARGET_PRESENT = 436;
    private static final int OFF_TARGET_DIST = 440;
    private static final int OFF_TARGET_HP = 444;
    private static final int OFF_TARGET_MAXHP = 448;
    private static final int OFF_TARGET_NAME = 452; // i32 len + 44 bytes
    private static final int OFF_PLAYTIME = 500;    // i32 session seconds
    private static final int OFF_SERVER = 504;      // i32 len + 48 bytes
    private static final int OFF_PLAYER_NAME = 556; // i32 len + 24 bytes

    // PvP Utils — schema 4. Two contiguous records: jump-reset, then hit-range.
    private static final int OFF_PVP_JUMP = 584;       // i32 tier, i32 offset_ms, i32 age_ticks, i32 fade_total
    private static final int OFF_PVP_HIT = 600;        // f32 distance, i32 color_rgb, i32 age_ticks, i32 fade_total

    // Combat HUD additions — schema 5. CPS pair + four tracked item counts.
    private static final int OFF_CPS_LEFT = 616;       // i32 clicks-per-second, left mouse
    private static final int OFF_CPS_RIGHT = 620;      // i32 clicks-per-second, right mouse
    private static final int OFF_ITEM_PEARLS = 624;    // i32 ender pearls in inventory
    private static final int OFF_ITEM_ARROWS = 628;    // i32 arrows
    private static final int OFF_ITEM_TOTEMS = 632;    // i32 totems of undying
    private static final int OFF_ITEM_GAPPLES = 636;   // i32 enchanted golden apples

    // Indicators block — schema 6. World-anchored per-entity overhead data:
    // an i32 count followed by MAX_TRACKED × 40-byte records (see EwoIndicators).
    private static final int OFF_INDICATORS = 640;     // i32 count + 16 × 40-byte records
    // Indicator block ends at 640 + 4 + 16*40 = 1284. Plenty of room left in CAPACITY 4096.

    // Combat HUD additions — schema 7. Local-player shield cooldown fraction.
    private static final int OFF_SHIELD_COOLDOWN = 1284; // f32, 0 = ready, 1 = just disabled

    // Hit indicator — schema 8. Direction of the most recent attacker, for the
    // screen-edge chevron. i32 present + f32 relative-yaw (deg) + f32 age (sec).
    private static final int OFF_HIT_PRESENT = 1288;
    private static final int OFF_HIT_REL_YAW = 1292;
    private static final int OFF_HIT_AGE = 1296;

    // Attack charge — schema 9. Local-player attack-strength scale (0..1).
    private static final int OFF_ATTACK_CHARGE = 1300; // f32, 0 = freshly attacked, 1 = ready

    // Combo counter — schema 10. Consecutive-hit tracker + seconds since
    // last hit (so the renderer can fade the chip as the combo ages).
    private static final int OFF_COMBO_COUNT = 1304;
    private static final int OFF_COMBO_AGE = 1308;

    // flags bits
    private static final int FLAG_WORLD = 1;
    private static final int FLAG_PING = 1 << 1;
    private static final int FLAG_ARMOR = 1 << 2;
    private static final int FLAG_TARGET = 1 << 3;
    private static final int FLAG_OVERLAY = 1 << 4; // the EwoClient overlay is open
    private static final int FLAG_PVP_JUMP = 1 << 5; // jump-reset result is live
    private static final int FLAG_PVP_HIT = 1 << 6;  // hit-range result is live

    // jump-reset tier values written into OFF_PVP_JUMP (matches the enum order
    // in EwoJumpReset.Tier; reordering needs a SCHEMA_VERSION bump).
    private static final int TIER_NONE = 0;
    private static final int TIER_PERFECT = 1;
    private static final int TIER_SLIGHTLY_EARLY = 2;
    private static final int TIER_EARLY = 3;
    private static final int TIER_SLIGHTLY_LATE = 4;
    private static final int TIER_LATE = 5;

    // keys bits
    private static final int K_FWD = 1;
    private static final int K_LEFT = 1 << 1;
    private static final int K_BACK = 1 << 2;
    private static final int K_RIGHT = 1 << 3;
    private static final int K_JUMP = 1 << 4;
    private static final int K_SNEAK = 1 << 5;
    private static final int K_SPRINT = 1 << 6;
    private static final int K_ATTACK = 1 << 7;
    private static final int K_USE = 1 << 8;

    private static final EquipmentSlot[] ARMOR_SLOTS = {
        EquipmentSlot.HEAD, EquipmentSlot.CHEST, EquipmentSlot.LEGS, EquipmentSlot.FEET
    };

    /** Wall-clock millis when this class loaded — the session-playtime base. */
    private static final long SESSION_START = System.currentTimeMillis();

    /** Cached single-shield stack for ItemCooldowns probing — allocated once,
     *  never modified. Avoids a per-frame ItemStack alloc just to query the
     *  shield cooldown fraction.
     *
     *  <p>Lazy: initialised on first {@link #capture} call rather than at
     *  class load. {@code new ItemStack(Items.SHIELD)} reads the item's data
     *  components (1.20.5+); those aren't bound until MC's resource-manager
     *  reload runs, which happens AFTER Fabric mod init. Initialising this
     *  field at class load (via the {@code allocate}-from-{@code
     *  onInitializeClient} call chain) hits
     *  "{@code Components not bound yet}" and takes the whole HUD bridge
     *  down with it. capture() always runs from the render loop, well past
     *  registry bind time, so deferring is safe. */
    private static ItemStack shieldProbe;

    private static ByteBuffer buffer;

    /** Allocate the shared block. Called once, before {@code nativeInit}. */
    public static ByteBuffer allocate() {
        buffer = ByteBuffer.allocateDirect(CAPACITY).order(ByteOrder.nativeOrder());
        buffer.putInt(OFF_VERSION, SCHEMA_VERSION);
        return buffer;
    }

    /** Fill the block from the live game. Called once per frame, pre-render. */
    public static void capture() {
        ByteBuffer b = buffer;
        if (b == null) {
            return;
        }
        Minecraft mc = Minecraft.getInstance();
        if (mc == null) {
            return;
        }

        // Drive the PvP-Utils trackers + fire any sounds before we sample
        // their state into the shared block. Pure Java work, no rendering.
        EwoPvpModule.tick();

        // Sample the per-frame click rate before reading it into the buffer.
        EwoClickTracker.tick();

        b.putInt(OFF_FPS, mc.getFps());

        int flags = 0;
        LocalPlayer player = mc.player;

        // Coords — only when a player is in a loaded world.
        if (player != null && mc.level != null) {
            flags |= FLAG_WORLD;
            b.putDouble(OFF_X, player.getX());
            b.putDouble(OFF_Y, player.getY());
            b.putDouble(OFF_Z, player.getZ());
        }

        // Ping — the local player's own latency, when on a server connection.
        int ping = 0;
        if (player != null) {
            ClientPacketListener conn = mc.getConnection();
            if (conn != null) {
                PlayerInfo info = conn.getPlayerInfo(player.getUUID());
                if (info != null) {
                    ping = info.getLatency();
                    flags |= FLAG_PING;
                }
            }
        }
        b.putInt(OFF_PING, ping);

        // Keystrokes — bitmask of held movement/action keys.
        int keys = 0;
        Options o = mc.options;
        if (o != null) {
            if (o.keyUp.isDown())     keys |= K_FWD;
            if (o.keyLeft.isDown())   keys |= K_LEFT;
            if (o.keyDown.isDown())   keys |= K_BACK;
            if (o.keyRight.isDown())  keys |= K_RIGHT;
            if (o.keyJump.isDown())   keys |= K_JUMP;
            if (o.keyShift.isDown())  keys |= K_SNEAK;
            if (o.keySprint.isDown()) keys |= K_SPRINT;
            if (o.keyAttack.isDown()) keys |= K_ATTACK;
            if (o.keyUse.isDown())    keys |= K_USE;
        }
        b.putInt(OFF_KEYS, keys);

        // Armor durability, potions, and the looked-at entity.
        if (player != null && captureArmor(b, player)) {
            flags |= FLAG_ARMOR;
        }
        capturePotions(b, player);
        if (player != null && captureTarget(b, mc, player)) {
            flags |= FLAG_TARGET;
        } else {
            b.putInt(OFF_TARGET_PRESENT, 0);
        }

        if (mc.screen instanceof EwoOverlayScreen) {
            flags |= FLAG_OVERLAY;
        }

        // PvP Utils — write the jump-reset record. Always writes the four
        // ints; the flag bit tells the HUD whether the result is "live" or a
        // stale value the renderer should ignore.
        if (EwoJumpReset.hasResult()) {
            flags |= FLAG_PVP_JUMP;
        }
        b.putInt(OFF_PVP_JUMP,     tierToInt(EwoJumpReset.currentTier()));
        b.putInt(OFF_PVP_JUMP + 4, EwoJumpReset.currentOffsetMs());
        b.putInt(OFF_PVP_JUMP + 8, EwoJumpReset.ageTicks());
        b.putInt(OFF_PVP_JUMP + 12, EwoJumpReset.fadeTotalTicks());

        if (EwoHitRange.hasResult()) {
            flags |= FLAG_PVP_HIT;
        }
        b.putFloat(OFF_PVP_HIT,     EwoHitRange.lastDistance());
        b.putInt(OFF_PVP_HIT + 4,   EwoHitRange.matchedZoneColor() & 0xFFFFFF);
        b.putInt(OFF_PVP_HIT + 8,   EwoHitRange.ageTicks());
        b.putInt(OFF_PVP_HIT + 12,  EwoHitRange.fadeTotalTicks());

        // CPS — always written, gated by world_active on the renderer side.
        b.putInt(OFF_CPS_LEFT,  EwoClickTracker.leftCps());
        b.putInt(OFF_CPS_RIGHT, EwoClickTracker.rightCps());

        // Item counts — sum every matching stack across the player's inventory.
        int pearls = 0, arrows = 0, totems = 0, gapples = 0;
        if (player != null) {
            Inventory inv = player.getInventory();
            for (int i = 0; i < inv.getContainerSize(); i++) {
                ItemStack st = inv.getItem(i);
                if (st.isEmpty()) {
                    continue;
                }
                Item it = st.getItem();
                if (it == Items.ENDER_PEARL) pearls += st.getCount();
                else if (it == Items.ARROW) arrows += st.getCount();
                else if (it == Items.TOTEM_OF_UNDYING) totems += st.getCount();
                else if (it == Items.ENCHANTED_GOLDEN_APPLE) gapples += st.getCount();
            }
        }
        b.putInt(OFF_ITEM_PEARLS,  pearls);
        b.putInt(OFF_ITEM_ARROWS,  arrows);
        b.putInt(OFF_ITEM_TOTEMS,  totems);
        b.putInt(OFF_ITEM_GAPPLES, gapples);

        // World-anchored indicators — per-entity overhead data for the totem
        // counter + floating-health widgets. The fill writes the i32 count
        // header itself, then up to MAX_TRACKED records.
        EwoIndicators.fill(b, OFF_INDICATORS);

        // Local-player shield cooldown — 0 when ready, 1 just after disable,
        // fading back to 0 as the cooldown ticks down. Drives the shield
        // cooldown bar widget. Always written; renderer gates on > 0.
        float shieldPct = 0f;
        if (player != null) {
            if (shieldProbe == null) {
                // First-tick lazy init — see the field's javadoc for why
                // we can't do this at class load.
                shieldProbe = new ItemStack(Items.SHIELD);
            }
            shieldPct = player.getCooldowns().getCooldownPercent(shieldProbe, 0f);
        }
        b.putFloat(OFF_SHIELD_COOLDOWN, shieldPct);

        // Hit indicator — direction of the most recent attacker, in degrees
        // relative to the player's look direction (0 = ahead, ±180 = behind).
        // Uses LivingEntity.getLastHurtByMob — vanilla's revenge-target field,
        // which is the most reliable "who hit me" attribution available.
        int hitPresent = 0;
        float hitYaw = 0f;
        float hitAgeSec = 99f;
        if (player != null) {
            LivingEntity attacker = player.getLastHurtByMob();
            int ageTicks = player.tickCount - player.getLastHurtByMobTimestamp();
            if (attacker != null && attacker != player && ageTicks >= 0 && ageTicks < 60) {
                double dx = attacker.getX() - player.getX();
                double dz = attacker.getZ() - player.getZ();
                float attackerYawDeg = (float) (Math.toDegrees(Math.atan2(-dx, dz)));
                float rel = attackerYawDeg - player.getYRot();
                while (rel > 180f) rel -= 360f;
                while (rel < -180f) rel += 360f;
                hitPresent = 1;
                hitYaw = rel;
                hitAgeSec = ageTicks / 20f;
            }
        }
        b.putInt(OFF_HIT_PRESENT, hitPresent);
        b.putFloat(OFF_HIT_REL_YAW, hitYaw);
        b.putFloat(OFF_HIT_AGE, hitAgeSec);

        // Attack charge — vanilla's attack-strength scale, 0 (freshly used)
        // ramping back to 1 (ready for full-damage attack). Drives the
        // Attack Charge HUD widget + the Auto Hit Timing trigger threshold.
        float charge = 1f;
        if (player != null) {
            charge = player.getAttackStrengthScale(0f);
        }
        b.putFloat(OFF_ATTACK_CHARGE, charge);

        // Combo counter — driven by the EwoComboTracker static state, which
        // PlayerAttackMixin increments and tick() resets on health drops +
        // timeout.
        EwoComboTracker.tick(player);
        b.putInt(OFF_COMBO_COUNT, EwoComboTracker.count());
        b.putFloat(OFF_COMBO_AGE, EwoComboTracker.ageSec());

        // Session playtime, server address, and account name — for the
        // overlay's HOME / overview tab.
        b.putInt(OFF_PLAYTIME, (int) ((System.currentTimeMillis() - SESSION_START) / 1000L));
        ServerData sd = mc.getCurrentServer();
        String server = sd != null ? sd.ip : (mc.level != null ? "Singleplayer" : "");
        putString(b, OFF_SERVER, SERVER_CAP, server);
        putString(b, OFF_PLAYER_NAME, PLAYER_NAME_CAP,
                mc.getUser() != null ? mc.getUser().getName() : "");

        b.putInt(OFF_FLAGS, flags);
    }

    /** Write the 4 armor slots (head/chest/legs/feet). Returns true if any worn. */
    private static boolean captureArmor(ByteBuffer b, LivingEntity player) {
        boolean any = false;
        for (int i = 0; i < ARMOR_SLOTS.length; i++) {
            int slot = OFF_ARMOR + i * 8;
            ItemStack st = player.getItemBySlot(ARMOR_SLOTS[i]);
            if (st.isEmpty()) {
                b.putInt(slot, 0);
                b.putFloat(slot + 4, 0f);
            } else {
                any = true;
                b.putInt(slot, 1);
                float durability = 1f;
                if (st.isDamageableItem() && st.getMaxDamage() > 0) {
                    durability = 1f - (float) st.getDamageValue() / (float) st.getMaxDamage();
                }
                b.putFloat(slot + 4, durability);
            }
        }
        return any;
    }

    /** Write the active potion effects (capped at {@link #MAX_POTIONS}). */
    private static void capturePotions(ByteBuffer b, LivingEntity player) {
        int count = 0;
        if (player != null) {
            Collection<MobEffectInstance> effects = player.getActiveEffects();
            for (MobEffectInstance e : effects) {
                if (count >= MAX_POTIONS) {
                    break;
                }
                int rec = OFF_POTIONS + count * POTION_REC;
                b.putInt(rec, e.getDuration());     // ticks; negative = infinite
                b.putInt(rec + 4, e.getAmplifier()); // 0-based
                MobEffect effect = e.getEffect().value();
                b.putInt(rec + 8, effect.getColor());
                putString(b, rec + 12, POTION_NAME_CAP, effect.getDisplayName().getString());
                count++;
            }
        }
        b.putInt(OFF_POTION_COUNT, count);
    }

    // Persistence window for the TargetHUD + Reach widgets — once a target
    // leaves the crosshair the last-seen values stay live for this long, so
    // the widgets don't flicker on briefly-lost targets in fights.
    private static final long TARGET_PERSIST_MS = 3500L;
    private static long lastTargetMs = 0L;
    private static float lastTargetDist = 0f;
    private static float lastTargetHp = 0f;
    private static float lastTargetMaxHp = 0f;
    private static String lastTargetName = "";

    /**
     * Write the entity under the crosshair. Returns true if one is present —
     * either live now, or last-seen within {@link #TARGET_PERSIST_MS}.
     */
    private static boolean captureTarget(ByteBuffer b, Minecraft mc, LocalPlayer player) {
        Entity target = mc.crosshairPickEntity;
        if (target != null) {
            float dist = player.distanceTo(target);
            float hp = 0f;
            float maxHp = 0f;
            if (target instanceof LivingEntity living) {
                hp = living.getHealth();
                maxHp = living.getMaxHealth();
            }
            String name = target.getName().getString();

            // Update the cache before writing — next no-target frame uses this.
            lastTargetMs = System.currentTimeMillis();
            lastTargetDist = dist;
            lastTargetHp = hp;
            lastTargetMaxHp = maxHp;
            lastTargetName = name;

            b.putInt(OFF_TARGET_PRESENT, 1);
            b.putFloat(OFF_TARGET_DIST, dist);
            b.putFloat(OFF_TARGET_HP, hp);
            b.putFloat(OFF_TARGET_MAXHP, maxHp);
            putString(b, OFF_TARGET_NAME, TARGET_NAME_CAP, name);
            return true;
        }

        // No live target — keep the last-seen values for the persistence
        // window so the TargetHUD doesn't flicker when a target briefly slips
        // off the crosshair (combat camera shake, line-of-sight blip, etc.).
        long since = System.currentTimeMillis() - lastTargetMs;
        if (lastTargetMs == 0L || since > TARGET_PERSIST_MS) {
            return false;
        }
        b.putInt(OFF_TARGET_PRESENT, 1);
        b.putFloat(OFF_TARGET_DIST, lastTargetDist);
        b.putFloat(OFF_TARGET_HP, lastTargetHp);
        b.putFloat(OFF_TARGET_MAXHP, lastTargetMaxHp);
        putString(b, OFF_TARGET_NAME, TARGET_NAME_CAP, lastTargetName);
        return true;
    }

    /** Tier → wire int (matches the constants at the top of this class). */
    private static int tierToInt(EwoJumpReset.Tier tier) {
        return switch (tier) {
            case PERFECT -> TIER_PERFECT;
            case SLIGHTLY_EARLY -> TIER_SLIGHTLY_EARLY;
            case EARLY -> TIER_EARLY;
            case SLIGHTLY_LATE -> TIER_SLIGHTLY_LATE;
            case LATE -> TIER_LATE;
            case NONE -> TIER_NONE;
        };
    }

    /**
     * Write a length-prefixed UTF-8 string: an i32 length then the bytes,
     * truncated to {@code cap} bytes without splitting a multi-byte character.
     */
    private static void putString(ByteBuffer b, int offset, int cap, String s) {
        byte[] bytes = s.getBytes(StandardCharsets.UTF_8);
        int n = Math.min(bytes.length, cap);
        // Back off if the cut would land inside a multi-byte UTF-8 sequence.
        while (n > 0 && n < bytes.length && (bytes[n] & 0xC0) == 0x80) {
            n--;
        }
        b.putInt(offset, n);
        for (int i = 0; i < n; i++) {
            b.put(offset + 4 + i, bytes[i]);
        }
    }
}
