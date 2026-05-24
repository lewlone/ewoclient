package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.Items;

/**
 * Phase H — Auto Eat module.
 *
 * <p>When hunger drops below the threshold and a known food is in the hotbar,
 * swap to that slot and trigger the same {@code MultiPlayerGameMode.useItem}
 * call right-clicking would. Vanilla's continuous-use logic handles the eat
 * animation, sound, packet, and consumption. We just wait for
 * {@code LocalPlayer.isUsingItem()} to clear, then optionally swap back to
 * the slot the user was holding.
 *
 * <p>Food source is hotbar-only on purpose — opening the inventory mid-fight
 * to fetch food is more intrusive than just keeping food in the hotbar.
 * Gapples + enchanted gapples are excluded from the preferred list so the
 * module doesn't burn through their absorption/regen as plain hunger food.
 *
 * <p>Won't fire while the user is attacking ({@code keyAttack} held) — right-
 * click would compete with the left-click action and break their PvP rhythm.
 */
public final class EwoAutoEat {

    private EwoAutoEat() {}

    /** Foods Auto Eat will reach for, in priority order. Golden carrot first
     *  (best saturation:cost ratio for sustain eating), then cooked meats,
     *  then garden staples. Excludes gapples — those are combat consumables
     *  the user should choose manually. */
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

    /** Hotbar slot the user was holding when we started eating — swapped back
     *  after the eat completes (unless they manually changed mid-eat). */
    private static int previousSlot = -1;
    /** Hotbar slot we swapped to for the eat. Used to detect "user manually
     *  switched slots mid-eat" and skip the restore. */
    private static int eatingSlot = -1;
    /** True from the frame we triggered {@code useItem} until vanilla's use
     *  action completes. */
    private static boolean eating = false;

    public static void tick() {
        if (!EwoModuleData.enabled(EwoModuleData.AUTO_EAT)) {
            resetEatState();
            return;
        }

        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.options == null) {
            resetEatState();
            return;
        }
        LocalPlayer player = mc.player;

        // In-progress eat: wait for vanilla's use action to finish, then
        // restore the previous slot (if we changed it AND the user didn't
        // manually swap away during the eat).
        if (eating) {
            if (player.isUsingItem()) {
                return; // still chewing
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

        // Not eating yet — check trigger gates.
        if (mc.screen != null) return;
        if (mc.options.keyAttack.isDown()) return; // never interrupt an attack
        if (EwoActionMotor.busy()) return;
        if (mc.gameMode == null) return;

        int threshold = (int) EwoModuleData.setting(EwoModuleData.AUTO_EAT, 0);
        if (player.getFoodData().getFoodLevel() > threshold) {
            return;
        }

        int foodSlot = findFoodHotbarSlot(player.getInventory());
        if (foodSlot < 0) return;

        // Engage: swap to food slot (if needed) and start the eat.
        previousSlot = player.getInventory().getSelectedSlot();
        eatingSlot = foodSlot;
        if (foodSlot != previousSlot) {
            swapHotbar(foodSlot);
        }
        mc.gameMode.useItem(player, InteractionHand.MAIN_HAND);
        eating = true;
    }

    /** First hotbar slot whose item matches a {@link #PREFERRED_FOODS}
     *  preference; iterates by preference so golden carrot beats apple. */
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

    /** Hotbar swap the way a number-key press does: set the inventory index +
     *  send the standard packet. */
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
