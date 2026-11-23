//! Pure geometry shared by the store and by the routes that choose a request size: aspect-preserving dimensions
//! under a total-pixel budget, or with an exact long edge.

use vivianne_core::attachment::Dimensions;

/// Aspect-preserving integer dimensions within a hard total-pixel budget, rounded inward. A small image is not
/// enlarged.
#[must_use]
pub fn request_image_dimensions(width: u32, height: u32, max_pixels: u64) -> Dimensions {
    let (w, h) = (f64::from(width), f64::from(height));
    let scale = (max_pixels as f64 / (w * h)).sqrt().min(1.0);
    if scale >= 1.0 {
        return Dimensions { width, height };
    }
    let fits = |a: u64, b: u64| a * b <= max_pixels;
    let round = |v: f64| (v.round() as u64).max(1);
    if width >= height {
        let mut projected_w = ((w * scale).floor() as u64).max(1);
        let mut projected_h = round(projected_w as f64 * h / w);
        while !fits(projected_w, projected_h) && projected_w > 1 {
            projected_w -= 1;
            projected_h = round(projected_w as f64 * h / w);
        }
        return Dimensions {
            width: projected_w as u32,
            height: projected_h as u32,
        };
    }
    let mut projected_h = ((h * scale).floor() as u64).max(1);
    let mut projected_w = round(projected_h as f64 * w / h);
    while !fits(projected_w, projected_h) && projected_h > 1 {
        projected_h -= 1;
        projected_w = round(projected_h as f64 * w / h);
    }
    Dimensions {
        width: projected_w as u32,
        height: projected_h as u32,
    }
}

/// Aspect-preserving dimensions with an exact long edge; the short edge rounds to the nearest pixel. A long edge at
/// or above the source returns the source unchanged.
#[must_use]
pub fn long_edge_dimensions(width: u32, height: u32, long_edge: u32) -> Dimensions {
    if long_edge >= width.max(height) {
        return Dimensions { width, height };
    }
    let (w, h, l) = (f64::from(width), f64::from(height), f64::from(long_edge));
    if width >= height {
        Dimensions {
            width: long_edge,
            height: ((l * h / w).round() as u32).max(1),
        }
    } else {
        Dimensions {
            width: ((l * w / h).round() as u32).max(1),
            height: long_edge,
        }
    }
}
