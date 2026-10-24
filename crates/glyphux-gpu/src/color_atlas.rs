//! Rectangle allocation on the pages of a color glyph atlas (a backend keeps the pages):
//! best fit from a list of free rectangles, merging neighbors when one is released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Takes the smallest free rectangle that fits `w` by `h` and returns that much of it (its
/// top-left corner); the rest stays free.
pub fn allocate(free: &mut Vec<Rect>, w: u32, h: u32) -> Option<Rect> {
    let i = free
        .iter()
        .enumerate()
        .filter(|(_, r)| r.w >= w && r.h >= h)
        .min_by_key(|(_, r)| u64::from(r.w) * u64::from(r.h))
        .map(|(i, _)| i)?;
    let r = free.swap_remove(i);
    if r.w > w {
        free.push(Rect {
            x: r.x + w,
            y: r.y,
            w: r.w - w,
            h,
        });
    }
    if r.h > h {
        free.push(Rect {
            x: r.x,
            y: r.y + h,
            w: r.w,
            h: r.h - h,
        });
    }
    Some(Rect { x: r.x, y: r.y, w, h })
}

/// Returns `r` to the free list, merged with free neighbors of the same width or height.
pub fn release(free: &mut Vec<Rect>, mut r: Rect) {
    loop {
        let merge = free.iter().enumerate().find_map(|(i, b)| {
            if r.y == b.y && r.h == b.h && (r.x + r.w == b.x || b.x + b.w == r.x) {
                Some((
                    i,
                    Rect {
                        x: r.x.min(b.x),
                        w: r.w + b.w,
                        ..r
                    },
                ))
            } else if r.x == b.x && r.w == b.w && (r.y + r.h == b.y || b.y + b.h == r.y) {
                Some((
                    i,
                    Rect {
                        y: r.y.min(b.y),
                        h: r.h + b.h,
                        ..r
                    },
                ))
            } else {
                None
            }
        });
        let Some((i, merged)) = merge else {
            break;
        };
        free.swap_remove(i);
        r = merged;
    }
    free.push(r);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reuse_and_coalesce() {
        let whole = Rect {
            x: 0,
            y: 0,
            w: 32,
            h: 32,
        };
        let mut free = vec![whole];
        let a = allocate(&mut free, 16, 16).unwrap();
        let b = allocate(&mut free, 16, 16).unwrap();
        let c = allocate(&mut free, 32, 16).unwrap();
        assert!(allocate(&mut free, 1, 1).is_none());
        release(&mut free, a);
        release(&mut free, c);
        release(&mut free, b);
        assert_eq!(allocate(&mut free, 32, 32), Some(whole));
    }
}
