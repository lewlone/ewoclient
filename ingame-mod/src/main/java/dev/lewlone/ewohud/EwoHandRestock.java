package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.screens.inventory.InventoryScreen;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.inventory.AbstractContainerMenu;
import net.minecraft.world.inventory.ContainerInput;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;

/**
 * Phase H — Hand Restock module.
 *
 * <p>When the held hotbar slot transitions from non-empty to empty (you shot
 * the last arrow, ate the last gapple, broke a pickaxe), find another stack
 * of the same item in inventory and SWAP it into the held slot through a
 * real inventory open + click sequence. The slot stays the same — spatial
 * memory is preserved.
 *
 * <p>"Same slot" is the key trigger gate: switching hotbar slots manually
 * doesn't fire, since the comparison is "previous tick, this slot, had X;
 * this tick, this slot, has nothing". Sliding the held slot mid-tick is
 * a deliberate user action, not a refill.
 *
 * <p>Restock source priority: main inventory first (the user's "reserves"),
 * other hotbar slots second (they're probably committed to other items, but
 * better than nothing).
 */
public final class EwoHandRestock {

    private EwoHandRestock() {}

    /** Item type in the held hotbar slot on the previous tick. {@code AIR}
     *  means we have nothing to refill yet — first tick, or last tick's
     *  slot was empty too. */
    private static Item lastHeldItem = Items.AIR;
    /** Hotbar slot index (0..8) the player was on last tick. The combination
     *  of {@code lastHeldSlot == currentSlot} AND {@code lastHeldItem != AIR}
     *  is the "this slot just emptied" trigger. */
    private static int lastHeldSlot = -1;

    /** Per-frame check + maybe-queue the open/swap/close sequence. */
    public static void tick() {
        if (!EwoModuleData.enabled(EwoModuleData.HAND_RESTOCK)) {
            lastHeldItem = Items.AIR;
            lastHeldSlot = -1;
            return;
        }
        if (EwoActionMotor.busy()) {
            return;
        }

        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.player.connection == null
                || mc.screen != null) {
            // Mid-sequence ticks (screen open) skip the state update too —
            // when the screen closes the post-refill state is what we want
            // to compare against, not the pre-sequence state.
            return;
        }

        LocalPlayer player = mc.player;
        Inventory inv = player.getInventory();
        int currentSlot = inv.getSelectedSlot();
        if (currentSlot < 0 || currentSlot >= 9) {
            return; // defensive — selected should always be a hotbar index
        }
        Item currentItem = inv.getItem(currentSlot).getItem();

        // Trigger: this slot just emptied with something we can refill.
        boolean justEmptied = currentItem == Items.AIR
                && lastHeldItem != Items.AIR
                && lastHeldSlot == currentSlot;

        if (justEmptied) {
            final Item target = lastHeldItem;
            final int destHotbar = currentSlot;
            int sourceContainerSlot = findItemContainerSlot(inv, target, destHotbar);
            if (sourceContainerSlot >= 0) {
                final int srcSlot = sourceContainerSlot;
                // Open → SWAP click on the source slot with button =
                // destHotbar (0..8 = "swap this slot with that hotbar slot",
                // exactly what pressing a number key over a slot in the
                // inventory does) → close.
                EwoActionMotor.enqueue(EwoHandRestock::openInventoryStep, 80);
                EwoActionMotor.enqueue(() -> swapToHotbarStep(srcSlot, destHotbar), 60);
                EwoActionMotor.enqueue(EwoHandRestock::closeInventoryStep, 0);
            }
        }

        lastHeldItem = currentItem;
        lastHeldSlot = currentSlot;
    }

    /** Container-menu slot of the first matching stack — main inventory first
     *  (the reserve), then other hotbar slots (last-resort cannibalisation). */
    private static int findItemContainerSlot(Inventory inv, Item target, int excludeHotbarSlot) {
        // Main inventory: Inv[9..35] maps to container slots 9..35 identically.
        for (int i = 9; i <= 35; i++) {
            if (inv.getItem(i).is(target)) {
                return i;
            }
        }
        // Other hotbar slots — Inv[0..8] maps to container 36..44.
        for (int i = 0; i < 9; i++) {
            if (i == excludeHotbarSlot) continue;
            if (inv.getItem(i).is(target)) {
                return 36 + i;
            }
        }
        return -1;
    }

    private static void openInventoryStep() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.screen != null) {
            EwoActionMotor.abort();
            return;
        }
        mc.setScreen(new InventoryScreen(mc.player));
    }

    /** SWAP click on {@code sourceContainerSlot} with button = destination
     *  hotbar slot (0..8). The source's contents land in the held hotbar
     *  slot; whatever was there (nothing, in our case) goes back to source. */
    private static void swapToHotbarStep(int sourceContainerSlot, int destHotbarButton) {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.gameMode == null
                || !(mc.screen instanceof InventoryScreen)) {
            EwoActionMotor.abort();
            return;
        }
        AbstractContainerMenu menu = mc.player.containerMenu;
        if (sourceContainerSlot < 0 || sourceContainerSlot >= menu.slots.size()) {
            EwoActionMotor.abort();
            closeInventoryStep();
            return;
        }
        // Verify the slot still holds something — the item we were going to
        // grab might have moved between queue and run.
        ItemStack stack = menu.getSlot(sourceContainerSlot).getItem();
        if (stack.isEmpty()) {
            EwoActionMotor.abort();
            closeInventoryStep();
            return;
        }
        mc.gameMode.handleContainerInput(
            menu.containerId, sourceContainerSlot, destHotbarButton,
            ContainerInput.SWAP, mc.player);
    }

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
