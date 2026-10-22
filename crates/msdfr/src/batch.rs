//! Parallel batch helpers.
//!
//! Distance fields for font atlases are small and numerous; distributing whole glyphs over
//! threads scales much better than splitting each small glyph's rows. Use these helpers with
//! `GeneratorConfig { threads: 1, .. }` for the per-glyph work.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// The number of cores to use when a caller asks for "all of them". Looked up once: the
/// lookup reads cgroup files on Linux, too slow to repeat for every generated field.
pub(crate) fn available_cores() -> usize {
    static CORES: OnceLock<usize> = OnceLock::new();
    *CORES.get_or_init(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get))
}

/// Applies `f` to every item using up to `threads` worker threads (0 = all available
/// cores), returning the results in input order. Work is handed out dynamically, so items of
/// very different cost balance well.
pub fn par_map<T: Sync, R: Send>(items: &[T], threads: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let n = items.len();
    let threads = if threads == 0 { available_cores() } else { threads };
    let threads = threads.clamp(1, n.max(1));
    if threads == 1 {
        return items.iter().map(f).collect();
    }
    let results: Vec<Mutex<Option<R>>> = (0..n).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let r = f(&items[i]);
                    *results[i].lock().unwrap() = Some(r);
                }
            });
        }
    });
    results.into_iter().map(|m| m.into_inner().unwrap().unwrap()).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn order_preserved() {
        let v: Vec<u32> = (0..1000).collect();
        let r = super::par_map(&v, 4, |x| x * 2);
        assert_eq!(r, v.iter().map(|x| x * 2).collect::<Vec<_>>());
    }
}
