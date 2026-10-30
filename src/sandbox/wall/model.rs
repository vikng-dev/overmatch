//! The wall prototype's model, about the game's environment destruction (game state only): the
//! game's own fracture laws ([`crate::buildings::fracture`], the laws' one home), and the cut the
//! sandbox draws them on.

pub(crate) use crate::buildings::fracture::*;

/// A cut: the structure the laws read, and per chunk and joint what the view draws.
#[derive(Clone, Debug)]
pub(crate) struct Cut {
    pub structure: Structure,
    /// Each chunk's shape, index-aligned with the table.
    pub blocks: Vec<Block>,
    /// Each joint's contact polygon, index-aligned with the joints.
    pub faces: Vec<Vec<bevy::math::Vec3>>,
}
