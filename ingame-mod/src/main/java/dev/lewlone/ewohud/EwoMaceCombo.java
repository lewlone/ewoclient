package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;
import net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.phys.EntityHitResult;
import net.minecraft.world.phys.HitResult;

/**
 * Phase H — Mace Combo (axe → spear → mace stun-slam).
 *
 * <p>Modern post-1.21 PvP combo: an axe hit disables the opponent's shield
 * (5 s cooldown), a spear hit lands during the shield-down window, and a
 * mace hit applies its smash damage. Each weapon has its own attack-strength
 * cooldown, so a fast swap chain lets all three connect inside the server-
 * side invulnerability window before the next i-frame applies.
 *
 * <p>Each motor step does a hotbar swap then a real attack: same
 * {@code ServerboundSetCarriedItemPacket} pressing a number key would send,
 * and {@code mc.gameMode.attack(player, entity) + player.swing(MAIN_HAND)}
 * — the same two calls vanilla makes when you left-click an entity in
 * {@code Minecraft.startAttack}. No fake or impossible packets.
 *
 * <p>Per-hit delay is configurable: at 50 ms it fires three swap+attacks in
 * a single 20 Hz game tick (the MustyKrab "stun slam" pattern); at 500 ms
 * it paces like a hurried-but-deliberate human combo. The motor's per-step
 * jitter applies on top, so even at 50 ms the rhythm isn't bit-exact.
 *
 * <p>Hotbar slots are user-configured per-module — the user arranges their
 * axe/spear/mace in the slots they like. Default 0/1/2.
 */
public final class EwoMaceCombo {

    private EwoMaceCombo() {}

    /** Bound key fired — queue the 3-step combo if the module is enabled. */
    public static void trigger() {
        if (!EwoModuleData.enabled(EwoModuleData.MACE_COMBO)) {
            return;
        }
        if (EwoActionMotor.busy()) {
            return;
        }

        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.screen != null) {
            return;
        }

        int axeSlot = clampSlot((int) EwoModuleData.setting(EwoModuleData.MACE_COMBO, 0));
        int spearSlot = clampSlot((int) EwoModuleData.setting(EwoModuleData.MACE_COMBO, 1));
        int maceSlot = clampSlot((int) EwoModuleData.setting(EwoModuleData.MACE_COMBO, 2));
        int perHitDelay = (int) EwoModuleData.setting(EwoModuleData.MACE_COMBO, 3);
        if (perHitDelay < 50) perHitDelay = 50;
        if (perHitDelay > 1000) perHitDelay = 1000;

        final int aSlot = axeSlot;
        final int sSlot = spearSlot;
        final int mSlot = maceSlot;
        // axe → wait → spear → wait → mace.  Final step's delay is 0 (motor
        // drains, busy() flips back).
        EwoActionMotor.enqueue(() -> swapAndAttack(aSlot), perHitDelay);
        EwoActionMotor.enqueue(() -> swapAndAttack(sSlot), perHitDelay);
        EwoActionMotor.enqueue(() -> swapAndAttack(mSlot), 0);
    }

    /** One combo step: swap (if not already on the slot) then attack the
     *  current crosshair target. Both go through the exact code paths
     *  vanilla uses for a number-key press + a left-click on an entity. */
    private static void swapAndAttack(int slot) {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.player.connection == null
                || mc.gameMode == null) {
            EwoActionMotor.abort();
            return;
        }
        Inventory inv = mc.player.getInventory();
        if (inv.getSelectedSlot() != slot) {
            inv.setSelectedSlot(slot);
            mc.player.connection.send(new ServerboundSetCarriedItemPacket(slot));
        }
        // Attack the crosshair target if it's a living entity; otherwise just
        // swing for visual feedback (same as vanilla left-click on air/block).
        HitResult hr = mc.hitResult;
        if (hr instanceof EntityHitResult ehr) {
            mc.gameMode.attack(mc.player, ehr.getEntity());
        }
        mc.player.swing(InteractionHand.MAIN_HAND);
    }

    private static int clampSlot(int s) {
        if (s < 0) return 0;
        if (s > 8) return 8;
        return s;
    }
}
