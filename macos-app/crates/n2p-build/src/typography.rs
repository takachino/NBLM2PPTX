//! 実在するフォントの字形と原稿の画素を照合する。曖昧な判定は既存書式へ戻す。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ab_glyph::{Font, FontArc, FontVec, VariableFont};
use n2p_core::{BBox, RgbImage};
use n2p_ooxml::{TextBox, shape::TextRun};

#[derive(Clone, Copy)]
struct GlyphMeasure {
    aspect: f64,
    density: f64,
    height_em: f64,
}

fn label_emphasis(text: &str, scores: &[Option<f64>], continuation: &[f64]) -> bool {
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(colon) = chars.iter().position(|c| matches!(c, ':' | '：')) else {
        return false;
    };
    if scores.len() != chars.len() || colon < 5 {
        return false;
    }
    let prefix: Vec<f64> = scores[..colon].iter().flatten().copied().collect();
    let mut body: Vec<f64> = scores[colon + 1..].iter().flatten().copied().collect();
    if body.len() < 8 {
        body.extend(continuation);
    }
    if prefix.len() < 5 || body.len() < 3 {
        return false;
    }
    let median = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values[values.len() / 2]
    };
    let a = median(prefix);
    let b = median(body);
    a >= 0.75 && b <= 0.75 && a - b >= 0.35
}

fn relative_weight(source: GlyphMeasure, regular: GlyphMeasure, bold: GlyphMeasure) -> Option<f64> {
    if (source.aspect / regular.aspect).ln().abs() > 0.65 || bold.density - regular.density < 0.025
    {
        return None;
    }
    if let Some(heavy) = choose_weight(source, regular, bold) {
        return Some(if heavy { 1.0 } else { 0.0 });
    }
    Some(((source.density - regular.density) / (bold.density - regular.density)).clamp(-0.5, 2.0))
}

fn choose_weight(source: GlyphMeasure, regular: GlyphMeasure, bold: GlyphMeasure) -> Option<bool> {
    if (source.aspect / regular.aspect).ln().abs() > 0.35 || bold.density - regular.density < 0.025
    {
        return None;
    }
    let a = (source.density - regular.density).abs();
    let b = (source.density - bold.density).abs();
    if a.min(b) > 0.08 || (a - b).abs() < 0.03 {
        None
    } else {
        Some(b < a)
    }
}

struct FacePair {
    family: String,
    regular: FontArc,
    bold: FontArc,
}

impl FacePair {
    fn load(family: &str, regular: &Path, bold: Option<&Path>) -> Option<Self> {
        let mut normal = FontVec::try_from_vec(std::fs::read(regular).ok()?).ok()?;
        let variable = normal.set_variation(b"wght", 400.0);
        let mut heavy = FontVec::try_from_vec(std::fs::read(bold.unwrap_or(regular)).ok()?).ok()?;
        if bold.is_none() && !variable {
            return None;
        }
        heavy.set_variation(b"wght", 700.0);
        Some(Self {
            family: family.into(),
            regular: normal.into(),
            bold: heavy.into(),
        })
    }

    fn measure(&self, c: char, bold: bool) -> Option<GlyphMeasure> {
        let font = if bold { &self.bold } else { &self.regular };
        let id = font.glyph_id(c);
        if id.0 == 0 {
            return None;
        }
        let scale = 64.0 * font.height_unscaled() / font.units_per_em()?;
        let outline = font.outline_glyph(id.with_scale(scale))?;
        let mut left = u32::MAX;
        let mut right = 0;
        let mut top = u32::MAX;
        let mut bottom = 0;
        let mut count = 0;
        outline.draw(|x, y, coverage| {
            if coverage > 0.5 {
                left = left.min(x);
                right = right.max(x + 1);
                top = top.min(y);
                bottom = bottom.max(y + 1);
                count += 1;
            }
        });
        if count < 2 || right <= left || bottom <= top {
            return None;
        }
        let w = (right - left) as f64;
        let h = (bottom - top) as f64;
        Some(GlyphMeasure {
            aspect: w / h,
            density: count as f64 / (w * h),
            height_em: h / 64.0,
        })
    }

    fn advance(&self, c: char, bold: bool) -> Option<f64> {
        let font = if bold { &self.bold } else { &self.regular };
        let id = font.glyph_id(c);
        if id.0 == 0 {
            return None;
        }
        Some((font.h_advance_unscaled(id) / font.units_per_em()?) as f64)
    }
}

fn dominant_band(rows: &[usize]) -> Option<(usize, usize)> {
    let mut bands = Vec::new();
    let mut start = None;
    for (y, count) in rows.iter().copied().chain(std::iter::once(0)).enumerate() {
        if count > 0 {
            if start.is_none() {
                start = Some(y);
            }
        } else if let Some(top) = start {
            bands.push((top, y, rows[top..y].iter().sum::<usize>()));
            start = None;
        }
    }
    bands
        .into_iter()
        .max_by_key(|b| b.2)
        .map(|(top, bottom, _)| (top, bottom))
}

struct GlyphSample {
    c: char,
    measure: GlyphMeasure,
    height_px: f64,
}

/// 空白列で分離でき、認識文字数と一致する行だけを使用する。
fn samples_for(
    text: &str,
    image: &RgbImage,
    bbox: BBox,
    fonts: &[&FacePair],
) -> Option<Vec<GlyphSample>> {
    let crop = image.crop(bbox);
    if crop.width < 3 || crop.height < 3 {
        return None;
    }
    let mut buckets: BTreeMap<[u8; 3], usize> = BTreeMap::new();
    for pixel in crop.data.chunks_exact(3) {
        *buckets
            .entry([pixel[0] & 0xf0, pixel[1] & 0xf0, pixel[2] & 0xf0])
            .or_default() += 1;
    }
    let bg = buckets
        .into_iter()
        .max_by_key(|(_, count)| *count)?
        .0
        .map(|v| v.saturating_add(8));
    let contrast: Vec<u8> = crop
        .data
        .chunks_exact(3)
        .map(|p| p.iter().zip(bg).map(|(a, b)| a.abs_diff(b)).max().unwrap())
        .collect();
    let max = *contrast.iter().max()? as f64;
    if max < 70.0 {
        return None;
    }
    let mut bits: Vec<bool> = contrast.iter().map(|v| *v as f64 > max * 0.5).collect();
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    if chars.len() < 4 {
        return None;
    }
    let w = crop.width as usize;
    let h = crop.height as usize;
    // OCR の外接矩形に隣行の先端が入る場合、主なインク帯を選ぶ。
    let row_counts: Vec<usize> = (0..h)
        .map(|y| (0..w).filter(|x| bits[y * w + x]).count())
        .collect();
    if let Some((top, bottom)) = dominant_band(&row_counts) {
        for y in 0..h {
            if y < top || y >= bottom {
                bits[y * w..(y + 1) * w].fill(false);
            }
        }
    }
    let ink: Vec<bool> = (0..w)
        .map(|x| (0..h).filter(|y| bits[y * w + x]).count() >= 2)
        .collect();
    let mut ranges = Vec::new();
    let mut start = None;
    for (x, hit) in ink
        .iter()
        .copied()
        .chain(std::iter::once(false))
        .enumerate()
    {
        if hit {
            if start.is_none() {
                start = Some(x);
            }
        } else if let Some(left) = start {
            ranges.push((left, x));
            start = None;
        }
    }
    let n = chars.len();
    let m = ranges.len();
    if m < n || m > n * 3 {
        return None;
    }
    let reference: Vec<Vec<f64>> = chars
        .iter()
        .map(|c| {
            fonts
                .iter()
                .filter(|font| c.is_ascii() || font.family == fonts[0].family)
                .flat_map(|font| [font.measure(*c, false), font.measure(*c, true)])
                .flatten()
                .map(|g| g.aspect)
                .collect()
        })
        .collect();
    if reference.iter().any(Vec::is_empty) {
        return None;
    }
    let mut measures = vec![vec![None; 4]; m];
    for begin in 0..m {
        for length in 1..=3 {
            if begin + length > m {
                continue;
            }
            let left = ranges[begin].0;
            let right = ranges[begin + length - 1].1;
            let mut top = h;
            let mut bottom = 0;
            let mut count = 0;
            for x in left..right {
                for y in 0..h {
                    if bits[y * w + x] {
                        top = top.min(y);
                        bottom = bottom.max(y + 1);
                        count += 1;
                    }
                }
            }
            if count > 1 && bottom > top {
                let height = (bottom - top) as f64;
                let width = (right - left) as f64;
                measures[begin][length] = Some((
                    GlyphMeasure {
                        aspect: width / height,
                        density: count as f64 / (width * height),
                        height_em: 0.0,
                    },
                    height,
                ));
            }
        }
    }
    let mut costs = vec![vec![f64::INFINITY; m + 1]; n + 1];
    let mut prev = vec![vec![0; m + 1]; n + 1];
    costs[0][0] = 0.0;
    for i in 0..n {
        for begin in 0..m {
            if !costs[i][begin].is_finite() {
                continue;
            }
            for length in 1..=3 {
                if begin + length > m {
                    continue;
                }
                let Some((sample, _)) = measures[begin][length] else {
                    continue;
                };
                let mismatch = reference[i]
                    .iter()
                    .map(|aspect| (sample.aspect / aspect).ln().abs())
                    .fold(f64::INFINITY, f64::min);
                let cost = costs[i][begin] + mismatch + 0.02 * (length - 1) as f64;
                if cost < costs[i + 1][begin + length] {
                    costs[i + 1][begin + length] = cost;
                    prev[i + 1][begin + length] = length;
                }
            }
        }
    }
    if costs[n][m] / n as f64 > 0.35 {
        return None;
    }
    let mut result = Vec::new();
    let mut end = m;
    for i in (1..=n).rev() {
        let length = prev[i][end];
        if length == 0 {
            return None;
        }
        let begin = end - length;
        let (measure, height) = measures[begin][length]?;
        result.push(GlyphSample {
            c: chars[i - 1],
            measure,
            height_px: height,
        });
        end = begin;
    }
    result.reverse();
    Some(result)
}

pub(crate) struct FontCatalog {
    base: Option<FacePair>,
    latin: Vec<FacePair>,
}

impl FontCatalog {
    pub(crate) fn load(family: &str, automatic: bool) -> Self {
        let files = crate::fonts::font_files();
        let find = |filename: &str| {
            files
                .iter()
                .find(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(filename))
                })
                .map(PathBuf::as_path)
        };
        let base = match family {
            "Noto Sans JP" => {
                find("NotoSansJP[wght].ttf").and_then(|p| FacePair::load(family, p, None))
            }
            "Arial" => {
                find("Arial.ttf").and_then(|p| FacePair::load(family, p, find("Arial Bold.ttf")))
            }
            _ => None,
        };
        let mut latin = Vec::new();
        if automatic {
            for family in ["Arial Narrow", "Arial"] {
                if let Some(pair) = find(&format!("{family}.ttf"))
                    .and_then(|p| FacePair::load(family, p, find(&format!("{family} Bold.ttf"))))
                {
                    latin.push(pair);
                }
            }
        }
        Self { base, latin }
    }

    pub(crate) fn apply(
        &self,
        text_box: &mut TextBox,
        image: &RgbImage,
        bbox: BBox,
        slide_scale: f64,
        image_scale: f64,
        continuation: Option<(&str, BBox)>,
    ) {
        let Some(base) = &self.base else {
            return;
        };
        if text_box
            .east_asian_font_family
            .as_deref()
            .is_some_and(|family| family != base.family)
            && !text_box.text.is_ascii()
        {
            return;
        }

        if text_box.rotation_deg.abs() >= 3.0 || text_box.text.contains('\n') {
            return;
        }
        let faces: Vec<&FacePair> = std::iter::once(base).chain(self.latin.iter()).collect();
        let Some(samples) = samples_for(&text_box.text, image, bbox, &faces) else {
            return;
        };
        let ascii: Vec<_> = samples
            .iter()
            .filter(|s| s.c.is_ascii_alphanumeric())
            .collect();
        let score = |font: &FacePair| -> Option<f64> {
            if ascii.len() < 6 {
                return None;
            }
            let mut total = 0.0;
            for sample in &ascii {
                let a = font.measure(sample.c, false)?;
                let b = font.measure(sample.c, true)?;
                total += (sample.measure.aspect / a.aspect)
                    .ln()
                    .abs()
                    .min((sample.measure.aspect / b.aspect).ln().abs());
            }
            Some(total / ascii.len() as f64)
        };
        let mut chosen = base;
        if let Some(base_score) = score(base) {
            let mut best = base_score;
            for font in &self.latin {
                if let Some(candidate) = score(font) {
                    if candidate < best - 0.06 && candidate < 0.2 {
                        chosen = font;
                        best = candidate;
                    }
                }
            }
        }
        let changed = chosen.family != base.family;
        let measures = |samples: &[GlyphSample]| -> Vec<Option<f64>> {
            samples
                .iter()
                .map(|sample| {
                    if !sample.c.is_alphabetic() || sample.c == 'ー' {
                        return None;
                    }
                    let font = if sample.c.is_ascii() { chosen } else { base };
                    let (a, b) = font
                        .measure(sample.c, false)
                        .zip(font.measure(sample.c, true))?;
                    relative_weight(sample.measure, a, b)
                })
                .collect()
        };
        let weights = measures(&samples);
        let continuation_scores = continuation
            .and_then(|(text, bounds)| samples_for(text, image, bounds, &faces))
            .map(|rows| measures(&rows).into_iter().flatten().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut estimates = Vec::new();
        for sample in &samples {
            let font = if sample.c.is_ascii() { chosen } else { base };
            if let Some(metric) = font.measure(sample.c, text_box.bold) {
                if sample.c.is_ascii_alphanumeric() {
                    estimates.push(sample.height_px / metric.height_em * slide_scale / image_scale);
                }
            }
        }
        if label_emphasis(&text_box.text, &weights, &continuation_scores) {
            let end = text_box
                .text
                .char_indices()
                .find(|(_, c)| matches!(c, ':' | '：'))
                .map(|(i, c)| i + c.len_utf8())
                .unwrap();
            text_box.styled_runs = vec![
                TextRun {
                    text: text_box.text[..end].into(),
                    bold: true,
                },
                TextRun {
                    text: text_box.text[end..].into(),
                    bold: false,
                },
            ];
        } else {
            let mut scores: Vec<f64> = weights.iter().flatten().copied().collect();
            if scores.len() >= 6 && scores.len() * 2 >= weights.len() {
                scores.sort_by(f64::total_cmp);
                let median = scores[scores.len() / 2];
                if median > 0.85 {
                    text_box.bold = true;
                } else if median < 0.55 {
                    text_box.bold = false;
                }
            }
        }
        if changed {
            text_box.font_family = chosen.family.clone();
            estimates.sort_by(f64::total_cmp);
            let desired = estimates[estimates.len() / 2].min(text_box.font_size_pt * 1.2);
            let width_units: Option<f64> = text_box
                .text
                .chars()
                .map(|c| (if c.is_ascii() { chosen } else { base }).advance(c, text_box.bold))
                .sum();
            if let Some(units) = width_units.filter(|v| *v > 0.0) {
                text_box.font_size_pt = desired.min(text_box.rect.width_pt() * 0.9 / units);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn label_emphasis_requires_a_measured_change_across_the_colon() {
        let text = "項目の見出し：説明の本文";
        let mut scores = vec![Some(1.0); 7];
        scores.extend(vec![Some(0.4); 5]);
        assert!(label_emphasis(text, &scores, &[]));
        assert!(!label_emphasis(text, &[Some(1.0); 12], &[]));
        assert!(!label_emphasis(text, &[None; 12], &[]));
        assert!(label_emphasis(
            "項目の見出し：文",
            &[Some(1.0); 8],
            &[0.4; 8]
        ));
    }
    #[test]
    fn neighbouring_line_tips_do_not_change_the_main_ink_band() {
        assert_eq!(dominant_band(&[0, 1, 0, 0, 5, 6, 7, 6, 0, 1]), Some((4, 8)));
        assert_eq!(dominant_band(&[0; 10]), None);
        assert_eq!(dominant_band(&[1, 2, 3]), Some((0, 3)));
    }

    fn measure(density: f64) -> GlyphMeasure {
        GlyphMeasure {
            aspect: 0.6,
            density,
            height_em: 0.7,
        }
    }

    #[test]
    fn weight_comparison_is_specific_to_the_same_glyph() {
        assert_eq!(
            choose_weight(measure(0.27), measure(0.26), measure(0.43)),
            Some(false)
        );
        assert_eq!(
            choose_weight(measure(0.42), measure(0.26), measure(0.43)),
            Some(true)
        );
        // 密度だけなら太字に見える複雑な文字も、その文字自身の通常字形と比べる。
        assert_eq!(
            choose_weight(measure(0.49), measure(0.48), measure(0.63)),
            Some(false)
        );
    }

    #[test]
    fn noisy_shapes_and_indistinguishable_weights_do_not_override_style() {
        assert_eq!(
            choose_weight(measure(0.35), measure(0.26), measure(0.43)),
            None
        );
        assert_eq!(
            choose_weight(measure(0.28), measure(0.27), measure(0.28)),
            None
        );
        let mut distorted = measure(0.42);
        distorted.aspect = 1.4;
        assert_eq!(choose_weight(distorted, measure(0.26), measure(0.43)), None);
    }
}
