package dev.lewlone.ewohud.assist;

import dev.lewlone.ewohud.EwoCompat;

import dev.lewlone.ewohud.EwoModuleData;
import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.item.Items;

/**
 * Auto Mace Swap module (1.21+ mace tech).
 *
 * <p>Vanilla mace mechanic: hitting an entity after falling 1.5+ blocks
 * triggers a "smash attack" with damage scaling per block fallen. To land
 * smash attacks you must have the mace in your held slot the instant you
 * hit, which is awkward to swap mid-air without a macro.
 *
 * <p>This module: while in free-fall past the configured minimum and the
 * held slot isn't already a mace, swap to the first hotbar mace via a real
 * number-key-equivalent swap. Swaps once per fall — landing resets the
 * fall-distance and rearms.
 */
public final class EwoAutoMaceSwap {

    private EwoAutoMaceSwap() {}

    private static boolean swappedThisFall;

    public static void tick() {
        if (!EwoModuleData.enabled(AssistSlots.AUTO_MACE_SWAP)) {
            swappedThisFall = false;
            return;
        }
        if (EwoActionMotor.busy()) {
            return;
        }

        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.player.connection == null) {
            return;
        }
        if (EwoCompat.screen(mc) != null) {
            return;
        }

        LocalPlayer player = mc.player;
        if (player.onGround()) {
            swappedThisFall = false;
            return;
        }
        if (swappedThisFall) {
            return;
        }

        float minFall = EwoModuleData.setting(AssistSlots.AUTO_MACE_SWAP, 0);
        if (minFall < 1.5f) minFall = 1.5f;
        if (player.fallDistance < minFall) {
            return;
        }

        Inventory inv = player.getInventory();
        if (inv.getItem(inv.getSelectedSlot()).is(Items.MACE)) {
            swappedThisFall = true;
            return;
        }

        int maceSlot = -1;
        for (int i = 0; i < 9; i++) {
            if (inv.getItem(i).is(Items.MACE)) {
                maceSlot = i;
                break;
            }
        }
        if (maceSlot < 0) {
            swappedThisFall = true;
            return;
        }

        inv.setSelectedSlot(maceSlot);
        mc.player.connection.send(new ServerboundSetCarriedItemPacket(maceSlot));
        swappedThisFall = true;
    }
}
