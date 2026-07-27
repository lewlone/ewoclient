package dev.lewlone.ewohud;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.Collections;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;

import net.minecraft.client.Minecraft;
import net.minecraft.client.multiplayer.ClientPacketListener;
import net.minecraft.client.multiplayer.PlayerInfo;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.Player;

/**
 * Exports the *targeted* player's skin so the TargetHUD widget can show their
 * face instead of the first letter of their name.
 *
 * <p>Sibling of {@link EwoSkinExport}, which does the same once for the local
 * player. The differences are what make this its own class rather than a
 * parameter:
 *
 * <ul>
 *   <li>The target changes constantly, so this is a cache keyed by name rather
 *       than a one-shot.</li>
 *   <li>The renderer must never draw one player's face labelled with another's
 *       name. The PNG is written first and the name marker second, and the
 *       renderer only trusts the PNG when the marker matches the name it is
 *       about to draw — so a half-finished swap shows the monogram, never the
 *       wrong face.</li>
 * </ul>
 *
 * <p>Mobs have no skin and are not handled here; the widget keeps its monogram
 * for them.
 */
public final class EwoTargetSkin {
    private EwoTargetSkin() {}

    /** File the renderer reads the head from. */
    private static final String PNG = "ewo-target.png";
    /** Marker naming whose face {@link #PNG} currently holds. */
    private static final String MARKER = "ewo-target.txt";

    /** Names whose export has been attempted — success or failure. */
    private static final Set<String> attempted =
            Collections.newSetFromMap(new ConcurrentHashMap<String, Boolean>());

    /** Name currently published in the marker file. */
    private static volatile String published = "";

    /** In-flight export, so a steady crosshair does not spawn a thread a frame. */
    private static volatile boolean busy = false;

    /** Called once per frame from {@link EwoFrameHook}. */
    public static void tick() {
        try {
            Minecraft mc = Minecraft.getInstance();
            if (mc == null || mc.player == null) {
                return;
            }
            Entity target = mc.crosshairPickEntity;
            if (!(target instanceof Player player)) {
                return;
            }
            String name = player.getName().getString();
            if (name.isEmpty() || name.equals(published) || busy) {
                return;
            }
            // One attempt per name per session. A player whose profile has no
            // textures (or whose download fails) must not be retried every
            // frame for as long as the crosshair rests on them.
            if (!attempted.add(name)) {
                return;
            }
            ClientPacketListener conn = mc.getConnection();
            if (conn == null) {
                return;
            }
            PlayerInfo info = conn.getPlayerInfo(player.getUUID());
            if (info == null) {
                return;
            }
            Object profile = info.getProfile();
            if (profile == null) {
                return;
            }
            String b64 = EwoSkinExport.texturesBlobOf(profile);
            if (b64 == null) {
                return;
            }
            busy = true;
            Path gameDir = mc.gameDirectory.toPath();
            Thread t = new Thread(() -> export(b64, name, gameDir), "ewo-target-skin");
            t.setDaemon(true);
            t.start();
        } catch (Throwable e) {
            // Cosmetic — never take down the render thread for a face.
            System.err.println("[ewo-hud] target skin skipped: " + e);
        }
    }

    private static void export(String texturesB64, String name, Path gameDir) {
        try {
            String url = EwoSkinExport.skinUrlOf(texturesB64);
            if (url == null) {
                return;
            }
            // Download to a temp file and move into place, so the renderer can
            // never observe a partially-written PNG.
            Path tmp = gameDir.resolve(PNG + ".part");
            EwoSkinExport.downloadTo(url, tmp);
            Files.move(tmp, gameDir.resolve(PNG),
                    StandardCopyOption.REPLACE_EXISTING);
            // Marker last: it is what makes the renderer trust the PNG.
            Files.write(gameDir.resolve(MARKER), name.getBytes(StandardCharsets.UTF_8));
            published = name;
        } catch (Exception e) {
            System.err.println("[ewo-hud] target skin failed for " + name + ": " + e);
        } finally {
            busy = false;
        }
    }

}
