//! The login-state packets `ClientHandshakePacketListenerImpl` answers beyond
//! the encryption handshake, as free functions so the bytes are testable
//! without a socket (`Connection` owns one).
//!
//! Before these existed the login loop logged and dropped `custom_query` and
//! `cookie_request`. A server (or a proxy such as Velocity, whose modern
//! forwarding is a login `custom_query`) that waits for the answer then stalls
//! the login until its own timeout.

use rewo_proto::reader::PacketReader;
use rewo_proto::writer::PacketWriter;
use rewo_proto::Result;

/// `ClientboundLoginCompressionPacket` → `Connection.setupCompression(
/// threshold, false)` (`ClientHandshakePacketListenerImpl.java:225-228`,
/// `Connection.java:544-566`): a threshold `>= 0` installs (or retunes) the
/// compressor, and a **negative** one removes it, so the frames that follow
/// are plain again. `None` is "no compression" to `FrameCodec`.
pub fn compression_threshold(body: &[u8]) -> Result<Option<i32>> {
    let threshold = PacketReader::new(body).varint()?;
    Ok((threshold >= 0).then_some(threshold))
}

/// `ClientboundLoginFinishedPacket`'s profile id: the first field of
/// `ByteBufCodecs.GAME_PROFILE` (`UUIDUtil.STREAM_CODEC`, two longs), ahead of
/// the name and the properties this does not read.
///
/// On an offline server this is the only place the player's UUID arrives (the
/// hello sends a zero one): the server names the player
/// `UUID.nameUUIDFromBytes("OfflinePlayer:" + name)`, and `DefaultPlayerSkin`
/// picks the skin from it.
pub fn profile_uuid(body: &[u8]) -> Result<u128> {
    PacketReader::new(body).uuid()
}

/// `handleCustomQuery` (`ClientHandshakePacketListenerImpl.java:232-235`):
/// vanilla understands no login query, so it answers every one with
/// `new ServerboundCustomQueryAnswerPacket(transactionId, null)`: the VarInt
/// transaction id, then `writeNullable(null)` = a `false` byte
/// (`ServerboundCustomQueryAnswerPacket.java:37-40`,
/// `FriendlyByteBuf.java:267-273`).
///
/// The query is `VarInt transactionId, Identifier channel, rest = payload`
/// (`ClientboundCustomQueryPacket.java:17-31`; the payload is up to 1 MiB and
/// discarded unread). Only the id is needed for the answer. Returns the
/// answer and the channel, for the log line.
pub fn answer_custom_query(answer_id: i32, body: &[u8]) -> Result<(PacketWriter, String)> {
    let mut r = PacketReader::new(body);
    let transaction_id = r.varint()?;
    let channel = r.identifier()?;
    let mut p = PacketWriter::packet(answer_id);
    p.varint(transaction_id).bool(false);
    Ok((p, channel))
}

/// `handleRequestCookie` (`ClientHandshakePacketListenerImpl.java:242-244`):
/// `send(new ServerboundCookieResponsePacket(key, this.cookies.get(key)))`.
/// The login listener's `cookies` is the transfer state's jar, empty on a
/// fresh connection (`:92`); Rewo's jar is [`crate::session::SessionState`],
/// the same one configuration and play answer from.
pub fn answer_cookie_request(
    response_id: i32,
    body: &[u8],
    jar: &crate::session::SessionState,
) -> Result<PacketWriter> {
    let key = PacketReader::new(body).identifier()?;
    Ok(crate::session::write_cookie_response(
        response_id,
        &key,
        jar.cookie(&key),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_negative_threshold_removes_compression() {
        // VarInt -1 is five bytes: FF FF FF FF 0F.
        assert_eq!(
            compression_threshold(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]).unwrap(),
            None
        );
        // VarInt i32::MIN.
        assert_eq!(
            compression_threshold(&[0x80, 0x80, 0x80, 0x80, 0x08]).unwrap(),
            None
        );
        // 0 compresses everything; 256 is the vanilla default.
        assert_eq!(compression_threshold(&[0x00]).unwrap(), Some(0));
        assert_eq!(compression_threshold(&[0x80, 0x02]).unwrap(), Some(256));
        assert!(compression_threshold(&[]).is_err());
    }

    #[test]
    fn the_login_finished_profile_names_the_player() {
        // `UUID.nameUUIDFromBytes("OfflinePlayer:Puppet")`, then the name and
        // an empty property list, as `GAME_PROFILE` writes them.
        let id: u128 = 0x4ec89bdb_6fa0_38e0_88fd_c71ec9524c16;
        let mut body = id.to_be_bytes().to_vec();
        body.push(6);
        body.extend_from_slice(b"Puppet");
        body.push(0);
        assert_eq!(profile_uuid(&body).unwrap(), id);
        // Fifteen bytes are not a UUID.
        assert!(profile_uuid(&body[..15]).is_err());
    }

    #[test]
    fn a_custom_query_is_answered_not_understood() {
        // transactionId 300 (AC 02), "velocity:player_info", a payload byte.
        let mut body = vec![0xAC, 0x02, 20];
        body.extend_from_slice(b"velocity:player_info");
        body.push(0x01);
        let (p, channel) = answer_custom_query(2, &body).unwrap();
        assert_eq!(channel, "velocity:player_info");
        // id 2, VarInt 300, `writeNullable(null)` = false.
        assert_eq!(p.into_bytes(), vec![0x02, 0xAC, 0x02, 0x00]);
        // A truncated query is a decode error, not an answer to id 0.
        assert!(answer_custom_query(2, &[0xAC]).is_err());
    }

    #[test]
    fn a_cookie_request_is_answered_from_the_jar() {
        let mut jar = crate::session::SessionState::default();
        let mut body = vec![14];
        body.extend_from_slice(b"minecraft:test");
        // A miss: the key, then `writeNullable(null)` = false.
        let mut want = vec![0x04, 14];
        want.extend_from_slice(b"minecraft:test");
        want.push(0x00);
        assert_eq!(
            answer_cookie_request(4, &body, &jar).unwrap().into_bytes(),
            want
        );
        // A hit: true, then `byteArray` (VarInt length + bytes).
        jar.store_cookie("minecraft:test".into(), vec![0xCA, 0xFE]);
        want.pop();
        want.extend_from_slice(&[0x01, 0x02, 0xCA, 0xFE]);
        assert_eq!(
            answer_cookie_request(4, &body, &jar).unwrap().into_bytes(),
            want
        );
        assert!(answer_cookie_request(4, &[], &jar).is_err());
    }
}
