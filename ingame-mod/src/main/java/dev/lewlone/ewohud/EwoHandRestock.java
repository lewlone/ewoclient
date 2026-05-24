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
 * <p>Watches the held hotbar slot for two refill triggers:
 *
 * <ol>
 *   <li><b>Tool broke</b>: the slot transitioned from non-empty to empty
 *       (last arrow shot, last gapple eaten, pickaxe broke). Refill with
 *       any non-empty stack of the same item from inventory.</li>
 *   <li><b>Threshold hit</b>: the held stack's count dropped from
 *       {@code &gt; threshold} to {@code &le; threshold}. Refill only if a
 *       <i>fuller</i> stack of the same item exists — no degenerate
 *       "swap 1 arrow for 1 arrow" flickers.</li>
 * </ol>
 *
 * <p>Threshold defaults to 0 (= behave like classic Hand Restock; only the
 * tool-broke path fires). Dial it up via the MODULES tab to pre-empt mid-
 * fight droughts — e.g. threshold 1 swaps arrows when you have 1 left, so
 * you never actually shoot dry.
 *
 * <p>"Same slot" is the key trigger gate: switching hotbar slots manually
 * doesn't fire, since both comparisons require the previous tick's slot to
 * match the current one. Sliding the held slot mid-tick is a deliberate
 * user action, not a refill.
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
    /** Hotbar slot index (0..8) the player was on last tick. */
    private static int lastHeldSlot = -1;
    /** Stack count on the held slot the previous tick. */
    private static int lastHeldCount = 0;

    /** Per-frame check + maybe-queue the open/swap/close sequence. */
    public static void tick() {
        if (!EwoModuleData.enabled(EwoModuleData.HAND_RESTOCK)) {
            lastHeldItem = Items.AIR;
            lastHeldSlot = -1;
            lastHeldCount = 0;
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
        ItemStack heldStack = inv.getItem(currentSlot);
        Item currentItem = heldStack.getItem();
        int currentCount = heldStack.getCount();

        int threshold = (int) EwoModuleData.setting(EwoModuleData.HAND_RESTOCK, 0);

        // Path 1 — held slot just emptied (tool broke, last consumable used).
        boolean toolBroke = currentItem == Items.AIR
                && lastHeldItem != Items.AIR
                && lastHeldSlot == currentSlot;

        // Path 2 — same item, but the count dropped (this tick) while at or
        // below the threshold. Each drop is its own trigger so reserves
        // showing up later still get a refill attempt; if no source exists
        // the find returns -1 and nothing happens (cheap).
        boolean thresholdHit = !toolBroke
                && currentItem != Items.AIR
                && currentItem == lastHeldItem
                && lastHeldSlot == currentSlot
                && currentCount < lastHeldCount
                && currentCount <= threshold;

        Item targetItem = null;
        int sourceMinCount = 0; // source.count must be strictly > this
        if (toolBroke) {
            targetItem = lastHeldItem;
            sourceMinCount = 0; // any non-empty stack qualifies
        } else if (thresholdHit) {
            targetItem = currentItem;
            sourceMinCount = currentCount; // only swap to an actual fuller stack
        }

        if (targetItem != null) {
            final int destHotbar = currentSlot;
            int sourceContainerSlot = findItemContainerSlot(inv, targetItem, destHotbar, sourceMinCount);
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
        lastHeldCount = currentCount;
    }

    /** Container-menu slot of the first matching stack with count strictly
     *  greater than {@code minCount} — main inventory first (the reserve),
     *  then other hotbar slots (last-resort cannibalisation). */
    private static int findItemContainerSlot(Inventory inv, Item target,
                                             int excludeHotbarSlot, int minCount) {
        // Main inventory: Inv[9..35] maps to container slots 9..35 identically.
        for (int i = 9; i <= 35; i++) {
            ItemStack s = inv.getItem(i);
            if (s.is(target) && s.getCount() > minCount) {
                return i;
            }
        }
        // Other hotbar slots — Inv[0..8] maps to container 36..44.
        for (int i = 0; i < 9; i++) {
            if (i == excludeHotbarSlot) continue;
            ItemStack s = inv.getItem(i);
            if (s.is(target) && s.getCount() > minCount) {
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
