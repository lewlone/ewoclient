package dev.lewlone.ewohud.assist;

import dev.lewlone.ewohud.EwoCompat;

import dev.lewlone.ewohud.EwoModuleData;
import net.minecraft.client.Minecraft;
import net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.item.Items;

/**
 * Auto Pearl module (clutch escape).
 *
 * <p>Bound key fires a real swap + use + swap-back: same {@code Inventory}
 * mutate + {@code ServerboundSetCarriedItemPacket} pressing a number key
 * does, then {@code MultiPlayerGameMode.useItem(player, MAIN_HAND)} which is
 * exactly what a right-click on the pearl does. The pearl entity is spawned
 * by vanilla's {@code EnderPearlItem.use}; the 1 s cooldown vanilla applies
 * means a single click per press — the macro doesn't and can't pearl-spam.
 *
 * <p>No-ops if no pearl is in the hotbar, the inventory screen is open, or
 * another motor sequence is in flight.
 */
public final class EwoAutoPearl {

    private EwoAutoPearl() {}

    public static void trigger() {
        if (!EwoModuleData.enabled(AssistSlots.AUTO_PEARL)) {
            return;
        }
        if (EwoActionMotor.busy()) {
            return;
        }
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.player.connection == null
                || mc.gameMode == null || EwoCompat.screen(mc) != null) {
            return;
        }

        Inventory inv = mc.player.getInventory();
        int pearlSlot = -1;
        for (int i = 0; i < 9; i++) {
            if (inv.getItem(i).is(Items.ENDER_PEARL)) {
                pearlSlot = i;
                break;
            }
        }
        if (pearlSlot < 0) {
            return;
        }

        final int slot = pearlSlot;
        final int origSlot = inv.getSelectedSlot();
        EwoActionMotor.enqueue(() -> swapHotbar(slot), 50);
        EwoActionMotor.enqueue(EwoAutoPearl::useMainhand, 50);
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

    private static void useMainhand() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.gameMode == null) return;
        mc.gameMode.useItem(mc.player, InteractionHand.MAIN_HAND);
    }
}
