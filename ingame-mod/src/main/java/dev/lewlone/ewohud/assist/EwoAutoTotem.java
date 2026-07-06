package dev.lewlone.ewohud.assist;

import dev.lewlone.ewohud.EwoCompat;

import dev.lewlone.ewohud.EwoModuleData;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.screens.inventory.InventoryScreen;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.inventory.AbstractContainerMenu;
import net.minecraft.world.inventory.ContainerInput;
import net.minecraft.world.item.Items;

/**
 * Auto Totem module.
 *
 * <p>When the offhand goes empty and the inventory has a totem of undying,
 * queue a real client-side sequence — open inventory, F-swap (offhand-swap
 * click) the totem to the offhand, close — through the action motor. Every
 * step goes through the same code path a real player's keys + clicks would
 * trigger; no synthetic packets, no movement-while-inventory-open combos
 * vanilla can't produce.
 *
 * <p>Throttled to one attempt per 2 s so the inventory doesn't spam-open
 * when the player has run out of totems entirely. The throttle resets the
 * instant the offhand becomes non-empty again, so a real pop-and-re-totem
 * cycle fires instantly.
 */
public final class EwoAutoTotem {

    private EwoAutoTotem() {}

    private static long lastAttemptMs = 0L;
    private static final long ATTEMPT_THROTTLE_MS = 2000L;

    public static void tick() {
        if (!EwoModuleData.enabled(AssistSlots.AUTO_TOTEM)) {
            lastAttemptMs = 0L;
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
        Inventory inv = player.getInventory();

        if (!inv.getItem(40).isEmpty()) {
            lastAttemptMs = 0L;
            return;
        }

        long now = System.currentTimeMillis();
        if (now - lastAttemptMs < ATTEMPT_THROTTLE_MS) {
            return;
        }

        int sourceContainerSlot = findTotemContainerSlot(inv);
        if (sourceContainerSlot < 0) {
            lastAttemptMs = now;
            return;
        }

        lastAttemptMs = now;
        final int srcSlot = sourceContainerSlot;
        EwoActionMotor.enqueue(EwoAutoTotem::openInventoryStep, 80);
        EwoActionMotor.enqueue(() -> swapTotemStep(srcSlot), 80);
        EwoActionMotor.enqueue(EwoAutoTotem::closeInventoryStep, 0);
    }

    private static int findTotemContainerSlot(Inventory inv) {
        for (int i = 0; i < 9; i++) {
            if (inv.getItem(i).is(Items.TOTEM_OF_UNDYING)) {
                return 36 + i;
            }
        }
        for (int i = 9; i <= 35; i++) {
            if (inv.getItem(i).is(Items.TOTEM_OF_UNDYING)) {
                return i;
            }
        }
        return -1;
    }

    private static void openInventoryStep() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null) {
            EwoActionMotor.abort();
            return;
        }
        if (EwoCompat.screen(mc) != null) {
            EwoActionMotor.abort();
            return;
        }
        EwoCompat.setScreen(mc, new InventoryScreen(mc.player));
    }

    private static void swapTotemStep(int containerSlot) {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.gameMode == null
                || !(EwoCompat.screen(mc) instanceof InventoryScreen)) {
            EwoActionMotor.abort();
            return;
        }
        AbstractContainerMenu menu = mc.player.containerMenu;
        if (containerSlot < 0 || containerSlot >= menu.slots.size()
                || !menu.getSlot(containerSlot).getItem().is(Items.TOTEM_OF_UNDYING)) {
            EwoActionMotor.abort();
            closeInventoryStep();
            return;
        }
        mc.gameMode.handleContainerInput(
            menu.containerId, containerSlot, 40, ContainerInput.SWAP, mc.player);
    }

    private static void closeInventoryStep() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null) {
            return;
        }
        if (EwoCompat.screen(mc) instanceof InventoryScreen) {
            mc.player.closeContainer();
        }
    }
}
