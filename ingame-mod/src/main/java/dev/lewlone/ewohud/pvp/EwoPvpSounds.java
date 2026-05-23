package dev.lewlone.ewohud.pvp;

import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.sounds.SoundEvent;
import net.minecraft.sounds.SoundEvents;

/**
 * The PvP-Utils sound palette &mdash; the same eight vanilla cues the source
 * mod offered, ported to 26.1.1 Mojmap. Pure enum, no per-instance state.
 *
 * <p>Note-block sounds are {@code Holder.Reference<SoundEvent>} in 26.x; the
 * bare ones (amethyst chime, anvil land, XP-orb pickup) stayed as
 * {@code SoundEvent}. Both shapes normalise to a bare {@link SoundEvent}.
 */
public enum EwoPvpSounds {
    BELL(SoundEvents.NOTE_BLOCK_BELL.value()),
    PLING(SoundEvents.NOTE_BLOCK_PLING.value()),
    CHIME(SoundEvents.NOTE_BLOCK_CHIME.value()),
    HARP(SoundEvents.NOTE_BLOCK_HARP.value()),
    BASS(SoundEvents.NOTE_BLOCK_BASS.value()),
    XP_ORB(SoundEvents.EXPERIENCE_ORB_PICKUP),
    ANVIL(SoundEvents.ANVIL_LAND),
    AMETHYST(SoundEvents.AMETHYST_BLOCK_CHIME);

    private final SoundEvent sound;

    EwoPvpSounds(SoundEvent sound) {
        this.sound = sound;
    }

    public SoundEvent sound() {
        return sound;
    }

    /** Parse a token from {@code pvp.toml}; falls back to BELL on a typo. */
    public static EwoPvpSounds fromToken(String s) {
        if (s == null) return BELL;
        try {
            return EwoPvpSounds.valueOf(s.toUpperCase());
        } catch (IllegalArgumentException e) {
            return BELL;
        }
    }

    /** Play this sound on the local player at {@code pitch} / {@code volume}. */
    public void play(float volume, float pitch) {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null) return;
        LocalPlayer p = mc.player;
        if (p == null) return;
        p.playSound(sound, volume, pitch);
    }
}
