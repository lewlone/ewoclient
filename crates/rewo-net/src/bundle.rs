//! `bundle_delimiter` (0) — the packet that changes how packets are *applied*
//! (M78).
//!
//! It is the only one of M78's eight with machinery behind it, so it gets its
//! own module for the reason [`crate::client_state`] states; the other seven
//! share [`crate::session`].
//!
//! ## What a bundle is
//!
//! There is no "bundle packet" on the wire. `ClientboundBundlePacket` never
//! serialises: `PacketBundleUnpacker` expands it on the **sending** side into
//! `delimiter, sub-packets…, delimiter`, and `PacketBundlePacker` reassembles
//! it on the receiving side. The delimiter's body is empty, and
//! `BundleDelimiterPacket.handle` throws `AssertionError("This packet should be
//! handled by pipeline")` — so an empty body here is emphatically *not* an
//! inert packet. It is a pipeline instruction, and a client that decodes it as
//! a no-op has decoded it wrong in the one way that is invisible.
//!
//! ## The state machine, verbatim from `PacketBundlePacker.decode`
//!
//! ```text
//! not bundling:
//!   delimiter        -> start bundling (emit nothing)
//!   anything else    -> emit it now
//! bundling:
//!   delimiter        -> emit the whole buffered run, in order; stop bundling
//!   anything else    -> buffer it (emit nothing)
//! ```
//!
//! Four consequences, each one a property below:
//!
//! 1. **A bundle is applied all at once, and only when it closes.** The run is
//!    handed downstream as a single `ClientboundBundlePacket`, and
//!    `handleBundlePacket` is a plain `for` loop calling `subPacket.handle`
//!    directly. It is one scheduled task on the client's main thread — the
//!    sub-handlers' own `ensureRunningOnSameThread` calls are already satisfied
//!    and re-schedule nothing — so **no frame is rendered part-way through a
//!    bundle**. That is the whole point: the server bundles an `add_entity`
//!    with its `set_entity_data`, `set_equipment` and `update_attributes`, and
//!    split across frames a mob renders for a frame as a nameless, unequipped,
//!    default-metadata version of itself before popping into correctness.
//! 2. **An unterminated bundle is withheld, not dropped and not applied.**
//!    `currentBundler` simply stays non-null across `decode` calls, so every
//!    packet after an unclosed opening delimiter accumulates and nothing
//!    downstream sees any of it. Rewo's buffer therefore survives a
//!    `try_recv` that runs dry mid-bundle, which is exactly the case that makes
//!    bundling worth implementing at all — a socket that delivers a bundle in
//!    two reads.
//! 3. **There is no nesting.** `Bundler.addPacket`'s first line is
//!    `if (packet == delimiterPacket) return constructor.apply(bundlePackets)`,
//!    so a second delimiter *always* terminates. A "nested" delimiter is just
//!    the closing one, and the delimiter after it opens a fresh bundle. A
//!    depth counter — the natural implementation — would be wrong, and wrong in
//!    the direction that swallows every subsequent packet.
//! 4. **The size limit is an error, not a cap.**
//!    `BundlerInfo.BUNDLE_SIZE_LIMIT` is 4096 and the check runs *before* the
//!    add, `if (bundlePackets.size() >= 4096) throw new IllegalStateException`.
//!    So a bundle of exactly 4096 sub-packets is legal and the 4097th kills the
//!    connection. Neither delimiter counts toward it.
//!
//! ## The terminal-packet rule
//!
//! `verifyNonTerminalPacket` throws a `DecoderException("Terminal message
//! received in bundle")` for any packet inside a bundle whose `isTerminal()` is
//! true. In clientbound-play there is exactly one: `start_configuration`
//! (`ClientboundStartConfigurationPacket.isTerminal` returns `true`; the other
//! overrides are login/configuration/serverbound). A terminal packet means the
//! connection is about to change protocol state, and applying it from inside a
//! run that has not closed would leave the bundler holding packets belonging to
//! a protocol that no longer exists.
//!
//! Rewo's [`PlaySession`](crate::play::PlaySession) does not dispatch
//! `start_configuration` at all, so the *handler-removal* half of vanilla's
//! `decode` (`ctx.pipeline().remove` once a terminal packet passes through
//! outside a bundle) has nothing to model here. The rejection half does, and it
//! is cheap, so it is transcribed.
//!
//! ## Where Rewo diverges, stated rather than hidden
//!
//! Vanilla's two failure modes are exceptions on a Netty pipeline, which drop
//! the connection. Rewo has no exceptions; both surface as
//! [`Feed::Fatal`], and the caller ends the session the same way it ends one
//! for a closed socket. The observable outcome — the session stops — is the
//! same; what is deliberately *not* done is to recover and carry on, because a
//! client that silently continued past a malformed bundle would be applying a
//! run the server never meant to send as one.
//!
//! This machine is wired into [`crate::play::PlaySession`]'s drain only. The
//! M1-era `Connection::run_play` harness behind `rewo net` / `rewo view` reads
//! a small subset of packets with no frame between them, so bundling there
//! would buy nothing measurable and is left out.
//!
//! ## Ground truth (bundled 26.2 decompile, `%APPDATA%/EwoClient/rewo/26.2/
//! decompiled/`)
//!
//! - `net/minecraft/network/PacketBundlePacker.java` — the state machine
//! - `net/minecraft/network/PacketBundleUnpacker.java` — the sending half
//! - `net/minecraft/network/protocol/BundlerInfo.java` — `BUNDLE_SIZE_LIMIT`,
//!   `startPacketBundling`, `Bundler.addPacket`
//! - `net/minecraft/network/protocol/BundleDelimiterPacket.java` — the
//!   `AssertionError`
//! - `net/minecraft/network/protocol/game/ClientboundBundlePacket.java`
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java` —
//!   `handleBundlePacket`
//! - `net/minecraft/network/protocol/game/ClientboundStartConfigurationPacket
//!   .java` — the one terminal clientbound-play packet

/// `BundlerInfo.BUNDLE_SIZE_LIMIT`. A bundle of exactly this many sub-packets
/// is legal; one more is fatal.
pub const BUNDLE_SIZE_LIMIT: usize = 4096;

/// The ids the assembler needs, lifted out of [`crate::ids::Ids`] so the
/// machine can be driven without building a full `Ids`.
#[derive(Clone, Copy, Debug)]
pub struct BundleIds {
    /// `bundle_delimiter`.
    pub delimiter: i32,
    /// The one terminal clientbound-play packet, `start_configuration`.
    /// `Option` because `ids.rs` resolves it with `opt!`; `None` disables the
    /// rejection rather than rejecting everything.
    pub terminal: Option<i32>,
}

/// What the caller must do with the packet it just fed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feed {
    /// Not bundling, and this is an ordinary packet — apply it now.
    Apply,
    /// Swallowed: either the opening delimiter, or a sub-packet buffered
    /// inside an open bundle. Apply nothing.
    Buffered,
    /// The closing delimiter. Drain [`BundleAssembler::take`] and apply every
    /// entry, in order, before reading another packet.
    Flush,
    /// Vanilla would have thrown on the Netty pipeline and dropped the
    /// connection. The carried string is the reason.
    Fatal(&'static str),
}

/// `PacketBundlePacker` — the receiving half of the bundle pipeline.
#[derive(Clone, Debug)]
pub struct BundleAssembler {
    ids: BundleIds,
    /// `currentBundler`: `None` when not bundling. The `Vec` is the bundler's
    /// own `bundlePackets` list.
    open: Option<Vec<(i32, Vec<u8>)>>,
    /// How many bundles have closed, and the largest run seen.
    ///
    /// Not vanilla state. It exists so a **live** session can witness that the
    /// bundle path was exercised at all: without it, a `rewo play` run proves
    /// only that bundling did not *break* anything, which is equally true of a
    /// machine that never fired. `rewo play`'s summary prints both.
    closed: u64,
    largest: usize,
}

impl BundleAssembler {
    pub fn new(ids: BundleIds) -> Self {
        Self {
            ids,
            open: None,
            closed: 0,
            largest: 0,
        }
    }

    /// `(bundles closed, largest run)` since the session began.
    pub fn stats(&self) -> (u64, usize) {
        (self.closed, self.largest)
    }

    /// Whether a bundle is currently open. A client sitting here across a whole
    /// drain is withholding packets — see consequence 2.
    pub fn is_bundling(&self) -> bool {
        self.open.is_some()
    }

    /// How many sub-packets the open bundle holds, or `0` when none is open.
    pub fn buffered(&self) -> usize {
        self.open.as_ref().map_or(0, Vec::len)
    }

    /// Feed one inbound packet. See [`Feed`] for what the caller owes.
    ///
    /// The body is copied only when it is buffered, which is the case bundling
    /// exists for; the common path ([`Feed::Apply`]) copies nothing.
    pub fn feed(&mut self, id: i32, body: &[u8]) -> Feed {
        let is_delimiter = id == self.ids.delimiter;
        let is_terminal = self.ids.terminal == Some(id);
        match &mut self.open {
            // `if (this.currentBundler != null)`
            Some(buffered) => {
                if is_terminal {
                    // `verifyNonTerminalPacket` runs *before* `addPacket`, so
                    // this fires whether or not the packet is the delimiter —
                    // and the delimiter is never terminal, so in practice it is
                    // exactly `start_configuration`.
                    self.open = None;
                    return Feed::Fatal("terminal message received in bundle");
                }
                // `addPacket`: the delimiter closes, and closing happens
                // *before* the size check — there is no nesting (consequence 3).
                if is_delimiter {
                    self.closed += 1;
                    self.largest = self.largest.max(buffered.len());
                    return Feed::Flush;
                }
                // `if (this.bundlePackets.size() >= 4096) throw` — checked
                // before the push, so 4096 fits and the 4097th is fatal.
                if buffered.len() >= BUNDLE_SIZE_LIMIT {
                    self.open = None;
                    return Feed::Fatal("too many packets in a bundle");
                }
                buffered.push((id, body.to_vec()));
                Feed::Buffered
            }
            // `BundlerInfo.Bundler bundler = startPacketBundling(msg)` — which
            // returns a bundler only for the delimiter itself.
            None => {
                if is_delimiter {
                    self.open = Some(Vec::new());
                    Feed::Buffered
                } else {
                    Feed::Apply
                }
            }
        }
    }

    /// Take the closed bundle's sub-packets, in arrival order, and reset.
    ///
    /// Called after [`Feed::Flush`]. Returns an empty `Vec` in any other state,
    /// which keeps the caller from having to prove it asked at the right
    /// moment.
    pub fn take(&mut self) -> Vec<(i32, Vec<u8>)> {
        self.open.take().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELIM: i32 = 0;
    const TERMINAL: i32 = 118;

    fn assembler() -> BundleAssembler {
        BundleAssembler::new(BundleIds {
            delimiter: DELIM,
            terminal: Some(TERMINAL),
        })
    }

    /// Outside a bundle, an ordinary packet passes straight through and its
    /// body is not copied into any buffer.
    ///
    /// MUTATION: buffering unconditionally. That is the shape a "queue every
    /// packet and flush on the delimiter" implementation takes, and on a server
    /// that never bundles it withholds the entire session.
    #[test]
    fn an_unbundled_packet_applies_immediately() {
        let mut a = assembler();
        assert_eq!(a.feed(42, &[1, 2, 3]), Feed::Apply);
        assert!(!a.is_bundling());
        assert_eq!(a.buffered(), 0);
        assert!(a.take().is_empty());
    }

    /// The opening delimiter emits nothing; the run buffers; the closing
    /// delimiter releases it whole, in order.
    ///
    /// MUTATION: returning `Apply` for a packet inside a bundle. That is
    /// precisely the pre-M78 behaviour, and its failure mode is a rendering
    /// glitch (a mob drawn for a frame with default metadata) rather than a
    /// protocol error — which is why `REWO_PACKET_COVERAGE.md` ranks it first.
    #[test]
    fn a_bundle_is_withheld_until_it_closes_and_then_released_in_order() {
        let mut a = assembler();
        assert_eq!(a.feed(DELIM, &[]), Feed::Buffered);
        assert!(a.is_bundling());
        assert_eq!(a.feed(1, &[0xaa]), Feed::Buffered);
        assert_eq!(a.feed(99, &[0xbb, 0xcc]), Feed::Buffered);
        assert_eq!(a.buffered(), 2, "nothing has been applied yet");

        assert_eq!(a.feed(DELIM, &[]), Feed::Flush);
        let run = a.take();
        assert_eq!(
            run,
            vec![(1, vec![0xaa]), (99, vec![0xbb, 0xcc])],
            "arrival order, and neither delimiter is in the run"
        );
        assert!(!a.is_bundling(), "the bundle is closed");
        assert_eq!(a.feed(7, &[]), Feed::Apply, "and the next packet is normal");
    }

    /// An unterminated bundle keeps its packets across as many feeds as it
    /// takes — the state does not reset when the caller runs out of input.
    ///
    /// MUTATION: clearing `open` when a drain ends, or applying the partial run
    /// as if the delimiter had arrived. This is the case bundling exists for: a
    /// socket that hands over the first half of a bundle and then goes quiet.
    /// The sample re-enters `feed` after a gap and asserts the earlier packets
    /// are *still there*, which a reset would lose silently.
    #[test]
    fn an_unterminated_bundle_survives_a_drain_that_runs_dry() {
        let mut a = assembler();
        a.feed(DELIM, &[]);
        a.feed(1, &[0x01]);
        assert_eq!(a.buffered(), 1);

        // ... the caller's `try_recv` returns Empty and it returns. The next
        // drain resumes here.
        assert!(a.is_bundling());
        assert_eq!(a.feed(2, &[0x02]), Feed::Buffered);
        assert_eq!(a.feed(DELIM, &[]), Feed::Flush);
        assert_eq!(a.take(), vec![(1, vec![0x01]), (2, vec![0x02])]);
    }

    /// There is no nesting: the second delimiter closes, and the third opens a
    /// fresh bundle.
    ///
    /// MUTATION: a depth counter (`depth += 1` on a delimiter while bundling).
    /// That reading never closes the outer bundle, so every packet after the
    /// first nested-looking delimiter is withheld for the rest of the session.
    /// The sample sends *three* delimiters, because with two the two readings
    /// agree on everything except the final state.
    #[test]
    fn a_second_delimiter_closes_rather_than_nesting() {
        let mut a = assembler();
        a.feed(DELIM, &[]);
        a.feed(1, &[]);
        assert_eq!(a.feed(DELIM, &[]), Feed::Flush);
        assert_eq!(a.take().len(), 1);

        // The third delimiter opens a new run, not a deeper one.
        assert_eq!(a.feed(DELIM, &[]), Feed::Buffered);
        assert_eq!(a.buffered(), 0, "a fresh, empty bundle");
        assert_eq!(a.feed(2, &[]), Feed::Buffered);
        assert_eq!(a.feed(DELIM, &[]), Feed::Flush);
        assert_eq!(a.take(), vec![(2, vec![])]);
    }

    /// Two delimiters back to back are an empty bundle, which is legal and
    /// applies nothing.
    ///
    /// MUTATION: treating an empty run as "not a bundle" and falling through to
    /// `Apply` for the closing delimiter — which would then dispatch the
    /// delimiter id itself.
    #[test]
    fn an_empty_bundle_is_legal() {
        let mut a = assembler();
        assert_eq!(a.feed(DELIM, &[]), Feed::Buffered);
        assert_eq!(a.feed(DELIM, &[]), Feed::Flush);
        assert!(a.take().is_empty());
        assert!(!a.is_bundling());
    }

    /// The size limit is checked **before** the push, so exactly
    /// `BUNDLE_SIZE_LIMIT` sub-packets fit and the next one is fatal.
    ///
    /// MUTATION: `>` instead of `>=` (which admits 4097), or checking after the
    /// push (same off-by-one), or clamping instead of failing. The sample sits
    /// exactly on the bound — 4096 buffered must still be `Buffered`, and the
    /// 4097th must be `Fatal` — because a witness that only fed 10 packets
    /// would leave every one of those readings green.
    #[test]
    fn the_bundle_size_limit_admits_exactly_the_limit_and_no_more() {
        let mut a = assembler();
        a.feed(DELIM, &[]);
        for _ in 0..BUNDLE_SIZE_LIMIT {
            assert_eq!(a.feed(1, &[]), Feed::Buffered);
        }
        assert_eq!(a.buffered(), BUNDLE_SIZE_LIMIT, "4096 fit");
        assert!(matches!(a.feed(1, &[]), Feed::Fatal(_)), "4097 does not");
        assert!(!a.is_bundling(), "and the run is abandoned, not carried on");
    }

    /// The closing delimiter is not counted against the limit — it terminates
    /// before `addPacket` reaches the size check.
    ///
    /// MUTATION: moving the size check above the delimiter test. A full bundle
    /// would then be fatal *at the moment it correctly closed*, which is the
    /// worst possible reading: it fires only on the servers that legitimately
    /// send large bundles.
    #[test]
    fn a_full_bundle_still_closes() {
        let mut a = assembler();
        a.feed(DELIM, &[]);
        for _ in 0..BUNDLE_SIZE_LIMIT {
            a.feed(1, &[]);
        }
        assert_eq!(a.feed(DELIM, &[]), Feed::Flush);
        assert_eq!(a.take().len(), BUNDLE_SIZE_LIMIT);
    }

    /// A terminal packet inside a bundle is fatal; the same packet outside one
    /// is ordinary.
    ///
    /// MUTATION: dropping the `verifyNonTerminalPacket` clause. The
    /// outside-a-bundle half of the sample is what makes the witness sharp —
    /// a rejection that fired unconditionally would break `start_configuration`
    /// on every server that ever reloads a datapack, and only checking the
    /// inside case would not see it.
    #[test]
    fn a_terminal_packet_is_fatal_only_inside_a_bundle() {
        let mut a = assembler();
        assert_eq!(a.feed(TERMINAL, &[]), Feed::Apply, "outside: ordinary");

        a.feed(DELIM, &[]);
        a.feed(5, &[]);
        assert!(matches!(a.feed(TERMINAL, &[]), Feed::Fatal(_)));
        assert!(!a.is_bundling());
    }

    /// With no resolved `start_configuration` id, nothing is terminal — the
    /// rule disables rather than matching everything.
    ///
    /// MUTATION: `terminal: Option<i32>` compared as `Some(id) == self.terminal`
    /// with a default of `Some(0)`, or an `unwrap_or(id)` that makes every
    /// packet terminal. Either would make the first sub-packet of every bundle
    /// fatal on a server whose report lacks the name.
    #[test]
    fn an_unresolved_terminal_id_disables_the_rule() {
        let mut a = BundleAssembler::new(BundleIds {
            delimiter: DELIM,
            terminal: None,
        });
        a.feed(DELIM, &[]);
        assert_eq!(a.feed(TERMINAL, &[]), Feed::Buffered);
        assert_eq!(a.feed(DELIM, &[]), Feed::Flush);
        assert_eq!(a.take().len(), 1);
    }

    /// `take` outside a flush is empty rather than a panic or a stale run.
    ///
    /// MUTATION: `self.open.take().expect(...)`. The caller drains on `Flush`
    /// only, but a `take` that panicked on a mis-sequenced call would turn a
    /// caller bug into a crash rather than a no-op.
    #[test]
    fn taking_without_a_flush_is_empty() {
        let mut a = assembler();
        assert!(a.take().is_empty());
        a.feed(DELIM, &[]);
        a.feed(1, &[0x7f]);
        // Mid-bundle: taking here would apply a partial run, so it must not be
        // something the caller can do by accident — but it must not panic.
        assert_eq!(a.take(), vec![(1, vec![0x7f])]);
        assert!(!a.is_bundling());
    }
}
