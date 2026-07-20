//! Exact grip-field checkpoint encoding and bounded assembly.

use bevy::prelude::*;
use lightyear::prelude::Tick;

use crate::net::protocol::{GripCheckpointChunk, GripCheckpointEntry};
use crate::track::forces::GRIP_SHEAR_MODULUS_M;
use crate::track::sim::TrackGripElements;

use super::{CHECKPOINT_ENTRIES_PER_CHUNK, MAX_CHECKPOINT_CHUNKS, MAX_CHECKPOINT_LEDGERS};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ExactCheckpoint {
    pub(super) tank: Entity,
    pub(super) epoch: u32,
    pub(super) state_entering_tick: Tick,
    pub(super) field: TrackGripElements,
    pub(super) hash: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CheckpointKey {
    pub(super) tank: Entity,
    pub(super) epoch: u32,
    pub(super) tick: Tick,
    pub(super) hash: u64,
}

impl From<&ExactCheckpoint> for CheckpointKey {
    fn from(checkpoint: &ExactCheckpoint) -> Self {
        Self {
            tank: checkpoint.tank,
            epoch: checkpoint.epoch,
            tick: checkpoint.state_entering_tick,
            hash: checkpoint.hash,
        }
    }
}

#[derive(Clone, Debug)]
struct PartialCheckpoint {
    key: CheckpointKey,
    elements_per_side: u16,
    chunks: Vec<Option<Vec<GripCheckpointEntry>>>,
}

#[derive(Resource, Default)]
pub(super) struct CheckpointAssembler {
    partials: Vec<PartialCheckpoint>,
    completed: Vec<CheckpointKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AssemblyError {
    BadChunkShape,
    FieldShapeMismatch,
    ConflictingChunk,
    InvalidEntries,
    InvalidStrain,
    HashMismatch,
}

pub(super) fn hash_write(hash: &mut u64, bytes: impl IntoIterator<Item = u8>) {
    for byte in bytes {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn checkpoint_entries(field: &TrackGripElements) -> Option<(u16, Vec<GripCheckpointEntry>)> {
    let count = field.sides[0].strain.len();
    if count == 0
        || count > usize::from(u16::MAX)
        || field.sides.iter().any(|side| {
            side.strain.len() != count || side.dwell.len() != count || !count.is_multiple_of(3)
        })
    {
        return None;
    }
    let mut entries = Vec::new();
    for (side_index, side) in field.sides.iter().enumerate() {
        for (element, (&strain, &dwell)) in side.strain.iter().zip(&side.dwell).enumerate() {
            if dwell != 0 || strain.to_array().iter().any(|axis| axis.to_bits() != 0) {
                entries.push(GripCheckpointEntry {
                    side: side_index as u8,
                    element: element as u16,
                    strain,
                    contact_generation: dwell,
                });
            }
        }
    }
    Some((count as u16, entries))
}

pub(super) fn checkpoint_hash(
    epoch: u32,
    state_entering_tick: Tick,
    elements_per_side: u16,
    entries: &[GripCheckpointEntry],
) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    hash_write(&mut hash, epoch.to_le_bytes());
    hash_write(&mut hash, state_entering_tick.0.to_le_bytes());
    hash_write(&mut hash, elements_per_side.to_le_bytes());
    for entry in entries {
        hash_write(&mut hash, [entry.side]);
        hash_write(&mut hash, entry.element.to_le_bytes());
        for axis in entry.strain.to_array() {
            hash_write(&mut hash, axis.to_bits().to_le_bytes());
        }
        hash_write(&mut hash, [entry.contact_generation]);
    }
    hash
}

pub(super) fn make_checkpoint_chunks(
    tank: Entity,
    epoch: u32,
    state_entering_tick: Tick,
    field: &TrackGripElements,
) -> Option<Vec<GripCheckpointChunk>> {
    let (elements_per_side, entries) = checkpoint_entries(field)?;
    let hash = checkpoint_hash(epoch, state_entering_tick, elements_per_side, &entries);
    let chunk_count = entries.len().max(1).div_ceil(CHECKPOINT_ENTRIES_PER_CHUNK);
    if chunk_count > MAX_CHECKPOINT_CHUNKS || chunk_count > usize::from(u8::MAX) {
        return None;
    }
    let mut chunks = Vec::with_capacity(chunk_count);
    for chunk_index in 0..chunk_count {
        let start = chunk_index * CHECKPOINT_ENTRIES_PER_CHUNK;
        let end = (start + CHECKPOINT_ENTRIES_PER_CHUNK).min(entries.len());
        chunks.push(GripCheckpointChunk {
            tank,
            epoch,
            state_entering_tick,
            elements_per_side,
            chunk_index: chunk_index as u8,
            chunk_count: chunk_count as u8,
            entries: entries.get(start..end).unwrap_or_default().to_vec(),
            checkpoint_hash: hash,
        });
    }
    Some(chunks)
}

fn entries_bit_equal(a: &[GripCheckpointEntry], b: &[GripCheckpointEntry]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.side == b.side
                && a.element == b.element
                && a.contact_generation == b.contact_generation
                && a.strain
                    .to_array()
                    .iter()
                    .zip(b.strain.to_array())
                    .all(|(a, b)| a.to_bits() == b.to_bits())
        })
}

impl CheckpointAssembler {
    pub(super) fn push(
        &mut self,
        chunk: GripCheckpointChunk,
        expected_elements_per_side: usize,
    ) -> Result<Option<ExactCheckpoint>, AssemblyError> {
        let count = usize::from(chunk.chunk_count);
        let index = usize::from(chunk.chunk_index);
        if count == 0
            || count > MAX_CHECKPOINT_CHUNKS
            || index >= count
            || chunk.entries.len() > CHECKPOINT_ENTRIES_PER_CHUNK
            || chunk.elements_per_side == 0
            || !chunk.elements_per_side.is_multiple_of(3)
        {
            return Err(AssemblyError::BadChunkShape);
        }
        if usize::from(chunk.elements_per_side) != expected_elements_per_side {
            return Err(AssemblyError::FieldShapeMismatch);
        }
        for entry in &chunk.entries {
            if entry.side >= 2 || usize::from(entry.element) >= expected_elements_per_side {
                return Err(AssemblyError::InvalidEntries);
            }
            let axes = entry.strain.to_array();
            if axes
                .iter()
                .any(|axis| !axis.is_finite() || axis.abs() > GRIP_SHEAR_MODULUS_M)
                || entry.strain.length_squared() > GRIP_SHEAR_MODULUS_M * GRIP_SHEAR_MODULUS_M
            {
                return Err(AssemblyError::InvalidStrain);
            }
        }
        let key = CheckpointKey {
            tank: chunk.tank,
            epoch: chunk.epoch,
            tick: chunk.state_entering_tick,
            hash: chunk.checkpoint_hash,
        };
        if self.completed.contains(&key) {
            return Ok(None);
        }
        let partial_index = self
            .partials
            .iter()
            .position(|partial| partial.key == key)
            .unwrap_or_else(|| {
                if self.partials.len() >= MAX_CHECKPOINT_LEDGERS {
                    self.partials.remove(0);
                }
                self.partials.push(PartialCheckpoint {
                    key,
                    elements_per_side: chunk.elements_per_side,
                    chunks: vec![None; count],
                });
                self.partials.len() - 1
            });
        let partial = &mut self.partials[partial_index];
        if partial.elements_per_side != chunk.elements_per_side || partial.chunks.len() != count {
            self.partials.remove(partial_index);
            return Err(AssemblyError::ConflictingChunk);
        }
        if let Some(existing) = &partial.chunks[index] {
            if entries_bit_equal(existing, &chunk.entries) {
                return Ok(None);
            }
            self.partials.remove(partial_index);
            return Err(AssemblyError::ConflictingChunk);
        }
        partial.chunks[index] = Some(chunk.entries);
        if partial.chunks.iter().any(Option::is_none) {
            return Ok(None);
        }

        let partial = self.partials.remove(partial_index);
        let entries: Vec<_> = partial.chunks.into_iter().flatten().flatten().collect();
        let mut previous = None;
        let limit = usize::from(partial.elements_per_side);
        for entry in &entries {
            let id = (entry.side, entry.element);
            if entry.side >= 2
                || usize::from(entry.element) >= limit
                || previous.is_some_and(|previous| previous >= id)
            {
                return Err(AssemblyError::InvalidEntries);
            }
            previous = Some(id);
        }
        let actual_hash = checkpoint_hash(
            partial.key.epoch,
            partial.key.tick,
            partial.elements_per_side,
            &entries,
        );
        if actual_hash != partial.key.hash {
            return Err(AssemblyError::HashMismatch);
        }
        let link_count = limit / 3;
        let mut field = TrackGripElements::for_links(link_count);
        for entry in entries {
            let side = usize::from(entry.side);
            let element = usize::from(entry.element);
            field.sides[side].strain[element] = entry.strain;
            field.sides[side].dwell[element] = entry.contact_generation;
        }
        self.completed.push(partial.key);
        if self.completed.len() > MAX_CHECKPOINT_LEDGERS {
            self.completed.remove(0);
        }
        Ok(Some(ExactCheckpoint {
            tank: partial.key.tank,
            epoch: partial.key.epoch,
            state_entering_tick: partial.key.tick,
            field,
            hash: partial.key.hash,
        }))
    }
}
