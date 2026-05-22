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

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.mojang.authlib.GameProfile;
import com.mojang.authlib.properties.Property;

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
 * property — a base64 JSON blob, the same data Minecraft itself renders the
 * skin from. The download runs once on a background thread.
 */
public final class EwoSkinExport {
    private EwoSkinExport() {}

    private static volatile boolean started = false;

    /** Called each frame; fires the export once the player profile is ready. */
    public static void tick() {
        if (started) {
            return;
        }
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
        GameProfile profile = info.getProfile();
        if (profile == null) {
            return;
        }
        String texturesB64 = null;
        for (Property p : profile.getProperties().get("textures")) {
            texturesB64 = p.getValue();
            break;
        }
        if (texturesB64 == null) {
            return;
        }
        started = true;
        Path gameDir = mc.gameDirectory.toPath();
        String b64 = texturesB64;
        Thread t = new Thread(() -> exportSkin(b64, gameDir), "ewo-skin-export");
        t.setDaemon(true);
        t.start();
    }

    private static void exportSkin(String texturesB64, Path gameDir) {
        try {
            String json = new String(Base64.getDecoder().decode(texturesB64), StandardCharsets.UTF_8);
            JsonObject root = JsonParser.parseString(json).getAsJsonObject();
            JsonObject textures = root.getAsJsonObject("textures");
            if (textures == null) {
                return;
            }
            String skinUrl = urlOf(textures, "SKIN");
            String capeUrl = urlOf(textures, "CAPE");
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
