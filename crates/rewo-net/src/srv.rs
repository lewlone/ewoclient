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
//! second per nameserver, three nameservers at most, no TCP retry (a truncated
//! answer is "no redirect"). The nameservers are the system's own
//! (`/etc/resolv.conf`; `GetNetworkParams` on Windows) — never a public
//! resolver Rewo picked.
//!
//! # Untrusted input
//!
//! The answer comes off the network. Names are decoded with a jump limit and a
//! length cap, the target must be plain hostname characters, a port of zero or
//! a root target (`.`, "service not available", RFC 2782) is skipped, only
//! answers whose owner is the question name (or a CNAME alias of it) count, and
//! a response whose id, question or source is not ours is ignored.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// `ServerAddress.parseString`'s default.
pub const DEFAULT_PORT: u16 = 25565;

/// Per-nameserver wait for an answer.
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
    /// The `TC` bit: the answer did not fit in a UDP datagram. Rewo does not
    /// retry over TCP; the lookup is "no redirect".
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
/// `None` is "no redirect" whatever the reason — no nameserver, a timeout, a
/// refusal, a truncated or malformed answer, or no record. The reason is
/// logged at debug.
pub fn lookup_with(host: &str, servers: &[SocketAddr], timeout: Duration) -> Option<SrvTarget> {
    let name = srv_name(host)?;
    for server in servers.iter().take(MAX_NAMESERVERS) {
        match query_one(*server, &name, timeout) {
            Ok(records) => return pick(&records),
            Err(e) => log::debug!("srv: {server}: {e}"),
        }
    }
    None
}

fn query_one(server: SocketAddr, name: &str, timeout: Duration) -> Result<Vec<SrvRecord>, String> {
    let id: u16 = rand::random();
    let query = build_query(id, name).ok_or("unencodable name")?;
    let bind: SocketAddr = if server.is_ipv4() {
        ([0, 0, 0, 0], 0).into()
    } else {
        ([0u16; 8], 0).into()
    };
    let sock = UdpSocket::bind(bind).map_err(|e| format!("bind: {e}"))?;
    // `connect` makes the kernel drop datagrams from any other source.
    sock.connect(server).map_err(|e| format!("connect: {e}"))?;
    sock.send(&query).map_err(|e| format!("send: {e}"))?;
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 1500];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("timed out".into());
        }
        sock.set_read_timeout(Some(left))
            .map_err(|e| format!("timeout: {e}"))?;
        let n = match sock.recv(&mut buf) {
            Ok(n) => n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err("timed out".into());
            }
            Err(e) => return Err(format!("recv: {e}")),
        };
        match parse_response(id, name, &buf[..n]) {
            // Someone else's datagram (wrong id or question): keep waiting for
            // ours until the deadline.
            Err(SrvError::NotOurs) => continue,
            Err(e) => return Err(format!("{e:?}")),
            Ok(records) => return Ok(records),
        }
    }
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

#[cfg(windows)]
fn windows_nameservers() -> Vec<SocketAddr> {
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
    /// them through `GetNetworkParams`, elsewhere resolv.conf). It does not
    /// query anything.
    #[test]
    fn the_system_nameservers_can_be_listed() {
        let servers = system_nameservers();
        println!("system nameservers: {servers:?}");
        // A CI container may legitimately have none; the call must just work.
        assert!(servers.len() <= 64);
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
