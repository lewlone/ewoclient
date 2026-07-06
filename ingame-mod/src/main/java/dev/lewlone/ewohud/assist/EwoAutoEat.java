package dev.lewlone.ewohud.assist;

import dev.lewlone.ewohud.EwoCompat;

import dev.lewlone.ewohud.EwoModuleData;
import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.Items;

/**
 * Auto Eat module.
 *
 * <p>When hunger drops below the threshold and a known food is in the hotbar,
 * swap to that slot and trigger the same {@code MultiPlayerGameMode.useItem}
 * call right-clicking would. Vanilla's continuous-use logic handles the eat
 * animation, sound, packet, and consumption. We just wait for
 * {@code LocalPlayer.isUsingItem()} to clear, then optionally swap back to
 * the slot the user was holding.
 *
 * <p>Food source is hotbar-only on purpose. Gapples + enchanted gapples are
 * excluded from the preferred list so the module doesn't burn through their
 * absorption/regen as plain hunger food.
 *
 * <p>Won't fire while the user is attacking ({@code keyAttack} held).
 */
public final class EwoAutoEat {

    private EwoAutoEat() {}

    private static final Item[] PREFERRED_FOODS = {
        Items.GOLDEN_CARROT,
        Items.COOKED_BEEF,
        Items.COOKED_PORKCHOP,
        Items.COOKED_MUTTON,
        Items.COOKED_SALMON,
        Items.COOKED_COD,
        Items.COOKED_CHICKEN,
        Items.COOKED_RABBIT,
        Items.BAKED_POTATO,
        Items.BREAD,
        Items.CARROT,
        Items.APPLE,
    };

    private static int previousSlot = -1;
    private static int eatingSlot = -1;
    private static boolean eating = false;

    public static void tick() {
        if (!EwoModuleData.enabled(AssistSlots.AUTO_EAT)) {
            resetEatState();
            return;
        }

        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.options == null) {
            resetEatState();
            return;
        }
        LocalPlayer player = mc.player;

        if (eating) {
            if (player.isUsingItem()) {
                return;
            }
            int slotNow = player.getInventory().getSelectedSlot();
            if (previousSlot >= 0 && previousSlot < 9
                    && eatingSlot == slotNow
                    && previousSlot != eatingSlot) {
                swapHotbar(previousSlot);
            }
            resetEatState();
            return;
        }

        if (EwoCompat.screen(mc) != null) return;
        if (mc.options.keyAttack.isDown()) return;
        if (EwoActionMotor.busy()) return;
        if (mc.gameMode == null) return;

        int threshold = (int) EwoModuleData.setting(AssistSlots.AUTO_EAT, 0);
        if (player.getFoodData().getFoodLevel() > threshold) {
            return;
        }

        int foodSlot = findFoodHotbarSlot(player.getInventory());
        if (foodSlot < 0) return;

        previousSlot = player.getInventory().getSelectedSlot();
        eatingSlot = foodSlot;
        if (foodSlot != previousSlot) {
            swapHotbar(foodSlot);
        }
        mc.gameMode.useItem(player, InteractionHand.MAIN_HAND);
        eating = true;
    }

    private static int findFoodHotbarSlot(Inventory inv) {
        for (Item food : PREFERRED_FOODS) {
            for (int i = 0; i < 9; i++) {
                if (inv.getItem(i).is(food)) {
                    return i;
                }
            }
        }
        return -1;
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

    private static void resetEatState() {
        eating = false;
        previousSlot = -1;
        eatingSlot = -1;
    }
}
