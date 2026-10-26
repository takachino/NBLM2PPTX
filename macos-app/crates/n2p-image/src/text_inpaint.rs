//! 文字消去専用の背景推定。透かし除去の数値互換性には影響させない。

use std::collections::BTreeMap;

use n2p_core::{BBox, RgbImage};

use crate::{Mask, inpaint};

const COLOR_LIMIT: u32 = 60 * 60;

#[derive(Debug, Clone)]
pub struct BackgroundEstimate {
    pub color: [u8; 3],
    pub sample_bbox: BBox,
    pub samples: usize,
    pub flat: bool,
}

fn distance_squared(a: [u8; 3], b: [u8; 3]) -> u32 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (*a as i32 - b as i32).pow(2) as u32)
        .sum()
}

fn dominant_color(image: &RgbImage, bbox: BBox, excluded: &Mask) -> Option<([u8; 3], usize)> {
    let bbox = bbox.clamp(image.width as i64, image.height as i64);
    let mut buckets: BTreeMap<[u8; 3], (usize, [u64; 3])> = BTreeMap::new();
    let mut samples = 0;
    for y in bbox.top..bbox.bottom {
        for x in bbox.left..bbox.right {
            if excluded.get(x as u32, y as u32) {
                continue;
            }
            let pixel = image.pixel(x as u32, y as u32);
            let key = pixel.map(|channel| channel & 0xf0);
            let entry = buckets.entry(key).or_default();
            entry.0 += 1;
            for (total, channel) in entry.1.iter_mut().zip(pixel) {
                *total += channel as u64;
            }
            samples += 1;
        }
    }
    let (_, (count, sums)) = buckets.into_iter().max_by_key(|(_, (count, _))| *count)?;
    Some((sums.map(|sum| (sum / count as u64) as u8), samples))
}

fn contains_background(image: &RgbImage, bbox: BBox, excluded: &Mask, color: [u8; 3]) -> bool {
    let bbox = bbox.clamp(image.width as i64, image.height as i64);
    let (mut similar, mut samples) = (0, 0);
    for y in bbox.top..bbox.bottom {
        for x in bbox.left..bbox.right {
            if !excluded.get(x as u32, y as u32) {
                samples += 1;
                if distance_squared(image.pixel(x as u32, y as u32), color) <= COLOR_LIMIT {
                    similar += 1;
                }
            }
        }
    }
    // 枠からはみ出した文字の行にも、文字間には背景が残る。
    // 異なる帯や全幅の罫線では背景候補がなくなる。
    samples == 0 || similar * 5 >= samples
}

/// 外側の広い環よりも、対象文字と同じ枠内の背景を優先する。
/// 候補がない場合は白色で代用せず、呼び出し元へ `None` を返す。
pub fn estimate_background(
    source: &RgbImage,
    text_bbox: BBox,
    all_text: &Mask,
) -> Option<BackgroundEstimate> {
    let sample_bbox = text_bbox.clamp(source.width as i64, source.height as i64);
    let (color, samples) = dominant_color(source, sample_bbox, all_text)?;
    let mut similar = 0;
    for y in sample_bbox.top..sample_bbox.bottom {
        for x in sample_bbox.left..sample_bbox.right {
            if !all_text.get(x as u32, y as u32)
                && distance_squared(source.pixel(x as u32, y as u32), color) <= COLOR_LIMIT
            {
                similar += 1;
            }
        }
    }
    Some(BackgroundEstimate {
        color,
        sample_bbox,
        samples,
        flat: similar as f64 / samples as f64 >= 0.95,
    })
}

/// 拡張で隣の帯や罫線へ越境しないよう、各行と列の背景色を確認する。
/// 初期マスクの画素は保持し、追加する画素だけを制限する。
pub fn expand_mask(
    source: &RgbImage,
    initial: &Mask,
    all_text: &Mask,
    text_bbox: BBox,
    radius: u32,
    background: &BackgroundEstimate,
) -> Mask {
    let mut expanded = initial.clone();
    let pad = radius.min(16) as i64 + 2;
    let limit = BBox::new(
        text_bbox.left - pad,
        text_bbox.top - pad,
        text_bbox.right + pad,
        text_bbox.bottom + pad,
    )
    .clamp(source.width as i64, source.height as i64);
    expanded.dilate_within(radius, limit);
    let rows: Vec<bool> = (limit.top..limit.bottom)
        .map(|y| {
            contains_background(
                source,
                BBox::new(text_bbox.left, y, text_bbox.right, y + 1),
                all_text,
                background.color,
            )
        })
        .collect();
    let columns: Vec<bool> = (limit.left..limit.right)
        .map(|x| {
            contains_background(
                source,
                BBox::new(x, text_bbox.top, x + 1, text_bbox.bottom),
                all_text,
                background.color,
            )
        })
        .collect();
    for y in limit.top..limit.bottom {
        for x in limit.left..limit.right {
            if !(initial.get(x as u32, y as u32)
                || rows[(y - limit.top) as usize] && columns[(x - limit.left) as usize])
            {
                expanded.set(x as u32, y as u32, false);
            }
        }
    }
    expanded
}

/// 採色と補間は常に元画像から行い、別の文字と先に処理した消去跡を使わない。
pub fn inpaint_in_place(
    output: &mut RgbImage,
    source: &RgbImage,
    mask: &Mask,
    all_text: &Mask,
    background: &BackgroundEstimate,
) {
    let Some(bbox) = mask.padded_bbox(6) else {
        return;
    };
    let crop = source.crop(bbox);
    let mut excluded = Mask::new(crop.width, crop.height);
    for y in 0..crop.height {
        for x in 0..crop.width {
            let (sx, sy) = (bbox.left as u32 + x, bbox.top as u32 + y);
            excluded.set(x, y, mask.get(sx, sy) || all_text.get(sx, sy));
        }
    }
    let cleaned =
        inpaint::nearest_neighbour_with_limit(&crop, &excluded, 40, background.color, 24.0);
    for y in 0..crop.height {
        for x in 0..crop.width {
            let (sx, sy) = (bbox.left as u32 + x, bbox.top as u32 + y);
            if mask.get(sx, sy) {
                output.set_pixel(sx, sy, cleaned.pixel(x, y));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fully_masked_background_has_no_white_default() {
        let image = RgbImage::filled(20, 10, [20, 30, 40]);
        let mask = Mask::from_bbox(20, 10, BBox::new(0, 0, 20, 10));
        assert!(estimate_background(&image, BBox::new(0, 0, 20, 10), &mask).is_none());
    }

    #[test]
    fn neighbouring_text_and_residual_gray_cannot_seed_a_repair() {
        let color = [232, 242, 248];
        let mut source = RgbImage::filled(40, 30, color);
        let mask = Mask::from_bbox(40, 30, BBox::new(10, 8, 20, 22));
        let mut all_text = mask.clone();
        for y in 8..22 {
            for x in 10..20 {
                source.set_pixel(x, y, [10; 3]);
            }
            source.set_pixel(9, y, [180; 3]);
            source.set_pixel(20, y, [10; 3]);
            all_text.set(20, y, true);
        }
        let background = estimate_background(&source, BBox::new(5, 5, 30, 25), &all_text).unwrap();
        assert_eq!(background.color, color);
        let mut output = source.clone();
        inpaint_in_place(&mut output, &source, &mask, &all_text, &background);
        for y in 0..source.height {
            for x in 0..source.width {
                assert_eq!(
                    output.pixel(x, y),
                    if mask.get(x, y) {
                        color
                    } else {
                        source.pixel(x, y)
                    }
                );
            }
        }
    }

    #[test]
    fn expansion_preserves_the_band_boundary_and_a_nearby_rule() {
        let mut source = RgbImage::filled(80, 50, [255; 3]);
        for y in 20..50 {
            for x in 0..80 {
                source.set_pixel(x, y, [12, 30, 54]);
            }
        }
        for x in 0..80 {
            source.set_pixel(x, 38, [100, 150, 200]);
        }
        let bbox = BBox::new(10, 23, 70, 34);
        let mut mask = Mask::new(80, 50);
        for y in 23..34 {
            for x in [12, 24, 36, 48, 60] {
                mask.set(x, y, true);
                source.set_pixel(x, y, [255; 3]);
            }
        }
        let background = estimate_background(&source, bbox, &mask).unwrap();
        let expanded = expand_mask(&source, &mask, &mask, bbox, 8, &background);
        for y in 0..20 {
            for x in 0..80 {
                assert!(!expanded.get(x, y), "白いページへ越境: ({x}, {y})");
            }
        }
        for x in 0..80 {
            assert!(!expanded.get(x, 38), "罫線を保護: {x}");
        }
        assert!(expanded.count() > mask.count());
    }

    #[test]
    fn expansion_removes_ink_that_extends_past_a_tight_ocr_box() {
        let mut source = RgbImage::filled(80, 40, [245; 3]);
        let bbox = BBox::new(10, 12, 70, 26);
        let mut mask = Mask::new(80, 40);
        for y in 8..30 {
            for x in 10..70 {
                if (x - 10) % 10 < 7 {
                    source.set_pixel(x, y, [20; 3]);
                    if (12..26).contains(&y) {
                        mask.set(x, y, true);
                    }
                }
            }
        }
        let background = estimate_background(&source, bbox, &mask).unwrap();
        let expanded = expand_mask(&source, &mask, &mask, bbox, 6, &background);
        let mut output = source.clone();
        inpaint_in_place(&mut output, &source, &expanded, &expanded, &background);
        for y in 0..output.height {
            for x in 0..output.width {
                assert_eq!(output.pixel(x, y), [245; 3], "({x}, {y})");
            }
        }
    }
}
