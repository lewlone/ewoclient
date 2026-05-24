package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.item.Items;

/**
 * Phase H — Wind Charge MLG.
 *
 * <p>Vanilla 1.21+ trick: while falling, throw a wind charge at the ground
 * below you — the charge's wind explosion pushes you upward, cancelling the
 * landing impact. To work, the charge must hit the ground close enough that
 * its AoE reaches you, which means it has to be thrown roughly straight
 * down.
 *
 * <p>The module auto-fires when:
 * <ul>
 *   <li>You're in air and fall-distance has crossed the configured threshold
 *       (default 15 blocks ≈ 12 HP fall damage = often fatal)</li>
 *   <li>Off-mode (default): your pitch is already at least {@code min_pitch}
 *       degrees downward — you'd manually MLG by looking down anyway</li>
 *   <li>Snap-mode: any pitch; the module briefly snaps view straight down,
 *       throws, restores. Snap is visible to other players as a one-tick
 *       head-flick — legal vanilla action (a 90° rotation is humanly
 *       possible with high sensitivity), but bot-looking</li>
 * </ul>
 *
 * <p>One save attempt per fall. Latches off on landing, rearms on next
 * lift-off.
 */
public final class EwoWindChargeMLG {

    private EwoWindChargeMLG() {}

    /** True after we've queued a save for the current fall; cleared on landing. */
    private static boolean savedThisFall;
    /** Pitch + yaw saved when we snap; restored after the use. */
    private static float origPitch;
    private static float origYaw;

    public static void tick() {
        if (!EwoModuleData.enabled(EwoModuleData.WIND_CHARGE_MLG)) {
            savedThisFall = false;
            return;
        }
        if (EwoActionMotor.busy()) {
            return;
        }

        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.player.connection == null
                || mc.gameMode == null || mc.screen != null) {
            return;
        }

        LocalPlayer player = mc.player;

        if (player.onGround()) {
            savedThisFall = false;
            return;
        }
        if (savedThisFall) {
            return;
        }

        float minFall = EwoModuleData.setting(EwoModuleData.WIND_CHARGE_MLG, 0);
        if (player.fallDistance < minFall) {
            return;
        }

        boolean snap = EwoModuleData.setting(EwoModuleData.WIND_CHARGE_MLG, 2) >= 0.5f;
        float minPitch = EwoModuleData.setting(EwoModuleData.WIND_CHARGE_MLG, 1);

        // Snap-off path: only fire if the user is already looking sufficiently
        // downward — otherwise the throw flies sideways and doesn't save us.
        if (!snap && player.getXRot() < minPitch) {
            return;
        }

        Inventory inv = player.getInventory();
        int wcSlot = -1;
        for (int i = 0; i < 9; i++) {
            if (inv.getItem(i).is(Items.WIND_CHARGE)) {
                wcSlot = i;
                break;
            }
        }
        if (wcSlot < 0) {
            // No wind charge — latch so we don't keep scanning every frame.
            savedThisFall = true;
            return;
        }

        savedThisFall = true;
        final int slot = wcSlot;
        final int origSlot = inv.getSelectedSlot();
        origPitch = player.getXRot();
        origYaw = player.getYRot();

        // Sequence: swap → [snap pitch + wait one tick so vanilla's move
        // packet carries the new rotation to server] → use → [restore pitch]
        // → swap back. Snap-off skips the rotation steps.
        EwoActionMotor.enqueue(() -> swapHotbar(slot), 0);
        if (snap) {
            EwoActionMotor.enqueue(EwoWindChargeMLG::snapDown, 50);
        }
        EwoActionMotor.enqueue(EwoWindChargeMLG::useMainhand, 50);
        if (snap) {
            EwoActionMotor.enqueue(EwoWindChargeMLG::restorePitch, 0);
        }
        EwoActionMotor.enqueue(() -> swapHotbar(origSlot), 0);
    }

    private static void swapHotbar(int slot) {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.player.connection == null) return;
        if (slot < 0 || slot >= 9) return;
        Inventory inv = mc.player.getInventory();
        if (inv.getSelectedSlot() == slot) return;
        inv.setSelectedSlot(slot);
        mc.player.connection.send(new ServerboundSetCarriedItemPacket(slot));
    }

    /** Snap pitch to straight down (+90°). The next vanilla move-packet
     *  tick carries the rotation to the server before the use packet. */
    private static void snapDown() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null) return;
        mc.player.setXRot(90f);
        mc.player.setYRot(origYaw);
    }

    private static void useMainhand() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.gameMode == null) return;
        mc.gameMode.useItem(mc.player, InteractionHand.MAIN_HAND);
    }

    private static void restorePitch() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null) return;
        mc.player.setXRot(origPitch);
        mc.player.setYRot(origYaw);
    }
}
