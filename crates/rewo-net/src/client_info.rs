//! `ServerboundClientInformationPacket`'s body (M206) — what the client tells
//! the server about itself in configuration.
//!
//! Vanilla builds it in `Options.buildPlayerInformation`
//! (`Options.java:1847-1865`) from the loaded options:
//!
//! ```text
//! new ClientInformation(languageCode, renderDistance.get(), chatVisibility.get(),
//!     chatColors.get(), parts /* OR of every enabled PlayerModelPart mask */,
//!     mainHand.get(), isTextFilteringEnabled(), allowServerListing.get(),
//!     particles.get())
//! ```
//!
//! **Three fields come from `options.txt`, the rest are pinned to what Rewo
//! draws.** The render distance decides how many columns the server sends
//! (`ChunkMap.getPlayerViewDistance` clamps it to the server's own), the model
//! parts are echoed back to every client as the player's customisation byte,
//! and `allowServerListing` is the player's privacy choice for the server's
//! status sample. None of the three changes what Rewo itself draws, so reading
//! them is parity. The others would: `mainHand:"left"` makes every other
//! client draw a left-handed player while Rewo still draws its own right arm,
//! `chatVisibility` makes the server withhold chat Rewo would still show
//! space for, and `particles` lets the server thin what Rewo draws in full. So
//! those stay at the values Rewo actually renders (right hand, full chat, all
//! particles), recorded rather than read. The language is `en_us` (Rewo is
//! English only) and text filtering is off (an online-account property, not
//! an option).

use crate::options::Options;
use crate::PacketWriter;

/// The fields of `ClientInformation` that Rewo takes from the options file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientInformation {
    /// `options.renderDistance`, in chunks (2..=32).
    pub view_distance: u8,
    /// The `PlayerModelPart` mask: bit `n` set when part `n` is shown.
    pub model_parts: u8,
    /// `options.allowServerListing`.
    pub allow_server_listing: bool,
}

impl Default for ClientInformation {
    /// What a vanilla client with no `options.txt` sends: render distance 12,
    /// every model part, listing allowed.
    fn default() -> Self {
        Self::from(&Options::default())
    }
}

impl From<&Options> for ClientInformation {
    fn from(o: &Options) -> Self {
        Self {
            view_distance: o.render_distance.clamp(2, 32) as u8,
            model_parts: o.model_parts,
            allow_server_listing: o.allow_server_listing,
        }
    }
}

/// `ChatVisiblity.FULL`'s id.
const CHAT_FULL: i32 = 0;
/// `HumanoidArm.RIGHT`'s id.
const MAIN_HAND_RIGHT: i32 = 1;
/// `ParticleStatus.ALL`'s id.
const PARTICLES_ALL: i32 = 0;

impl ClientInformation {
    /// `ClientInformation.write`'s field order: language, view distance as a
    /// byte, chat visibility, chat colours, the model-part byte, main hand,
    /// text filtering, server listing, particle status.
    pub fn write(&self, w: &mut PacketWriter) {
        w.string("en_us")
            .u8(self.view_distance)
            .varint(CHAT_FULL)
            .bool(true) // chat colours: Rewo draws them
            .u8(self.model_parts)
            .varint(MAIN_HAND_RIGHT)
            .bool(false) // text filtering
            .bool(self.allow_server_listing)
            .varint(PARTICLES_ALL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire bytes, field by field, for a non-default option set — a
    /// swapped pair of fields or a constant left in place of an option fails.
    #[test]
    fn the_packet_carries_the_three_options_in_vanilla_order() {
        let o = Options::parse("renderDistance:6\nmodelPart_hat:false\nallowServerListing:false\n");
        let info = ClientInformation::from(&o);
        let mut w = PacketWriter::default();
        info.write(&mut w);
        let mut want = vec![5u8];
        want.extend_from_slice(b"en_us");
        want.extend_from_slice(&[6, 0, 1, 0x3f, 1, 0, 0, 0]);
        assert_eq!(w.buf, want);
    }

    #[test]
    fn no_options_file_sends_vanillas_defaults() {
        let info = ClientInformation::default();
        assert_eq!(
            info,
            ClientInformation {
                view_distance: 12,
                model_parts: 0x7f,
                allow_server_listing: true
            }
        );
    }
}
