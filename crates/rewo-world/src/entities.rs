//! Entity table — id → transform + type. M1 stores what Add Entity carries;
//! interpolation history + rendering land in M3.

use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
pub struct EntityState {
    pub uuid: u128,
    pub type_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Default)]
pub struct EntityTable {
    map: HashMap<i32, EntityState>,
}

impl EntityTable {
    pub fn add(&mut self, id: i32, state: EntityState) {
        self.map.insert(id, state);
    }

    pub fn remove(&mut self, id: i32) {
        self.map.remove(&id);
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn get(&self, id: i32) -> Option<&EntityState> {
        self.map.get(&id)
    }
}
