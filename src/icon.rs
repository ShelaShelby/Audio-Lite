//! The tray icon, drawn in code (no image files): a blue circle with a white speaker.

/// What covers a point (x, y in 0..1): 0 = nothing, 1 = blue circle, 2 = white speaker.
fn shape(x: f32, y: f32) -> u8 {
    let (dx, dy) = (x - 0.5, y - 0.5);
    if dx * dx + dy * dy > 0.25 {
        return 0;
    }
    let ay = dy.abs();
    let body = (0.20..0.34).contains(&x) && ay < 0.10;
    let cone = (0.34..0.52).contains(&x) && ay < 0.10 + (x - 0.34) * 0.9;
    let (wx, wy) = (x - 0.52, y - 0.5);
    let r = (wx * wx + wy * wy).sqrt();
    let wave = wx > 0.06 && ay < wx * 1.1 && (0.18..0.25).contains(&r);
    if body || cone || wave {
        2
    } else {
        1
    }
}

/// `size` x `size` pixels, top row first, 4 bytes each in BGRA order (straight alpha).
/// Edges are smoothed with 4x4 samples per pixel.
pub fn pixels(size: usize) -> Vec<u8> {
    const BLUE: [f32; 3] = [0xD9 as f32, 0x6F as f32, 0x1E as f32]; // B, G, R
    const S: usize = 4;
    let mut out = Vec::with_capacity(size * size * 4);
    for py in 0..size {
        for px in 0..size {
            let (mut cover, mut white) = (0.0f32, 0.0f32);
            for sy in 0..S {
                for sx in 0..S {
                    let x = (px as f32 + (sx as f32 + 0.5) / S as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / S as f32) / size as f32;
                    match shape(x, y) {
                        1 => cover += 1.0,
                        2 => {
                            cover += 1.0;
                            white += 1.0;
                        }
                        _ => {}
                    }
                }
            }
            let a = cover / (S * S) as f32;
            let w = if cover > 0.0 { white / cover } else { 0.0 };
            for c in BLUE {
                out.push((c + (255.0 - c) * w).round() as u8);
            }
            out.push((a * 255.0).round() as u8);
        }
    }
    out
}

/// The same picture with the bytes in RGBA order (for the tray icon and the window).
pub fn rgba(size: usize) -> Vec<u8> {
    let mut p = pixels(size);
    for px in p.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_are_clear_and_middle_is_solid() {
        for size in [16, 20, 24, 32] {
            let p = pixels(size);
            assert_eq!(p.len(), size * size * 4);
            assert_eq!(p[3], 0, "top-left corner is transparent");
            let mid = (size / 2 * size + size / 2) * 4;
            assert_eq!(p[mid + 3], 255, "middle is opaque");
        }
    }

    #[test]
    fn rgba_swaps_red_and_blue() {
        let (b, r) = (pixels(16), rgba(16));
        assert_eq!(b.len(), r.len());
        for (x, y) in b.chunks_exact(4).zip(r.chunks_exact(4)) {
            assert_eq!([x[2], x[1], x[0], x[3]], [y[0], y[1], y[2], y[3]]);
        }
    }
}
