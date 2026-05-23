package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.screens.inventory.InventoryScreen;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.inventory.AbstractContainerMenu;
import net.minecraft.world.inventory.ContainerInput;
import net.minecraft.world.item.Items;

/**
 * Phase H2 — Auto Totem module.
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

    /** Wall-clock ms of the most recent attempt; back-off so a totem-less
     *  player doesn't see their inventory flicker open every frame. */
    private static long lastAttemptMs = 0L;
    /** ms between re-attempts when offhand stays empty. */
    private static final long ATTEMPT_THROTTLE_MS = 2000L;

    /** Per-frame check + maybe-queue the open/swap/close sequence. */
    public static void tick() {
        if (!EwoModuleData.enabled(EwoModuleData.AUTO_TOTEM)) {
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
        // Don't intrude on any user-owned screen — only act from world view.
        if (mc.screen != null) {
            return;
        }

        LocalPlayer player = mc.player;
        Inventory inv = player.getInventory();

        // The trigger is offhand empty. If it isn't, reset the throttle so a
        // future pop fires immediately rather than waiting out a stale window.
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
            // No totem to grab — back off the throttle so we don't poll this
            // every frame when the player's out of totems for the whole fight.
            lastAttemptMs = now;
            return;
        }

        lastAttemptMs = now;
        final int srcSlot = sourceContainerSlot;
        // Three real client-side actions, paced like a human reacting fast:
        //   open inv → ~80 ms → F-swap totem to offhand → ~80 ms → close.
        // Motor jitter (±15%) varies the cadence so it doesn't tick-align.
        EwoActionMotor.enqueue(EwoAutoTotem::openInventoryStep, 80);
        EwoActionMotor.enqueue(() -> swapTotemStep(srcSlot), 80);
        EwoActionMotor.enqueue(EwoAutoTotem::closeInventoryStep, 0);
    }

    /** Container-menu slot of the first totem in the player's hotbar (preferred)
     *  or main inventory, or {@code -1} if no totem is reachable. */
    private static int findTotemContainerSlot(Inventory inv) {
        // Hotbar first — Inv[0..8] maps to container slots 36..44, so the
        // F-swap is closer to where the cursor naturally rests.
        for (int i = 0; i < 9; i++) {
            if (inv.getItem(i).is(Items.TOTEM_OF_UNDYING)) {
                return 36 + i;
            }
        }
        // Main inventory: Inv[9..35] maps to container slots 9..35 identically.
        for (int i = 9; i <= 35; i++) {
            if (inv.getItem(i).is(Items.TOTEM_OF_UNDYING)) {
                return i;
            }
        }
        return -1;
    }

    /** Step 1 — open the player's inventory. Aborts the queue if the user
     *  already has a screen open (we'd corrupt whatever they're doing). */
    private static void openInventoryStep() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null) {
            EwoActionMotor.abort();
            return;
        }
        if (mc.screen != null) {
            // User opened something in the wait window — bail out safely.
            EwoActionMotor.abort();
            return;
        }
        mc.setScreen(new InventoryScreen(mc.player));
    }

    /** Step 2 — F-swap the totem at {@code containerSlot} to the offhand.
     *  Aborts if the inventory is no longer open or the slot no longer holds
     *  a totem (the world has shifted out from under us). */
    private static void swapTotemStep(int containerSlot) {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.gameMode == null
                || !(mc.screen instanceof InventoryScreen)) {
            EwoActionMotor.abort();
            return;
        }
        AbstractContainerMenu menu = mc.player.containerMenu;
        if (containerSlot < 0 || containerSlot >= menu.slots.size()
                || !menu.getSlot(containerSlot).getItem().is(Items.TOTEM_OF_UNDYING)) {
            // Totem moved between queueing and now — close politely and bail.
            EwoActionMotor.abort();
            closeInventoryStep();
            return;
        }
        // Button 40 + ContainerInput.SWAP is exactly what F does while hovering
        // a slot in the inventory: swap that slot with the offhand. One packet.
        // (ContainerInput is the 26.x rename of the old ClickType enum.)
        mc.gameMode.handleContainerInput(
            menu.containerId, containerSlot, 40, ContainerInput.SWAP, mc.player);
    }

    /** Step 3 — close the inventory we opened. Safe to call even if the
     *  screen has already gone away (e.g. step 2 aborted partway). */
    private static void closeInventoryStep() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null) {
            return;
        }
        if (mc.screen instanceof InventoryScreen) {
            mc.player.closeContainer();
        }
    }
}
