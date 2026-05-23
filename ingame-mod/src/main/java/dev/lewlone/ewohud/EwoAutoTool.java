package dev.lewlone.ewohud;

import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.core.BlockPos;
import net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.HitResult;

/**
 * Phase H1 — Auto Tool module.
 *
 * <p>While the attack key is held against a block, swap to the most-effective
 * hotbar slot for that block before the swing lands. The slot swap is a real
 * client-side action: {@code Inventory.selected} is updated and the same
 * {@link ServerboundSetCarriedItemPacket} a real number-key press would send
 * is forwarded to the server. No fake packets, no impossible-state combos.
 *
 * <p>Triggers exactly once per <i>new</i> block target — looking at the same
 * block doesn't re-swap, so the user can manually pick a different slot
 * mid-mine and the auto-swap won't immediately undo it.
 */
public final class EwoAutoTool {

    private EwoAutoTool() {}

    /** Block we last swapped for. {@code null} = no current target or attack
     *  key released since the last swap; either case re-arms the swap. */
    private static BlockPos lastTarget;

    /** Per-frame check. Queues at most one slot-swap step via the motor. */
    public static void tick() {
        if (!EwoModuleData.enabled(EwoModuleData.AUTO_TOOL)) {
            lastTarget = null;
            return;
        }
        if (EwoActionMotor.busy()) {
            return;
        }

        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.level == null
                || mc.options == null || mc.screen != null) {
            lastTarget = null;
            return;
        }

        // Only act while the user is actively trying to mine — `keyAttack` is
        // released when no screen is open, so this also filters out menu
        // clicks. Releasing the key re-arms a fresh swap on the next press.
        if (!mc.options.keyAttack.isDown()) {
            lastTarget = null;
            return;
        }

        HitResult hr = mc.hitResult;
        if (!(hr instanceof BlockHitResult bhr) || bhr.getType() != HitResult.Type.BLOCK) {
            lastTarget = null;
            return;
        }
        BlockPos pos = bhr.getBlockPos();
        if (pos.equals(lastTarget)) {
            return;
        }

        LocalPlayer player = mc.player;
        BlockState state = mc.level.getBlockState(pos);
        Inventory inv = player.getInventory();
        int current = inv.getSelectedSlot();
        float bestSpeed = inv.getItem(current).getDestroySpeed(state);
        int bestSlot = current;

        for (int i = 0; i < 9; i++) {
            if (i == current) continue;
            ItemStack stack = inv.getItem(i);
            if (stack.isEmpty()) continue;
            float speed = stack.getDestroySpeed(state);
            if (speed > bestSpeed) {
                bestSpeed = speed;
                bestSlot = i;
            }
        }

        // Don't bother swapping if the best slot is the current one, or if no
        // hotbar tool is faster than bare hands (speed 1.0 on most blocks).
        if (bestSlot == current || bestSpeed <= 1.0f) {
            lastTarget = pos;
            return;
        }

        lastTarget = pos;
        final int target = bestSlot;
        // Single-step sequence — no perceptible delay before swap; the motor
        // would normally pace inter-step delays, but a hotbar press has no
        // sub-actions. Re-queueable on the next block target.
        EwoActionMotor.enqueue(() -> swapHotbar(target), 0);
    }

    /** Switch hotbar slot the way a number-key press does: set the inventory
     *  index + send the standard set-carried-item packet. */
    private static void swapHotbar(int slot) {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.player == null || mc.player.connection == null) {
            return;
        }
        if (slot < 0 || slot >= 9) {
            return;
        }
        Inventory inv = mc.player.getInventory();
        if (inv.getSelectedSlot() == slot) {
            return;
        }
        inv.setSelectedSlot(slot);
        mc.player.connection.send(new ServerboundSetCarriedItemPacket(slot));
    }
}
