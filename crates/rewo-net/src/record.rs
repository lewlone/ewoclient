//! Packet record/replay. Records every decoded (post-decompression)
//! inbound packet as `[u8 state][i32 id][i32 len][bytes]` framed records
//! with a small header. Replay feeds the same bytes back through the world
//! decoder with no socket — the deterministic fixture the DoD compares
//! digests against (REWO_PLAN.md §6.4).

use std::io::{Read, Write};

use rewo_data::packets::State;

const MAGIC: &[u8; 8] = b"REWOREC1";

fn state_byte(s: State) -> u8 {
    match s {
        State::Handshake => 0,
        State::Status => 1,
        State::Login => 2,
        State::Configuration => 3,
        State::Play => 4,
    }
}

fn state_from_byte(b: u8) -> Option<State> {
    Some(match b {
        0 => State::Handshake,
        1 => State::Status,
        2 => State::Login,
        3 => State::Configuration,
        4 => State::Play,
        _ => return None,
    })
}

pub struct Recorder {
    out: std::io::BufWriter<std::fs::File>,
    pub count: u64,
}

impl Recorder {
    pub fn create(path: &std::path::Path) -> std::io::Result<Self> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(MAGIC)?;
        Ok(Self {
            out: std::io::BufWriter::new(file),
            count: 0,
        })
    }

    /// Record one inbound clientbound packet (id already split from body).
    pub fn record(&mut self, state: State, id: i32, body: &[u8]) -> std::io::Result<()> {
        self.out.write_all(&[state_byte(state)])?;
        self.out.write_all(&id.to_be_bytes())?;
        self.out.write_all(&(body.len() as i32).to_be_bytes())?;
        self.out.write_all(body)?;
        self.count += 1;
        Ok(())
    }

    pub fn finish(mut self) -> std::io::Result<u64> {
        self.out.flush()?;
        Ok(self.count)
    }
}

pub struct RecordedPacket {
    pub state: State,
    pub id: i32,
    pub body: Vec<u8>,
}

/// Replay a recording through the world decoder with no socket, returning
/// `(world_digest, loaded_columns, chunks_decoded)`. This is the DoD's
/// equivalence check: a live session and its replay must agree bit-for-bit.
pub fn replay(
    path: &std::path::Path,
    data: &rewo_data::GameData,
    ids: &crate::ids::Ids,
) -> Result<(u64, usize, u64), String> {
    use rewo_proto::reader::PacketReader;
    use rewo_world::dimension::DimensionShape;

    let packets = read_all(path)?;
    let mut world = rewo_world::World::new(DimensionShape::OVERWORLD);
    let mut dim_shapes: Vec<DimensionShape> = Vec::new();
    let mut chunks = 0u64;

    for p in &packets {
        if p.state != State::Play {
            // Replay dimension registry from config so the shape resolves.
            if p.state == State::Configuration && p.id == ids.cb_config_registry_data {
                parse_registry_shapes(&p.body, &mut dim_shapes);
            }
            continue;
        }
        if p.id == ids.cb_play_login {
            let mut r = PacketReader::new(&p.body);
            if let Some(shape) = read_login_shape(&mut r, &dim_shapes) {
                world.shape = shape;
            }
        } else if p.id == ids.cb_play_level_chunk {
            let mut r = PacketReader::new(&p.body);
            if let Ok(col) =
                rewo_world::chunk::read_level_chunk(&mut r, &world.shape, &data.blocks)
            {
                world.insert_column(col.cx, col.cz, col);
                chunks += 1;
            }
        } else if p.id == ids.cb_play_block_update {
            let mut r = PacketReader::new(&p.body);
            if let (Ok((x, y, z)), Ok(state)) = (r.position(), r.varint()) {
                world.set_block(x, y, z, state as u32);
            }
        } else if p.id == ids.cb_play_forget_chunk {
            let mut r = PacketReader::new(&p.body);
            if let Ok(v) = r.i64() {
                world.forget_column(v as i32, (v >> 32) as i32);
            }
        }
    }
    Ok((world.digest(), world.loaded_columns(), chunks))
}

fn parse_registry_shapes(
    body: &[u8],
    dim_shapes: &mut Vec<rewo_world::dimension::DimensionShape>,
) {
    use rewo_proto::nbt::Nbt;
    use rewo_proto::reader::PacketReader;
    use rewo_world::dimension::DimensionShape;
    let mut r = PacketReader::new(body);
    let Ok(registry) = r.identifier() else { return };
    if registry != "minecraft:dimension_type" {
        return;
    }
    let Ok(count) = r.count("registry entries", 1) else {
        return;
    };
    dim_shapes.clear();
    for _ in 0..count {
        if r.identifier().is_err() {
            return;
        }
        let has_nbt = r.bool().unwrap_or(false);
        if !has_nbt {
            dim_shapes.push(DimensionShape::OVERWORLD);
            continue;
        }
        let Ok(nbt) = r.nbt() else { return };
        let min_y = nbt.get("min_y").and_then(Nbt::as_i64).unwrap_or(-64) as i32;
        let height = nbt.get("height").and_then(Nbt::as_i64).unwrap_or(384) as i32;
        dim_shapes.push(DimensionShape { min_y, height });
    }
}

fn read_login_shape(
    r: &mut rewo_proto::reader::PacketReader,
    dim_shapes: &[rewo_world::dimension::DimensionShape],
) -> Option<rewo_world::dimension::DimensionShape> {
    use rewo_world::dimension::DimensionShape;
    r.i32().ok()?; // player id
    r.bool().ok()?; // hardcore
    let dim_count = r.count("dimensions", 1).ok()?;
    for _ in 0..dim_count {
        r.identifier().ok()?;
    }
    r.varint().ok()?; // max players
    r.varint().ok()?; // view dist
    r.varint().ok()?; // sim dist
    r.bool().ok()?; // reduced debug
    r.bool().ok()?; // show death
    r.bool().ok()?; // limited crafting
    let holder = r.varint().ok()?;
    if holder > 0 {
        dim_shapes.get((holder - 1) as usize).copied()
    } else {
        Some(DimensionShape::OVERWORLD)
    }
}

/// Read all recorded packets back into memory.
pub fn read_all(path: &std::path::Path) -> Result<Vec<RecordedPacket>, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("open recording: {e}"))?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic).map_err(|e| format!("read magic: {e}"))?;
    if &magic != MAGIC {
        return Err("not a rewo recording".into());
    }
    let mut out = Vec::new();
    loop {
        let mut header = [0u8; 9];
        match file.read_exact(&mut header) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format!("read header: {e}")),
        }
        let state = state_from_byte(header[0]).ok_or("bad state byte in recording")?;
        let id = i32::from_be_bytes(header[1..5].try_into().unwrap());
        let len = i32::from_be_bytes(header[5..9].try_into().unwrap());
        if len < 0 {
            return Err("negative record length".into());
        }
        let mut body = vec![0u8; len as usize];
        file.read_exact(&mut body).map_err(|e| format!("read body: {e}"))?;
        out.push(RecordedPacket { state, id, body });
    }
    Ok(out)
}
