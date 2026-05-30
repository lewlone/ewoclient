package dev.lewlone.ewohud.assist;

import dev.lewlone.ewohud.EwoModuleData;
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
 * Hand Restock module.
 *
 * <p>Watches the held hotbar slot for two refill triggers:
 *
 * <ol>
 *   <li><b>Tool broke</b>: the slot transitioned from non-empty to empty
 *       (last arrow shot, last gapple eaten, pickaxe broke). Refill with
 *       any non-empty stack of the same item from inventory.</li>
 *   <li><b>Threshold hit</b>: the held stack's count dropped from
 *       {@code &gt; threshold} to {@code &le; threshold}. Refill only if a
 *       <i>fuller</i> stack of the same item exists.</li>
 * </ol>
 *
 * <p>"Same slot" is the key trigger gate: switching hotbar slots manually
 * doesn't fire, since both comparisons require the previous tick's slot to
 * match the current one.
 *
 * <p>Restock source priority: main inventory first (the user's "reserves"),
 * other hotbar slots second.
 */
public final class EwoHandRestock {

    private EwoHandRestock() {}

    private static Item lastHeldItem = Items.AIR;
    private static int lastHeldSlot = -1;
    private static int lastHeldCount = 0;

    public static void tick() {
        if (!EwoModuleData.enabled(AssistSlots.HAND_RESTOCK)) {
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
            return;
        }

        LocalPlayer player = mc.player;
        Inventory inv = player.getInventory();
        int currentSlot = inv.getSelectedSlot();
        if (currentSlot < 0 || currentSlot >= 9) {
            return;
        }
        ItemStack heldStack = inv.getItem(currentSlot);
        Item currentItem = heldStack.getItem();
        int currentCount = heldStack.getCount();

        int threshold = (int) EwoModuleData.setting(AssistSlots.HAND_RESTOCK, 0);

        boolean toolBroke = currentItem == Items.AIR
                && lastHeldItem != Items.AIR
                && lastHeldSlot == currentSlot;

        boolean thresholdHit = !toolBroke
                && currentItem != Items.AIR
                && currentItem == lastHeldItem
                && lastHeldSlot == currentSlot
                && currentCount < lastHeldCount
                && currentCount <= threshold;

        Item targetItem = null;
        int sourceMinCount = 0;
        if (toolBroke) {
            targetItem = lastHeldItem;
            sourceMinCount = 0;
        } else if (thresholdHit) {
            targetItem = currentItem;
            sourceMinCount = currentCount;
        }

        if (targetItem != null) {
            final int destHotbar = currentSlot;
            int sourceContainerSlot = findItemContainerSlot(inv, targetItem, destHotbar, sourceMinCount);
            if (sourceContainerSlot >= 0) {
                final int srcSlot = sourceContainerSlot;
                EwoActionMotor.enqueue(EwoHandRestock::openInventoryStep, 80);
                EwoActionMotor.enqueue(() -> swapToHotbarStep(srcSlot, destHotbar), 60);
                EwoActionMotor.enqueue(EwoHandRestock::closeInventoryStep, 0);
            }
        }

        lastHeldItem = currentItem;
        lastHeldSlot = currentSlot;
        lastHeldCount = currentCount;
    }

    private static int findItemContainerSlot(Inventory inv, Item target,
                                             int excludeHotbarSlot, int minCount) {
        for (int i = 9; i <= 35; i++) {
            ItemStack s = inv.getItem(i);
            if (s.is(target) && s.getCount() > minCount) {
                return i;
            }
        }
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
