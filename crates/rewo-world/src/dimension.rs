//! Dimension vertical shape — `min_y` and `height` drive how many 16³
//! sections a column has. These come from the `minecraft:dimension_type`
//! registry synced during Configuration (each entry's NBT carries `min_y`
//! and `height`); the game's Login packet references one by registry index.

/// Vertical extent of a dimension, in blocks/sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DimensionShape {
    pub min_y: i32,
    pub height: i32,
}

impl DimensionShape {
    /// Vanilla overworld (-64..320). The safe default when the dimension
    /// registry hasn't been parsed yet — the M1 flat-world test lives here.
    pub const OVERWORLD: DimensionShape = DimensionShape {
        min_y: -64,
        height: 384,
    };

    pub fn section_count(&self) -> usize {
        (self.height / 16) as usize
    }

    pub fn min_section(&self) -> i32 {
        self.min_y >> 4
    }

    /// Section index (0-based from the bottom) for a world y, if in range.
    pub fn section_index(&self, y: i32) -> Option<usize> {
        if y < self.min_y || y >= self.min_y + self.height {
            return None;
        }
        Some(((y >> 4) - self.min_section()) as usize)
    }
}
