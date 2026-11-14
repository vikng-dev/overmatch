//! Miscallaneous utility functions.

use bevy::ecs::{
    query::{IterQueryData, QueryFilter, QueryItem},
    system::Query,
};

pub(crate) use bevy::platform::time::Instant;

/// OVERMATCH PATCH: the matched bodies below which a per-body loop runs serially. Resolution
/// (cost): a two-thread `par_iter` costs 2.6 µs per call whatever it finds and these loops cost
/// 6-66 ns per body, so a perfect two-way split breaks even at 80-880 bodies; this is the next
/// power of two above the highest (MEASURED and derivation in `OVERMATCH_PATCH.md`).
pub(crate) const MIN_PAR_BODIES: usize = 1024;

/// OVERMATCH PATCH: query iteration that dispatches to the compute pool only from
/// `min_parallel_len` matched entities, as upstream's `ParallelQueryForEach` (avian #1035):
/// Bevy's `par_iter` opens a task scope whenever the pool has two threads, even for one batch.
/// Each item is processed by the same closure either way, so the choice cannot change a result.
pub(crate) trait ParallelQueryForEach<'s, D: IterQueryData> {
    /// Calls `f` for every mutable query item, in parallel only from `min_parallel_len` matches.
    fn par_for_each_mut<FN>(&mut self, min_parallel_len: usize, f: FN)
    where
        FN: for<'a> Fn(QueryItem<'a, 's, D>) + Send + Sync + Clone;
}

impl<'s, D: IterQueryData, F: QueryFilter> ParallelQueryForEach<'s, D> for Query<'_, 's, D, F> {
    #[inline(always)]
    #[allow(unused_variables)]
    fn par_for_each_mut<FN>(&mut self, min_parallel_len: usize, f: FN)
    where
        FN: for<'a> Fn(QueryItem<'a, 's, D>) + Send + Sync + Clone,
    {
        #[cfg(feature = "parallel")]
        {
            if bevy::tasks::ComputeTaskPool::get().thread_num() > 1
                && self.count() >= min_parallel_len
            {
                self.par_iter_mut().for_each(f);
                return;
            }
        }
        self.iter_mut().for_each(f);
    }
}

// TODO: The single-threaded and multi-threaded versions are duplicated here because
//       of the different trait bounds on `F`. Unify them somehow?

/// A helper function for iterating over a slice in parallel or serially
/// based on the `parallel` feature.
///
/// If `slice.len() < min_len`, serial iteration will be used.
///
/// The `ComputeTaskPool` is used if parallelism is enabled.
/// OVERMATCH PATCH: returns whether parallel chunks were actually dispatched.
///
/// # Example
///
/// ```ignore
/// let mut slice = vec![1, 2, 3, 4];
///
/// par_for_each(&mut slice, 64, |index, item| {
///     *item += index;
/// });
///
/// assert_eq!(slice, vec![1, 3, 5, 7]);
/// ```
#[inline(always)]
#[allow(unused_variables, unused_mut)]
#[cfg(not(feature = "parallel"))]
pub fn par_for_each<T, F>(mut slice: &mut [T], min_len: usize, mut f: F) -> bool
where
    T: Send + Sync,
    F: FnMut(usize, &mut T) + Send + Sync,
{
    slice.iter_mut().enumerate().for_each(|(index, item)| {
        f(index, item);
    });
    false
}

/// A helper function for iterating over a slice in parallel or serially
/// based on the `parallel` feature.
///
/// If `slice.len() < min_len`, serial iteration will be used.
///
/// The `ComputeTaskPool` is used if parallelism is enabled.
/// OVERMATCH PATCH: returns whether parallel chunks were actually dispatched.
///
/// # Example
///
/// ```ignore
/// let mut slice = vec![1, 2, 3, 4];
///
/// par_for_each(&mut slice, 64, |index, item| {
///     *item += index;
/// });
///
/// assert_eq!(slice, vec![1, 3, 5, 7]);
/// ```
#[inline(always)]
#[allow(unused_variables, unused_mut)]
#[cfg(feature = "parallel")]
pub fn par_for_each<T, F>(mut slice: &mut [T], min_len: usize, mut f: F) -> bool
where
    T: Send + Sync,
    F: Fn(usize, &mut T) + Send + Sync,
{
    let task_pool_ = bevy::tasks::ComputeTaskPool::get();

    if task_pool_.thread_num() == 1 || slice.len() < min_len {
        slice.iter_mut().enumerate().for_each(|(index, item)| {
            f(index, item);
        });
        false
    } else {
        // TODO: Is there a better approach than `par_chunk_map_mut`?
        let chunk_size_ = (slice.len() / task_pool_.thread_num()).max(1);
        bevy::tasks::ParallelSliceMut::par_chunk_map_mut(
            &mut slice,
            task_pool_,
            chunk_size_,
            |chunk_index_, chunk_| {
                let index_offset_ = chunk_index_ * chunk_size_;
                chunk_.iter_mut().enumerate().for_each(|(i, item)| {
                    let index = index_offset_ + i;
                    f(index, item);
                });
            },
        );
        true
    }
}
