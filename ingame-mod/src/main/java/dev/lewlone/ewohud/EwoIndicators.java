package dev.lewlone.ewohud;

import java.nio.ByteBuffer;
import java.util.HashMap;
import java.util.Iterator;
import java.util.Map;

import net.minecraft.client.Camera;
import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.phys.Vec3;

import org.joml.Quaternionf;
import org.joml.Vector3f;

/**
 * Per-entity world-anchored combat indicators (Commit 3) — totem-pop counter
 * and floating health/damage above visible LivingEntities.
 *
 * <p>Maintains a per-entity-id state map (running totem-use count, last-known
 * health for damage-delta detection, last-damage timer). Each frame, iterates
 * every {@link LivingEntity} within {@link #TRACK_RADIUS_BLOCKS} of the local
 * player, projects its head position to screen pixels via the live camera +
 * options.fov, and writes one record per entity into the shared block.
 *
 * <p>The Rust HUD reads the array and draws billboards at the projected
 * coords. Records are written even for off-screen entities (the {@code in_view}
 * flag is the cull signal) so the Rust side can decide whether to draw or skip.
 */
public final class EwoIndicators {

    private EwoIndicators() {}

    /** Per-entity state stored across frames. Survives until the entity is no
     *  longer tracked client-side. */
    private static final class State {
        int totemCount;        // running tally since first sighting
        float lastHealth = -1; // -1 = unknown; for damage-delta detection
        float lastDamage;      // delta from the most recent health drop
        long lastDamageMs;     // wall-clock millis of the most recent drop
    }

    /** Max entities written per frame. Capped so the buffer stays small +
     *  iteration is bounded on busy servers (lobbies, hub worlds). */
    public static final int MAX_TRACKED = 16;
    /** Bytes per indicator record — must match the Rust side mirror. */
    public static final int RECORD = 40;
    /** Header bytes (i32 count). */
    public static final int HEADER = 4;
    /** Track only entities within this many blocks of the player. */
    private static final float TRACK_RADIUS_BLOCKS = 32f;
    /** A damage delta is "live" for this long after the hit landed. */
    private static final long DAMAGE_FADE_MS = 1500L;

    private static final Map<Integer, State> STATES = new HashMap<>(64);

    /** Mixin entry: an entity received a status packet — status 35 is totem
     *  activation. Any other status is ignored. */
    public static void onEntityStatus(int entityId, byte status) {
        if (status == 35) {
            STATES.computeIfAbsent(entityId, k -> new State()).totemCount++;
        }
    }

    /**
     * Fill the indicator block at {@code baseOff}. Writes:
     *   <pre>i32 count
     *   count × { i32 entity_id, f32 screen_x, f32 screen_y, f32 distance,
     *             i32 in_view, i32 totem_count, f32 health, f32 max_health,
     *             f32 last_damage, f32 last_damage_age_sec }</pre>
     */
    public static void fill(ByteBuffer buf, int baseOff) {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.level == null || mc.player == null
                || mc.gameRenderer == null || mc.getWindow() == null) {
            buf.putInt(baseOff, 0);
            return;
        }
        LocalPlayer player = mc.player;
        Camera cam = EwoCompat.mainCamera(mc.gameRenderer);
        Vec3 camPos = cam.position();
        Quaternionf invRot = cam.rotation().conjugate(new Quaternionf());
        float fovDeg = cam.getFov();
        if (fovDeg <= 1f) fovDeg = 70f; // safety; vanilla default
        float halfFovTan = (float) Math.tan(Math.toRadians(fovDeg) * 0.5);
        float w = mc.getWindow().getWidth();
        float h = mc.getWindow().getHeight();
        if (w <= 0 || h <= 0) {
            buf.putInt(baseOff, 0);
            return;
        }
        float aspect = w / h;
        long now = System.currentTimeMillis();

        int count = 0;
        Vector3f rel = new Vector3f();
        int recordsOff = baseOff + HEADER;

        for (Entity entity : mc.level.entitiesForRendering()) {
            if (count >= MAX_TRACKED) break;
            if (!(entity instanceof LivingEntity le) || entity == player) continue;
            float dist = player.distanceTo(le);
            if (dist > TRACK_RADIUS_BLOCKS) continue;

            // Head position — entity Y + bounding-box height + small offset so
            // indicators sit comfortably above the head + any vanilla nametag.
            double wx = le.getX();
            double wy = le.getY() + le.getBbHeight() + 0.55;
            double wz = le.getZ();

            // Project: translate world→camera-local, rotate by inverse camera,
            // perspective-divide. After the inverse rotation MC's camera looks
            // down -Z, so rel.z < 0 means the point is in front of the camera.
            rel.set((float) (wx - camPos.x), (float) (wy - camPos.y), (float) (wz - camPos.z));
            invRot.transform(rel);
            int inView = 0;
            float sx = 0f, sy = 0f;
            if (rel.z < -0.05f) {
                float ndcX = -rel.x / rel.z / (halfFovTan * aspect);
                float ndcY = -rel.y / rel.z / halfFovTan;
                sx = (ndcX * 0.5f + 0.5f) * w;
                sy = (1f - (ndcY * 0.5f + 0.5f)) * h;
                // Generous off-screen margin — billboards can extend past the
                // anchor point so we allow slight overshoot before culling.
                if (sx > -80f && sx < w + 80f && sy > -80f && sy < h + 80f) {
                    inView = 1;
                }
            }

            // Per-entity persistent state — damage delta + age.
            State st = STATES.computeIfAbsent(le.getId(), k -> new State());
            float health = le.getHealth();
            if (st.lastHealth >= 0f && health + 0.05f < st.lastHealth) {
                st.lastDamage = st.lastHealth - health;
                st.lastDamageMs = now;
            }
            st.lastHealth = health;
            float damageAgeSec;
            if (st.lastDamageMs == 0L) {
                damageAgeSec = -1f;
            } else {
                long ageMs = now - st.lastDamageMs;
                damageAgeSec = ageMs <= DAMAGE_FADE_MS ? ageMs / 1000f : -1f;
            }

            int off = recordsOff + count * RECORD;
            buf.putInt(off, le.getId());
            buf.putFloat(off + 4, sx);
            buf.putFloat(off + 8, sy);
            buf.putFloat(off + 12, dist);
            buf.putInt(off + 16, inView);
            buf.putInt(off + 20, st.totemCount);
            buf.putFloat(off + 24, health);
            buf.putFloat(off + 28, le.getMaxHealth());
            buf.putFloat(off + 32, st.lastDamage);
            buf.putFloat(off + 36, damageAgeSec);
            count++;
        }
        buf.putInt(baseOff, count);

        // Garbage-collect state for entities the client no longer tracks. Run
        // only when the map has grown well past MAX_TRACKED — bounds the cost.
        if (STATES.size() > MAX_TRACKED * 4) {
            Iterator<Map.Entry<Integer, State>> it = STATES.entrySet().iterator();
            while (it.hasNext()) {
                if (mc.level.getEntity(it.next().getKey()) == null) {
                    it.remove();
                }
            }
        }
    }
}
