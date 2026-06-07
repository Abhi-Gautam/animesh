//! Cover-art → palette extraction.
//!
//! We no longer render the cover itself. Instead, we extract ~5
//! dominant saturated colors and use them to gradient-paint the title.
//! The cover image becomes *data driving identity*, not a thing
//! displayed — Spotify-color / Linear-spine territory.
//!
//! Stored as a sentinel-prefixed string in `tracked_item.cover_ascii`:
//!   `v4:rrggbb,rrggbb,rrggbb,rrggbb,rrggbb`
//! ~40 bytes per cover, vs ~9.6KB for the prior pixel encoding.

use std::collections::HashMap;

use anyhow::{Context, Result};
use image::imageops::FilterType;

/// Format sentinel — bump when the encoded layout changes so the
/// startup backfill auto-upgrades rows written by an older version.
pub const FORMAT_TAG: &str = "v4:";

/// Number of palette entries to extract. Five gives smooth gradient
/// interpolation across an average title (12–25 chars) without
/// over-fragmenting at short titles.
const PALETTE_K: usize = 5;

/// Public entry — back-compat name (still called by `follow.rs`).
/// The `cols`/`rows` args are kept so the existing call sites don't
/// churn; they're unused now (no longer a pixel grid).
pub fn render_ascii(bytes: &[u8], _cols: u32, _rows: u32) -> Result<String> {
    let palette = extract_palette(bytes, PALETTE_K)?;
    let mut out = String::with_capacity(FORMAT_TAG.len() + PALETTE_K * 7);
    out.push_str(FORMAT_TAG);
    for (i, (r, g, b)) in palette.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!("{:02x}{:02x}{:02x}", r, g, b));
    }
    Ok(out)
}

/// Decode a stored palette string. Returns `None` if the input is
/// missing/malformed; callers fall back to the default title color.
pub fn decode(stored: &str) -> Option<Vec<(u8, u8, u8)>> {
    let body = stored.strip_prefix(FORMAT_TAG)?;
    let mut out = Vec::new();
    for part in body.split(',') {
        if part.len() != 6 {
            return None;
        }
        let bs = part.as_bytes();
        out.push((
            hex_byte(bs[0], bs[1])?,
            hex_byte(bs[2], bs[3])?,
            hex_byte(bs[4], bs[5])?,
        ));
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Smooth-interpolate a position `t ∈ [0,1]` across the palette.
/// Used by the title-painter to color each character.
pub fn lerp(palette: &[(u8, u8, u8)], t: f32) -> (u8, u8, u8) {
    if palette.is_empty() {
        return (200, 200, 200);
    }
    if palette.len() == 1 {
        return palette[0];
    }
    let t = t.clamp(0.0, 1.0);
    let seg = t * (palette.len() - 1) as f32;
    let i = (seg as usize).min(palette.len() - 2);
    let frac = seg - i as f32;
    let a = palette[i];
    let b = palette[i + 1];
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * frac) as u8;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

// -------------------------------------------------------------------------
// Extraction. Strategy: aggressively downsample → bucket each pixel into a
// coarse RGB cube (4 bits per channel = 4096 buckets) → average within each
// bucket → drop near-grays and luminance extremes → take the top K by
// occurrence count. Matches the Python prototype's quantize+filter output.
// -------------------------------------------------------------------------

fn extract_palette(bytes: &[u8], k: usize) -> Result<Vec<(u8, u8, u8)>> {
    let img = image::load_from_memory(bytes).context("decode cover image")?;
    // Small thumbnail: 24×24 = 576 pixels is plenty for palette stats
    // and trivially fast to bucket.
    let small = img.resize_exact(24, 24, FilterType::Lanczos3).to_rgb8();

    // bucket_key (12-bit) → (count, sum_r, sum_g, sum_b)
    let mut buckets: HashMap<u16, (u32, u32, u32, u32)> = HashMap::new();
    for px in small.pixels() {
        let key = ((px[0] as u16 >> 4) << 8)
            | ((px[1] as u16 >> 4) << 4)
            | (px[2] as u16 >> 4);
        let e = buckets.entry(key).or_insert((0, 0, 0, 0));
        e.0 += 1;
        e.1 += px[0] as u32;
        e.2 += px[1] as u32;
        e.3 += px[2] as u32;
    }

    let mut sorted: Vec<_> = buckets.into_iter().collect();
    sorted.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));

    let mut out: Vec<(u8, u8, u8)> = Vec::with_capacity(k);
    for (_key, (count, sr, sg, sb)) in &sorted {
        let r = (sr / count) as u8;
        let g = (sg / count) as u8;
        let b = (sb / count) as u8;
        if is_useful_accent(r, g, b) {
            out.push((r, g, b));
            if out.len() == k {
                break;
            }
        }
    }

    // Fallback if filters were too aggressive (e.g. very desaturated cover):
    // just take the top bucket unfiltered.
    if out.is_empty() {
        if let Some((_, (c, sr, sg, sb))) = sorted.first() {
            out.push(((sr / c) as u8, (sg / c) as u8, (sb / c) as u8));
        } else {
            out.push((200, 200, 200));
        }
    }
    while out.len() < k {
        out.push(*out.last().expect("non-empty by construction"));
    }
    Ok(out)
}

/// Reject near-grays (no chroma to read), and the luminance extremes
/// where a "color" is really background black or text-on-card white.
fn is_useful_accent(r: u8, g: u8, b: u8) -> bool {
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let sat = if mx == 0 { 0.0 } else { (mx - mn) as f32 / mx as f32 };
    let lum = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
    sat >= 0.20 && (30..=230).contains(&lum)
}

fn hex_byte(hi: u8, lo: u8) -> Option<u8> {
    Some((hex_nibble(hi)? << 4) | hex_nibble(lo)?)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    fn synthetic_png(w: u32, h: u32, color: (u8, u8, u8)) -> Vec<u8> {
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(w, h, Rgb([color.0, color.1, color.2]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn round_trip_preserves_palette() {
        let bytes = synthetic_png(64, 64, (200, 50, 100));
        let stored = render_ascii(&bytes, 0, 0).unwrap();
        assert!(stored.starts_with(FORMAT_TAG));
        let decoded = decode(&stored).expect("decode");
        assert!(!decoded.is_empty());
        // Solid-color cover should land near the source color.
        let (r, g, b) = decoded[0];
        assert!((r as i16 - 200).abs() < 10, "r drift: {r}");
        assert!((g as i16 - 50).abs() < 10, "g drift: {g}");
        assert!((b as i16 - 100).abs() < 10, "b drift: {b}");
    }

    #[test]
    fn decode_rejects_missing_sentinel() {
        assert!(decode("ff0000,00ff00,0000ff").is_none());
    }

    #[test]
    fn decode_rejects_bad_hex() {
        assert!(decode("v4:zzzzzz").is_none());
        assert!(decode("v4:abc").is_none());
    }

    #[test]
    fn lerp_endpoints_match_palette_extremes() {
        let pal = vec![(0, 0, 0), (100, 100, 100), (255, 255, 255)];
        assert_eq!(lerp(&pal, 0.0), (0, 0, 0));
        assert_eq!(lerp(&pal, 1.0), (255, 255, 255));
    }

    #[test]
    fn is_useful_accent_filters_neargrays() {
        assert!(!is_useful_accent(128, 128, 128));     // neutral gray
        assert!(!is_useful_accent(10, 10, 10));        // near-black
        assert!(!is_useful_accent(245, 245, 245));     // near-white
        assert!(is_useful_accent(200, 50, 30));        // rich red
        assert!(is_useful_accent(30, 60, 140));        // navy
    }
}
