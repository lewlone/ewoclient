//! Mid-session reconfiguration (a proxy server switch) against a scripted
//! in-process server: play `start_configuration` → the client acknowledges,
//! answers the configuration phase through the shared handler, and returns to
//! play on `finish_configuration`.
//!
//! Needs the 26.2 datagen report for packet ids. Without it the test cannot
//! build a `GameData` and says so rather than passing silently.

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use rewo_net::ids::Ids;
use rewo_net::play::PumpBudget;
use rewo_net::Connection;
use rewo_proto::frame::FrameCodec;
use rewo_proto::reader::PacketReader;
use rewo_proto::writer::PacketWriter;

struct Server {
    stream: TcpStream,
    codec: FrameCodec,
}

impl Server {
    fn send(&mut self, p: PacketWriter) {
        self.codec.write_frame(&mut self.stream, &p.buf).unwrap();
        self.stream.flush().unwrap();
    }

    /// Next client packet as (id, body).
    fn recv(&mut self) -> (i32, Vec<u8>) {
        let (mut scratch, mut out) = (Vec::new(), Vec::new());
        self.codec
            .read_frame(&mut self.stream, &mut scratch, &mut out)
            .unwrap();
        let mut pos = 0;
        let id = rewo_proto::varint::read_varint(&out, &mut pos).unwrap();
        (id, out[pos..].to_vec())
    }

    /// Read client packets until one with `id`, returning its body.
    fn expect(&mut self, id: i32, what: &str) -> Vec<u8> {
        for _ in 0..64 {
            let (got, body) = self.recv();
            if got == id {
                return body;
            }
        }
        panic!("client never sent {what}");
    }
}

fn load_data() -> Option<rewo_data::GameData> {
    match rewo_data::GameData::load_for_version("26.2") {
        Ok(d) => Some(d),
        Err(e) => {
            rewo_data::skip_test!("no 26.2 datagen report ({e})");
            None
        }
    }
}

#[test]
fn start_configuration_round_trips_back_to_play() {
    let Some(data) = load_data() else { return };
    let ids = Ids::resolve(&data.packets).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let sids = Ids::resolve(&data.packets).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut s = Server {
            stream,
            codec: FrameCodec::default(),
        };
        let ids = sids;
        // Handshake + login.
        s.expect(ids.sb_handshake_intention, "intention");
        s.expect(ids.sb_login_hello, "hello");
        let mut fin = PacketWriter::packet(ids.cb_login_finished);
        fin.uuid(1).string("Rewo").varint(0);
        s.send(fin);
        s.expect(ids.sb_login_acknowledged, "login_acknowledged");
        // First configuration: finish straight away.
        s.send(PacketWriter::packet(ids.cb_config_finish));
        s.expect(ids.sb_config_finish, "finish_configuration ack");

        // Play: a keep-alive, then the switch back to configuration.
        let mut ka = PacketWriter::packet(ids.cb_play_keep_alive);
        ka.i64(7);
        s.send(ka);
        let body = s.expect(ids.sb_play_keep_alive, "play keep_alive reply");
        assert_eq!(PacketReader::new(&body).i64().unwrap(), 7);
        s.send(PacketWriter::packet(
            ids.cb_play_start_configuration.unwrap(),
        ));
        s.expect(
            ids.sb_play_config_acknowledged,
            "configuration_acknowledged",
        );

        // Reconfiguration: a config keep-alive and ping, then finish.
        let mut ka = PacketWriter::packet(ids.cb_config_keep_alive);
        ka.i64(42);
        s.send(ka);
        let body = s.expect(ids.sb_config_keep_alive, "config keep_alive reply");
        assert_eq!(PacketReader::new(&body).i64().unwrap(), 42);
        let mut ping = PacketWriter::packet(ids.cb_config_ping);
        ping.i32(-3);
        s.send(ping);
        let body = s.expect(ids.sb_config_pong, "config pong");
        assert_eq!(PacketReader::new(&body).i32().unwrap(), -3);
        s.send(PacketWriter::packet(ids.cb_config_finish));
        s.expect(ids.sb_config_finish, "second finish_configuration ack");

        // Back in play: ids are play ids again.
        let mut ka = PacketWriter::packet(ids.cb_play_keep_alive);
        ka.i64(9);
        s.send(ka);
        let body = s.expect(ids.sb_play_keep_alive, "post-reconfig keep_alive reply");
        assert_eq!(PacketReader::new(&body).i64().unwrap(), 9);
    });

    let conn = Connection::connect("127.0.0.1", port, &data).unwrap();
    let mut session = conn
        .into_play(
            "127.0.0.1",
            port,
            "Rewo",
            None,
            Vec::new(),
            data.blocks.global_palette_bits,
            rewo_world::biome::Colormaps::neutral(),
        )
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut saw_config = false;
    while !server.is_finished() && Instant::now() < deadline {
        session.pump(PumpBudget::UNLIMITED).unwrap();
        assert!(session.disconnect.is_none(), "{:?}", session.disconnect);
        saw_config |= session.is_reconfiguring();
        std::thread::sleep(Duration::from_millis(2));
    }
    server.join().expect("server script failed");
    session.pump(PumpBudget::UNLIMITED).unwrap();
    assert!(
        saw_config,
        "the client never entered the configuration sub-state"
    );
    assert!(!session.is_reconfiguring());
    assert_eq!(session.reconfigurations, 1);
    let _ = ids;
}

#[test]
fn a_bad_frame_disconnects_with_its_reason() {
    let Some(data) = load_data() else { return };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let sids = Ids::resolve(&data.packets).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut s = Server {
            stream,
            codec: FrameCodec::default(),
        };
        let ids = sids;
        s.expect(ids.sb_login_hello, "hello");
        let mut fin = PacketWriter::packet(ids.cb_login_finished);
        fin.uuid(1).string("Rewo").varint(0);
        s.send(fin);
        s.expect(ids.sb_login_acknowledged, "login_acknowledged");
        s.send(PacketWriter::packet(ids.cb_config_finish));
        s.expect(ids.sb_config_finish, "finish_configuration ack");
        // A frame length past the 21-bit protocol limit.
        s.stream.write_all(&[0xff, 0xff, 0xff, 0x0f]).unwrap();
        s.stream.flush().unwrap();
        std::thread::sleep(Duration::from_millis(500));
    });
    let conn = Connection::connect("127.0.0.1", port, &data).unwrap();
    let mut session = conn
        .into_play(
            "127.0.0.1",
            port,
            "Rewo",
            None,
            Vec::new(),
            data.blocks.global_palette_bits,
            rewo_world::biome::Colormaps::neutral(),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while session.disconnect.is_none() && Instant::now() < deadline {
        session.pump(PumpBudget::UNLIMITED).unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    server.join().unwrap();
    let reason = session.disconnect.clone().expect("no disconnect");
    assert!(reason.contains("frame length"), "{reason}");
    assert_eq!(
        session.disconnect_cause,
        Some(rewo_world::disconnect_screen::DisconnectCause::ClientError)
    );
}

#[test]
fn pump_stops_at_its_packet_budget() {
    let Some(data) = load_data() else { return };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let sids = Ids::resolve(&data.packets).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut s = Server {
            stream,
            codec: FrameCodec::default(),
        };
        let ids = sids;
        s.expect(ids.sb_login_hello, "hello");
        let mut fin = PacketWriter::packet(ids.cb_login_finished);
        fin.uuid(1).string("Rewo").varint(0);
        s.send(fin);
        s.expect(ids.sb_login_acknowledged, "login_acknowledged");
        s.send(PacketWriter::packet(ids.cb_config_finish));
        s.expect(ids.sb_config_finish, "finish_configuration ack");
        for i in 0..10 {
            let mut ping = PacketWriter::packet(ids.cb_play_ping);
            ping.i32(i);
            s.send(ping);
        }
        for i in 0..10 {
            let body = s.expect(ids.sb_play_pong, "pong");
            assert_eq!(PacketReader::new(&body).i32().unwrap(), i);
        }
    });
    let conn = Connection::connect("127.0.0.1", port, &data).unwrap();
    let mut session = conn
        .into_play(
            "127.0.0.1",
            port,
            "Rewo",
            None,
            Vec::new(),
            data.blocks.global_palette_bits,
            rewo_world::biome::Colormaps::neutral(),
        )
        .unwrap();
    let budget = PumpBudget {
        max_packets: 3,
        max_time: Duration::from_secs(1),
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut total = 0;
    while total < 10 && Instant::now() < deadline {
        let n = session.pump(budget).unwrap();
        assert!(n <= 3, "pump applied {n} packets over a budget of 3");
        total += n;
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(total, 10);
    server.join().unwrap();
}

/// The login packets `ClientHandshakePacketListenerImpl` answers beyond the
/// encryption handshake, end to end through `Connection::login`:
///
/// - `login_compression` 64 then **-1**: `Connection.setupCompression` removes
///   compression for a negative threshold (`Connection.java:544-566`), so the
///   client's next frames must be plain. Kept as `Some(-1)` they were not.
/// - `custom_query`: answered `ServerboundCustomQueryAnswerPacket(id, null)`
///   (`ClientHandshakePacketListenerImpl.java:232-235`) instead of dropped.
/// - `cookie_request`: answered from the (empty, fresh-login) jar
///   (`:242-244`).
#[test]
fn login_answers_queries_and_cookies_and_honours_a_negative_threshold() {
    let Some(data) = load_data() else { return };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let sids = Ids::resolve(&data.packets).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut s = Server {
            stream,
            codec: FrameCodec::default(),
        };
        let ids = sids;
        // Both are id 0 in their states, so read them in order rather than
        // let `expect(hello)` stop at the handshake.
        let (id, _) = s.recv();
        assert_eq!(id, ids.sb_handshake_intention);
        let (id, _) = s.recv();
        assert_eq!(id, ids.sb_login_hello);

        let mut c = PacketWriter::packet(ids.cb_login_compression);
        c.varint(64);
        s.send(c);
        s.codec.compression_threshold = Some(64);
        let mut c = PacketWriter::packet(ids.cb_login_compression);
        c.varint(-1);
        s.send(c); // still framed compressed: the switch is after the packet
        s.codec.compression_threshold = None;

        let mut q = PacketWriter::packet(ids.cb_login_custom_query);
        q.varint(300).string("velocity:player_info").raw(&[1, 2, 3]);
        s.send(q);
        // Read with PLAIN framing: a client still compressing would prefix a
        // data-length VarInt and this would not be the answer's id.
        let (id, body) = s.recv();
        assert_eq!(id, ids.sb_login_custom_query_answer, "custom_query answer");
        assert_eq!(body, [0xAC, 0x02, 0x00], "VarInt 300, then null payload");

        let mut q = PacketWriter::packet(ids.cb_login_cookie_request.unwrap());
        q.string("minecraft:session");
        s.send(q);
        let (id, body) = s.recv();
        assert_eq!(id, ids.sb_login_cookie_response, "cookie_response");
        let mut want = vec![17u8];
        want.extend_from_slice(b"minecraft:session");
        want.push(0); // no cookie stored
        assert_eq!(body, want);

        let mut fin = PacketWriter::packet(ids.cb_login_finished);
        fin.uuid(1).string("Rewo").varint(0);
        s.send(fin);
        s.expect(ids.sb_login_acknowledged, "login_acknowledged");
    });
    let mut conn = Connection::connect("127.0.0.1", port, &data).unwrap();
    conn.login_offline("127.0.0.1", port, "Rewo").unwrap();
    server.join().expect("server script failed");
}

/// Every malformed body the 2026-10 audit found dropped or half-applied
/// *without* a count now reaches `PlaySession::decode_failures`, once each,
/// and the all-or-nothing ones apply nothing (vanilla decodes a packet whole
/// before its handler runs). One configuration-phase `registry_data` fault is
/// carried into the session's counter at `into_play`.
#[test]
fn malformed_bodies_are_counted_once_each() {
    let Some(data) = load_data() else { return };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let sids = Ids::resolve(&data.packets).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut s = Server {
            stream,
            codec: FrameCodec::default(),
        };
        let ids = sids;
        s.expect(ids.sb_login_hello, "hello");
        let mut fin = PacketWriter::packet(ids.cb_login_finished);
        fin.uuid(1).string("Rewo").varint(0);
        s.send(fin);
        s.expect(ids.sb_login_acknowledged, "login_acknowledged");

        // registry_data claiming two entries and carrying one.
        let mut reg = PacketWriter::packet(ids.cb_config_registry_data);
        reg.string("minecraft:painting_variant").varint(2);
        reg.string("minecraft:kebab").bool(false);
        s.send(reg);
        s.send(PacketWriter::packet(ids.cb_config_finish));
        s.expect(ids.sb_config_finish, "finish_configuration ack");

        let bodies: Vec<(i32, Vec<u8>)> = vec![
            (ids.cb_play_update_mob_effect, vec![]),
            (ids.cb_play_remove_mob_effect, vec![]),
            // health present, food and saturation missing
            (
                ids.cb_play_set_health.unwrap(),
                5.0f32.to_be_bytes().to_vec(),
            ),
            // no chunk coords at all
            (ids.cb_play_light_update.unwrap(), vec![]),
            // one entry promised: a ChunkPos, then no biome byte array
            (ids.cb_play_chunks_biomes.unwrap(), {
                let mut b = vec![1];
                b.extend_from_slice(&0i64.to_be_bytes());
                b
            }),
            // ADD_PLAYER mask, one entry, a uuid cut short
            (ids.cb_play_player_info_update, vec![0x01, 0x01, 0, 0, 0]),
            // a section, two changes promised, one sent
            (ids.cb_play_section_blocks_update, {
                let mut b = 0u64.to_be_bytes().to_vec();
                b.extend_from_slice(&[2, 0x01]);
                b
            }),
            // game time, one clock promised, its holder only
            (ids.cb_play_set_time, {
                let mut b = 1234i64.to_be_bytes().to_vec();
                b.extend_from_slice(&[1, 0]);
                b
            }),
            // two entity ids promised, one sent
            (ids.cb_play_remove_entities, vec![2, 7]),
            // two UUIDs promised, one sent
            (ids.cb_play_player_info_remove, {
                let mut b = vec![2];
                b.extend_from_slice(&[0xAB; 16]);
                b
            }),
        ];
        for (id, body) in bodies {
            let mut p = PacketWriter::packet(id);
            p.raw(&body);
            s.send(p);
        }
        // A keep-alive after them: its reply proves the ten were handled.
        let mut ka = PacketWriter::packet(ids.cb_play_keep_alive);
        ka.i64(77);
        s.send(ka);
        let body = s.expect(ids.sb_play_keep_alive, "keep_alive reply");
        assert_eq!(PacketReader::new(&body).i64().unwrap(), 77);
    });

    let conn = Connection::connect("127.0.0.1", port, &data).unwrap();
    let mut session = conn
        .into_play(
            "127.0.0.1",
            port,
            "Rewo",
            None,
            Vec::new(),
            data.blocks.global_palette_bits,
            rewo_world::biome::Colormaps::neutral(),
        )
        .unwrap();
    // The configuration-phase fault, carried over at `into_play`.
    assert_eq!(session.decode_failures, 1, "registry_data");
    let (health, food) = (session.health, session.food);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !server.is_finished() && Instant::now() < deadline {
        session.pump(PumpBudget::UNLIMITED).unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    server.join().expect("server script failed");
    session.pump(PumpBudget::UNLIMITED).unwrap();
    // (The scripted server has hung up by now; that is not a failure.)
    assert_eq!(session.decode_failures, 1 + 10);
    // `set_health` without its food half applied nothing.
    assert_eq!((session.health, session.food), (health, food));
}
