//! `_minecraft._tcp.<host>` SRV redirects — how a server address typed
//! without a port reaches the real host and port (vanilla's
//! `ServerNameResolver` / `ServerRedirectHandler`).
//!
//! # What vanilla does
//!
//! ```java
//! // ServerAddress.parseString: HostAndPort.fromString(input).withDefaultPort(25565)
//! // ServerRedirectHandler.createDnsSrvRedirectHandler:
//! if (originalAddress.getPort() == 25565) {
//!    Attributes attributes = context.getAttributes("_minecraft._tcp." + originalAddress.getHost(), new String[]{"SRV"});
//!    Attribute srv = attributes.get("srv");
//!    if (srv != null) {
//!       String[] arguments = srv.get().toString().split(" ", 4);        // prio weight port target
//!       return Optional.of(new ServerAddress(arguments[3], ServerAddress.parsePort(arguments[2])));
//!    }
//! }   // any Throwable: no redirect
//! // ServerNameResolver.resolveAddress: the redirect REPLACES the address
//! // ConnectScreen then sends `address.getHostName()` / `getPort()` in the handshake.
//! ```
//!
//! Three details that decide behaviour:
//!
//! * **The trigger is the port.** An address with no port gets the default
//!   25565, and only 25565 is looked up. Rewo narrows that to what the player
//!   typed: a port that was *written* (even `:25565`) is never redirected
//!   ([`ServerAddress::port`] is an `Option`, so the difference survives
//!   parsing). An IP literal has no SRV name and is never looked up.
//! * **The target keeps the record's trailing dot.** JNDI returns the target as
//!   `mc.example.com.`, and the handshake carries it verbatim, so a server
//!   matching forced hosts sees the dotted name. [`SrvTarget::host`] is the
//!   target exactly as the wire spelled it, trailing dot included.
//! * **Any failure is "no redirect".** Rewo falls back to the A/AAAA lookup of
//!   the typed host on port 25565, which is also what vanilla ends up with when
//!   the lookup throws. (Vanilla fails the connection when a redirect *is*
//!   found but its target does not resolve; Rewo tries the typed host as well,
//!   which is the more forgiving of the two.)
//!
//! # Why this file speaks DNS itself
//!
//! The std resolver answers A/AAAA only, and a sync SRV query needs a resolver
//! library with either an async runtime (forbidden by the architecture) or a
//! dependency this client does not otherwise need. One UDP question and one
//! answer is small enough to write, test over captured packets, and bound: one
//! second per nameserver for the datagram, three nameservers at most, and a
//! truncated answer (the `TC` bit) is asked again over TCP to the same
//! nameserver, as the JDK's JNDI resolver does (RFC 7766: a 2-byte big-endian
//! length before each message). The whole lookup shares one deadline of
//! [`MAX_NAMESERVERS`] x the per-nameserver timeout (3 s by default), TCP
//! included, so a stalling nameserver costs the connect screen at most that.
//! The nameservers are the system's own (`/etc/resolv.conf`;
//! `GetAdaptersAddresses` and `GetNetworkParams` on Windows) — never a public
//! resolver Rewo picked.
//!
//! # Untrusted input
//!
//! The answer comes off the network. Names are decoded with a jump limit and a
//! length cap, the target must be plain hostname characters, a port of zero or
//! a root target (`.`, "service not available", RFC 2782) is skipped, only
//! answers whose owner is the question name (or a CNAME alias of it) count, and
//! a response whose id, question or source is not ours is ignored.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

/// `ServerAddress.parseString`'s default.
pub const DEFAULT_PORT: u16 = 25565;

/// Per-nameserver wait for an answer: the UDP datagram's, and separately the
/// TCP retry's (connect, request and response together). The lookup as a whole
/// never exceeds [`MAX_NAMESERVERS`] times this.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(1);
/// Nameservers asked, in order, before giving up.
pub const MAX_NAMESERVERS: usize = 3;

const TYPE_CNAME: u16 = 5;
const TYPE_SRV: u16 = 33;
const CLASS_IN: u16 = 1;

/// A server address as the player typed it: a host and, maybe, a port.
///
/// `HostAndPort.fromString`'s shapes: `host`, `host:port`, `[v6]`, `[v6]:port`,
/// and a bare IPv6 literal (two or more colons, taken whole as the host).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerAddress {
    pub host: String,
    /// `None` when the input carried no port — which is what makes the SRV
    /// lookup eligible.
    pub port: Option<u16>,
}

impl ServerAddress {
    /// `HostAndPort.fromString`, minus Guava's exceptions: `None` for an empty
    /// host, an unparseable or out-of-range port, or an unbalanced bracket.
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        let (host, port_text): (&str, Option<&str>) = if let Some(rest) = input.strip_prefix('[') {
            let (host, after) = rest.split_once(']')?;
            match after {
                "" => (host, None),
                _ => (host, Some(after.strip_prefix(':')?)),
            }
        } else {
            match input.matches(':').count() {
                0 => (input, None),
                1 => {
                    let (h, p) = input.split_once(':')?;
                    (h, Some(p))
                }
                // A bare IPv6 literal: Guava treats it as a host with no port.
                _ => (input, None),
            }
        };
        if host.is_empty() {
            return None;
        }
        let port = match port_text {
            None | Some("") => None,
            // Guava: digits only, 0..=65535.
            Some(p) if p.bytes().all(|b| b.is_ascii_digit()) => Some(p.parse::<u16>().ok()?),
            Some(_) => return None,
        };
        Some(Self {
            host: host.to_owned(),
            port,
        })
    }

    /// A host with the port already decided — never redirected.
    pub fn explicit(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port: Some(port),
        }
    }

    /// The port the connection uses absent a redirect.
    pub fn port_or_default(&self) -> u16 {
        self.port.unwrap_or(DEFAULT_PORT)
    }
}

/// One redirect: where the SRV record sent us.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SrvTarget {
    /// The record's target, spelled as on the wire with a trailing dot.
    pub host: String,
    pub port: u16,
}

/// Where to connect and what to put in the handshake: the typed host first or
/// the redirect, in the order to try them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    /// Whether this came from an SRV record.
    pub via_srv: bool,
}

/// The ordered endpoints for `addr`: the SRV redirect (when the address is
/// eligible and `lookup` finds one) followed by the typed host on its port.
///
/// `lookup` is handed the host and returns the redirect, if any. It is a
/// parameter so the decision is testable without a network.
pub fn plan(addr: &ServerAddress, lookup: &dyn Fn(&str) -> Option<SrvTarget>) -> Vec<Endpoint> {
    let mut out = Vec::new();
    if addr.port.is_none() && is_srv_eligible(&addr.host) {
        if let Some(t) = lookup(&addr.host) {
            out.push(Endpoint {
                host: t.host,
                port: t.port,
                via_srv: true,
            });
        }
    }
    out.push(Endpoint {
        host: addr.host.clone(),
        port: addr.port_or_default(),
        via_srv: false,
    });
    out
}

/// A DNS name that can have an SRV record: not an IP literal (those have no
/// `_minecraft._tcp.` name).
pub fn is_srv_eligible(host: &str) -> bool {
    host.parse::<IpAddr>().is_err() && !host.contains(':')
}

// ---------------------------------------------------------------------------
// The wire.
// ---------------------------------------------------------------------------

/// Why a response was not usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SrvError {
    /// Shorter than its own structure says, or a name that never ends.
    Malformed,
    /// Not the answer to our question (id, flags or question mismatch).
    NotOurs,
    /// The `TC` bit: the answer did not fit in a UDP datagram. The transport
    /// asks again over TCP; a TCP answer that still says so is a failure.
    Truncated,
    /// A nonzero RCODE (SERVFAIL, REFUSED, ...). NXDOMAIN is not an error
    /// here: it is a definitive "no records" and parses as an empty list.
    ServerFailure(u8),
}

/// `_minecraft._tcp.<host>` as a DNS question name, or `None` when it cannot be
/// encoded (a non-ASCII host, an empty label, a label over 63 bytes, a name over
/// 253).
pub fn srv_name(host: &str) -> Option<String> {
    let host = host.trim_end_matches('.');
    if host.is_empty() || !host.is_ascii() {
        return None;
    }
    let name = format!("_minecraft._tcp.{host}");
    (name.len() <= 253 && name.split('.').all(|l| !l.is_empty() && l.len() <= 63)).then_some(name)
}

/// One SRV question for `name`, recursion desired.
pub fn build_query(id: u16, name: &str) -> Option<Vec<u8>> {
    let mut q = Vec::with_capacity(18 + name.len());
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&0x0100u16.to_be_bytes()); // RD
    q.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]); // QDCOUNT 1, the rest 0
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 || !label.is_ascii() {
            return None;
        }
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&TYPE_SRV.to_be_bytes());
    q.extend_from_slice(&CLASS_IN.to_be_bytes());
    Some(q)
}

/// A decoded SRV record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SrvRecord {
    pub priority: u16,
    pub weight: u16,
    pub port: u16,
    /// Dotted, with the trailing dot.
    pub target: String,
}

/// Read a (possibly compressed) name at `*pos`, advancing `*pos` past the name
/// *as it appears at that position* (a pointer ends it after two bytes).
/// Returns the labels joined by dots, lower-cased, with a trailing dot (the
/// root alone is `"."`).
fn read_name(buf: &[u8], pos: &mut usize) -> Option<String> {
    let mut out = String::new();
    let mut cursor = *pos;
    let mut jumped = false;
    let mut jumps = 0;
    loop {
        let len = *buf.get(cursor)? as usize;
        match len & 0xC0 {
            0x00 => {
                cursor += 1;
                if len == 0 {
                    if !jumped {
                        *pos = cursor;
                    }
                    if out.is_empty() {
                        out.push('.');
                    }
                    return Some(out);
                }
                let label = buf.get(cursor..cursor + len)?;
                // Hostname characters only: the target is handed to the OS
                // resolver and echoed into the handshake.
                if !label
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
                {
                    return None;
                }
                out.push_str(&String::from_utf8_lossy(label).to_ascii_lowercase());
                out.push('.');
                if out.len() > 254 {
                    return None;
                }
                cursor += len;
            }
            0xC0 => {
                let lo = *buf.get(cursor + 1)? as usize;
                let target = ((len & 0x3F) << 8) | lo;
                if !jumped {
                    *pos = cursor + 2;
                    jumped = true;
                }
                jumps += 1;
                // A pointer must go backwards, and there is a hard cap: either
                // alone stops a loop.
                if jumps > 16 || target >= cursor {
                    return None;
                }
                cursor = target;
            }
            // 0x40 / 0x80 are reserved label types.
            _ => return None,
        }
    }
}

fn be16(buf: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*buf.get(at)?, *buf.get(at + 1)?]))
}

/// Decode `response` as the answer to `build_query(id, name)`.
///
/// Returns every SRV record whose owner is the question name or reachable from
/// it through CNAMEs in the answer section. An answer with no such records —
/// NXDOMAIN, NOERROR/NODATA — is `Ok(vec![])`.
pub fn parse_response(id: u16, name: &str, response: &[u8]) -> Result<Vec<SrvRecord>, SrvError> {
    if response.len() < 12 {
        return Err(SrvError::Malformed);
    }
    let flags = be16(response, 2).ok_or(SrvError::Malformed)?;
    if be16(response, 0) != Some(id) || flags & 0x8000 == 0 || (flags >> 11) & 0xF != 0 {
        return Err(SrvError::NotOurs);
    }
    if flags & 0x0200 != 0 {
        return Err(SrvError::Truncated);
    }
    let rcode = (flags & 0xF) as u8;
    // NXDOMAIN (3) is a definitive empty answer; anything else nonzero is a
    // server failure worth trying the next nameserver for.
    if rcode != 0 && rcode != 3 {
        return Err(SrvError::ServerFailure(rcode));
    }
    let qd = be16(response, 4).ok_or(SrvError::Malformed)?;
    let an = be16(response, 6).ok_or(SrvError::Malformed)?;

    let mut pos = 12;
    let want = format!("{}.", name.trim_end_matches('.').to_ascii_lowercase());
    for i in 0..qd {
        let qname = read_name(response, &mut pos).ok_or(SrvError::Malformed)?;
        let qtype = be16(response, pos).ok_or(SrvError::Malformed)?;
        let qclass = be16(response, pos + 2).ok_or(SrvError::Malformed)?;
        pos += 4;
        if i == 0 && (qname != want || qtype != TYPE_SRV || qclass != CLASS_IN) {
            return Err(SrvError::NotOurs);
        }
    }
    if qd != 1 {
        return Err(SrvError::NotOurs);
    }

    // (owner, type, rdata start, rdata end)
    let mut answers: Vec<(String, u16, usize, usize)> = Vec::new();
    for _ in 0..an {
        let owner = read_name(response, &mut pos).ok_or(SrvError::Malformed)?;
        let rtype = be16(response, pos).ok_or(SrvError::Malformed)?;
        let rclass = be16(response, pos + 2).ok_or(SrvError::Malformed)?;
        let rdlen = be16(response, pos + 8).ok_or(SrvError::Malformed)? as usize;
        let start = pos + 10;
        let end = start.checked_add(rdlen).ok_or(SrvError::Malformed)?;
        if end > response.len() {
            return Err(SrvError::Malformed);
        }
        if rclass == CLASS_IN {
            answers.push((owner, rtype, start, end));
        }
        pos = end;
    }

    // The names the answer may be about: the question, plus CNAME aliases of
    // it. Iterate to a fixpoint so answer order does not matter.
    let mut owners = vec![want];
    loop {
        let before = owners.len();
        for (owner, rtype, start, _) in &answers {
            if *rtype == TYPE_CNAME && owners.contains(owner) {
                let mut at = *start;
                if let Some(target) = read_name(response, &mut at) {
                    if !owners.contains(&target) {
                        owners.push(target);
                    }
                }
            }
        }
        if owners.len() == before {
            break;
        }
    }

    let mut records = Vec::new();
    for (owner, rtype, start, end) in &answers {
        if *rtype != TYPE_SRV || !owners.contains(owner) || end - start < 7 {
            continue;
        }
        let priority = be16(response, *start).ok_or(SrvError::Malformed)?;
        let weight = be16(response, start + 2).ok_or(SrvError::Malformed)?;
        let port = be16(response, start + 4).ok_or(SrvError::Malformed)?;
        let mut at = start + 6;
        let Some(target) = read_name(response, &mut at) else {
            continue; // a target with odd characters: skip the record, keep the rest
        };
        records.push(SrvRecord {
            priority,
            weight,
            port,
            target,
        });
    }
    Ok(records)
}

/// Pick the record to use: the lowest priority, then the highest weight, then
/// the first. Skips a port of zero and the root target `.` ("this service is
/// decidedly not available", RFC 2782).
///
/// Vanilla takes whichever record JNDI lists first; RFC 2782's order is
/// deterministic here so a multi-record zone gives one answer.
pub fn pick(records: &[SrvRecord]) -> Option<SrvTarget> {
    records
        .iter()
        .filter(|r| r.port != 0 && r.target != ".")
        .min_by_key(|r| (r.priority, std::cmp::Reverse(r.weight)))
        .map(|r| SrvTarget {
            host: r.target.clone(),
            port: r.port,
        })
}

// ---------------------------------------------------------------------------
// The transport.
// ---------------------------------------------------------------------------

/// Ask `servers` (in order, at most [`MAX_NAMESERVERS`]) for the SRV records of
/// `_minecraft._tcp.<host>`.
///
/// `timeout` is each nameserver's wait, for its UDP datagram and again for a
/// TCP retry; the whole call ends by `timeout x` the number of nameservers
/// asked (at most [`MAX_NAMESERVERS`]).
///
/// `None` is "no redirect" whatever the reason — no nameserver, a timeout, a
/// refusal, a malformed answer, or no record. The reason is logged at debug.
pub fn lookup_with(host: &str, servers: &[SocketAddr], timeout: Duration) -> Option<SrvTarget> {
    let name = srv_name(host)?;
    let asked = servers.len().min(MAX_NAMESERVERS);
    let overall = Instant::now() + timeout * asked as u32;
    for server in servers.iter().take(MAX_NAMESERVERS) {
        if Instant::now() >= overall {
            break;
        }
        match query_one(*server, &name, timeout, overall) {
            Ok(records) => return pick(&records),
            Err(e) => log::debug!("srv: {server}: {e}"),
        }
    }
    None
}

/// `timeout` from now, but never past `overall`.
fn bounded(timeout: Duration, overall: Instant) -> Instant {
    (Instant::now() + timeout).min(overall)
}

fn query_one(
    server: SocketAddr,
    name: &str,
    timeout: Duration,
    overall: Instant,
) -> Result<Vec<SrvRecord>, String> {
    let id: u16 = rand::random();
    let query = build_query(id, name).ok_or("unencodable name")?;
    match query_udp(server, id, name, &query, bounded(timeout, overall)) {
        // The answer did not fit a datagram: ask the same nameserver over TCP.
        Err(UdpFailure::Parsed(SrvError::Truncated)) => {
            log::debug!("srv: {server}: truncated over UDP, retrying over TCP");
            query_tcp(server, id, name, &query, bounded(timeout, overall))
        }
        Err(e) => Err(e.to_string()),
        Ok(records) => Ok(records),
    }
}

/// Why the UDP leg failed: a socket or timing failure, or an answer that was
/// received and refused.
enum UdpFailure {
    Io(String),
    Parsed(SrvError),
}

impl std::fmt::Display for UdpFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UdpFailure::Io(e) => f.write_str(e),
            UdpFailure::Parsed(e) => write!(f, "{e:?}"),
        }
    }
}

fn query_udp(
    server: SocketAddr,
    id: u16,
    name: &str,
    query: &[u8],
    deadline: Instant,
) -> Result<Vec<SrvRecord>, UdpFailure> {
    let io = |what: &str, e: std::io::Error| UdpFailure::Io(format!("{what}: {e}"));
    let bind: SocketAddr = if server.is_ipv4() {
        ([0, 0, 0, 0], 0).into()
    } else {
        ([0u16; 8], 0).into()
    };
    let sock = UdpSocket::bind(bind).map_err(|e| io("bind", e))?;
    // `connect` makes the kernel drop datagrams from any other source.
    sock.connect(server).map_err(|e| io("connect", e))?;
    sock.send(query).map_err(|e| io("send", e))?;
    let mut buf = [0u8; 1500];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(UdpFailure::Io("timed out".into()));
        }
        sock.set_read_timeout(Some(left))
            .map_err(|e| io("timeout", e))?;
        let n = match sock.recv(&mut buf) {
            Ok(n) => n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err(UdpFailure::Io("timed out".into()));
            }
            Err(e) => return Err(io("recv", e)),
        };
        match parse_response(id, name, &buf[..n]) {
            // Someone else's datagram (wrong id or question): keep waiting for
            // ours until the deadline.
            Err(SrvError::NotOurs) => continue,
            Err(e) => return Err(UdpFailure::Parsed(e)),
            Ok(records) => return Ok(records),
        }
    }
}

/// The request as DNS-over-TCP frames it (RFC 1035 4.2.2, RFC 7766): the
/// message behind a big-endian `u16` length.
fn tcp_frame(query: &[u8]) -> Option<Vec<u8>> {
    let len = u16::try_from(query.len()).ok()?;
    let mut out = Vec::with_capacity(2 + query.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(query);
    Some(out)
}

/// Read exactly `buf.len()` bytes before `deadline`: the socket's timeout is
/// re-armed with what is left before every read, so a peer that drips bytes
/// cannot stretch the wait past it.
fn read_exact_by(stream: &mut TcpStream, buf: &mut [u8], deadline: Instant) -> Result<(), String> {
    let mut got = 0;
    while got < buf.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("timed out".into());
        }
        stream
            .set_read_timeout(Some(left))
            .map_err(|e| format!("timeout: {e}"))?;
        match stream.read(&mut buf[got..]) {
            Ok(0) => return Err("tcp: closed before the whole answer".into()),
            Ok(n) => got += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err("timed out".into());
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("tcp recv: {e}")),
        }
    }
    Ok(())
}

/// The truncated answer's retry: connect to the same nameserver on TCP, send
/// the same question, read one length-prefixed answer. Everything, connect
/// included, is bounded by `deadline`.
fn query_tcp(
    server: SocketAddr,
    id: u16,
    name: &str,
    query: &[u8],
    deadline: Instant,
) -> Result<Vec<SrvRecord>, String> {
    let frame = tcp_frame(query).ok_or("tcp: question too long")?;
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err("timed out".into());
    }
    let mut stream =
        TcpStream::connect_timeout(&server, left).map_err(|e| format!("tcp connect: {e}"))?;
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err("timed out".into());
    }
    stream
        .set_write_timeout(Some(left))
        .map_err(|e| format!("timeout: {e}"))?;
    stream
        .write_all(&frame)
        .map_err(|e| format!("tcp send: {e}"))?;
    let mut len = [0u8; 2];
    read_exact_by(&mut stream, &mut len, deadline)?;
    // The length is a u16, so the answer is at most 64 KiB: bounded by the
    // wire format, no further cap needed.
    let mut answer = vec![0u8; u16::from_be_bytes(len) as usize];
    read_exact_by(&mut stream, &mut answer, deadline)?;
    parse_response(id, name, &answer).map_err(|e| format!("tcp answer: {e:?}"))
}

/// The system's configured nameservers, in preference order.
pub fn system_nameservers() -> Vec<SocketAddr> {
    #[cfg(windows)]
    {
        windows_nameservers()
    }
    #[cfg(not(windows))]
    {
        std::fs::read_to_string("/etc/resolv.conf")
            .map(|t| parse_resolv_conf(&t))
            .unwrap_or_default()
    }
}

/// `nameserver <ip>` lines of a resolv.conf. A scoped IPv6 address
/// (`fe80::1%eth0`) is skipped: a bare `UdpSocket` cannot name the scope.
pub fn parse_resolv_conf(text: &str) -> Vec<SocketAddr> {
    text.lines()
        .filter_map(|l| {
            let l = l.split('#').next()?.trim();
            let ip = l.strip_prefix("nameserver")?.trim();
            ip.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, 53))
        })
        .collect()
}

/// The SRV redirect for `host` through the system's nameservers: the lookup
/// `Connection::connect_address` hands to [`plan`].
pub fn lookup_system(host: &str) -> Option<SrvTarget> {
    lookup_with(host, &system_nameservers(), QUERY_TIMEOUT)
}

/// Windows' nameservers: what `GetNetworkParams` reports first (the system's
/// effective IPv4 list), then every operational adapter's DNS server,
/// IPv4 and IPv6, from `GetAdaptersAddresses`, that the first list lacks (see
/// [`merge_nameservers`]). Only [`MAX_NAMESERVERS`] are ever asked, so the
/// order decides which: the adapter list is in adapter order and includes
/// VPN, WSL and Hyper-V virtual adapters whose resolvers may be unreachable,
/// which must not push the working one out of the first three. The adapter
/// call still contributes IPv6 and anything the other omits.
#[cfg(windows)]
fn windows_nameservers() -> Vec<SocketAddr> {
    merge_nameservers(network_params_nameservers(), adapter_nameservers())
}

/// `effective` (the system's own list, in its order) followed by each of
/// `others` it does not already contain, order kept and no duplicates.
#[cfg(any(windows, test))]
fn merge_nameservers(effective: Vec<SocketAddr>, others: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let mut out = Vec::with_capacity(effective.len() + others.len());
    for server in effective.into_iter().chain(others) {
        if !out.contains(&server) {
            out.push(server);
        }
    }
    out
}

/// A DNS server as the OS reports it, from the raw bytes of its `SOCKADDR`
/// (little-endian Windows: `AF_INET` = 2, `AF_INET6` = 23), on port 53.
///
/// `None` for an unreadable or short address, the unspecified address, the
/// deprecated site-local `fec0::/10` (Windows lists `fec0:0:0:ffff::1` for an
/// adapter with no IPv6 DNS configured, and it never answers), and a
/// link-local IPv6 address with no scope id (it cannot be reached without
/// one). A link-local address WITH its scope id is kept, which is what lets a
/// router-advertised resolver work: std's `SocketAddrV6` carries the scope.
#[cfg(any(windows, test))]
fn nameserver_from_sockaddr(sa: &[u8]) -> Option<SocketAddr> {
    const AF_INET: u16 = 2;
    const AF_INET6: u16 = 23;
    let family = u16::from_ne_bytes([*sa.first()?, *sa.get(1)?]);
    match family {
        // sockaddr_in: family u16, port u16, addr [u8; 4], zero [u8; 8].
        AF_INET => {
            let a: [u8; 4] = sa.get(4..8)?.try_into().ok()?;
            let ip = std::net::Ipv4Addr::from(a);
            (!ip.is_unspecified()).then(|| SocketAddr::new(IpAddr::V4(ip), 53))
        }
        // sockaddr_in6: family u16, port u16, flowinfo u32, addr [u8; 16],
        // scope_id u32.
        AF_INET6 => {
            let a: [u8; 16] = sa.get(8..24)?.try_into().ok()?;
            let scope = u32::from_ne_bytes(sa.get(24..28)?.try_into().ok()?);
            let ip = std::net::Ipv6Addr::from(a);
            let top = ip.segments()[0];
            let link_local = top & 0xffc0 == 0xfe80;
            let site_local = top & 0xffc0 == 0xfec0;
            if ip.is_unspecified() || site_local || (link_local && scope == 0) {
                return None;
            }
            Some(SocketAddr::V6(std::net::SocketAddrV6::new(
                ip,
                53,
                0,
                if link_local { scope } else { 0 },
            )))
        }
        _ => None,
    }
}

/// The DNS servers of every operational, non-loopback adapter, IPv4 and IPv6,
/// from `GetAdaptersAddresses`. Empty when the call fails.
#[cfg(windows)]
fn adapter_nameservers() -> Vec<SocketAddr> {
    use std::ffi::c_void;

    /// `SOCKET_ADDRESS`.
    #[repr(C)]
    struct SocketAddress {
        sockaddr: *const u8,
        sockaddr_length: i32,
    }
    /// `IP_ADAPTER_DNS_SERVER_ADDRESS_XP`: the `Length`/`Reserved` pair is the
    /// header's anonymous union with a `ULONGLONG`, 8 bytes either way.
    #[allow(dead_code)]
    #[repr(C)]
    struct DnsServerAddress {
        length: u32,
        reserved: u32,
        next: *const DnsServerAddress,
        address: SocketAddress,
    }
    /// `IP_ADAPTER_ADDRESSES_LH` up to `OperStatus`, the last field read. It
    /// is a prefix of the OS's struct, only ever reached through the OS's own
    /// pointers, so the trailing fields it omits do not matter; `length` is
    /// the OS's own size for the entry, and the offsets below are checked.
    #[allow(dead_code)]
    #[repr(C)]
    struct AdapterAddresses {
        length: u32,
        if_index: u32,
        next: *const AdapterAddresses,
        adapter_name: *const u8,
        first_unicast: *const c_void,
        first_anycast: *const c_void,
        first_multicast: *const c_void,
        first_dns_server: *const DnsServerAddress,
        dns_suffix: *const u16,
        description: *const u16,
        friendly_name: *const u16,
        physical_address: [u8; 8],
        physical_address_length: u32,
        flags: u32,
        mtu: u32,
        if_type: u32,
        oper_status: u32,
    }
    // The offsets <iptypes.h> gives these structs: an 8-byte header, `Next`,
    // `AdapterName`, three address-list heads, then `FirstDnsServerAddress`,
    // and so on. A drifted declaration fails the build, not a read.
    #[cfg(target_pointer_width = "64")]
    const _: () = {
        assert!(std::mem::offset_of!(AdapterAddresses, next) == 8);
        assert!(std::mem::offset_of!(AdapterAddresses, first_dns_server) == 48);
        assert!(std::mem::offset_of!(AdapterAddresses, physical_address) == 80);
        assert!(std::mem::offset_of!(AdapterAddresses, oper_status) == 104);
        assert!(std::mem::offset_of!(DnsServerAddress, next) == 8);
        assert!(std::mem::offset_of!(DnsServerAddress, address) == 16);
    };
    #[cfg(target_pointer_width = "32")]
    const _: () = {
        assert!(std::mem::offset_of!(AdapterAddresses, next) == 8);
        assert!(std::mem::offset_of!(AdapterAddresses, first_dns_server) == 28);
        assert!(std::mem::offset_of!(AdapterAddresses, oper_status) == 68);
        assert!(std::mem::offset_of!(DnsServerAddress, next) == 8);
        assert!(std::mem::offset_of!(DnsServerAddress, address) == 12);
    };

    #[link(name = "iphlpapi")]
    extern "system" {
        fn GetAdaptersAddresses(
            family: u32,
            flags: u32,
            reserved: *mut c_void,
            addresses: *mut c_void,
            size: *mut u32,
        ) -> u32;
    }
    const AF_UNSPEC: u32 = 0;
    // SKIP_UNICAST | SKIP_ANYCAST | SKIP_MULTICAST | SKIP_FRIENDLY_NAME: only
    // the DNS server list is wanted.
    const FLAGS: u32 = 0x0001 | 0x0002 | 0x0004 | 0x0020;
    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    const NO_ERROR: u32 = 0;
    const IF_OPER_STATUS_UP: u32 = 1;
    const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;

    // Microsoft's advice is a 15 KiB first buffer; the list can grow between
    // the sizing call and the filling one, hence a few attempts.
    let mut len: u32 = 15 * 1024;
    let mut buf: Vec<u64> = Vec::new();
    let mut filled = false;
    for _ in 0..4 {
        // `u64` elements give the 8-byte alignment the structs' pointers need.
        buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: `buf` holds at least `len` writable bytes (rounded up to
        // eight) and `len` is the size passed; the other pointers are null as
        // documented. On `ERROR_BUFFER_OVERFLOW` `len` is the size required.
        let rc = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC,
                FLAGS,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if rc == NO_ERROR {
            filled = true;
            break;
        }
        if rc != ERROR_BUFFER_OVERFLOW {
            return Vec::new();
        }
    }
    if !filled {
        return Vec::new();
    }

    // Every pointer below is one the OS wrote into `buf`; before following
    // one, check that the `size` bytes it names lie inside `buf`, so a
    // garbled list can end the walk but never read outside the allocation.
    let base = buf.as_ptr() as usize;
    let end = base + buf.len() * 8;
    let inside = |p: *const u8, size: usize| {
        let at = p as usize;
        at >= base && at.checked_add(size).is_some_and(|e| e <= end)
    };

    let mut out: Vec<SocketAddr> = Vec::new();
    let mut adapter: *const AdapterAddresses = buf.as_ptr().cast();
    let mut adapters = 0;
    while !adapter.is_null() && adapters < 128 {
        if !inside(adapter.cast(), std::mem::size_of::<AdapterAddresses>()) {
            break;
        }
        // SAFETY: in bounds (checked), 8-aligned (the buffer is, and the OS
        // lays entries out aligned), initialised by the call; read-only.
        let a = unsafe { &*adapter };
        if a.oper_status == IF_OPER_STATUS_UP && a.if_type != IF_TYPE_SOFTWARE_LOOPBACK {
            let mut dns = a.first_dns_server;
            let mut hops = 0;
            while !dns.is_null() && hops < 16 {
                if !inside(dns.cast(), std::mem::size_of::<DnsServerAddress>()) {
                    break;
                }
                // SAFETY: as for the adapter entry.
                let d = unsafe { &*dns };
                let sa_len = d.address.sockaddr_length;
                if sa_len > 0 && inside(d.address.sockaddr, sa_len as usize) {
                    // SAFETY: `sa_len` bytes at `sockaddr` are inside `buf`
                    // (checked) and initialised by the call; `u8` has no
                    // alignment requirement.
                    let sa =
                        unsafe { std::slice::from_raw_parts(d.address.sockaddr, sa_len as usize) };
                    if let Some(server) = nameserver_from_sockaddr(sa) {
                        if !out.contains(&server) {
                            out.push(server);
                        }
                    }
                }
                dns = d.next;
                hops += 1;
            }
        }
        adapter = a.next;
        adapters += 1;
    }
    out
}

/// `GetNetworkParams`: the IPv4 servers of every adapter, merged.
#[cfg(windows)]
fn network_params_nameservers() -> Vec<SocketAddr> {
    use std::ffi::c_void;

    /// `IP_ADDR_STRING`. Fields the call fills but we do not read are kept so
    /// the layout is the OS's.
    #[allow(dead_code)]
    #[repr(C)]
    struct IpAddrString {
        next: *mut IpAddrString,
        ip_address: [u8; 16],
        ip_mask: [u8; 16],
        context: u32,
    }
    /// `FIXED_INFO`.
    #[allow(dead_code)]
    #[repr(C)]
    struct FixedInfo {
        host_name: [u8; 132],
        domain_name: [u8; 132],
        current_dns_server: *mut IpAddrString,
        dns_server_list: IpAddrString,
        node_type: u32,
        scope_id: [u8; 260],
        enable_routing: u32,
        enable_proxy: u32,
        enable_dns: u32,
    }
    #[link(name = "iphlpapi")]
    extern "system" {
        fn GetNetworkParams(info: *mut c_void, len: *mut u32) -> u32;
    }
    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    const NO_ERROR: u32 = 0;

    let mut len: u32 = 0;
    // SAFETY: the first call only reports the needed size (documented
    // `ERROR_BUFFER_OVERFLOW` protocol); a null buffer with len 0 is allowed.
    let first = unsafe { GetNetworkParams(std::ptr::null_mut(), &mut len) };
    if first != ERROR_BUFFER_OVERFLOW || (len as usize) < std::mem::size_of::<FixedInfo>() {
        return Vec::new();
    }
    // `u64` elements give the 8-byte alignment `FIXED_INFO`'s pointers need.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` is at least `len` bytes and suitably aligned, and the call
    // fills it with a `FIXED_INFO` whose `IP_ADDR_STRING` chain lives inside
    // the same buffer. Everything below only reads, and only while `buf` lives.
    let second = unsafe { GetNetworkParams(buf.as_mut_ptr().cast(), &mut len) };
    if second != NO_ERROR {
        return Vec::new();
    }
    let info = unsafe { &*(buf.as_ptr().cast::<FixedInfo>()) };
    let mut out = Vec::new();
    let mut node: *const IpAddrString = &info.dns_server_list;
    let mut hops = 0;
    while !node.is_null() && hops < 16 {
        // SAFETY: `node` is the embedded head or a `next` pointer the OS wrote
        // into `buf`; the loop is bounded.
        let entry = unsafe { &*node };
        let text: Vec<u8> = entry
            .ip_address
            .iter()
            .copied()
            .take_while(|b| *b != 0)
            .collect();
        if let Ok(ip) = String::from_utf8_lossy(&text).trim().parse::<IpAddr>() {
            if !ip.is_unspecified() && !out.contains(&SocketAddr::new(ip, 53)) {
                out.push(SocketAddr::new(ip, 53));
            }
        }
        node = entry.next;
        hops += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// A real SRV response, captured from a recursive resolver for
    /// `_minecraft._tcp.hypixel.net`: id `0x1234`, one answer, the owner as a
    /// compression pointer back to the question, priority 1, weight 1,
    /// port 0x63dd (25565), target `mc.hypixel.net.` spelled out in full.
    const HYPIXEL_RESPONSE: &str = "1234818000010001000000000a5f6d696e656372616674045f746370076879706978656c036e65740000210001c00c002100010000012600160001000163dd026d63076879706978656c036e657400";
    /// The question that produced it.
    const HYPIXEL_QUERY: &str = "1234010000010000000000000a5f6d696e656372616674045f746370076879706978656c036e65740000210001";
    /// The same resolver's answer for a name with no SRV record: NOERROR, no
    /// answers, an SOA in the authority section.
    const NODATA_RESPONSE: &str = "1234818000010000000100000a5f6d696e656372616674045f746370076578616d706c6503636f6d0000210001c01c0006000100000708003207656c6c696f7474026e730a636c6f7564666c617265c02403646e73c0449006f398000027100000096000093a8000000708";

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    const NAME: &str = "_minecraft._tcp.hypixel.net";

    /// Our query is byte-for-byte what a real client sent for the same name.
    ///
    /// MUTATION: a different QTYPE (A = 1), no RD flag, or a missing root
    /// label — each changes the bytes.
    #[test]
    fn the_query_matches_a_captured_one() {
        assert_eq!(build_query(0x1234, NAME).unwrap(), unhex(HYPIXEL_QUERY));
    }

    /// The captured answer decodes: compression pointer in the owner, the
    /// numbers in the right order, the target with its trailing dot.
    ///
    /// MUTATION: reading weight before priority, or the target from offset 4
    /// instead of 6 — port 25565 and `mc.hypixel.net.` pin all three fields.
    #[test]
    fn a_captured_answer_decodes() {
        let recs = parse_response(0x1234, NAME, &unhex(HYPIXEL_RESPONSE)).unwrap();
        assert_eq!(
            recs,
            vec![SrvRecord {
                priority: 1,
                weight: 1,
                port: 25565,
                target: "mc.hypixel.net.".into()
            }]
        );
        assert_eq!(
            pick(&recs),
            Some(SrvTarget {
                host: "mc.hypixel.net.".into(),
                port: 25565
            }),
            "the handshake carries the target with its trailing dot, as vanilla's does"
        );
    }

    /// NOERROR with no answers is "no redirect", not an error.
    #[test]
    fn a_nodata_answer_is_an_empty_list() {
        let recs = parse_response(
            0x1234,
            "_minecraft._tcp.example.com",
            &unhex(NODATA_RESPONSE),
        )
        .unwrap();
        assert!(recs.is_empty());
        assert_eq!(pick(&recs), None);
    }

    /// Anything that is not the answer to our question is refused: the id, the
    /// QR bit, the question name.
    #[test]
    fn a_response_that_is_not_ours_is_refused() {
        let good = unhex(HYPIXEL_RESPONSE);
        assert_eq!(
            parse_response(0x4321, NAME, &good),
            Err(SrvError::NotOurs),
            "wrong id"
        );
        assert_eq!(
            parse_response(0x1234, "_minecraft._tcp.other.net", &good),
            Err(SrvError::NotOurs),
            "wrong question"
        );
        let mut query_not_response = good.clone();
        query_not_response[2] &= 0x7f; // QR = 0
        assert_eq!(
            parse_response(0x1234, NAME, &query_not_response),
            Err(SrvError::NotOurs)
        );
    }

    #[test]
    fn truncation_and_server_failures_are_errors_not_empty_lists() {
        let mut tc = unhex(HYPIXEL_RESPONSE);
        tc[2] |= 0x02; // TC
        assert_eq!(parse_response(0x1234, NAME, &tc), Err(SrvError::Truncated));
        let mut servfail = unhex(HYPIXEL_RESPONSE);
        servfail[3] = (servfail[3] & 0xF0) | 2;
        assert_eq!(
            parse_response(0x1234, NAME, &servfail),
            Err(SrvError::ServerFailure(2))
        );
        // NXDOMAIN is a definitive empty answer.
        let mut nx = unhex(NODATA_RESPONSE);
        nx[3] = (nx[3] & 0xF0) | 3;
        assert_eq!(
            parse_response(0x1234, "_minecraft._tcp.example.com", &nx),
            Ok(vec![])
        );
    }

    /// Every prefix of a real response either parses or errors — none panics,
    /// and a truncated one is never read as a record.
    #[test]
    fn a_short_response_never_panics_or_invents_a_record() {
        let full = unhex(HYPIXEL_RESPONSE);
        for n in 0..full.len() {
            let r = parse_response(0x1234, NAME, &full[..n]);
            assert!(
                !matches!(&r, Ok(v) if !v.is_empty()),
                "{n} bytes parsed as a record: {r:?}"
            );
        }
    }

    /// A compression loop (a pointer to itself) and a forward pointer are
    /// rejected rather than followed.
    #[test]
    fn a_compression_loop_is_rejected() {
        let mut p = unhex(HYPIXEL_RESPONSE);
        // The answer's owner pointer (c00c) -> point it at itself.
        let owner_at = p.len() - 0x16 - 10 - 2;
        assert_eq!(&p[owner_at..owner_at + 2], &[0xc0, 0x0c]);
        p[owner_at + 1] = owner_at as u8;
        assert_eq!(parse_response(0x1234, NAME, &p), Err(SrvError::Malformed));
    }

    /// Wire builder for the synthetic cases below.
    struct Msg(Vec<u8>);
    impl Msg {
        fn new(id: u16, name: &str, an: u16) -> Self {
            let mut m = build_query(id, name).unwrap();
            m[2] = 0x81;
            m[3] = 0x80;
            m[6..8].copy_from_slice(&an.to_be_bytes());
            Msg(m)
        }
        fn name(&mut self, n: &str) {
            for l in n.trim_end_matches('.').split('.') {
                self.0.push(l.len() as u8);
                self.0.extend_from_slice(l.as_bytes());
            }
            self.0.push(0);
        }
        /// An answer whose owner is the question (pointer 0xC00C).
        fn rr(&mut self, rtype: u16, rdata: &[u8]) {
            self.0.extend_from_slice(&[0xC0, 0x0C]);
            self.0.extend_from_slice(&rtype.to_be_bytes());
            self.0.extend_from_slice(&[0, 1, 0, 0, 0, 60]);
            self.0
                .extend_from_slice(&(rdata.len() as u16).to_be_bytes());
            self.0.extend_from_slice(rdata);
        }
        fn srv(&mut self, prio: u16, weight: u16, port: u16, target: &str) {
            let mut d = Vec::new();
            d.extend_from_slice(&prio.to_be_bytes());
            d.extend_from_slice(&weight.to_be_bytes());
            d.extend_from_slice(&port.to_be_bytes());
            let mut m = Msg(Vec::new());
            m.name(target);
            d.extend_from_slice(&m.0);
            self.rr(TYPE_SRV, &d);
        }
    }

    /// RFC 2782's choice among several records: lowest priority, then highest
    /// weight; a zero port and the root target are skipped.
    ///
    /// MUTATION: highest priority number, or lowest weight.
    #[test]
    fn the_pick_is_lowest_priority_then_highest_weight_and_skips_unusable_records() {
        let mut m = Msg::new(7, "_minecraft._tcp.example.com", 5);
        m.srv(20, 99, 25570, "backup.example.com");
        m.srv(10, 1, 25565, "low-weight.example.com");
        m.srv(10, 50, 25566, "heavy.example.com");
        m.srv(0, 0, 25567, "."); // service not available
        m.srv(0, 100, 0, "zero-port.example.com");
        let recs = parse_response(7, "_minecraft._tcp.example.com", &m.0).unwrap();
        assert_eq!(recs.len(), 5);
        assert_eq!(
            pick(&recs),
            Some(SrvTarget {
                host: "heavy.example.com.".into(),
                port: 25566
            })
        );
    }

    /// An SRV record under a CNAME alias of the question counts; one owned by a
    /// stranger does not (an off-path attacker cannot add records for other
    /// names).
    #[test]
    fn only_records_for_the_question_or_its_cname_chain_count() {
        let mut m = Msg::new(9, "_minecraft._tcp.example.com", 3);
        // owner = question: CNAME to alias.example.net.
        let mut cname = Msg(Vec::new());
        cname.name("alias.example.net");
        m.rr(TYPE_CNAME, &cname.0);
        // an SRV owned by the alias (written out, no pointer).
        m.name("alias.example.net");
        m.0.extend_from_slice(&TYPE_SRV.to_be_bytes());
        m.0.extend_from_slice(&[0, 1, 0, 0, 0, 60]);
        let mut d = vec![0, 1, 0, 1, 0x63, 0xdd];
        let mut t = Msg(Vec::new());
        t.name("real.example.net");
        d.extend_from_slice(&t.0);
        m.0.extend_from_slice(&(d.len() as u16).to_be_bytes());
        m.0.extend_from_slice(&d);
        // an SRV owned by a stranger.
        m.name("stranger.example.org");
        m.0.extend_from_slice(&TYPE_SRV.to_be_bytes());
        m.0.extend_from_slice(&[0, 1, 0, 0, 0, 60]);
        let mut d2 = vec![0, 0, 0, 100, 0x63, 0xde];
        let mut t2 = Msg(Vec::new());
        t2.name("evil.example.org");
        d2.extend_from_slice(&t2.0);
        m.0.extend_from_slice(&(d2.len() as u16).to_be_bytes());
        m.0.extend_from_slice(&d2);
        let recs = parse_response(9, "_minecraft._tcp.example.com", &m.0).unwrap();
        assert_eq!(recs.len(), 1, "{recs:?}");
        assert_eq!(recs[0].target, "real.example.net.");
    }

    /// A target with characters a hostname cannot have is skipped, and the
    /// other records survive.
    #[test]
    fn a_target_with_odd_characters_is_skipped() {
        let mut m = Msg::new(3, "_minecraft._tcp.example.com", 2);
        // A label containing a space and a slash.
        let mut d = vec![0, 0, 0, 0, 0x63, 0xdd, 5];
        d.extend_from_slice(b"a b/c");
        d.push(0);
        m.rr(TYPE_SRV, &d);
        m.srv(0, 0, 25565, "good.example.com");
        let recs = parse_response(3, "_minecraft._tcp.example.com", &m.0).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].target, "good.example.com.");
    }

    #[test]
    fn names_that_cannot_be_asked_are_refused_up_front() {
        assert_eq!(
            srv_name("play.example.com").as_deref(),
            Some("_minecraft._tcp.play.example.com")
        );
        assert_eq!(
            srv_name("play.example.com.").as_deref(),
            Some("_minecraft._tcp.play.example.com"),
            "a trailing dot is the root, not an empty label"
        );
        assert_eq!(srv_name(""), None);
        assert_eq!(
            srv_name("bücher.example"),
            None,
            "non-ASCII is not punycoded here"
        );
        assert_eq!(srv_name("a..b"), None, "an empty label");
        assert_eq!(srv_name(&"a".repeat(64)), None, "a label over 63");
        assert_eq!(
            srv_name(&format!("{}.com", "a.".repeat(130))),
            None,
            "over 253"
        );
    }

    // ── the address ──────────────────────────────────────────────────────

    #[test]
    fn an_address_parses_like_host_and_port() {
        let p = |s: &str| ServerAddress::parse(s);
        let a = |h: &str, port: Option<u16>| {
            Some(ServerAddress {
                host: h.into(),
                port,
            })
        };
        assert_eq!(p("play.example.com"), a("play.example.com", None));
        assert_eq!(
            p("play.example.com:25566"),
            a("play.example.com", Some(25566))
        );
        assert_eq!(
            p("play.example.com:25565"),
            a("play.example.com", Some(25565))
        );
        assert_eq!(p("play.example.com:"), a("play.example.com", None));
        assert_eq!(p("[::1]"), a("::1", None));
        assert_eq!(p("[::1]:25570"), a("::1", Some(25570)));
        assert_eq!(p("::1"), a("::1", None), "a bare IPv6 literal is a host");
        assert_eq!(p("  host  "), a("host", None));
        assert_eq!(p(""), None);
        assert_eq!(p(":25565"), None, "no host");
        assert_eq!(p("host:99999"), None, "out of range");
        assert_eq!(p("host:abc"), None);
        assert_eq!(p("[::1"), None, "unbalanced bracket");
        assert_eq!(p("[::1]x"), None);
    }

    // ── the decision ─────────────────────────────────────────────────────

    fn found(host: &str, port: u16) -> impl Fn(&str) -> Option<SrvTarget> {
        let host = host.to_owned();
        move |_| {
            Some(SrvTarget {
                host: host.clone(),
                port,
            })
        }
    }

    /// No port: the redirect comes first and the typed host (on 25565) is the
    /// fallback. This is the headline: a bare `example.com` reaches the SRV
    /// target's port.
    ///
    /// MUTATION: dropping the fallback entry, or putting it first.
    #[test]
    fn a_portless_address_tries_the_srv_target_then_the_typed_host() {
        let addr = ServerAddress::parse("example.com").unwrap();
        let eps = plan(&addr, &found("mc.example.com.", 25599));
        assert_eq!(
            eps,
            vec![
                Endpoint {
                    host: "mc.example.com.".into(),
                    port: 25599,
                    via_srv: true
                },
                Endpoint {
                    host: "example.com".into(),
                    port: 25565,
                    via_srv: false
                },
            ]
        );
    }

    /// **An explicit port is never redirected** — not even `:25565`, which
    /// vanilla would look up. The lookup is not called at all.
    ///
    /// MUTATION: gating on `port == 25565` (vanilla's trigger) instead of
    /// `port.is_none()`.
    #[test]
    fn an_explicit_port_skips_the_lookup_entirely() {
        let boom = |_: &str| -> Option<SrvTarget> { panic!("the lookup must not run") };
        for text in ["example.com:25566", "example.com:25565"] {
            let eps = plan(&ServerAddress::parse(text).unwrap(), &boom);
            assert_eq!(eps.len(), 1);
            assert_eq!(eps[0].host, "example.com");
            assert!(!eps[0].via_srv);
        }
        assert_eq!(
            plan(&ServerAddress::explicit("example.com", 25577), &boom)[0].port,
            25577
        );
    }

    /// An IP literal has no SRV name, so there is no lookup; and a failed
    /// lookup leaves exactly the typed host on the default port.
    #[test]
    fn ip_literals_and_failed_lookups_fall_back_to_the_typed_host() {
        let boom = |_: &str| -> Option<SrvTarget> { panic!("the lookup must not run") };
        for text in ["127.0.0.1", "::1", "[2001:db8::1]"] {
            let eps = plan(&ServerAddress::parse(text).unwrap(), &boom);
            assert_eq!(eps.len(), 1, "{text}");
            assert_eq!(eps[0].port, 25565);
        }
        let none = |_: &str| -> Option<SrvTarget> { None };
        let eps = plan(&ServerAddress::parse("example.com").unwrap(), &none);
        assert_eq!(
            eps,
            vec![Endpoint {
                host: "example.com".into(),
                port: 25565,
                via_srv: false
            }]
        );
    }

    // ── resolv.conf ──────────────────────────────────────────────────────

    #[test]
    fn resolv_conf_nameservers_are_read_in_order() {
        let text = "# comment\n\
                    search lan\n\
                    nameserver 10.0.0.1\n\
                    nameserver 2001:db8::53   # trailing comment\n\
                    nameserver fe80::1%eth0\n\
                    nameserver not-an-ip\n\
                    options edns0\n";
        assert_eq!(
            parse_resolv_conf(text),
            vec![
                "10.0.0.1:53".parse().unwrap(),
                "[2001:db8::53]:53".parse().unwrap()
            ]
        );
        assert!(parse_resolv_conf("").is_empty());
    }

    // ── the transport, against a local fake nameserver ───────────────────

    /// A one-shot UDP nameserver on loopback. `respond` maps the query packet
    /// to the datagrams to send back (none = stay silent).
    fn fake_server(respond: impl Fn(&[u8]) -> Vec<Vec<u8>> + Send + 'static) -> SocketAddr {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = sock.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            if let Ok((n, from)) = sock.recv_from(&mut buf) {
                for reply in respond(&buf[..n]) {
                    let _ = sock.send_to(&reply, from);
                }
            }
        });
        addr
    }

    /// The answer to whatever id was asked, for `hypixel.net`.
    fn answer_for(query: &[u8]) -> Vec<u8> {
        let mut r = unhex(HYPIXEL_RESPONSE);
        r[0..2].copy_from_slice(&query[0..2]);
        r
    }

    /// The whole path over a real socket: question out, captured answer back
    /// (re-stamped with the random id), redirect out.
    #[test]
    fn a_nameserver_answer_becomes_a_redirect() {
        let server = fake_server(|q| vec![answer_for(q)]);
        let got = lookup_with("hypixel.net", &[server], Duration::from_secs(2));
        assert_eq!(
            got,
            Some(SrvTarget {
                host: "mc.hypixel.net.".into(),
                port: 25565
            })
        );
    }

    /// A datagram with the wrong id is ignored and the right one behind it is
    /// still accepted — an off-path forgery does not end the wait.
    #[test]
    fn a_forged_datagram_does_not_end_the_wait() {
        let server = fake_server(|q| {
            let mut forged = answer_for(q);
            forged[0] ^= 0xff;
            vec![forged, answer_for(q)]
        });
        assert!(lookup_with("hypixel.net", &[server], Duration::from_secs(2)).is_some());
    }

    /// A silent first nameserver times out and the second answers; when none
    /// does the result is "no redirect", within the timeout.
    #[test]
    fn a_silent_nameserver_falls_through_to_the_next_and_then_to_none() {
        let silent = fake_server(|_| Vec::new());
        let live = fake_server(|q| vec![answer_for(q)]);
        let t = Instant::now();
        assert!(lookup_with("hypixel.net", &[silent, live], Duration::from_millis(300)).is_some());
        assert!(t.elapsed() < Duration::from_secs(5));

        let silent2 = fake_server(|_| Vec::new());
        let t = Instant::now();
        assert_eq!(
            lookup_with("hypixel.net", &[silent2], Duration::from_millis(200)),
            None
        );
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "bounded by the timeout"
        );
        assert_eq!(
            lookup_with("hypixel.net", &[], Duration::from_millis(200)),
            None,
            "no nameserver at all is no redirect"
        );
    }

    /// A SERVFAIL from the first nameserver tries the second; a definitive
    /// empty answer does NOT (the second would say the same).
    #[test]
    fn servfail_tries_the_next_nameserver_and_nodata_does_not() {
        let fail = fake_server(|q| {
            let mut r = answer_for(q);
            r[3] = (r[3] & 0xF0) | 2;
            vec![r]
        });
        let live = fake_server(|q| vec![answer_for(q)]);
        assert!(lookup_with("hypixel.net", &[fail, live], Duration::from_secs(2)).is_some());

        let empty = fake_server(|q| {
            let mut r = answer_for(q);
            r[6..8].copy_from_slice(&[0, 0]); // ANCOUNT 0
            r.truncate(r.len() - 0x16 - 12); // drop the answer RR
            vec![r]
        });
        let live2 = fake_server(|q| vec![answer_for(q)]);
        assert_eq!(
            lookup_with("hypixel.net", &[empty, live2], Duration::from_secs(2)),
            None,
            "NOERROR/NODATA ends the search"
        );
    }

    /// An IP literal never reaches the wire.
    #[test]
    fn an_ip_literal_sends_nothing() {
        let hit = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = hit.clone();
        let server = fake_server(move |_| {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Vec::new()
        });
        let eps = plan(&ServerAddress::parse("127.0.0.1").unwrap(), &|h| {
            lookup_with(h, &[server], Duration::from_millis(200))
        });
        assert_eq!(eps.len(), 1);
        std::thread::sleep(Duration::from_millis(100));
        assert!(!hit.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// On this machine the system nameservers are discoverable (Windows reads
    /// them through `GetAdaptersAddresses` and `GetNetworkParams`, elsewhere
    /// resolv.conf). It does not query anything.
    #[test]
    fn the_system_nameservers_can_be_listed() {
        let servers = system_nameservers();
        println!("system nameservers: {servers:?}");
        // A CI container may legitimately have none; the call must just work.
        assert!(servers.len() <= 64);
        for (i, s) in servers.iter().enumerate() {
            assert_eq!(s.port(), 53);
            assert!(!s.ip().is_unspecified(), "{s}");
            assert!(!servers[..i].contains(s), "{s} listed twice");
        }
    }

    /// Windows' two sources, each on its own (the merge is covered above): the
    /// adapter list is the one that can carry IPv6.
    #[cfg(windows)]
    #[test]
    fn both_windows_sources_run_and_agree_on_the_shape() {
        let adapters = adapter_nameservers();
        let params = network_params_nameservers();
        println!("GetAdaptersAddresses: {adapters:?}\nGetNetworkParams: {params:?}");
        for s in adapters.iter().chain(&params) {
            assert_eq!(s.port(), 53);
            assert!(!s.ip().is_unspecified());
        }
        // The merge never loses an IPv4 server the old call knew.
        let all = windows_nameservers();
        for s in &params {
            assert!(all.contains(s), "{s} lost in the merge");
        }
    }

    // ── SOCKADDR -> nameserver (the Windows adapter list's element) ──────

    fn sockaddr_in(ip: [u8; 4]) -> Vec<u8> {
        let mut b = vec![0u8; 16];
        b[0..2].copy_from_slice(&2u16.to_ne_bytes());
        b[4..8].copy_from_slice(&ip);
        b
    }

    fn sockaddr_in6(ip: &str, scope: u32) -> Vec<u8> {
        let mut b = vec![0u8; 28];
        b[0..2].copy_from_slice(&23u16.to_ne_bytes());
        b[8..24].copy_from_slice(&ip.parse::<std::net::Ipv6Addr>().unwrap().octets());
        b[24..28].copy_from_slice(&scope.to_ne_bytes());
        b
    }

    #[test]
    fn a_sockaddr_becomes_a_nameserver_on_port_53() {
        assert_eq!(
            nameserver_from_sockaddr(&sockaddr_in([192, 168, 1, 1])),
            Some("192.168.1.1:53".parse().unwrap())
        );
        assert_eq!(
            nameserver_from_sockaddr(&sockaddr_in6("2001:4860:4860::8888", 0)),
            Some("[2001:4860:4860::8888]:53".parse().unwrap())
        );
        // Unique-local, a common router-advertised resolver.
        assert!(nameserver_from_sockaddr(&sockaddr_in6("fd00::1", 0)).is_some());
    }

    #[test]
    fn link_local_needs_its_scope_and_keeps_it() {
        let with = nameserver_from_sockaddr(&sockaddr_in6("fe80::1", 12)).unwrap();
        match with {
            SocketAddr::V6(v6) => {
                assert_eq!(v6.scope_id(), 12, "the interface survives");
                assert_eq!(v6.port(), 53);
            }
            SocketAddr::V4(_) => panic!("expected IPv6"),
        }
        assert_eq!(
            nameserver_from_sockaddr(&sockaddr_in6("fe80::1", 0)),
            None,
            "unreachable without a scope"
        );
    }

    #[test]
    fn the_systems_effective_nameservers_come_before_adapter_ones() {
        let sa = |s: &str| -> SocketAddr { s.parse().unwrap() };
        // GetNetworkParams: the resolver that actually works.
        let effective = vec![sa("192.168.1.1:53"), sa("1.1.1.1:53")];
        // GetAdaptersAddresses, in adapter order: a WSL and a Hyper-V virtual
        // adapter first, an IPv6 resolver, and the working one again.
        let adapters = vec![
            sa("172.20.0.1:53"),
            sa("172.30.0.1:53"),
            sa("[2001:db8::1]:53"),
            sa("192.168.1.1:53"),
        ];
        let got = merge_nameservers(effective, adapters);
        assert_eq!(
            got,
            vec![
                sa("192.168.1.1:53"),
                sa("1.1.1.1:53"),
                sa("172.20.0.1:53"),
                sa("172.30.0.1:53"),
                sa("[2001:db8::1]:53"),
            ],
            "effective first, adapter extras after in order, no duplicates"
        );
        // Of the three servers ever asked, the working one is first and the
        // virtual adapters cannot starve it.
        assert_eq!(&got[..MAX_NAMESERVERS][0], &sa("192.168.1.1:53"));

        // A failed GetNetworkParams leaves the adapter list as it was; a
        // failed adapter call leaves the effective list alone.
        let a = vec![sa("10.0.0.1:53")];
        assert_eq!(merge_nameservers(Vec::new(), a.clone()), a);
        assert_eq!(merge_nameservers(a.clone(), Vec::new()), a);
        assert!(merge_nameservers(Vec::new(), Vec::new()).is_empty());
    }

    #[test]
    fn junk_nameservers_are_skipped() {
        // Windows' placeholder for an adapter with no IPv6 DNS.
        assert_eq!(
            nameserver_from_sockaddr(&sockaddr_in6("fec0:0:0:ffff::1", 1)),
            None
        );
        assert_eq!(nameserver_from_sockaddr(&sockaddr_in([0, 0, 0, 0])), None);
        assert_eq!(nameserver_from_sockaddr(&sockaddr_in6("::", 0)), None);
        // Short buffers and other families never panic or invent an address.
        let v6 = sockaddr_in6("2001:db8::1", 0);
        for n in 0..v6.len() {
            let _ = nameserver_from_sockaddr(&v6[..n]);
        }
        assert_eq!(nameserver_from_sockaddr(&v6[..27]), None);
        assert_eq!(
            nameserver_from_sockaddr(&sockaddr_in([1, 1, 1, 1])[..7]),
            None
        );
        let mut other = sockaddr_in([1, 1, 1, 1]);
        other[0..2].copy_from_slice(&99u16.to_ne_bytes());
        assert_eq!(nameserver_from_sockaddr(&other), None);
        assert_eq!(nameserver_from_sockaddr(&[]), None);
    }

    // ── the TCP retry for a truncated answer ─────────────────────────────

    #[test]
    fn the_tcp_frame_is_length_then_message() {
        let q = build_query(0x1234, NAME).unwrap();
        let f = tcp_frame(&q).unwrap();
        assert_eq!(&f[..2], &(q.len() as u16).to_be_bytes());
        assert_eq!(&f[2..], &q[..]);
        assert_eq!(
            tcp_frame(&vec![0u8; 65536]),
            None,
            "a u16 length cannot say 64 KiB"
        );
        assert!(tcp_frame(&vec![0u8; 65535]).is_some());
    }

    /// The UDP answer with the TC bit set, as a nameserver whose real answer
    /// is too big for a datagram sends it.
    fn truncated_for(query: &[u8]) -> Vec<u8> {
        let mut r = answer_for(query);
        r[2] |= 0x02;
        r
    }

    /// One accepted TCP exchange: read the length-prefixed question.
    fn read_tcp_query(stream: &mut std::net::TcpStream) -> Vec<u8> {
        let mut len = [0u8; 2];
        stream.read_exact(&mut len).unwrap();
        let mut q = vec![0u8; u16::from_be_bytes(len) as usize];
        stream.read_exact(&mut q).unwrap();
        q
    }

    fn framed(answer: &[u8]) -> Vec<u8> {
        let mut f = (answer.len() as u16).to_be_bytes().to_vec();
        f.extend_from_slice(answer);
        f
    }

    type TcpHandler = Box<dyn Fn(std::net::TcpStream) + Send + 'static>;

    /// A nameserver on loopback whose UDP side answers `truncated_for` after
    /// `udp_delay` and whose TCP side (same port number; TCP and UDP ports are
    /// separate) runs `tcp` on the first connection. `None` leaves TCP closed.
    fn tc_server(udp_delay: Duration, tcp: Option<TcpHandler>) -> SocketAddr {
        // TCP first: Windows reserves large TCP port ranges (Hyper-V/WinNAT
        // "excluded port ranges", e.g. on GitHub's runners) that the UDP
        // ephemeral allocator does not avoid, so binding UDP first and then
        // TCP on the same number failed 100 times in a row there. Letting
        // the OS pick an allowed TCP port and pairing UDP with it does not.
        let (udp, listener) = (0..100)
            .find_map(|_| match tcp {
                None => Some((UdpSocket::bind("127.0.0.1:0").ok()?, None)),
                Some(_) => {
                    let l = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
                    let port = l.local_addr().ok()?.port();
                    let udp = UdpSocket::bind(("127.0.0.1", port)).ok()?;
                    Some((udp, Some(l)))
                }
            })
            .expect("a port free for both UDP and TCP");
        let addr = udp.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            if let Ok((n, from)) = udp.recv_from(&mut buf) {
                std::thread::sleep(udp_delay);
                let _ = udp.send_to(&truncated_for(&buf[..n]), from);
            }
        });
        if let (Some(listener), Some(handler)) = (listener, tcp) {
            std::thread::spawn(move || {
                if let Ok((stream, _)) = listener.accept() {
                    handler(stream);
                }
            });
        }
        addr
    }

    /// A truncated UDP answer is asked again over TCP, to the same nameserver,
    /// the same question (and id), and the TCP answer is the one used.
    ///
    /// MUTATION: treating TC as "no redirect" returns `None`; a missing length
    /// prefix hangs the fake server's `read_exact` and the lookup times out.
    #[test]
    fn a_truncated_answer_is_retried_over_tcp() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let server = tc_server(
            Duration::ZERO,
            Some(Box::new(move |mut s| {
                let q = read_tcp_query(&mut s);
                s.write_all(&framed(&answer_for(&q))).unwrap();
                seen2.lock().unwrap().push(q);
            })),
        );
        let got = lookup_with("hypixel.net", &[server], Duration::from_secs(2));
        assert_eq!(
            got,
            Some(SrvTarget {
                host: "mc.hypixel.net.".into(),
                port: 25565
            })
        );
        let q = seen.lock().unwrap()[0].clone();
        assert_eq!(
            &q[2..],
            &build_query(u16::from_be_bytes([q[0], q[1]]), NAME).unwrap()[2..]
        );
    }

    /// The answer may arrive in pieces: the length split from its body, the
    /// body in dribbles.
    #[test]
    fn a_dribbled_tcp_answer_is_reassembled() {
        let server = tc_server(
            Duration::ZERO,
            Some(Box::new(|mut s| {
                let q = read_tcp_query(&mut s);
                let f = framed(&answer_for(&q));
                s.set_nodelay(true).unwrap();
                for chunk in [&f[..1], &f[1..2], &f[2..20], &f[20..]] {
                    s.write_all(chunk).unwrap();
                    s.flush().unwrap();
                    std::thread::sleep(Duration::from_millis(30));
                }
            })),
        );
        assert!(lookup_with("hypixel.net", &[server], Duration::from_secs(2)).is_some());
    }

    /// A TCP answer that is not ours (another id), that says "truncated"
    /// again, or that is cut off is "no redirect", never a half-read record.
    #[test]
    fn a_bad_tcp_answer_is_no_redirect() {
        type Corrupt = fn(Vec<u8>) -> Vec<u8>;
        let cases: [(&str, Corrupt); 3] = [
            ("wrong id", |mut a| {
                a[0] ^= 0xff;
                a
            }),
            ("still truncated", |mut a| {
                a[2] |= 0x02;
                a
            }),
            ("cut off", |a| a[..a.len() - 5].to_vec()),
        ];
        for (what, corrupt) in cases {
            let server = tc_server(
                Duration::ZERO,
                Some(Box::new(move |mut s| {
                    let q = read_tcp_query(&mut s);
                    let a = corrupt(answer_for(&q));
                    // For "cut off" the length still claims the full answer.
                    let full = answer_for(&q).len() as u16;
                    let mut f = full.to_be_bytes().to_vec();
                    f.extend_from_slice(&a);
                    s.write_all(&f).unwrap();
                    // Close, so a short body ends the read at once.
                })),
            );
            let t = Instant::now();
            assert_eq!(
                lookup_with("hypixel.net", &[server], Duration::from_secs(2)),
                None,
                "{what}"
            );
            assert!(t.elapsed() < Duration::from_secs(2), "{what}");
        }
    }

    /// A nameserver that takes the TCP connection and never answers costs the
    /// per-nameserver timeout, not forever; one that refuses TCP costs nothing.
    #[test]
    fn a_silent_or_closed_tcp_side_is_bounded() {
        let silent = tc_server(
            Duration::ZERO,
            Some(Box::new(|s| {
                std::thread::sleep(Duration::from_millis(1500));
                drop(s);
            })),
        );
        let t = Instant::now();
        assert_eq!(
            lookup_with("hypixel.net", &[silent], Duration::from_millis(250)),
            None
        );
        let e = t.elapsed();
        assert!(
            e >= Duration::from_millis(200),
            "waited for the answer: {e:?}"
        );
        assert!(
            e < Duration::from_millis(1200),
            "bounded by the timeout: {e:?}"
        );

        let closed = tc_server(Duration::ZERO, None);
        let t = Instant::now();
        assert_eq!(
            lookup_with("hypixel.net", &[closed], Duration::from_millis(250)),
            None
        );
        assert!(t.elapsed() < Duration::from_millis(1200));
    }

    /// The TCP retry lives inside the same overall deadline as the UDP wait:
    /// three nameservers that each answer UDP slowly (just inside the
    /// per-nameserver timeout) and then stall TCP take no more than
    /// 3 x timeout in all, where UDP + TCP per nameserver would be ~1.9 x each.
    ///
    /// MUTATION: giving every TCP leg a fresh `timeout` instead of
    /// `bounded(timeout, overall)` makes this ~1.15 s instead of ~0.9 s.
    #[test]
    fn the_tcp_retry_fits_inside_the_overall_deadline() {
        let stall = || -> Option<TcpHandler> {
            Some(Box::new(|s| {
                std::thread::sleep(Duration::from_millis(2500));
                drop(s);
            }))
        };
        let timeout = Duration::from_millis(300);
        let servers = [
            tc_server(Duration::from_millis(270), stall()),
            tc_server(Duration::from_millis(270), stall()),
            tc_server(Duration::from_millis(270), stall()),
        ];
        let t = Instant::now();
        assert_eq!(lookup_with("hypixel.net", &servers, timeout), None);
        let e = t.elapsed();
        assert!(
            e < timeout * MAX_NAMESERVERS as u32 + Duration::from_millis(150),
            "overall deadline exceeded: {e:?}"
        );
    }

    /// A later nameserver still gets its turn after an earlier one's TCP
    /// retry failed fast.
    #[test]
    fn a_failed_tcp_retry_falls_through_to_the_next_nameserver() {
        let closed = tc_server(Duration::ZERO, None);
        let live = fake_server(|q| vec![answer_for(q)]);
        assert!(lookup_with("hypixel.net", &[closed, live], Duration::from_secs(2)).is_some());
    }

    /// Manual: run this crate's lib tests with `real_lookup -- --ignored
    /// --nocapture`. Asks THIS machine's nameservers for a record that exists
    /// (`_minecraft._tcp.hypixel.net`) and one that does not; the network is
    /// the only thing it needs, so it is not part of the default run.
    #[test]
    #[ignore = "needs a network"]
    fn real_lookup_through_the_system_resolver() {
        let servers = system_nameservers();
        println!("nameservers: {servers:?}");
        let hit = lookup_with("hypixel.net", &servers, Duration::from_secs(3));
        println!("hypixel.net -> {hit:?}");
        assert_eq!(hit.map(|t| t.port), Some(25565));
        assert_eq!(
            lookup_with("example.com", &servers, Duration::from_secs(3)),
            None
        );
    }
}
