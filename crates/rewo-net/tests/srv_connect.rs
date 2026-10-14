//! `Connection::connect_address`: which endpoint a typed address reaches, and
//! what the handshake will name. The DNS side is a test double (the packet
//! codec has its own captured-byte tests in `srv.rs`); the sockets are real.
//!
//! Needs the 26.2 datagen report to build a `GameData` for `Connection`;
//! without it the test says so rather than passing silently.

use std::net::TcpListener;

use rewo_net::srv::{Endpoint, ServerAddress, SrvTarget};
use rewo_net::Connection;

fn load_data() -> Option<rewo_data::GameData> {
    match rewo_data::GameData::load_for_version("26.2") {
        Ok(d) => Some(d),
        Err(e) => {
            rewo_data::skip_test!("no 26.2 datagen report ({e})");
            None
        }
    }
}

/// A portless address follows the SRV target, and the connection reports the
/// target (trailing dot and all) as what the handshake should name.
///
/// MUTATION: reporting the typed host instead of the target from `endpoint()`
/// — a server matching forced hosts by the handshake name would see the wrong
/// one.
#[test]
fn a_portless_address_connects_to_the_srv_target_and_reports_it() {
    let Some(data) = load_data() else { return };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = std::thread::spawn(move || listener.accept().is_ok());

    let addr = ServerAddress::parse("play.rewo-test.invalid").unwrap();
    assert_eq!(addr.port, None);
    let conn = Connection::connect_address_with(&addr, &data, &|host| {
        assert_eq!(
            host, "play.rewo-test.invalid",
            "the lookup gets the typed host"
        );
        Some(SrvTarget {
            host: "localhost.".into(),
            port,
        })
    })
    .expect("the redirect target is reachable");
    assert_eq!(conn.endpoint(), ("localhost.", port));
    assert!(accepted.join().unwrap(), "the listener saw the connection");
}

/// A redirect that cannot be reached falls back to the typed host: here the
/// first endpoint refuses the connection (a port that was just closed) and the
/// second is a live listener.
#[test]
fn an_unreachable_redirect_falls_back_to_the_next_endpoint() {
    let Some(data) = load_data() else { return };
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = std::thread::spawn(move || listener.accept().is_ok());

    let conn = Connection::connect_endpoints(
        &[
            Endpoint {
                host: "127.0.0.1".into(),
                port: dead,
                via_srv: true,
            },
            Endpoint {
                host: "127.0.0.1".into(),
                port,
                via_srv: false,
            },
        ],
        &data,
    )
    .expect("the second endpoint answers");
    assert_eq!(conn.endpoint(), ("127.0.0.1", port));
    assert!(accepted.join().unwrap());
}

/// An explicit port never consults the lookup (the closure would panic) and
/// connects to exactly that port.
#[test]
fn an_explicit_port_is_never_redirected() {
    let Some(data) = load_data() else { return };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = std::thread::spawn(move || listener.accept().is_ok());

    let addr = ServerAddress::explicit("127.0.0.1", port);
    let conn = Connection::connect_address_with(&addr, &data, &|_| {
        panic!("an explicit port must not be looked up")
    })
    .unwrap();
    assert_eq!(conn.endpoint(), ("127.0.0.1", port));
    assert!(accepted.join().unwrap());
}

/// No endpoint answering is an error naming the last attempt, not a hang.
#[test]
fn when_nothing_answers_the_error_names_the_last_attempt() {
    let Some(data) = load_data() else { return };
    // A port that was just closed: connection refused, immediately.
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let err = Connection::connect_endpoints(
        &[Endpoint {
            host: "127.0.0.1".into(),
            port,
            via_srv: false,
        }],
        &data,
    )
    .err()
    .expect("nothing is listening");
    assert!(err.contains("127.0.0.1"), "{err}");
}
