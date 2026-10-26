//! OCR の観測と画素を照合し、図と小さい UI 文字を画像に残す。

use std::collections::{BTreeMap, VecDeque};

use n2p_core::{BBox, RgbImage};
use n2p_layout::{Block, LayoutPage};
use serde::Serialize;

use crate::{MIN_FONT_SIZE_PT, diagnostics::bbox_array, natural_font_size};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreservationReason {
    GraphicSymbol,
    GraphicPrefix,
    InlineSymbol,
    SmallUiText,
    UncertainGraphic,
    AdjacentGraphic,
    PerspectiveText,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreservedText {
    /// 入力レイアウトの text_blocks() と、その中の行の索引 (0 始まり)。
    pub source_block_index: usize,
    pub source_line_index: Option<usize>,
    /// OCR で文字になっていない隣接図形では空文字列。
    pub text: String,
    /// 入力レイアウトの座標 (pt)。スライドの縮尺を適用する前の領域。
    pub bbox_pt: [i64; 4],
    pub confidence_milli: Option<u16>,
    pub reason: PreservationReason,
}

pub(crate) struct PlannedText {
    pub block: Block,
    pub source_block_index: usize,
    pub source_line_indices: Vec<usize>,
    pub font_size_pt: Option<f64>,
    pub bounded: bool,
}

pub(crate) struct ProtectionPlan {
    pub editable: Vec<PlannedText>,
    pub preserved: Vec<PreservedText>,
    pub protected_boxes: Vec<BBox>,
}

#[derive(Clone)]
struct Observation {
    block: Block,
    block_index: usize,
    line_index: Option<usize>,
    confidence: Option<u16>,
    natural_size: f64,
}

impl Observation {
    fn ink_height(&self) -> i64 {
        if self.block.rotation_mdeg.abs() >= 3_000 {
            self.block
                .lines
                .first()
                .and_then(|line| line.bbox)
                .unwrap_or(self.block.bbox)
                .height()
                .max(1)
        } else {
            self.block.bbox.height().max(1)
        }
    }

    fn preserve(&self, text: String, bbox: BBox, reason: PreservationReason) -> PreservedText {
        PreservedText {
            source_block_index: self.block_index,
            source_line_index: self.line_index,
            text,
            bbox_pt: bbox_array(bbox),
            confidence_milli: self.confidence,
            reason,
        }
    }
}

fn union(a: BBox, b: BBox) -> BBox {
    BBox::new(
        a.left.min(b.left),
        a.top.min(b.top),
        a.right.max(b.right),
        a.bottom.max(b.bottom),
    )
}

fn expand(bbox: BBox, pad: i64) -> BBox {
    BBox::new(
        bbox.left - pad,
        bbox.top - pad,
        bbox.right + pad,
        bbox.bottom + pad,
    )
}

fn intersects_most(region: BBox, bbox: BBox) -> bool {
    let width = (region.right.min(bbox.right) - region.left.max(bbox.left)).max(0);
    let height = (region.bottom.min(bbox.bottom) - region.top.max(bbox.top)).max(0);
    width * height * 10 >= bbox.width().max(1) * bbox.height().max(1) * 6
}

fn observations(layout: &LayoutPage, scale: f64) -> Vec<Observation> {
    let mut observations = Vec::new();
    for (block_index, block) in layout.text_blocks().enumerate() {
        if block.lines.is_empty() || block.lines.iter().any(|line| line.bbox.is_none()) {
            observations.push(Observation {
                block: block.clone(),
                block_index,
                line_index: None,
                confidence: None,
                natural_size: natural_font_size(block, scale),
            });
            continue;
        }
        for (line_index, line) in block.lines.iter().enumerate() {
            let mut single = block.clone();
            single.bbox = line.observation_bbox.or(line.bbox).unwrap_or(block.bbox);
            single.text = line.text();
            single.lines = vec![line.clone()];
            let natural_size = natural_font_size(&single, scale);
            observations.push(Observation {
                block: single,
                block_index,
                line_index: Some(line_index),
                confidence: line.confidence_milli,
                natural_size,
            });
        }
    }
    observations
}

fn ui_context(image: &RgbImage, bbox: BBox) -> bool {
    if image.width == 0 || image.height == 0 {
        return false;
    }
    let samples = [
        image.pixel(0, 0),
        image.pixel(image.width / 2, 0),
        image.pixel(image.width - 1, 0),
        image.pixel(0, image.height / 2),
        image.pixel(image.width - 1, image.height / 2),
        image.pixel(0, image.height - 1),
        image.pixel(image.width / 2, image.height - 1),
        image.pixel(image.width - 1, image.height - 1),
    ];
    let background: [u8; 3] = std::array::from_fn(|channel| {
        let mut values = samples.map(|color| color[channel]);
        values.sort_unstable();
        values[4]
    });
    let bbox = bbox.clamp(image.width as i64, image.height as i64);
    let (mut colored, mut dark, mut total) = (0, 0, 0);
    for y in (bbox.top..bbox.bottom).step_by(2) {
        for x in (bbox.left..bbox.right).step_by(2) {
            let color = image.pixel(x as u32, y as u32);
            let differs = color
                .iter()
                .zip(background)
                .any(|(a, b)| a.abs_diff(b) > 12);
            let chroma = color.iter().max().unwrap() - color.iter().min().unwrap();
            colored += usize::from(differs && chroma > 12);
            dark += usize::from(differs && color.iter().all(|v| *v < 110));
            total += 1;
        }
    }
    total > 0 && (colored * 100 >= total * 3 || dark * 10 >= total * 4)
}

fn small_ui_regions(
    observations: &[Observation],
    image: &RgbImage,
    layout_width: f64,
    scale: f64,
) -> Vec<BBox> {
    // 信頼度が高くても小さい画面文字は誤認する。最小字級未満、または
    // 小さいインク高の観測が集まる領域を、周囲の色と合わせて判定する。
    let image_scale = image.width as f64 / layout_width;
    let candidates: Vec<&Observation> = observations
        .iter()
        .filter(|observation| {
            (observation.natural_size < MIN_FONT_SIZE_PT - 0.5
                || observation.natural_size <= 16.0
                    && observation.ink_height() as f64 * scale <= 10.5)
                && observation.ink_height() as f64 * scale < 12.0
                && !observation.block.text.trim().is_empty()
        })
        .collect();
    let mut seen = vec![false; candidates.len()];
    let mut regions = Vec::new();
    for start in 0..candidates.len() {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        let mut queue = VecDeque::from([start]);
        let mut members = Vec::new();
        while let Some(index) = queue.pop_front() {
            members.push(index);
            let bbox = candidates[index].block.bbox;
            for (next, candidate) in candidates.iter().enumerate() {
                if seen[next] {
                    continue;
                }
                let other = candidate.block.bbox;
                let gap_x = (bbox.left - other.right)
                    .max(other.left - bbox.right)
                    .max(0);
                let gap_y = (bbox.top - other.bottom)
                    .max(other.top - bbox.bottom)
                    .max(0);
                let height = candidates[index].ink_height().max(candidate.ink_height());
                let reach = ((height * 7).max(32) as f64 * scale).min(60.0) / scale;
                if gap_x as f64 <= reach && gap_y as f64 <= reach {
                    seen[next] = true;
                    queue.push_back(next);
                }
            }
        }
        if members.len() < 3 {
            continue;
        }
        let bbox = members
            .iter()
            .map(|&i| candidates[i].block.bbox)
            .reduce(union)
            .unwrap();
        let height = members
            .iter()
            .map(|&i| candidates[i].ink_height())
            .max()
            .unwrap();
        // 隣接する通常本文へ広がらないよう、余白は UI 行高の 3 倍までにする。
        let mut region = expand(bbox, (height * 3).min((36.0 / scale).round() as i64));
        if !ui_context(image, region.scale(image_scale)) {
            continue;
        }
        let original_area = region.width().max(1) * region.height().max(1);
        for _ in 0..3 {
            let before = region;
            for observation in observations {
                let other = observation.block.bbox;
                if observation.natural_size > 16.0
                    || observation.ink_height() as f64 * scale > 20.0
                    || crate::fontsize::full_width_ratio(&observation.block.text) >= 0.3
                {
                    continue;
                }
                let gap_x = (region.left - other.right)
                    .max(other.left - region.right)
                    .max(0);
                let gap_y = (region.top - other.bottom)
                    .max(other.top - region.bottom)
                    .max(0);
                let reach = observation.ink_height() * 2;
                let enlarged = union(region, expand(other, 2));
                if gap_x <= reach
                    && gap_y <= reach
                    && enlarged.width() * enlarged.height() * 10 <= original_area * 16
                {
                    region = enlarged;
                }
            }
            if region == before {
                break;
            }
        }
        regions.push(region);
    }
    // 行が離れていて集合を作れないキャプチャでも、極小の観測は画面の色で判定する。
    // 独立した脚注は近傍がページ地色のため対象外になる。
    for observation in observations {
        if observation.natural_size >= MIN_FONT_SIZE_PT - 0.5
            || observation.ink_height() as f64 * scale > 8.0
            || observation.block.text.trim().is_empty()
        {
            continue;
        }
        let context = expand(observation.block.bbox, (24.0 / scale).round() as i64);
        if ui_context(image, context.scale(image_scale)) {
            regions.push(expand(observation.block.bbox, 2));
        }
    }
    regions
}

#[derive(Clone)]
struct Component {
    bbox: BBox,
    pixels: usize,
}

#[derive(Clone, Copy)]
enum InkColor {
    Any,
    Green,
    Chromatic,
}

struct Foreground {
    bbox: BBox,
    width: usize,
    bits: Vec<bool>,
    colors: Vec<[u8; 3]>,
}

impl Foreground {
    fn new(image: &RgbImage, bbox: BBox) -> Self {
        let bbox = bbox.clamp(image.width as i64, image.height as i64);
        let mut buckets: BTreeMap<[u8; 3], (usize, [u64; 3])> = BTreeMap::new();
        let mut colors = Vec::new();
        for y in bbox.top..bbox.bottom {
            for x in bbox.left..bbox.right {
                let color = image.pixel(x as u32, y as u32);
                let entry = buckets.entry(color.map(|v| v & 0xf0)).or_default();
                entry.0 += 1;
                for (sum, value) in entry.1.iter_mut().zip(color) {
                    *sum += value as u64;
                }
                colors.push(color);
            }
        }
        let background = buckets
            .into_values()
            .max_by_key(|(count, _)| *count)
            .map(|(count, sums)| sums.map(|sum| (sum / count as u64) as u8))
            .unwrap_or([0; 3]);
        let bits = colors
            .iter()
            .map(|color| {
                color
                    .iter()
                    .zip(background)
                    .any(|(a, b)| a.abs_diff(b) > 38)
            })
            .collect();
        Self {
            bbox,
            width: bbox.width().max(0) as usize,
            bits,
            colors,
        }
    }

    fn components(&self, filter: InkColor) -> Vec<Component> {
        if self.width == 0 {
            return Vec::new();
        }
        let height = self.bbox.height().max(0) as usize;
        let mut pending: Vec<bool> = self
            .bits
            .iter()
            .zip(&self.colors)
            .map(|(hit, color)| {
                *hit && match filter {
                    InkColor::Any => true,
                    InkColor::Green => {
                        color[1] > color[0].saturating_add(25)
                            && color[1] > color[2].saturating_add(10)
                    }
                    InkColor::Chromatic => {
                        color.iter().max().unwrap() - color.iter().min().unwrap() > 45
                    }
                }
            })
            .collect();
        let mut components = Vec::new();
        for index in 0..pending.len() {
            if !pending[index] {
                continue;
            }
            pending[index] = false;
            let mut queue = VecDeque::from([index]);
            let mut bbox = BBox::new(i64::MAX, i64::MAX, 0, 0);
            let mut pixels = 0;
            while let Some(index) = queue.pop_front() {
                let (x, y) = (index % self.width, index / self.width);
                bbox = BBox::new(
                    bbox.left.min(x as i64),
                    bbox.top.min(y as i64),
                    bbox.right.max(x as i64 + 1),
                    bbox.bottom.max(y as i64 + 1),
                );
                pixels += 1;
                for ny in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                    for nx in x.saturating_sub(1)..=(x + 1).min(self.width - 1) {
                        let neighbour = ny * self.width + nx;
                        if pending[neighbour] {
                            pending[neighbour] = false;
                            queue.push_back(neighbour);
                        }
                    }
                }
            }
            if pixels >= 4 {
                components.push(Component {
                    bbox: BBox::new(
                        bbox.left + self.bbox.left,
                        bbox.top + self.bbox.top,
                        bbox.right + self.bbox.left,
                        bbox.bottom + self.bbox.top,
                    ),
                    pixels,
                });
            }
        }
        components
    }

    fn color(&self, bbox: BBox) -> Option<[u8; 3]> {
        let mut sums = [0u64; 3];
        let mut count = 0;
        for y in bbox.top.max(self.bbox.top)..bbox.bottom.min(self.bbox.bottom) {
            for x in bbox.left.max(self.bbox.left)..bbox.right.min(self.bbox.right) {
                let index =
                    (y - self.bbox.top) as usize * self.width + (x - self.bbox.left) as usize;
                if self.bits[index] {
                    for (sum, value) in sums.iter_mut().zip(self.colors[index]) {
                        *sum += value as u64;
                    }
                    count += 1;
                }
            }
        }
        (count > 0).then(|| sums.map(|sum| (sum / count) as u8))
    }

    fn colorful_context(&self) -> bool {
        let colorful = self
            .colors
            .iter()
            .filter(|color| color.iter().max().unwrap() - color.iter().min().unwrap() > 45)
            .count();
        !self.colors.is_empty() && colorful * 5 > self.colors.len() * 2
    }

    fn is_tick(&self, bbox: BBox) -> bool {
        let mut sums = [0.0; 3];
        let mut counts = [0; 3];
        for y in bbox.top..bbox.bottom {
            for x in bbox.left..bbox.right {
                let index =
                    (y - self.bbox.top) as usize * self.width + (x - self.bbox.left) as usize;
                let color = self.colors[index];
                if !self.bits[index]
                    || color[1] <= color[0].saturating_add(25)
                    || color[1] <= color[2].saturating_add(10)
                {
                    continue;
                }
                let relative = (x - bbox.left) as f64 / bbox.width().max(1) as f64;
                let band = if relative < 0.25 {
                    0
                } else if relative < 0.55 {
                    1
                } else {
                    2
                };
                sums[band] += (y - bbox.top) as f64;
                counts[band] += 1;
            }
        }
        if counts.contains(&0) {
            return false;
        }
        let means = [
            sums[0] / counts[0] as f64,
            sums[1] / counts[1] as f64,
            sums[2] / counts[2] as f64,
        ];
        means[1] - means[0] > -(bbox.height() as f64) * 0.08
            && means[1] - means[2] > bbox.height() as f64 * 0.12
            && bbox.width() as f64 >= bbox.height() as f64 * 0.8
    }

    fn is_people(&self) -> bool {
        let height = self.bbox.height() as f64;
        let components = self.components(InkColor::Any);
        components.iter().any(|head| {
            let h = head.bbox.height() as f64;
            let w = head.bbox.width() as f64;
            h >= height * 0.18
                && h <= height * 0.60
                && (0.65..=1.5).contains(&(w / h))
                && head.bbox.bottom as f64 <= self.bbox.top as f64 + height * 0.65
                && components.iter().any(|body| {
                    body.bbox.top >= head.bbox.bottom - 2
                        && body.bbox.width() >= head.bbox.width()
                        && body.bbox.left < head.bbox.right
                        && body.bbox.right > head.bbox.left
                        && body.pixels > head.pixels / 2
                })
        })
    }

    fn is_arrow(&self) -> bool {
        if self.width == 0 || (self.bbox.width() as f64) < self.bbox.height() as f64 * 1.3 {
            return false;
        }
        let height = self.bbox.height().max(0) as usize;
        (height / 3..height * 2 / 3).any(|y| {
            let mut run = 0;
            for x in 0..self.width {
                if self.bits[y * self.width + x] {
                    run += 1;
                } else {
                    run = 0;
                }
                if run * 10 >= self.width * 6 {
                    return true;
                }
            }
            false
        })
    }

    fn is_clock(&self) -> bool {
        self.components(InkColor::Any).iter().any(|outer| {
            let aspect = outer.bbox.width() as f64 / outer.bbox.height().max(1) as f64;
            if !(0.75..=1.3).contains(&aspect) || outer.bbox.height() * 10 < self.bbox.height() * 7
            {
                return false;
            }
            let inner = BBox::new(
                outer.bbox.left + outer.bbox.width() * 3 / 10,
                outer.bbox.top + outer.bbox.height() * 3 / 10,
                outer.bbox.right - outer.bbox.width() * 3 / 10,
                outer.bbox.bottom - outer.bbox.height() / 5,
            );
            let mut ink = 0;
            for y in inner.top..inner.bottom {
                for x in inner.left..inner.right {
                    let index =
                        (y - self.bbox.top) as usize * self.width + (x - self.bbox.left) as usize;
                    ink += usize::from(self.bits[index]);
                }
            }
            ink >= 6 && ink as i64 * 20 >= inner.width().max(1) * inner.height().max(1)
        })
    }

    fn prefix(&self) -> Option<(BBox, i64)> {
        if self.width == 0 {
            return None;
        }
        let mut ink = vec![false; self.width];
        for (index, hit) in self.bits.iter().enumerate() {
            ink[index % self.width] |= *hit;
        }
        let start = ink.iter().position(|v| *v)?;
        let end = start + ink[start..].iter().position(|v| !*v)?;
        let next = end + ink[end..].iter().position(|v| *v)?;
        let box_height = self.bbox.height().max(1) as f64;
        if (end - start) as f64 > box_height * 2.2
            || ((next - end) as f64) < box_height * 0.18
            || self.width - next < end - start
        {
            return None;
        }
        Some((
            BBox::new(
                self.bbox.left + start as i64,
                self.bbox.top,
                self.bbox.left + end as i64,
                self.bbox.bottom,
            ),
            self.bbox.left + next as i64,
        ))
    }
}

fn suspicion(text: &str) -> bool {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    !compact.is_empty()
        && compact
            .chars()
            .all(|c| "0Oo〇○く<>♥Vv✓✔✕×Xx◎→⇒▶".contains(c))
}

fn graphic_token(foreground: &Foreground, text: &str) -> bool {
    if !suspicion(text) {
        return false;
    }
    let zeros = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|c| "0Oo〇○".contains(c));
    if zeros && foreground.is_people() {
        return true;
    }
    if foreground
        .components(InkColor::Green)
        .iter()
        .any(|component| foreground.is_tick(component.bbox))
    {
        return true;
    }
    let color = foreground.color(foreground.bbox).unwrap_or([0; 3]);
    let chroma = color.iter().max().unwrap() - color.iter().min().unwrap();
    if chroma > 50
        && (zeros && text.chars().count() == 1 && foreground.is_clock()
            || text.trim().chars().count() == 1
                && "<>Vv→⇒▶".contains(text.trim())
                && foreground.is_arrow())
    {
        return true;
    }
    // 赤いバツは、正しい英字 X と同じ内容でも原稿の図として保つ。
    text.trim().chars().count() == 1
        && "✕×Xx".contains(text.trim())
        && chroma > 50
        && color[0] > color[1].saturating_add(35)
}

fn line_fragment(
    observation: &Observation,
    text: String,
    bbox: BBox,
    bounded: bool,
    scale: f64,
) -> PlannedText {
    let mut block = observation.block.clone();
    block.text = text.clone();
    block.bbox = bbox;
    if let Some(line) = block.lines.first_mut() {
        line.spans = vec![n2p_layout::Span {
            text: Some(text),
            image_path: None,
        }];
        line.bbox = Some(bbox);
    }
    PlannedText {
        block,
        source_block_index: observation.block_index,
        source_line_indices: observation.line_index.into_iter().collect(),
        font_size_pt: bounded.then(|| crate::estimate_font_size(&observation.block, scale)),
        bounded,
    }
}

fn process_observation(
    observation: &Observation,
    image: &RgbImage,
    image_scale: f64,
    scale: f64,
    plan: &mut ProtectionPlan,
) -> Vec<PlannedText> {
    let mut block = observation.block.clone();
    let foreground = Foreground::new(image, block.bbox.scale(image_scale));
    let pure_bullet = crate::strip_leading_bullet(&block.text).0.trim().is_empty()
        && !block.text.trim().is_empty();
    let uncertain_graphic = observation.confidence.is_none_or(|c| c < 800)
        && foreground.colorful_context()
        && crate::is_decoration_block(image, block.bbox, image_scale, &block.text);
    if pure_bullet || graphic_token(&foreground, &block.text) || uncertain_graphic {
        let bbox = expand(block.bbox, 3);
        let reason = if uncertain_graphic {
            PreservationReason::UncertainGraphic
        } else {
            PreservationReason::GraphicSymbol
        };
        plan.preserved
            .push(observation.preserve(block.text, bbox, reason));
        plan.protected_boxes.push(bbox);
        return Vec::new();
    }
    if block.rotation_mdeg.abs() >= 3_000 {
        return vec![line_fragment(
            observation,
            block.text,
            block.bbox,
            false,
            scale,
        )];
    }
    let mut changed = false;
    if let Some((prefix, next)) = foreground.prefix() {
        let prefix_color = foreground.color(prefix);
        let rest = BBox::new(
            next,
            foreground.bbox.top,
            foreground.bbox.right,
            foreground.bbox.bottom,
        );
        let rest_color = foreground.color(rest);
        let first = block.text.trim_start().chars().next();
        let different_color = prefix_color
            .zip(rest_color)
            .is_some_and(|(a, b)| a.iter().zip(b).any(|(a, b)| a.abs_diff(b) > 55));
        let prefix_features = Foreground::new(image, prefix);
        let matches_shape = first.is_some_and(|c| graphic_token(&prefix_features, &c.to_string()));
        let known_symbol =
            first.is_some_and(|c| crate::BULLET_CHARS.contains(&c) || "♥✕×".contains(c));
        let colored_zero = first.is_some_and(|c| "0Oo〇○".contains(c)) && different_color;
        if matches_shape || known_symbol || colored_zero {
            let bbox = expand(prefix.scale(1.0 / image_scale), 2);
            let leading = first.unwrap().to_string();
            plan.preserved.push(observation.preserve(
                leading,
                bbox,
                PreservationReason::GraphicPrefix,
            ));
            plan.protected_boxes.push(bbox);
            block.text = block
                .text
                .trim_start()
                .chars()
                .skip(1)
                .collect::<String>()
                .trim_start()
                .to_owned();
            block.bbox.left = (next as f64 / image_scale).round() as i64;
            changed = true;
        }
    }
    // 色付きチェックが本文中の < や V として結合された場合も、その画素だけを残す。
    let foreground = Foreground::new(image, block.bbox.scale(image_scale));
    let mut symbols: Vec<(usize, BBox)> = Vec::new();
    let units = crate::fontsize::full_width_units(&block.text).max(0.5);
    for component in foreground.components(InkColor::Green) {
        if !foreground.is_tick(component.bbox) {
            continue;
        }
        let center = (component.bbox.left + component.bbox.right) as f64 * 0.5;
        let mut advance = 0.0;
        let mut best: Option<(usize, f64)> = None;
        for (index, ch) in block.text.char_indices() {
            let width = crate::fontsize::full_width_units(&ch.to_string());
            if "<くVv✓✔♥".contains(ch) {
                let expected = foreground.bbox.left as f64
                    + (advance + width * 0.5) / units * foreground.bbox.width() as f64;
                let distance = (expected - center).abs();
                if best.is_none_or(|(_, d)| distance < d) {
                    best = Some((index, distance));
                }
            }
            advance += width;
        }
        if let Some((index, distance)) = best {
            if distance <= foreground.bbox.height().max(1) as f64 * 0.8 {
                symbols.push((index, expand(component.bbox.scale(1.0 / image_scale), 2)));
            }
        }
    }
    symbols.sort_by_key(|(index, _)| *index);
    symbols.dedup_by_key(|(index, _)| *index);
    if symbols.is_empty() {
        return vec![line_fragment(
            observation,
            block.text,
            block.bbox,
            changed,
            scale,
        )];
    }
    let mut editable = Vec::new();
    let mut offset = 0;
    let mut left = block.bbox.left;
    for (index, bbox) in symbols {
        let text = block.text[offset..index].trim().to_owned();
        if !text.is_empty() && bbox.left > left {
            editable.push(line_fragment(
                observation,
                text,
                BBox::new(left, block.bbox.top, bbox.left, block.bbox.bottom),
                true,
                scale,
            ));
        }
        let symbol = block.text[index..].chars().next().unwrap();
        plan.preserved.push(observation.preserve(
            symbol.to_string(),
            bbox,
            PreservationReason::InlineSymbol,
        ));
        plan.protected_boxes.push(bbox);
        offset = index + symbol.len_utf8();
        left = bbox.right;
    }
    let text = block.text[offset..].trim().to_owned();
    if !text.is_empty() && left < block.bbox.right {
        editable.push(line_fragment(
            observation,
            text,
            BBox::new(left, block.bbox.top, block.bbox.right, block.bbox.bottom),
            true,
            scale,
        ));
    }
    editable
}

fn adjacent_graphics(
    observations: &[Observation],
    image: &RgbImage,
    image_scale: f64,
    plan: &mut ProtectionPlan,
) {
    for observation in observations {
        let editable = plan.editable.iter().any(|text| {
            text.source_block_index == observation.block_index
                && observation
                    .line_index
                    .is_none_or(|line| text.source_line_indices.contains(&line))
        });
        if !editable {
            continue;
        }
        let body = observation.block.bbox;
        let height = body.height().max(1);
        let context = BBox::new(
            body.left - height * 2,
            body.top - height / 3,
            body.right + height * 2,
            body.bottom + height / 3,
        )
        .scale(image_scale);
        let foreground = Foreground::new(image, context);
        for component in foreground.components(InkColor::Chromatic) {
            let bbox = component.bbox.scale(1.0 / image_scale);
            let at_side =
                bbox.right <= body.left + height / 2 || bbox.left >= body.right - height / 2;
            if !at_side
                // 観測の内部に収まる字形を隣接アイコンと誤判定しない。
                || intersects_most(body, bbox)
                || bbox.height() < height * 4 / 10
                || bbox.height() > height * 2
                || bbox.width() > height * 3
            {
                continue;
            }
            if plan
                .protected_boxes
                .iter()
                .any(|region| intersects_most(*region, bbox))
            {
                continue;
            }
            // 別の正しい文字観測に属する画素を、隣接アイコンとして固定しない。
            if observations.iter().any(|other| {
                (other.block_index != observation.block_index
                    || other.line_index != observation.line_index)
                    && intersects_most(other.block.bbox, bbox)
                    && plan.editable.iter().any(|text| {
                        text.source_block_index == other.block_index
                            && other
                                .line_index
                                .is_none_or(|line| text.source_line_indices.contains(&line))
                    })
            }) {
                continue;
            }
            let color = foreground.color(component.bbox).unwrap_or([0; 3]);
            let aspect = bbox.width() as f64 / bbox.height().max(1) as f64;
            let density = component.pixels as f64
                / (component.bbox.width() * component.bbox.height()).max(1) as f64;
            let red_cross = color[0] > color[1].saturating_add(35)
                && (0.7..=1.4).contains(&aspect)
                && (0.15..=0.7).contains(&density);
            let shape = Foreground::new(image, component.bbox);
            if foreground.is_tick(component.bbox)
                || red_cross
                || shape.is_arrow()
                || shape.is_clock()
                || shape.is_people()
            {
                let bbox = expand(bbox, 3);
                plan.preserved.push(observation.preserve(
                    String::new(),
                    bbox,
                    PreservationReason::AdjacentGraphic,
                ));
                plan.protected_boxes.push(bbox);
            }
        }
    }
}

fn bound_text_beside_graphics(plan: &mut ProtectionPlan, scale: f64) {
    for text in &mut plan.editable {
        // 傾斜文字の外接矩形は図形と重なるため、水平な境界では裁ち落とさない。
        if text.block.rotation_mdeg.abs() >= 3_000 {
            continue;
        }
        let original = text.block.bbox;
        let mut bounds = original;
        for protected in &plan.protected_boxes {
            let overlap =
                (original.bottom.min(protected.bottom) - original.top.max(protected.top)).max(0);
            if overlap * 2 < original.height() {
                continue;
            }
            if protected.left <= bounds.left
                && protected.right > bounds.left
                && protected.right < original.left + original.width() * 45 / 100
            {
                bounds.left = protected.right + 1;
            }
            if protected.right >= bounds.right
                && protected.left < bounds.right
                && protected.left > original.right - original.width() * 45 / 100
            {
                bounds.right = protected.left - 1;
            }
        }
        if bounds != original && !bounds.is_empty() {
            text.font_size_pt
                .get_or_insert_with(|| crate::estimate_font_size(&text.block, scale));
            text.block.bbox = bounds;
            for line in &mut text.block.lines {
                if let Some(bbox) = &mut line.bbox {
                    bbox.left = bbox.left.max(bounds.left);
                    bbox.right = bbox.right.min(bounds.right);
                }
            }
            text.bounded = true;
        }
    }
}

/// 複数の面に沿って折れたラベルを、中央が下がる基線と異常な外接高さから判定する。
fn bent_label(text: &str, foreground: &Foreground) -> bool {
    let len = text.chars().count();
    let width = foreground.width;
    let height = foreground.bbox.height().max(0) as usize;
    let units = crate::fontsize::full_width_units(text);
    if !(5..=18).contains(&len)
        || crate::fontsize::full_width_ratio(text) < 0.5
        || units <= 0.0
        || width < height * 5 / 2
        || (height as f64) < width as f64 / units * 1.7
    {
        return false;
    }
    let mut bottoms: [Vec<usize>; 3] = std::array::from_fn(|_| Vec::new());
    for x in 0..width {
        let ys: Vec<usize> = (0..height)
            .filter(|y| foreground.bits[y * width + x])
            .collect();
        if ys.len() >= 4 {
            bottoms[(x * 3 / width).min(2)].push(*ys.last().unwrap());
        }
    }
    if bottoms.iter().any(|values| values.len() < width / 12) {
        return false;
    }
    let median: Vec<f64> = bottoms
        .iter_mut()
        .map(|values| {
            values.sort_unstable();
            values[values.len() / 2] as f64
        })
        .collect();
    median[1] - median[0] > height as f64 * 0.14 && median[1] - median[2] > height as f64 * 0.14
}

pub(crate) fn plan(layout: &LayoutPage, image: &RgbImage, scale: f64) -> ProtectionPlan {
    let observations = observations(layout, scale);
    let ui_regions = small_ui_regions(&observations, image, layout.size_pt.0, scale);
    let image_scale = image.width as f64 / layout.size_pt.0;
    let perspective_regions: Vec<BBox> = observations
        .iter()
        .filter(|o| {
            o.block.rotation_mdeg.abs() < 3_000
                && bent_label(
                    &o.block.text,
                    &Foreground::new(image, o.block.bbox.scale(image_scale)),
                )
        })
        .map(|o| {
            let bbox = o.block.bbox;
            // 同じ面に続く欧文の副ラベルも画像に残す。
            BBox::new(
                bbox.left - 2,
                bbox.top - 2,
                bbox.right + 2,
                bbox.bottom + bbox.height() * 3 / 4,
            )
        })
        .collect();
    let mut plan = ProtectionPlan {
        editable: Vec::new(),
        preserved: Vec::new(),
        protected_boxes: ui_regions.clone(),
    };
    plan.protected_boxes.extend(&perspective_regions);
    for (block_index, original) in layout.text_blocks().enumerate() {
        let rows: Vec<&Observation> = observations
            .iter()
            .filter(|o| o.block_index == block_index)
            .collect();
        let mut pending = Vec::new();
        let before = plan.preserved.len();
        for observation in &rows {
            if perspective_regions
                .iter()
                .any(|region| intersects_most(*region, observation.block.bbox))
            {
                let bbox = expand(observation.block.bbox, 2);
                plan.preserved.push(observation.preserve(
                    observation.block.text.clone(),
                    bbox,
                    PreservationReason::PerspectiveText,
                ));
                plan.protected_boxes.push(bbox);
            } else if ui_regions
                .iter()
                .any(|region| intersects_most(*region, observation.block.bbox))
            {
                let bbox = expand(observation.block.bbox, 2);
                plan.preserved.push(observation.preserve(
                    observation.block.text.clone(),
                    bbox,
                    PreservationReason::SmallUiText,
                ));
                plan.protected_boxes.push(bbox);
            } else {
                pending.extend(process_observation(
                    observation,
                    image,
                    image_scale,
                    scale,
                    &mut plan,
                ));
            }
        }
        if plan.preserved.len() == before {
            plan.editable.push(PlannedText {
                block: original.clone(),
                source_block_index: block_index,
                source_line_indices: (0..original.lines.len()).collect(),
                font_size_pt: None,
                bounded: false,
            });
        } else {
            // 保護した観測だけを取り除き、残る本文は行単位でも編集可能に保つ。
            plan.editable.extend(pending);
        }
    }
    adjacent_graphics(&observations, image, image_scale, &mut plan);
    bound_text_beside_graphics(&mut plan, scale);
    plan
}
