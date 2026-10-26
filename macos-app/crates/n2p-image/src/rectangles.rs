//! 配置制約の候補となる、四辺がつながった矩形の検出。

use n2p_core::{BBox, RgbImage};

const MAX_SIDE: u32 = 1600;
const MIN_SPAN: i64 = 12;
const JOIN: i64 = 3;

#[derive(Debug, Clone)]
pub struct RectangularRegion {
    /// 元画像の px 座標。
    pub bbox: BBox,
    /// 四辺の連続した支持長の最小値 (0〜1000)。分類器の確率ではない。
    pub confidence_milli: u16,
}

#[derive(Clone, Copy)]
struct Edge {
    at: i64,
    start: i64,
    end: i64,
}

impl Edge {
    fn covers(self, start: i64, end: i64) -> bool {
        self.start <= start + JOIN && self.end >= end - JOIN
    }
}

/// OCR 領域内の画素を除き、水平辺と垂直辺が閉じる矩形を返す。
/// 写真の一般的な物体検出や、途切れた枠の推測は行わない。
pub fn find_rectangular_regions(image: &RgbImage, text_boxes: &[BBox]) -> Vec<RectangularRegion> {
    if image.width < 2 || image.height < 2 {
        return Vec::new();
    }
    let factor = (MAX_SIDE as f64 / image.width.max(image.height) as f64).min(1.0);
    let resized;
    let image = if factor < 1.0 {
        resized = downsample(image, factor);
        &resized
    } else {
        image
    };
    let (width, height) = (image.width as usize, image.height as usize);
    let mut excluded = vec![false; width * height];
    for bbox in text_boxes {
        let bbox = bbox.scale(factor).clamp(width as i64, height as i64);
        if bbox.is_empty() {
            continue;
        }
        for y in bbox.top..bbox.bottom {
            excluded
                [y as usize * width + bbox.left as usize..y as usize * width + bbox.right as usize]
                .fill(true);
        }
    }
    let mut horizontal = edges(image, &excluded, false);
    horizontal.sort_by_key(|e| e.at);
    let vertical = edges(image, &excluded, true);
    let mut regions: Vec<RectangularRegion> = Vec::new();
    let mut checks = 0usize;
    for top in &horizontal {
        let mut sides: Vec<Edge> = vertical
            .iter()
            .copied()
            .filter(|v| {
                v.at >= top.start - JOIN
                    && v.at <= top.end + JOIN
                    && v.start <= top.at + JOIN
                    && v.end >= top.at + MIN_SPAN
            })
            .collect();
        sides.sort_by_key(|v| v.at);
        // 罫線が続く表では、隣り合う縦辺をセルの左右として使う。
        for pair in sides.windows(2) {
            let (left, right) = (pair[0], pair[1]);
            if right.at - left.at < MIN_SPAN {
                continue;
            }
            let mut bottom = None;
            for candidate in &horizontal {
                checks += 1;
                // 写真や細かい模様を無制限に組み合わせず、従来の配置へ戻す。
                if checks > 2_000_000 {
                    return Vec::new();
                }
                if candidate.at >= top.at + MIN_SPAN
                    && candidate.covers(left.at, right.at)
                    && left.covers(top.at, candidate.at)
                    && right.covers(top.at, candidate.at)
                {
                    bottom = Some(candidate);
                    break;
                }
            }
            let Some(bottom) = bottom else { continue };
            let bbox = BBox::new(left.at, top.at, right.at, bottom.at);
            // ページ全体の外枠は文字の所属を示さない。
            if bbox.width() * 100 >= width as i64 * 95 && bbox.height() * 100 >= height as i64 * 95
            {
                continue;
            }
            let support = |edge: Edge, lo: i64, hi: i64| {
                ((edge.end.min(hi) - edge.start.max(lo)).max(0) * 1000 / (hi - lo)).min(1000) as u16
            };
            let confidence = [
                support(*top, left.at, right.at),
                support(*bottom, left.at, right.at),
                support(left, top.at, bottom.at),
                support(right, top.at, bottom.at),
            ]
            .into_iter()
            .min()
            .unwrap_or(0);
            if confidence < 900 {
                continue;
            }
            let bbox = bbox.scale(1.0 / factor);
            if regions.iter().any(|r| r.bbox.iou(&bbox) > 0.9) {
                continue;
            }
            regions.push(RectangularRegion {
                bbox,
                confidence_milli: confidence,
            });
            if regions.len() == 2048 {
                return regions;
            }
        }
    }
    regions
}

fn downsample(image: &RgbImage, factor: f64) -> RgbImage {
    let width = (image.width as f64 * factor).round().max(2.0) as u32;
    let height = (image.height as f64 * factor).round().max(2.0) as u32;
    let mut out = RgbImage::new(width, height);
    for y in 0..height {
        let y0 = y as u64 * image.height as u64 / height as u64;
        let y1 = (y + 1) as u64 * image.height as u64 / height as u64;
        for x in 0..width {
            let x0 = x as u64 * image.width as u64 / width as u64;
            let x1 = (x + 1) as u64 * image.width as u64 / width as u64;
            let mut sums = [0u64; 3];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    for (sum, value) in sums.iter_mut().zip(image.pixel(sx as u32, sy as u32)) {
                        *sum += value as u64;
                    }
                }
            }
            let count = ((x1 - x0) * (y1 - y0)).max(1);
            out.set_pixel(x, y, sums.map(|s| (s / count) as u8));
        }
    }
    out
}

fn edges(image: &RgbImage, excluded: &[bool], vertical: bool) -> Vec<Edge> {
    let (outer, inner) = if vertical {
        (image.width, image.height)
    } else {
        (image.height, image.width)
    };
    let mut result: Vec<Edge> = Vec::new();
    for at in 1..outer {
        let mut start = None;
        for along in 0..=inner {
            let hit = if along == inner {
                false
            } else {
                let (x, y) = if vertical { (at, along) } else { (along, at) };
                let (px, py) = if vertical { (x - 1, y) } else { (x, y - 1) };
                !excluded[(y * image.width + x) as usize]
                    && !excluded[(py * image.width + px) as usize]
                    && image
                        .pixel(x, y)
                        .into_iter()
                        .zip(image.pixel(px, py))
                        .any(|(a, b)| a.abs_diff(b) >= 18)
            };
            if hit {
                start.get_or_insert(along);
            } else if let Some(lo) = start.take() {
                if (along - lo) as i64 >= MIN_SPAN {
                    let edge = Edge {
                        at: at as i64,
                        start: lo as i64,
                        end: along as i64,
                    };
                    if let Some(previous) = result.iter_mut().rev().take(256).find(|e| {
                        (e.at - edge.at).abs() <= JOIN
                            && e.covers(edge.start, edge.end)
                            && edge.covers(e.start, e.end)
                    }) {
                        previous.at = (previous.at + edge.at) / 2;
                        previous.start = previous.start.min(edge.start);
                        previous.end = previous.end.max(edge.end);
                    } else {
                        result.push(edge);
                        if result.len() > 2048 {
                            return Vec::new();
                        }
                    }
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(image: &mut RgbImage, b: BBox) {
        for y in b.top..b.bottom {
            for x in b.left..b.right {
                if x < b.left + 2 || x >= b.right - 2 || y < b.top + 2 || y >= b.bottom - 2 {
                    image.set_pixel(x as u32, y as u32, [30, 60, 90]);
                }
            }
        }
    }

    #[test]
    fn closed_table_cells_are_found_at_different_resolutions() {
        for scale in [1, 2, 8] {
            let mut image = RgbImage::filled(320 * scale, 180 * scale, [255; 3]);
            let cells = [BBox::new(20, 30, 160, 130), BBox::new(158, 30, 300, 130)];
            for cell in cells {
                frame(&mut image, cell.scale(scale as f64));
            }
            let regions = find_rectangular_regions(&image, &[]);
            for cell in cells {
                assert!(
                    regions
                        .iter()
                        .any(|r| r.bbox.iou(&cell.scale(scale as f64)) > 0.9),
                    "{scale}: {regions:?}"
                );
            }
        }
    }

    #[test]
    fn underline_and_open_frame_are_not_closed_regions() {
        let mut image = RgbImage::filled(320, 180, [255; 3]);
        frame(&mut image, BBox::new(20, 30, 160, 130));
        for y in 20..140 {
            for x in 155..170 {
                image.set_pixel(x, y, [255; 3]);
            }
        }
        for x in 190..290 {
            image.set_pixel(x, 90, [0; 3]);
        }
        assert!(find_rectangular_regions(&image, &[]).is_empty());
    }

    #[test]
    fn ocr_glyph_boxes_are_not_graphical_frames() {
        let mut image = RgbImage::filled(320, 180, [255; 3]);
        let text = BBox::new(50, 60, 180, 100);
        frame(&mut image, text);
        assert!(find_rectangular_regions(&image, &[text]).is_empty());
    }
}
