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
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut s = Server { stream, codec: FrameCodec::default() };
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
        s.send(PacketWriter::packet(ids.cb_play_start_configuration.unwrap()));
        s.expect(ids.sb_play_config_acknowledged, "configuration_acknowledged");

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
    assert!(saw_config, "the client never entered the configuration sub-state");
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
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut s = Server { stream, codec: FrameCodec::default() };
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
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut s = Server { stream, codec: FrameCodec::default() };
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
    let budget = PumpBudget { max_packets: 3, max_time: Duration::from_secs(1) };
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
