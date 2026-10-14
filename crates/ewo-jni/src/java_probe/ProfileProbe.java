// Test driver for ewo-jni's `java_profile_rule_matches_rust` (profile_pin_tests.rs).
// Not part of the mod: it is compiled by that test next to EwoProfileName.java
// and fed one request per stdin line:
//   S <hex utf-8 name>   -> "S 1" / "S 0"   EwoProfileName.isSafe(name)
//   A <hex utf-8 toml>   -> "A <hex>"       EwoProfileName.activeFromToml(lines)

import dev.lewlone.ewohud.EwoProfileName;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.PrintStream;
import java.io.StringReader;
import java.nio.charset.StandardCharsets;
import java.util.List;

public final class ProfileProbe {
    public static void main(String[] args) throws IOException {
        BufferedReader in = new BufferedReader(
                new InputStreamReader(System.in, StandardCharsets.UTF_8));
        PrintStream out = new PrintStream(System.out, false, StandardCharsets.UTF_8);
        for (String line; (line = in.readLine()) != null; ) {
            String text = unhex(line.substring(2));
            if (line.charAt(0) == 'S') {
                out.println("S " + (EwoProfileName.isSafe(text) ? 1 : 0));
            } else {
                // The same line splitting as Files.readAllLines in the mod.
                List<String> lines = new BufferedReader(new StringReader(text)).lines().toList();
                out.println("A " + hex(EwoProfileName.activeFromToml(lines)));
            }
        }
        out.flush();
    }

    private static String hex(String s) {
        StringBuilder sb = new StringBuilder();
        for (byte b : s.getBytes(StandardCharsets.UTF_8)) {
            sb.append(String.format("%02x", b));
        }
        return sb.toString();
    }

    private static String unhex(String h) {
        byte[] b = new byte[h.length() / 2];
        for (int i = 0; i < b.length; i++) {
            b[i] = (byte) Integer.parseInt(h.substring(2 * i, 2 * i + 2), 16);
        }
        return new String(b, StandardCharsets.UTF_8);
    }
}
