//! Live progress of an indexing run, shared with other threads (the explorer's progress
//! bar, the terminal) without locking: the indexer bumps atomics, readers take snapshots.

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use serde::Serialize;

/// Indexing stages, in run order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Idle,
    /// Walking the project for source files (total unknown; `done` counts files found).
    Discover,
    /// Reading and hashing changed files.
    Read,
    /// Tree-sitter extraction.
    Parse,
    /// Writing symbols and facts.
    Store,
    /// Resolving references into edges.
    Resolve,
    /// Project-wide relations (Unity messages, tests, routes, error flow, literals).
    Link,
    /// Committing the transaction.
    Save,
    Done,
}

const STAGES: [Stage; 9] =
    [Stage::Idle, Stage::Discover, Stage::Read, Stage::Parse, Stage::Store, Stage::Resolve, Stage::Link, Stage::Save, Stage::Done];

impl Stage {
    /// Share of the whole run (percent range) a stage covers, from measured release timings
    /// of a first index of a 1.4k-file Unity project (bulk load: rebuilding indexes and
    /// committing take about a third).
    fn span(self) -> (f32, f32) {
        match self {
            Stage::Idle => (0.0, 0.0),
            Stage::Discover => (0.0, 4.0),
            Stage::Read => (4.0, 6.0),
            Stage::Parse => (6.0, 23.0),
            Stage::Store => (23.0, 42.0),
            Stage::Resolve => (42.0, 60.0),
            Stage::Link => (60.0, 66.0),
            Stage::Save => (66.0, 100.0),
            Stage::Done => (100.0, 100.0),
        }
    }
}

#[derive(Default, Debug)]
pub struct Progress {
    stage: AtomicU8,
    done: AtomicUsize,
    total: AtomicUsize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProgressSnapshot {
    pub stage: Stage,
    /// 1-based position of the stage among the working stages (Discover … Save).
    pub step: u8,
    pub steps: u8,
    pub done: usize,
    /// 0 when the stage has no known size (discovery, commit).
    pub total: usize,
    /// Overall completion, 0–100.
    pub percent: f32,
}

impl Progress {
    pub fn new() -> Progress {
        Progress::default()
    }

    /// Enters `stage` with `total` work items (0 = unknown).
    pub fn begin(&self, stage: Stage, total: usize) {
        self.done.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        self.stage.store(stage as u8, Ordering::Release);
    }

    /// One more work item finished in the current stage.
    pub fn tick(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_done(&self, done: usize) {
        self.done.store(done, Ordering::Relaxed);
    }

    pub fn stage(&self) -> Stage {
        STAGES.get(self.stage.load(Ordering::Acquire) as usize).copied().unwrap_or(Stage::Idle)
    }

    pub fn snapshot(&self) -> ProgressSnapshot {
        let stage = self.stage();
        let done = self.done.load(Ordering::Relaxed);
        let total = self.total.load(Ordering::Relaxed);
        let (lo, hi) = stage.span();
        let frac = if total > 0 { (done.min(total) as f32) / total as f32 } else { 0.0 };
        let step = match stage {
            Stage::Idle => 0,
            Stage::Done => 7,
            s => s as u8,
        };
        ProgressSnapshot { stage, step, steps: 7, done, total, percent: ((lo + (hi - lo) * frac) * 10.0).round() / 10.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_moves_within_the_stage_span() {
        let p = Progress::new();
        assert_eq!(p.snapshot().percent, 0.0);
        p.begin(Stage::Store, 4);
        assert_eq!(p.snapshot().percent, 23.0);
        p.tick();
        p.tick();
        assert_eq!(p.snapshot().percent, 32.5);
        assert_eq!(p.snapshot().step, 4);
        p.begin(Stage::Done, 0);
        assert_eq!(p.snapshot().percent, 100.0);
    }
}
