package dev.lewlone.ewohud;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Base64;
import java.util.Collection;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;

import net.minecraft.client.Minecraft;
import net.minecraft.client.multiplayer.ClientPacketListener;
import net.minecraft.client.multiplayer.PlayerInfo;
import net.minecraft.client.player.LocalPlayer;

/**
 * One-shot export of the signed-in player's skin + cape PNGs to the
 * instance directory, where {@code ewo-jni}'s HOME-tab 3D viewer reads them
 * ({@code ewo-skin.png} / {@code ewo-cape.png}).
 *
 * <p>The skin / cape URLs come from the player's GameProfile {@code textures}
 * property — a base64 JSON blob, the same data Minecraft renders the skin
 * from. authlib's {@code GameProfile} / {@code Property} API differs across
 * versions (record {@code properties()}/{@code value()} vs. class
 * {@code getProperties()}/{@code getValue()}) and the build-classpath copy
 * can skew from the runtime one — so the property is read **reflectively**,
 * and the whole tick is guarded so skin export can never crash the game.
 */
public final class EwoSkinExport {
    private EwoSkinExport() {}

    private static volatile boolean started = false;

    /** Called each frame; fires the export once the player profile is ready. */
    public static void tick() {
        if (started) {
            return;
        }
        try {
            Minecraft mc = Minecraft.getInstance();
            if (mc == null) {
                return;
            }
            LocalPlayer player = mc.player;
            ClientPacketListener conn = mc.getConnection();
            if (player == null || conn == null) {
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
            String b64 = texturesBlob(profile);
            if (b64 == null) {
                return;
            }
            started = true;
            Path gameDir = mc.gameDirectory.toPath();
            Thread t = new Thread(() -> exportSkin(b64, gameDir), "ewo-skin-export");
            t.setDaemon(true);
            t.start();
        } catch (Throwable e) {
            // Skin export is cosmetic — never let it take down the render thread.
            started = true;
            System.err.println("[ewo-hud] skin export skipped: " + e);
        }
    }

    /**
     * The base64 {@code textures} property of a GameProfile — shared with
     * {@link EwoTargetSkin}, which needs the same reflective read for the
     * player under the crosshair.
     */
    static String texturesBlobOf(Object profile) throws Exception {
        return texturesBlob(profile);
    }

    /**
     * The SKIN url inside a base64 textures blob, or {@code null}. Shared with
     * {@link EwoTargetSkin} — the target head only needs the skin, never the
     * cape or the slim flag.
     */
    static String skinUrlOf(String texturesB64) {
        try {
            String json = new String(Base64.getDecoder().decode(texturesB64), StandardCharsets.UTF_8);
            JsonObject root = JsonParser.parseString(json).getAsJsonObject();
            JsonObject textures = root.getAsJsonObject("textures");
            return textures == null ? null : urlOf(textures, "SKIN");
        } catch (Exception e) {
            return null;
        }
    }

    /** Download {@code url} to {@code dest}. Shared with {@link EwoTargetSkin}. */
    static void downloadTo(String url, Path dest) throws IOException, InterruptedException {
        download(url, dest);
    }

    /** The base64 `textures` property of a GameProfile, read reflectively so
     *  it works whether authlib exposes the record or the class API. */
    private static String texturesBlob(Object profile) throws Exception {
        Object propMap = invokeAny(profile, "properties", "getProperties");
        if (propMap == null) {
            return null;
        }
        Object got = propMap.getClass().getMethod("get", Object.class).invoke(propMap, "textures");
        if (!(got instanceof Collection<?> props)) {
            return null;
        }
        for (Object p : props) {
            Object value = invokeAny(p, "value", "getValue");
            if (value instanceof String s) {
                return s;
            }
        }
        return null;
    }

    /** Invoke the first no-arg method named in `names` that `target` has. */
    private static Object invokeAny(Object target, String... names) throws Exception {
        for (String name : names) {
            try {
                return target.getClass().getMethod(name).invoke(target);
            } catch (NoSuchMethodException ignored) {
                // try the next name
            }
        }
        return null;
    }

    private static void exportSkin(String texturesB64, Path gameDir) {
        try {
            String json = new String(Base64.getDecoder().decode(texturesB64), StandardCharsets.UTF_8);
            JsonObject root = JsonParser.parseString(json).getAsJsonObject();
            JsonObject textures = root.getAsJsonObject("textures");
            if (textures == null) {
                return;
            }
            JsonObject skin = textures.has("SKIN") ? textures.getAsJsonObject("SKIN") : null;
            String skinUrl = (skin != null && skin.has("url")) ? skin.get("url").getAsString() : null;
            String capeUrl = urlOf(textures, "CAPE");

            // Slim ("Alex") vs. wide model — from SKIN.metadata.model. The
            // marker file's presence tells the renderer which arms to use.
            // Write it *before* the skin PNG so it's settled by the time the
            // viewer notices the new png and reloads both together.
            boolean slim = false;
            if (skin != null && skin.has("metadata")) {
                JsonObject meta = skin.getAsJsonObject("metadata");
                slim = meta != null && meta.has("model")
                        && "slim".equals(meta.get("model").getAsString());
            }
            Path slimMarker = gameDir.resolve("ewo-skin-slim");
            if (slim) {
                Files.writeString(slimMarker, "");
            } else {
                Files.deleteIfExists(slimMarker);
            }

            if (skinUrl != null) {
                download(skinUrl, gameDir.resolve("ewo-skin.png"));
            }
            if (capeUrl != null) {
                download(capeUrl, gameDir.resolve("ewo-cape.png"));
            } else {
                // No cape — clear any stale file so the viewer shows none.
                Files.deleteIfExists(gameDir.resolve("ewo-cape.png"));
            }
        } catch (Exception e) {
            System.err.println("[ewo-hud] skin export failed: " + e);
        }
    }

    private static String urlOf(JsonObject textures, String key) {
        if (!textures.has(key)) {
            return null;
        }
        JsonObject o = textures.getAsJsonObject(key);
        return (o != null && o.has("url")) ? o.get("url").getAsString() : null;
    }

    private static void download(String url, Path dest) throws IOException, InterruptedException {
        HttpClient client = HttpClient.newHttpClient();
        HttpRequest req = HttpRequest.newBuilder(URI.create(url)).GET().build();
        HttpResponse<InputStream> resp = client.send(req, HttpResponse.BodyHandlers.ofInputStream());
        if (resp.statusCode() != 200) {
            return;
        }
        try (InputStream in = resp.body(); OutputStream out = Files.newOutputStream(dest)) {
            in.transferTo(out);
        }
    }
}
