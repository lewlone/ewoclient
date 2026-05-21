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
import net.minecraft.world.item.ItemStack;

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
    public static final int SCHEMA_VERSION = 3;
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

    // flags bits
    private static final int FLAG_WORLD = 1;
    private static final int FLAG_PING = 1 << 1;
    private static final int FLAG_ARMOR = 1 << 2;
    private static final int FLAG_TARGET = 1 << 3;
    private static final int FLAG_OVERLAY = 1 << 4; // the EwoClient overlay is open

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

    /** Write the entity under the crosshair. Returns true if one is present. */
    private static boolean captureTarget(ByteBuffer b, Minecraft mc, LocalPlayer player) {
        Entity target = mc.crosshairPickEntity;
        if (target == null) {
            return false;
        }
        b.putInt(OFF_TARGET_PRESENT, 1);
        b.putFloat(OFF_TARGET_DIST, player.distanceTo(target));
        if (target instanceof LivingEntity living) {
            b.putFloat(OFF_TARGET_HP, living.getHealth());
            b.putFloat(OFF_TARGET_MAXHP, living.getMaxHealth());
        } else {
            b.putFloat(OFF_TARGET_HP, 0f);
            b.putFloat(OFF_TARGET_MAXHP, 0f);
        }
        putString(b, OFF_TARGET_NAME, TARGET_NAME_CAP, target.getName().getString());
        return true;
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
