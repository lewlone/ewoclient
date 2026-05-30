package dev.lewlone.ewohud.assist;

/**
 * Module-slot constants for the assist (PvP / packet-touching) catalog.
 *
 * <p>These match {@code ewo_core::modules::REGISTRY} slots 12..25 — the
 * assist portion of the registry, included only when the Rust side is built
 * with {@code --features pvp} and the Java side with {@code build.ps1 -Pvp}.
 *
 * <p>Legit slot constants (slots 0..11) live in {@code EwoModuleData} in the
 * parent package — those are always available. Splitting the assist slots
 * out here keeps their identifier names out of the legit jar's constant pool
 * so a class-name / string scan can't see them when this file isn't compiled.
 *
 * <p>Slot indices are stable across builds: a legit-build registry is a
 * prefix of a pvp-build registry, so re-enabling {@code --features pvp}
 * preserves saved {@code modules.toml} settings (the toml is id-keyed, not
 * slot-keyed, but the runtime buffer layout is slot-keyed and these indices
 * cannot drift).
 */
public final class AssistSlots {
    private AssistSlots() {}

    public static final int AUTO_TOOL          = 12;
    public static final int AUTO_TOTEM         = 13;
    public static final int LEGIT_ELYTRA_SWAP  = 14;
    public static final int HAND_RESTOCK       = 15;
    public static final int AUTO_EAT           = 16;
    public static final int AUTO_JUMP_RESET    = 17;
    public static final int SPRINT_TAP         = 18;
    public static final int KNOCKBACK_MAX      = 19;
    public static final int AUTO_MACE_SWAP     = 20;
    public static final int AUTO_PEARL         = 21;
    public static final int RIPTIDE_BOOST      = 22;
    public static final int REACH_LOCK         = 23;
    public static final int AUTO_HIT_TIMING    = 24;
    public static final int SWING_CADENCE      = 25;
}
