---
paths:
  - "crates/rewo-net/**"
  - "crates/rewo-proto/**"
  - "crates/rewo-data/**"
  - "REWO_PACKET_COVERAGE.md"
---

# Rewo protocol, decode and data tables

Reasoning and examples for each rule: `docs/rewo/` and `REWO_PLAN.md` §15.

## Ids and registries — the most-repeated bug in the project

- **Packet ids resolve by name from the datagen report**, so a version bump
  fails loud instead of misfiring.
- **`registry(...)` / `holderRegistry` write a RAW 0-based id; `holder` writes
  `id + 1`, with 0 meaning an inline definition follows.** It has bitten at
  M16 (dimension type), M21 (damage type), M55 (attributes), M87 (menu type),
  M92d/M93l (mob effect) and M93u (item). It's usually quiet: the off-by-one
  names a real neighbour. `OPTIONAL_VAR_INT` is the `+1` family in optional
  form.
- **Built-in registries** (`mob_effect`, `sound_event`, menu types, recipe
  display types) come **from the report, by name**. They never appear in
  `registry_data`; a branch reading them there can't fire (M92).
- **Datapack registries** (`dimension_type`, `enchantment`, `trim_material`,
  `trim_pattern`, `chat_type`, `biome`) come from Configuration
  `registry_data`, kept **in wire order: the index is the id**. Never select
  by name, and never substitute a default for a malformed entry.
- **`serde_json`'s default `Map` is a sorted `BTreeMap`.** Iterating a report
  gives alphabetical order, not id order. Read `protocol_id`; never
  `enumerate()` (M64: a wrong name for every one of 1,968 sounds, invisible to
  every decode gate). Use `indexmap` where file order matters (CEM).

## Wire shapes that read backwards

- **`DataComponentPatch` has no length prefix.** An untranscribed component's
  value can't be skipped, so the reader parks mid-value. Codecs are data
  (`component_wire.rs` `Shape` trees). Capture and walk must consume the same
  bytes by construction (`walk = read(..).is_some()`).
- **NBT lists are homogeneous**: a mixed list is compounds with each
  non-compound boxed as `{"": value}`, and must be unwrapped on read (M125).
- **Fixed-width fields among var-ints**: `container_set_slot`'s i16 slot (but
  `set_player_inventory`'s is a VarInt), `container_set_data`'s two signed
  shorts, `entity_event`'s BE i32 + byte, `explode`'s `blockCount` i32,
  `DyedItemColor` INT, merchant offer ints.
- `set_entity_motion` uses `LpVec3` (15-bit mantissas + shared scale + zero
  sentinel); `explode`'s knockback is `Vec3.STREAM_CODEC` doubles.
- `readEnum` is an array index where out-of-range is an **error**;
  `ByIdMap.continuous(…, ZERO)` falls back to the zero value. Check each field.
- **`bundle_delimiter` is a pipeline instruction**: buffer, apply all on close,
  withhold an unterminated bundle, and don't nest.
- The paletted long array is fixed-size (no length prefix), and each section
  starts with two shorts. Honour `empty_sky`, and handle
  `section_blocks_update`.
- Sound position is `(int)(coord * 8)` on the wire, read back as an
  **f32** divide.
- **Handlers on `ClientCommonPacketListener` exist in Configuration too**
  (`update_tags`, `custom_payload`, `server_links`). Look for the second copy.
  The Configuration tasks `resource-pack` and code-of-conduct **must be
  answered** (Rewo replies `FAILED_DOWNLOAD`, since only `DECLINED` kicks), or
  the login hangs forever with no error (M166).
- An unknown `game_event` type is a silent no-op, not a decode error.
- **NBT strings are Java modified UTF-8** (`DataInputStream.readUTF`: NUL is
  `C0 80`, supplementary characters are CESU-8 surrogate pairs), decoded by
  `rewo_proto::nbt::decode_modified_utf8`. Only NBT uses `readUTF` on the
  wire; every other protocol string is VarInt-prefixed UTF-8.
- **Login answers its queries**: `custom_query` gets a null answer and
  `cookie_request` an answer from the jar (`rewo-net/src/login.rs`), or a
  server waiting on them stalls the login. A negative compression threshold
  disables compression.

## Reports are file content

`blocks.json`'s state ids are dense (`0..N`, each once). `Blocks::from_json`
sizes its table by the number of states listed and rejects an id outside
`0..N`, a repeat, or one past `u32`, so a corrupt report is a named error and
never a `vec![..; max_id + 1]` allocation. Size anything built from a report by
what the report contains, never by an id it claims.

## Metadata indices are polymorphic

The same index means different things per class. `AgeableMob` declares
**two** accessors (baby at 16, `AGE_LOCKED` at 17), so a sniffer's or
armadillo's state is at 18 while the copper golem's is at 17. Disambiguate by
entity kind **and** serializer. Count `defineId` up the `extends` chain
mechanically, and calibrate the count against a known-good reading before
trusting it. Class and ancestry sets come from `tools/gen_entity_classes.py`
(fail-loud if empty).

## Dispatch discipline

- **One dispatcher per packet id.** The play loop is an `else if` chain, so a
  second claimant silently steals or never fires.
- **The coverage class "resolved but ignored" must stay at zero.** A resolved
  id with a dropped body reads as "handled" to every grep.
- **A clean 3-way apply isn't evidence of no collision.** Grep for duplicate
  fields and unreachable second arms after merging branches that touch
  `play/` (`mod.rs`, `dispatch.rs`, `outgoing.rs`, `tests.rs`), `ids.rs` or
  `lib.rs`.
- **Every dropped or malformed body is counted.** Play-state drops go through
  `PlaySession::decode_failed`, configuration through `route_decode_failed`
  (folded into the session counter), once per packet, and a packet applies
  all-or-nothing as vanilla's decode-then-handle does. `net soak` fails on any
  `decode_failures`. This includes `remove_entities` and `player_info_remove`:
  the list is decoded whole and applied in full or counted (vanilla decodes the
  entire list before the handler runs).
- **The inbound queue is bounded in bytes** (`play/inbound.rs`,
  `INBOUND_QUEUE_BYTES` = 64 MiB of decompressed frames). The reader reserves
  each frame's bytes before queueing and **blocks** when the budget is spent,
  so the kernel receive window closes (TCP backpressure); a frame is never
  dropped, since that would desync the session. A frame bigger than the whole
  budget is admitted on an empty queue; `PlaySession::drop` closes the budget.
- `update_tags` overrides jar tags (a datapack retag otherwise yields a wrong
  swing duration or a missing tooltip line with no error). Jar-derived tables
  (`tools/gen_*.py`, "Do not edit" generated files) carry that datapack caveat.

## Chat and components

`translate` components must resolve (`chat_translate.rs`): template literals
take the translatable's style, arguments apply their own on top, and the
numeric width survives (`IntTag(3)` → `3`). Legacy `§` rules: a colour code
clears the five format flags, `§r` resets to the enclosing style, and `#f00`
is `parseInt`, not CSS. Flatten `§` at wire time wherever text is drawn.
Styled spans (`ChatSpan`) live in `rewo-world`.

## Server-supplied data is untrusted

- **Skin and cape URLs** from a `textures` property must pass
  `rewo_net::skins::allowed_texture_url`: scheme http/https, host exactly
  `textures.minecraft.net` (authlib 9.0.75's `TextureUrlChecker`, which 26.2
  pins; 26.1's 7.0.63 is the same, and the wildcard `*.minecraft.net` /
  `*.mojang.com` rule is the older 7.0.61), no userinfo, port or odd
  characters. `rewo-app/src/skin_fetch.rs` re-checks at fetch time, upgrades
  http to https, uses timeouts and no redirects, caps the body at 1 MiB and
  checks PNG dimensions before decoding. The Java mod mirrors the rule.
- **Deadlines and write failures** (`lib.rs`): `Connection::connect` tries every
  resolved address with `CONNECT_TIMEOUT` (30 s each); everything up to play
  (login, configuration) runs under `HANDSHAKE_TIMEOUT` (90 s) enforced by
  `NetStream` capping each read at what is left, so a server dripping bytes
  cannot hold the pre-window phase forever; `split()` drops it and restores the
  30 s play read timeout. Play writes are synchronous on the caller's (frame)
  thread, so `WRITE_TIMEOUT` is 5 s, the worst freeze a stalled peer can cause.
  **A failed or timed-out write is fatal**: it may leave a torn frame, so
  `write_packet` records the first failure and refuses every later write, and
  `send` surfaces it as the `ClientError` disconnect.
- **SRV redirects** (`srv.rs`, `Connection::connect_address`): an address typed
  with **no port** (`ServerAddress::port` is `None`) asks the system's
  nameservers for `_minecraft._tcp.<host>` over a hand-written sync UDP query
  (1 s per nameserver, three at most; a truncated (`TC`) answer is asked again
  over TCP to the same nameserver, length-prefixed, as the JDK's resolver does;
  the whole lookup, TCP included, shares one deadline of 3 x the per-nameserver
  timeout; no async runtime, no new dependency) and tries the target, then the typed host on 25565. A written
  port, even `:25565`, and an IP literal are never looked up (vanilla looks up
  any 25565; Rewo narrows it to "the player typed no port"). The handshake
  names `Connection::endpoint()`, the SRV target **with its trailing dot**, as
  vanilla's does. `rewo live --host H` with no `--port` is eligible; neither
  flag is the dev server `127.0.0.1:25599`; the launcher omits `--port` when
  the player typed none. The answer is untrusted: id, question and owner (or
  its CNAME chain) must match, targets are hostname characters only, port 0 and
  the root target are skipped. Nameservers come from `/etc/resolv.conf` or
  Windows' `GetAdaptersAddresses` (operational non-loopback adapters, IPv4 and
  IPv6; link-local IPv6 keeps its scope id, site-local `fec0::/10` and
  scope-less link-local are skipped) merged with `GetNetworkParams` (a small
  hand-written FFI with compile-time offset asserts and in-buffer pointer
  checks, the crate's only `unsafe`). Tests use captured packets and loopback
  fake nameservers (UDP, and TCP for the truncated retry).
- **Recordings**: `record.rs` bounds each record's length by the bytes left in
  the file before allocating. `net soak` fails unless the session ended
  `Completed` (a server disconnect or client error mid-run is a failure).
- **Skin table** (`skins::SkinTable`): `MAX_TRACKED_SKINS` bounds the players
  present at once, not a lifetime count. An entry leaves on `player_info_remove`
  and on `clear_level` (reconfiguration); the cap logs once when it refuses.
- **Bounded logs**: `PlaySession::chat_log` keeps the newest
  `MAX_CHAT_HISTORY` (100) lines through `chat_wire::push_chat_log`.
- **Non-finite teleports** are dropped field-group by field-group
  (`resolve_teleport`) and counted; float sorts use `total_cmp`.

## Online mode

AES-128-CFB8 both directions, Mojang session join with the BigInteger server
hash, and signed chat over the verbatim `updateSignature` layout. Mojang's
private key is PKCS#8 DER under a PKCS#1 label: strip the armour and parse the
DER directly.

Outgoing commands parse against the block and item registries
(`signed_command::signable_command`, `CommandRegistries`), as vanilla's
`CommandBuildContext` does, so a `minecraft:message` argument after a
`block_state` or `item` argument is collected and signed. Vanilla does not sign
past a root redirect (`execute ... run say`); a test pins that.
