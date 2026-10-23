//! GPU timestamps around the generator's passes (and any render pass you add), read back
//! without stalling: results arrive a frame or two later.
//!
//! Needs [`wgpu::Features::TIMESTAMP_QUERY`] on the device ([`GpuProfiler::new`] returns
//! `None` without it). Native apps must poll the device now and then
//! (`device.poll(wgpu::PollType::Poll)`) for readbacks to complete; browsers do it on their own.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// States of a readback.
const PENDING: u8 = 0;
const MAPPED: u8 = 1;
const FAILED: u8 = 2;

/// Collects begin/end timestamps of labelled passes within a frame.
pub struct GpuProfiler {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    capacity: u32,
    labels: Vec<&'static str>,
    period_ns: f64,
    free: Vec<wgpu::Buffer>,
    in_flight: Vec<(wgpu::Buffer, Vec<&'static str>, Arc<AtomicU8>)>,
    last: Vec<(&'static str, f64)>,
}

impl GpuProfiler {
    /// Room for `max_passes` timed passes per frame.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, max_passes: u32) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let capacity = max_passes * 2;
        let set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("glyphux timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: capacity,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glyphux timestamps resolve"),
            size: capacity as u64 * 8,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Some(GpuProfiler {
            set,
            resolve,
            capacity,
            labels: Vec::new(),
            period_ns: queue.get_timestamp_period() as f64,
            free: Vec::new(),
            in_flight: Vec::new(),
            last: Vec::new(),
        })
    }

    /// Starts a frame: scopes reserved since the last [`Self::resolve`] are dropped. Only
    /// needed when a frame may reserve scopes without resolving them (scopes otherwise pile
    /// up until [`Self::scope`] runs out of room).
    pub fn begin_frame(&mut self) {
        self.labels.clear();
    }

    /// Reserves the timestamps of one pass; `None` when the frame's room is used up.
    pub fn scope(&mut self, label: &'static str) -> Option<u32> {
        let i = self.labels.len() as u32;
        if 2 * i + 2 > self.capacity {
            return None;
        }
        self.labels.push(label);
        Some(i)
    }

    pub fn compute_writes(&self, scope: u32) -> wgpu::ComputePassTimestampWrites<'_> {
        wgpu::ComputePassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: Some(2 * scope),
            end_of_pass_write_index: Some(2 * scope + 1),
        }
    }

    pub fn render_writes(&self, scope: u32) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: Some(2 * scope),
            end_of_pass_write_index: Some(2 * scope + 1),
        }
    }

    /// Records the copy of this frame's timestamps; call after the frame's passes, before
    /// `encoder.finish()`. Returns a token for [`Self::submitted`].
    pub fn resolve(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) -> Option<Readback> {
        let n = self.labels.len() as u32 * 2;
        if n == 0 {
            return None;
        }
        let buffer = self.free.pop().unwrap_or_else(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("glyphux timestamps readback"),
                size: self.capacity as u64 * 8,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        encoder.resolve_query_set(&self.set, 0..n, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &buffer, 0, n as u64 * 8);
        Some(Readback {
            buffer,
            labels: std::mem::take(&mut self.labels),
        })
    }

    /// Starts reading back a resolved frame once its commands are submitted.
    pub fn submitted(&mut self, readback: Readback) {
        let done = Arc::new(AtomicU8::new(PENDING));
        let flag = done.clone();
        let size = readback.labels.len() as u64 * 16;
        readback.buffer.slice(..size).map_async(wgpu::MapMode::Read, move |r| {
            flag.store(if r.is_ok() { MAPPED } else { FAILED }, Ordering::Release);
        });
        self.in_flight.push((readback.buffer, readback.labels, done));
    }

    /// Durations (ms) of the latest frame whose timestamps have arrived, by pass label.
    pub fn latest(&mut self) -> &[(&'static str, f64)] {
        let mut i = 0;
        while i < self.in_flight.len() {
            match self.in_flight[i].2.load(Ordering::Acquire) {
                PENDING => {
                    i += 1;
                    continue;
                }
                // A failed mapping (e.g. a lost device) never completes: drop it rather than
                // keep it forever.
                FAILED => {
                    self.in_flight.remove(i);
                    continue;
                }
                _ => {}
            }
            let (buffer, labels, _) = self.in_flight.remove(i);
            {
                let size = labels.len() as u64 * 16;
                let data = buffer.slice(..size).get_mapped_range().unwrap();
                let ticks: Vec<u64> = data
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
                    .collect();
                self.last = labels
                    .iter()
                    .enumerate()
                    .map(|(k, &l)| {
                        (
                            l,
                            ticks[2 * k + 1].saturating_sub(ticks[2 * k]) as f64 * self.period_ns * 1e-6,
                        )
                    })
                    .collect();
            }
            buffer.unmap();
            self.free.push(buffer);
        }
        &self.last
    }
}

/// A frame's timestamps on their way back (see [`GpuProfiler::resolve`]).
pub struct Readback {
    buffer: wgpu::Buffer,
    labels: Vec<&'static str>,
}
