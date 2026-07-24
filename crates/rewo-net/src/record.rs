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
/// the rebuilt `World` + chunks decoded. Callers digest/query it — the DoD
/// equivalence check compares a live session's digest against its replay's.
pub fn replay(
    path: &std::path::Path,
    data: &rewo_data::GameData,
    ids: &crate::ids::Ids,
) -> Result<(rewo_world::World, u64), String> {
    use rewo_proto::reader::PacketReader;
    use rewo_world::dimension::{DimensionShape, DimensionTypeDef};

    let packets = read_all(path)?;
    let mut world = rewo_world::World::new(DimensionShape::OVERWORLD);
    let mut dim_types: Vec<DimensionTypeDef> = Vec::new();
    let mut chunks = 0u64;

    for p in &packets {
        if p.state != State::Play {
            // Replay the dimension registry from config so the login holder
            // resolves. Same parser as the live path — a replay must not be
            // able to derive a different shape from the same bytes.
            if p.state == State::Configuration && p.id == ids.cb_config_registry_data {
                if let Some(defs) =
                    crate::dimension_parse::parse_dimension_registry_packet(&p.body)?
                {
                    dim_types = defs;
                }
            }
            continue;
        }
        if p.id == ids.cb_play_login {
            let mut r = PacketReader::new(&p.body);
            if let Some(def) = read_login_dimension(&mut r, &dim_types) {
                world.apply_dimension_type(&def);
            }
        } else if p.id == ids.cb_play_level_chunk {
            let mut r = PacketReader::new(&p.body);
            if let Ok(col) = rewo_world::chunk::read_level_chunk(&mut r, &world.shape, &data.blocks)
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
    Ok((world, chunks))
}

/// Read a recorded login packet's prefix and resolve the dimension-type
/// definition it names. The registry parse lives in `crate::dimension_parse`
/// and the selection in `crate::login_dimension_type` — replay derives its
/// shape from the same two functions the live session does, never a second
/// implementation of either.
fn read_login_dimension(
    r: &mut rewo_proto::reader::PacketReader,
    dim_types: &[rewo_world::dimension::DimensionTypeDef],
) -> Option<rewo_world::dimension::DimensionTypeDef> {
    crate::spawn_info::read_login_prefix(r).ok()?;
    // The dimension-type holder is the raw 0-based registry id (holderRegistry
    // idMapper — see `crate::parse_login_dimension_holder`); there is no
    // `0=inline`/`id+1` convention.
    let holder = r.varint().ok()?;
    Some(crate::login_dimension_type(holder, dim_types).into_owned())
}

/// Read all recorded packets back into memory.
pub fn read_all(path: &std::path::Path) -> Result<Vec<RecordedPacket>, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("open recording: {e}"))?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)
        .map_err(|e| format!("read magic: {e}"))?;
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
        file.read_exact(&mut body)
            .map_err(|e| format!("read body: {e}"))?;
        out.push(RecordedPacket { state, id, body });
    }
    Ok(out)
}
