package dev.lewlone.ewohud.assist;

import java.util.function.Predicate;

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
 * Legit Elytra Swap module.
 *
 * <p>Triggered by a bound key. Swaps the chest armor slot between elytra and
 * chestplate using three real inventory clicks — pick up what's in chest,
 * pick up the target from inventory (swap on cursor), drop the target back
 * into chest. The inventory open + per-click delays read as a fast human,
 * not a macro.
 *
 * <p>No swap is queued if the chest slot is empty, or no opposite-armor item
 * exists in the inventory — pressing the key is a no-op rather than opening
 * the inventory pointlessly.
 */
public final class EwoLegitElytraSwap {

    private EwoLegitElytraSwap() {}

    /** Container-menu slot index for the chest armor in {@code InventoryMenu}.
     *  (Head=5, chest=6, legs=7, feet=8.) */
    private static final int CHEST_ARMOR_CONTAINER_SLOT = 6;
    /** {@code Inventory.getItem} index for the chest armor slot. */
    private static final int CHEST_INVENTORY_INDEX = 38;

    /** Bound key fired — queue the swap if the module is enabled and the
     *  chest slot + inventory contain valid swap targets. */
    public static void trigger() {
        if (!EwoModuleData.enabled(AssistSlots.LEGIT_ELYTRA_SWAP)) {
            return;
        }
        if (EwoActionMotor.busy()) {
            return;
        }

        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.screen != null) {
            return;
        }

        LocalPlayer player = mc.player;
        Inventory inv = player.getInventory();
        ItemStack chest = inv.getItem(CHEST_INVENTORY_INDEX);

        Predicate<ItemStack> targetPred;
        if (chest.is(Items.ELYTRA)) {
            targetPred = EwoLegitElytraSwap::isChestplate;
        } else if (isChestplate(chest)) {
            targetPred = stack -> stack.is(Items.ELYTRA);
        } else {
            return;
        }

        int targetContainerSlot = findInInventory(inv, targetPred);
        if (targetContainerSlot < 0) {
            return;
        }

        final int target = targetContainerSlot;
        EwoActionMotor.enqueue(EwoLegitElytraSwap::openInventoryStep, 80);
        EwoActionMotor.enqueue(EwoLegitElytraSwap::clickChestStep, 60);
        EwoActionMotor.enqueue(() -> clickSlotStep(target), 60);
        EwoActionMotor.enqueue(EwoLegitElytraSwap::clickChestStep, 80);
        EwoActionMotor.enqueue(EwoLegitElytraSwap::closeInventoryStep, 0);
    }

    private static boolean isChestplate(ItemStack stack) {
        if (stack.isEmpty()) return false;
        Item it = stack.getItem();
        return it == Items.LEATHER_CHESTPLATE
            || it == Items.CHAINMAIL_CHESTPLATE
            || it == Items.IRON_CHESTPLATE
            || it == Items.GOLDEN_CHESTPLATE
            || it == Items.DIAMOND_CHESTPLATE
            || it == Items.NETHERITE_CHESTPLATE;
    }

    private static int findInInventory(Inventory inv, Predicate<ItemStack> pred) {
        for (int i = 0; i < 9; i++) {
            if (pred.test(inv.getItem(i))) return 36 + i;
        }
        for (int i = 9; i <= 35; i++) {
            if (pred.test(inv.getItem(i))) return i;
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

    private static void clickChestStep() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.gameMode == null
                || !(mc.screen instanceof InventoryScreen)) {
            EwoActionMotor.abort();
            return;
        }
        AbstractContainerMenu menu = mc.player.containerMenu;
        if (CHEST_ARMOR_CONTAINER_SLOT >= menu.slots.size()) {
            EwoActionMotor.abort();
            closeInventoryStep();
            return;
        }
        mc.gameMode.handleContainerInput(
            menu.containerId,
            CHEST_ARMOR_CONTAINER_SLOT,
            0,
            ContainerInput.PICKUP,
            mc.player);
    }

    private static void clickSlotStep(int containerSlot) {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.gameMode == null
                || !(mc.screen instanceof InventoryScreen)) {
            EwoActionMotor.abort();
            return;
        }
        AbstractContainerMenu menu = mc.player.containerMenu;
        if (containerSlot < 0 || containerSlot >= menu.slots.size()) {
            EwoActionMotor.abort();
            closeInventoryStep();
            return;
        }
        mc.gameMode.handleContainerInput(
            menu.containerId,
            containerSlot,
            0,
            ContainerInput.PICKUP,
            mc.player);
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
