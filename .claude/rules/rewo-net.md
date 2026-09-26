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
  `play.rs`, `ids.rs` or `lib.rs`.
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

## Online mode

AES-128-CFB8 both directions, Mojang session join with the BigInteger server
hash, and signed chat over the verbatim `updateSignature` layout. Mojang's
private key is PKCS#8 DER under a PKCS#1 label: strip the armour and parse the
DER directly.
