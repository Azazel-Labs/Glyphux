//! CPU preparation of glyphs off the calling thread.
//!
//! [`crate::GlyphAtlas::get`] only loads an outline and reserves its atlas rectangle (that is
//! all layout needs); the rest of the CPU work — normalize, orient, color edges, curve
//! preparation, tile binning, corner protection — is queued here and runs on worker threads
//! while the application goes on with its frame. [`crate::GlyphAtlas::flush`] collects the
//! results, doing any job no worker has started yet itself, so every glyph requested in a
//! frame is generated in that frame. Detail probes share the workers at lower priority; their
//! results may arrive in later frames. Without threads (`wasm32`), everything runs in `flush`.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use msdfr::gpu::GpuShape;
use msdfr::{PreparedShape, Shape, Transform};

use crate::atlas::{AtlasConfig, GlyphKey, probe_detail};
use crate::field::{FieldJob, PreparedJob, prepare_job_owned};

/// One glyph to prepare.
pub(crate) struct PrepJob {
    pub id: u64,
    pub shape: Shape,
    pub transform: Transform,
    pub width: u32,
    pub height: u32,
    pub origin: [u32; 3],
    /// Distance range of this glyph's field in texels (the atlas range times its tier).
    pub px_range: f64,
    /// Also queue a detail probe for this glyph.
    pub probe: Option<GlyphKey>,
}

pub(crate) enum Done {
    /// `None` when the outline turned out empty.
    Prep(u64, Option<Box<PreparedJob>>),
    Probe(GlyphKey, u32),
}

#[derive(Default)]
struct Queues {
    prep: VecDeque<PrepJob>,
    probe: VecDeque<(GlyphKey, Shape)>,
    shutdown: bool,
}

struct Shared {
    queues: Mutex<Queues>,
    wake: Condvar,
    done: Mutex<Vec<Done>>,
    /// Signalled whenever a result is pushed.
    done_wake: Condvar,
    config: AtlasConfig,
}

pub(crate) struct Workers {
    shared: Arc<Shared>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

/// Normalized, oriented and colored shape, or `None` if there is nothing to draw.
pub(crate) fn prepare_shape(mut shape: Shape, cfg: &AtlasConfig) -> Option<Shape> {
    shape.normalize();
    if shape.contours.is_empty() {
        return None;
    }
    // TrueType and PostScript outlines wind in opposite directions.
    shape.orient_contours(msdfr::FillRule::NonZero);
    if cfg.kind.channels == 3 {
        msdfr::edge_coloring(&mut shape, cfg.coloring, cfg.angle_threshold, 0);
    }
    Some(shape)
}

/// Locks `m`, ignoring poisoning: every critical section here leaves the data consistent, and
/// a panicking job must not take the atlas down with it.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Prepares `job` and reports it as done, also when preparing it panics (the glyph is then
/// treated as empty): [`Workers::finish_preps`] waits for every job's result.
fn run_prep(job: PrepJob, shared: &Shared) {
    let id = job.id;
    let out = catch_unwind(AssertUnwindSafe(|| prepare(job, shared))).unwrap_or(None);
    lock(&shared.done).push(Done::Prep(id, out));
    shared.done_wake.notify_all();
}

fn prepare(job: PrepJob, shared: &Shared) -> Option<Box<PreparedJob>> {
    let cfg = &shared.config;
    prepare_shape(job.shape, cfg).map(|shape| {
        let prepared = PreparedShape::new(&shape);
        let gpu = GpuShape::new(&prepared);
        let field_job = FieldJob {
            shape: &prepared,
            gpu: &gpu,
            transform: job.transform,
            width: job.width,
            height: job.height,
            kind: cfg.kind,
            overlap: cfg.overlap_support,
            error_correction: cfg.error_correction,
            atlas_origin: job.origin,
            band: if cfg.narrow_band { 0.5 * job.px_range + 2.0 } else { 0.0 },
            sign_correction: cfg.sign_correction,
        };
        let result = prepare_job_owned(&field_job, gpu.clone());
        if let Some(key) = job.probe {
            lock(&shared.queues).probe.push_back((key, shape));
            shared.wake.notify_one();
        }
        Box::new(result)
    })
}

/// Probes a glyph's detail; a probe that panics is dropped (the glyph keeps its size level).
fn run_probe(key: GlyphKey, shape: &Shape, shared: &Shared) {
    if let Ok(level) = catch_unwind(AssertUnwindSafe(|| probe_detail(shape, &shared.config))) {
        lock(&shared.done).push(Done::Probe(key, level));
        shared.done_wake.notify_all();
    }
}

impl Workers {
    pub fn new(config: &AtlasConfig) -> Self {
        let shared = Arc::new(Shared {
            queues: Mutex::new(Queues::default()),
            wake: Condvar::new(),
            done: Mutex::new(Vec::new()),
            done_wake: Condvar::new(),
            config: config.clone(),
        });
        // No threads on wasm32: `finish_preps` runs everything on the calling thread.
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut threads = Vec::new();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let n = match config.worker_threads {
                0 => std::thread::available_parallelism()
                    .map_or(1, |n| n.get().saturating_sub(1))
                    .clamp(1, 4),
                n => n,
            };
            for i in 0..n {
                let s = shared.clone();
                let spawned = std::thread::Builder::new()
                    .name(format!("msdfr-prep-{i}"))
                    .spawn(move || {
                        loop {
                            let job = {
                                let mut q = lock(&s.queues);
                                loop {
                                    if q.shutdown {
                                        return;
                                    }
                                    if let Some(j) = q.prep.pop_front() {
                                        break Ok(j);
                                    }
                                    if let Some(p) = q.probe.pop_front() {
                                        break Err(p);
                                    }
                                    q = s.wake.wait(q).unwrap_or_else(std::sync::PoisonError::into_inner);
                                }
                            };
                            match job {
                                Ok(j) => run_prep(j, &s),
                                Err((key, shape)) => run_probe(key, &shape, &s),
                            }
                        }
                    });
                if let Ok(handle) = spawned {
                    threads.push(handle);
                }
            }
        }
        Workers { shared, threads }
    }

    pub fn submit(&self, job: PrepJob) {
        lock(&self.shared.queues).prep.push_back(job);
        self.shared.wake.notify_one();
    }

    /// Results so far (preparations and probes).
    pub fn take_done(&self) -> Vec<Done> {
        std::mem::take(&mut *lock(&self.shared.done))
    }

    /// Runs queued preparations on the calling thread until none is left to start, then
    /// waits until `outstanding` preparation results have arrived in total (counting those
    /// returned by earlier `take_done` calls is the caller's job). Returns new results.
    pub fn finish_preps(&self, mut outstanding: usize) -> Vec<Done> {
        let mut out = Vec::new();
        loop {
            // Help with jobs nobody has picked up yet.
            let job = lock(&self.shared.queues).prep.pop_front();
            match job {
                Some(j) => run_prep(j, &self.shared),
                None => break,
            }
        }
        let mut done = lock(&self.shared.done);
        loop {
            for d in done.drain(..) {
                if matches!(d, Done::Prep(..)) {
                    outstanding = outstanding.saturating_sub(1);
                }
                out.push(d);
            }
            if outstanding == 0 {
                return out;
            }
            done = self
                .shared
                .done_wake
                .wait(done)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Without worker threads: run up to `n` queued probes on the calling thread.
    pub fn run_probes_here(&self, n: usize) {
        if !self.threads.is_empty() {
            return;
        }
        for _ in 0..n {
            let p = lock(&self.shared.queues).probe.pop_front();
            match p {
                Some((key, shape)) => run_probe(key, &shape, &self.shared),
                None => break,
            }
        }
    }
}

impl Drop for Workers {
    /// Stops the workers: queued work is dropped, a job in progress is finished.
    fn drop(&mut self) {
        {
            let mut q = lock(&self.shared.queues);
            q.shutdown = true;
            q.prep.clear();
            q.probe.clear();
        }
        self.shared.wake.notify_all();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}
