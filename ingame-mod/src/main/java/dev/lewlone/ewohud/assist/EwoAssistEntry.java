package dev.lewlone.ewohud.assist;

import net.fabricmc.api.ClientModInitializer;

import dev.lewlone.ewohud.EwoExtensionHook;

/**
 * The pvp build's client entrypoint. Declared only in
 * {@code fabric-pvp.mod.json}; the legit build has neither this class nor any
 * reference to it. Registers {@link EwoAssist}'s tick and key router into
 * {@link EwoExtensionHook}, which the legit-side {@code EwoModules} drives.
 */
public final class EwoAssistEntry implements ClientModInitializer {

    @Override
    public void onInitializeClient() {
        EwoExtensionHook.register(EwoAssist::tick, EwoAssist::handleKeyPress);
    }
}
