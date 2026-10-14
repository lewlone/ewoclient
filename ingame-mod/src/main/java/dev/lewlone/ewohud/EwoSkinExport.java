package dev.lewlone.ewohud;

import java.io.IOException;
import java.io.InputStream;
import java.net.IDN;
import java.net.URI;
import java.net.URISyntaxException;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.Base64;
import java.util.Collection;
import java.util.Locale;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

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
                try {
                    download(skinUrl, gameDir.resolve("ewo-skin.png"));
                } catch (IOException e) {
                    System.err.println("[ewo-hud] skin download failed: " + e.getMessage());
                }
            }
            if (capeUrl != null) {
                try {
                    download(capeUrl, gameDir.resolve("ewo-cape.png"));
                } catch (IOException e) {
                    System.err.println("[ewo-hud] cape download failed: " + e.getMessage());
                }
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

    /**
     * The one host a texture URL may point at: vanilla's rule, tightened.
     *
     * <p>Vanilla is authlib's {@code com.mojang.authlib.yggdrasil
     * .TextureUrlChecker.isAllowedTextureDomain}: a URI whose scheme is
     * exactly {@code http} or {@code https}, whose host is present and already
     * lowercase after {@code IDN.toUnicode}, and which is on the allow-list.
     * The allow-list differs by authlib version (read from the jars with
     * javap): 7.0.61 (1.21.x) takes any {@code *.minecraft.net} /
     * {@code *.mojang.com} host except bugs.mojang.com, education.minecraft.net
     * and feedback.minecraft.net; 7.0.63 (the 26.1 line) and 9.0.75 (26.2, as
     * pinned by 26.2.json) accept exactly {@code textures.minecraft.net}. We
     * use the 26.x rule, a subset of the 1.21.x one, so a URL passing here
     * passes in every vanilla.
     *
     * <p>Also refused, matching the Rust side: userinfo, an explicit port,
     * backslashes, percent escapes, non-ASCII or whitespace, and URLs over 512
     * bytes. {@code http} is accepted but fetched over https (Mojang's profile
     * property still carries {@code http://textures.minecraft.net/...}).
     *
     * <p>The URL comes from the {@code textures} property a <em>server</em>
     * sends in its GameProfile, so without this check a hostile server could
     * make the client GET a LAN address, learn its IP, or feed it a huge body.
     */
    static final String ALLOWED_HOST = "textures.minecraft.net";

    private static final int MAX_BODY_BYTES = 1 << 20; // 1 MiB
    private static final Duration CONNECT_TIMEOUT = Duration.ofSeconds(5);
    private static final Duration REQUEST_TIMEOUT = Duration.ofSeconds(10);
    private static final byte[] PNG_SIGNATURE =
            {(byte) 0x89, 'P', 'N', 'G', '\r', '\n', 0x1A, '\n'};

    /** Closes a stalled body stream: the request timeout only covers headers. */
    private static final ScheduledExecutorService WATCHDOG =
            Executors.newSingleThreadScheduledExecutor(r -> {
                Thread t = new Thread(r, "ewo-skin-watchdog");
                t.setDaemon(true);
                return t;
            });

    private static final int MAX_URL_LENGTH = 512;

    /** Whether {@code url} may be fetched as a texture (see {@link #ALLOWED_HOST}). */
    static boolean isAllowedTextureUrl(String url) {
        return secureUri(url) != null;
    }

    /** The https URI to fetch for {@code url}, or {@code null} if it is refused. */
    private static URI secureUri(String url) {
        if (url == null || url.length() > MAX_URL_LENGTH) {
            return null;
        }
        for (int i = 0; i < url.length(); i++) {
            char c = url.charAt(i);
            if (c <= ' ' || c >= 0x7f || c == '\\' || c == '%') {
                return null; // control/space/non-ASCII, backslash, percent escape
            }
        }
        URI uri;
        try {
            uri = new URI(url).normalize();
        } catch (URISyntaxException e) {
            return null;
        }
        String scheme = uri.getScheme();
        if (!"http".equals(scheme) && !"https".equals(scheme)) {
            return null; // case-sensitive, as in vanilla
        }
        // userinfo ("https://textures.minecraft.net@evil/") parses with the
        // right-hand host, but refuse the whole shape; no explicit port either.
        if (uri.getRawUserInfo() != null || uri.getPort() != -1) {
            return null;
        }
        String host = uri.getHost();
        if (host == null) {
            return null;
        }
        String unicode = IDN.toUnicode(host);
        if (!unicode.equals(unicode.toLowerCase(Locale.ROOT)) || !ALLOWED_HOST.equals(unicode)) {
            return null;
        }
        try {
            return new URI("https", null, host, -1, uri.getRawPath(), uri.getRawQuery(), null);
        } catch (URISyntaxException e) {
            return null;
        }
    }

    /**
     * Fetch a texture PNG into {@code dest}: allow-listed host (http upgraded
     * to https), 5 s
     * connect / 10 s request timeouts, redirects never followed, body capped at
     * 1 MiB, and the bytes must start with the PNG signature before anything
     * touches the disk. Throws {@link IOException} on any rejection.
     */
    private static void download(String url, Path dest) throws IOException, InterruptedException {
        URI target = secureUri(url);
        if (target == null) {
            throw new IOException("texture URL rejected (not on " + ALLOWED_HOST + ")");
        }
        HttpClient client = HttpClient.newBuilder()
                .connectTimeout(CONNECT_TIMEOUT)
                .followRedirects(HttpClient.Redirect.NEVER)
                .build();
        HttpRequest req = HttpRequest.newBuilder(target)
                .timeout(REQUEST_TIMEOUT)
                .GET()
                .build();
        HttpResponse<InputStream> resp = client.send(req, HttpResponse.BodyHandlers.ofInputStream());
        byte[] body;
        // The request timeout only covers the headers; the watchdog closes a
        // body stream that stalls, so this thread can never hang on a read.
        try (InputStream in = resp.body()) {
            ScheduledFuture<?> guard = WATCHDOG.schedule(() -> {
                try {
                    in.close();
                } catch (IOException ignored) {
                    // closing is the point
                }
            }, REQUEST_TIMEOUT.toMillis(), TimeUnit.MILLISECONDS);
            try {
                if (resp.statusCode() != 200) {
                    throw new IOException("texture HTTP " + resp.statusCode());
                }
                long declared = resp.headers().firstValueAsLong("content-length").orElse(-1L);
                if (declared > MAX_BODY_BYTES) {
                    throw new IOException("texture too large (" + declared + " bytes declared)");
                }
                body = in.readNBytes(MAX_BODY_BYTES + 1);
            } finally {
                guard.cancel(false);
            }
        }
        if (body.length > MAX_BODY_BYTES) {
            throw new IOException("texture exceeds " + MAX_BODY_BYTES + " bytes");
        }
        if (body.length < PNG_SIGNATURE.length) {
            throw new IOException("texture is not a PNG (too short)");
        }
        for (int i = 0; i < PNG_SIGNATURE.length; i++) {
            if (body[i] != PNG_SIGNATURE[i]) {
                throw new IOException("texture is not a PNG (bad signature)");
            }
        }
        Files.write(dest, body);
    }
}
