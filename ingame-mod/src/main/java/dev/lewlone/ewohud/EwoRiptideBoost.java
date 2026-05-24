package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;
import net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.item.Items;

/**
 * Phase H — Riptide Boost.
 *
 * <p>Bound key fires a real swap + hold-use + release + swap-back. The hold-
 * use phase pretends the right-click is held for the configured number of
 * milliseconds (default 700, slider 400..1500) — vanilla's
 * {@code TridentItem.use} reads {@code keyUse.isDown()} each tick during
 * the use action, and after 10 ticks of charge fires the Riptide launch on
 * key release if conditions are met (player in water, or it's raining, or
 * the trident has Riptide enchantment).
 *
 * <p>Vanilla self-gates the launch — if you're not in rain/water and the
 * trident lacks Riptide, releasing just ends the use harmlessly. The macro
 * doesn't try to detect conditions, just fires the cycle and lets vanilla
 * decide whether to launch.
 *
 * <p>No-ops if no trident is in the hotbar, the inventory screen is open,
 * or another motor sequence is in flight.
 */
public final class EwoRiptideBoost {

    private EwoRiptideBoost() {}

    public static void trigger() {
        if (!EwoModuleData.enabled(EwoModuleData.RIPTIDE_BOOST)) {
            return;
        }
        if (EwoActionMotor.busy()) {
            return;
        }
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.player.connection == null
                || mc.options == null || mc.screen != null) {
            return;
        }

        Inventory inv = mc.player.getInventory();
        int tridentSlot = -1;
        for (int i = 0; i < 9; i++) {
            if (inv.getItem(i).is(Items.TRIDENT)) {
                tridentSlot = i;
                break;
            }
        }
        if (tridentSlot < 0) {
            return; // no trident
        }

        int chargeMs = (int) EwoModuleData.setting(EwoModuleData.RIPTIDE_BOOST, 0);
        if (chargeMs < 400) chargeMs = 400;
        if (chargeMs > 1500) chargeMs = 1500;

        final int slot = tridentSlot;
        final int origSlot = inv.getSelectedSlot();
        final int hold = chargeMs;

        // Swap to trident (~one tick), hold use for the charge duration,
        // release (vanilla fires riptide on release if conditions met),
        // swap back.
        EwoActionMotor.enqueue(() -> swapHotbar(slot), 50);
        EwoActionMotor.enqueue(EwoRiptideBoost::pressUse, hold);
        EwoActionMotor.enqueue(EwoRiptideBoost::releaseUse, 100);
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

    private static void pressUse() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.options == null) return;
        mc.options.keyUse.setDown(true);
    }

    private static void releaseUse() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.options == null) return;
        mc.options.keyUse.setDown(false);
    }
}
