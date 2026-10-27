//! ファイルへ保存した根拠を、領域と分類ごとに再評価する。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail, ensure};
use n2p_core::RgbImage;
use n2p_ooxml::{
    deck::Deck,
    xml::{Element, Node},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    version: u32,
    provenance: Value,
    checks: Vec<Check>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Check {
    name: String,
    category: Category,
    #[serde(default = "required")]
    required: bool,
    page: Option<usize>,
    region: Option<Region>,
    metric: Metric,
}

fn required() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Category {
    Background,
    Protection,
    Text,
    Placement,
    Formatting,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Region {
    space: Space,
    canvas: [f64; 2],
    rect: [f64; 4],
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Space {
    ImagePx,
    SlidePt,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Metric {
    ImageDiff {
        actual: PathBuf,
        expected: PathBuf,
        mask: Option<PathBuf>,
        pixel_threshold: u8,
        max_changed_pixels: usize,
    },
    Luminance {
        image: PathBuf,
        below: Option<u8>,
        above: Option<u8>,
        max_pixels: usize,
    },
    MaskOverlap {
        preserved: PathBuf,
        erasure: PathBuf,
        max_pixels: usize,
    },
    Text {
        pptx: PathBuf,
        expected: Vec<String>,
    },
    Runs {
        pptx: PathBuf,
        expected: Vec<Vec<Run>>,
    },
    Bounds {
        pptx: PathBuf,
        measurements: PathBuf,
        diagnostics: PathBuf,
        tolerance_pt: f64,
    },
    JsonEqual {
        actual: PathBuf,
        pointer: String,
        expected: Value,
    },
    Unmeasured {
        reason: String,
    },
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Run {
    text: String,
    bold: bool,
    color: String,
    latin_font: String,
    east_asian_font: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Status {
    Passed,
    Failed,
    Unmeasured,
}

#[derive(Debug, Serialize)]
struct Outcome {
    name: String,
    category: Category,
    required: bool,
    page: Option<usize>,
    region: Option<Region>,
    metric: Metric,
    status: Status,
    values: Value,
    error: Option<String>,
}

pub fn run(config_path: &Path, report_path: &Path) -> Result<usize> {
    let config: Config = serde_json::from_slice(&std::fs::read(config_path)?)
        .with_context(|| format!("Invalid quality configuration: {}", config_path.display()))?;
    ensure!(
        config.version == 1,
        "Unsupported quality configuration version"
    );
    ensure!(!config.checks.is_empty(), "No quality checks configured");
    let mut names = std::collections::HashSet::new();
    for check in &config.checks {
        ensure!(
            !check.name.trim().is_empty() && names.insert(&check.name),
            "Empty or duplicate check name: {}",
            check.name
        );
    }
    let base = config_path.parent().unwrap_or_else(|| Path::new("."));
    let mut outcomes = Vec::new();
    for check in config.checks {
        let (status, values, error) = match measure(&check, base) {
            Ok((values, true)) => (Status::Passed, values, None),
            Ok((values, false)) => (Status::Failed, values, None),
            Err(error) => (Status::Unmeasured, Value::Null, Some(format!("{error:#}"))),
        };
        outcomes.push(Outcome {
            name: check.name,
            category: check.category,
            required: check.required,
            page: check.page,
            region: check.region,
            metric: check.metric,
            status,
            values,
            error,
        });
    }
    let count = |status| outcomes.iter().filter(|o| o.status == status).count();
    let failures = outcomes
        .iter()
        .filter(|o| o.required && o.status != Status::Passed)
        .count();
    let mut categories = serde_json::Map::new();
    for category in [
        "background",
        "protection",
        "text",
        "placement",
        "formatting",
    ] {
        let checks: Vec<_> = outcomes
            .iter()
            .filter(|o| serde_json::to_value(o.category).unwrap() == category)
            .collect();
        categories.insert(category.into(), json!({"passed":checks.iter().filter(|o| o.status==Status::Passed).count(), "failed":checks.iter().filter(|o|o.status==Status::Failed).count(),"unmeasured":checks.iter().filter(|o|o.status==Status::Unmeasured).count(), "configured":checks.len()}));
    }
    let summary = json!({"passed":count(Status::Passed),"failed":count(Status::Failed),"unmeasured":count(Status::Unmeasured),"required_failures":failures,"by_category":categories});
    let report =
        json!({"version":1,"provenance":config.provenance,"summary":summary,"checks":outcomes});
    if let Some(parent) = report_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        report_path,
        format!("{}\n", serde_json::to_string_pretty(&report)?),
    )?;
    println!(
        "Quality evaluation: {} passed, {} failed, {} unmeasured; {failures} required checks did not pass",
        count(Status::Passed),
        count(Status::Failed),
        count(Status::Unmeasured)
    );
    Ok(failures)
}

fn measure(check: &Check, base: &Path) -> Result<(Value, bool)> {
    if let Some(region) = &check.region {
        region.validate()?;
    }
    match &check.metric {
        Metric::ImageDiff {
            actual,
            expected,
            mask,
            pixel_threshold,
            max_changed_pixels,
        } => {
            let actual = RgbImage::open(base.join(actual))?;
            let expected = RgbImage::open(base.join(expected))?;
            same_size(&actual, &expected)?;
            let mask = mask
                .as_ref()
                .map(|p| RgbImage::open(base.join(p)))
                .transpose()?;
            if let Some(mask) = &mask {
                same_size(&actual, mask)?;
            }
            let [x0, y0, x1, y1] = pixel_rect(check.region.as_ref(), &actual)?;
            let mut selected = 0usize;
            let mut changed = 0usize;
            let mut max_channel = 0u8;
            for y in y0..y1 {
                for x in x0..x1 {
                    if mask.as_ref().is_some_and(|m| m.pixel(x, y) == [0; 3]) {
                        continue;
                    }
                    selected += 1;
                    let delta = actual
                        .pixel(x, y)
                        .iter()
                        .zip(expected.pixel(x, y))
                        .map(|(a, e)| a.abs_diff(e))
                        .max()
                        .unwrap();
                    max_channel = max_channel.max(delta);
                    if delta > *pixel_threshold {
                        changed += 1;
                    }
                }
            }
            ensure!(selected > 0, "No pixels selected (empty region or mask)");
            Ok((
                json!({"selected_pixels":selected,"changed_pixels":changed,"max_channel":max_channel}),
                changed <= *max_changed_pixels,
            ))
        }
        Metric::Luminance {
            image,
            below,
            above,
            max_pixels,
        } => {
            ensure!(
                below.is_some() ^ above.is_some(),
                "Specify exactly one luminance bound"
            );
            let image = RgbImage::open(base.join(image))?;
            let [x0, y0, x1, y1] = pixel_rect(check.region.as_ref(), &image)?;
            let mut count = 0usize;
            for y in y0..y1 {
                for x in x0..x1 {
                    let [r, g, b] = image.pixel(x, y);
                    let value = (299u32 * r as u32 + 587u32 * g as u32 + 114u32 * b as u32) / 1000;
                    if below.is_some_and(|v| value < (v as u32))
                        || above.is_some_and(|v| value > (v as u32))
                    {
                        count += 1;
                    }
                }
            }
            Ok((
                json!({"matching_pixels":count,"selected_pixels":(x1-x0) as usize*(y1-y0) as usize}),
                count <= *max_pixels,
            ))
        }
        Metric::MaskOverlap {
            preserved,
            erasure,
            max_pixels,
        } => {
            let preserved = RgbImage::open(base.join(preserved))?;
            let erasure = RgbImage::open(base.join(erasure))?;
            same_size(&preserved, &erasure)?;
            let [x0, y0, x1, y1] = pixel_rect(check.region.as_ref(), &preserved)?;
            let mut count = 0usize;
            let mut protected = 0usize;
            for y in y0..y1 {
                for x in x0..x1 {
                    if preserved.pixel(x, y) != [0; 3] {
                        protected += 1;
                        if erasure.pixel(x, y) != [0; 3] {
                            count += 1;
                        }
                    }
                }
            }
            // 保護対象がないページでも、両マスクを読み取って重なりを計測できる。
            Ok((
                json!({"overlap_pixels":count,"protected_pixels":protected}),
                count <= *max_pixels,
            ))
        }
        Metric::Text { pptx, expected } => {
            ensure!(!expected.is_empty(), "Empty expected text set");
            let deck = Deck::open(base.join(pptx))?;
            let shapes = selected_shapes(&deck, page(check)?, check.region.as_ref())?;
            ensure!(!shapes.is_empty(), "No text shapes in the selected region");
            let actual: Vec<_> = shapes.iter().map(|s| s.text.clone()).collect();
            Ok((
                json!({"actual":actual,"expected":expected}),
                actual == *expected,
            ))
        }
        Metric::Runs { pptx, expected } => {
            ensure!(
                !expected.is_empty() && expected.iter().all(|r| !r.is_empty()),
                "Empty expected run set"
            );
            let deck = Deck::open(base.join(pptx))?;
            let shapes = selected_shapes(&deck, page(check)?, check.region.as_ref())?;
            ensure!(!shapes.is_empty(), "No text shapes in the selected region");
            let actual: Vec<_> = shapes.into_iter().map(|s| s.runs).collect();
            Ok((
                json!({"actual":actual,"expected":expected}),
                actual == *expected,
            ))
        }
        Metric::Bounds {
            pptx,
            measurements,
            diagnostics,
            tolerance_pt,
        } => {
            ensure!(
                check.region.is_none(),
                "Bounds uses per-shape allowed regions; do not set a check region"
            );
            let deck = Deck::open(base.join(pptx))?;
            let measurements: Measurements =
                serde_json::from_slice(&std::fs::read(base.join(measurements))?)?;
            let diagnostics: Value =
                serde_json::from_slice(&std::fs::read(base.join(diagnostics))?)?;
            measure_bounds(
                &deck,
                &measurements,
                &diagnostics,
                page(check)?,
                *tolerance_pt,
            )
        }
        Metric::JsonEqual {
            actual,
            pointer,
            expected,
        } => {
            ensure!(check.region.is_none(), "JSON checks do not support regions");
            let actual: Value = serde_json::from_slice(&std::fs::read(base.join(actual))?)?;
            let value = actual
                .pointer(pointer)
                .ok_or_else(|| anyhow!("Missing JSON pointer: {pointer}"))?;
            Ok((
                json!({"actual":value,"expected":expected}),
                value == expected,
            ))
        }
        Metric::Unmeasured { reason } => bail!("{reason}"),
    }
}

impl Region {
    fn validate(&self) -> Result<()> {
        let [w, h] = self.canvas;
        let [l, t, r, b] = self.rect;
        ensure!(
            self.canvas
                .iter()
                .chain(self.rect.iter())
                .all(|v| v.is_finite())
                && w > 0.0
                && h > 0.0
                && l >= 0.0
                && t >= 0.0
                && r > l
                && b > t
                && r <= w
                && b <= h,
            "Invalid or out-of-canvas region"
        );
        Ok(())
    }
}

fn pixel_rect(region: Option<&Region>, image: &RgbImage) -> Result<[u32; 4]> {
    let Some(region) = region else {
        return Ok([0, 0, image.width, image.height]);
    };
    region.validate()?;
    ensure!(
        region.space == Space::ImagePx,
        "Image metrics require image_px regions"
    );
    let [w, h] = region.canvas;
    let [l, t, r, b] = region.rect;
    let scale = |value: f64, extent: f64, pixels: u32| {
        let factor = pixels as f64 / extent;
        if factor.is_finite() {
            value * factor
        } else {
            (value / extent) * pixels as f64
        }
    };
    let pixels = [
        scale(l, w, image.width).floor() as u32,
        scale(t, h, image.height).floor() as u32,
        (scale(r, w, image.width).ceil() as u32).min(image.width),
        (scale(b, h, image.height).ceil() as u32).min(image.height),
    ];
    ensure!(
        pixels[0] < pixels[2] && pixels[1] < pixels[3],
        "No pixels selected by the region"
    );
    Ok(pixels)
}

fn same_size(a: &RgbImage, b: &RgbImage) -> Result<()> {
    ensure!(
        a.width == b.width && a.height == b.height,
        "Image/mask dimensions differ"
    );
    Ok(())
}

fn page(check: &Check) -> Result<usize> {
    let page = check.page.ok_or_else(|| anyhow!("Page is required"))?;
    ensure!(page > 0, "Pages are one-based");
    Ok(page)
}

struct Shape {
    text: String,
    rect: [f64; 4],
    runs: Vec<Run>,
}

fn selected_shapes(deck: &Deck, page: usize, region: Option<&Region>) -> Result<Vec<Shape>> {
    ensure!(page > 0, "Pages are one-based");
    let doc = deck.slide(page - 1)?;
    let tree = doc
        .root
        .path(&["p:cSld", "p:spTree"])
        .ok_or_else(|| anyhow!("Missing shape tree"))?;
    ensure!(
        tree.child_el("p:grpSp").is_none(),
        "Grouped shapes are unsupported by quality evaluation"
    );
    let mut shapes = Vec::new();
    for sp in tree.elements("p:sp") {
        let Some(body) = sp.child_el("p:txBody") else {
            continue;
        };
        let off = sp
            .path(&["p:spPr", "a:xfrm", "a:off"])
            .ok_or_else(|| anyhow!("Missing shape offset"))?;
        let ext = sp
            .path(&["p:spPr", "a:xfrm", "a:ext"])
            .ok_or_else(|| anyhow!("Missing shape extent"))?;
        let number = |e: &Element, k: &str| -> Result<f64> {
            Ok(e.get_attr(k)
                .ok_or_else(|| anyhow!("Missing {k}"))?
                .parse::<i64>()? as f64
                / 12700.0)
        };
        let (x, y) = (number(off, "x")?, number(off, "y")?);
        let rect = [x, y, x + number(ext, "cx")?, y + number(ext, "cy")?];
        if let Some(region) = region {
            region.validate()?;
            ensure!(
                region.space == Space::SlidePt,
                "Text metrics require slide_pt regions"
            );
            let (w, h, _) = deck.slide_size()?;
            ensure!(
                (region.canvas[0] - w as f64 / 12700.0).abs() < 0.01
                    && (region.canvas[1] - h as f64 / 12700.0).abs() < 0.01,
                "Region canvas does not match PPTX slide size"
            );
            let [l, t, r, b] = region.rect;
            let (cx, cy) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
            if cx < l || cx >= r || cy < t || cy >= b {
                continue;
            }
        }
        let mut paragraphs = Vec::new();
        let mut runs = Vec::new();
        for paragraph in body.elements("a:p") {
            ensure!(
                paragraph.child_el("a:fld").is_none(),
                "Text fields are unsupported"
            );
            let mut text = String::new();
            for child in &paragraph.children {
                let Node::Element(el) = child else {
                    continue;
                };
                if el.name == "a:br" {
                    text.push('\n');
                    continue;
                }
                if el.name != "a:r" {
                    continue;
                }
                let t = el
                    .child_el("a:t")
                    .ok_or_else(|| anyhow!("Missing run text"))?;
                let content = t
                    .children
                    .iter()
                    .filter_map(|n| {
                        if let Node::Text(t) = n {
                            Some(t.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<String>();
                text.push_str(&content);
                let props = el
                    .child_el("a:rPr")
                    .ok_or_else(|| anyhow!("Missing explicit run properties"))?;
                let attr = |path: &[&str], key: &str| {
                    props
                        .path(path)
                        .and_then(|e| e.get_attr(key))
                        .map(str::to_owned)
                };
                runs.push(Run {
                    text: content,
                    bold: props.get_attr("b").is_some_and(|v| v == "1" || v == "true"),
                    color: attr(&["a:solidFill", "a:srgbClr"], "val")
                        .ok_or_else(|| anyhow!("Missing explicit RGB text color"))?,
                    latin_font: attr(&["a:latin"], "typeface")
                        .ok_or_else(|| anyhow!("Missing explicit Latin font"))?,
                    east_asian_font: attr(&["a:ea"], "typeface"),
                });
            }
            paragraphs.push(text);
        }
        shapes.push(Shape {
            text: paragraphs.join("\n"),
            rect,
            runs,
        });
    }
    Ok(shapes)
}

#[derive(Deserialize)]
struct Measurements {
    text_bounds: Vec<Bound>,
}
#[derive(Deserialize)]
struct Bound {
    page: usize,
    index: usize,
    frame: [f64; 4],
    ink: [f64; 4],
}

fn measure_bounds(
    deck: &Deck,
    measurements: &Measurements,
    diagnostics: &Value,
    page: usize,
    tolerance: f64,
) -> Result<(Value, bool)> {
    ensure!(
        tolerance.is_finite() && tolerance >= 0.0,
        "Invalid bounds tolerance"
    );
    let shapes = selected_shapes(deck, page, None)?;
    ensure!(!shapes.is_empty(), "No text shapes to measure");
    let blocks = diagnostics
        .pointer("/report/text_blocks")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Missing diagnostic text blocks"))?;
    let mut outputs = Vec::new();
    for block in blocks {
        outputs.extend(
            block["output_text_boxes"]
                .as_array()
                .ok_or_else(|| anyhow!("Missing diagnostic output boxes"))?,
        );
    }
    ensure!(
        outputs.len() == shapes.len(),
        "Diagnostic/PPTX shape count mismatch"
    );
    let measured: Vec<_> = measurements
        .text_bounds
        .iter()
        .filter(|b| b.page == page)
        .collect();
    ensure!(
        measured.len() == shapes.len(),
        "Incomplete PowerPoint bounds for page {page}"
    );
    let (w, h, _) = deck.slide_size()?;
    let slide = [0.0, 0.0, w as f64 / 12700.0, h as f64 / 12700.0];
    let mut seen = std::collections::HashSet::new();
    let mut maxima = [0.0f64; 3];
    for bound in measured {
        ensure!(
            bound.index > 0 && bound.index <= shapes.len() && seen.insert(bound.index),
            "Duplicate or invalid PowerPoint bounds index"
        );
        let shape = &shapes[bound.index - 1];
        let output = outputs[bound.index - 1];
        valid_rect(bound.frame)?;
        valid_rect(bound.ink)?;
        let diag_rect: [f64; 4] = serde_json::from_value(output["rect_pt"].clone())?;
        ensure!(
            shape
                .rect
                .iter()
                .zip(bound.frame)
                .all(|(a, b)| (a - b).abs() <= 0.01)
                && shape
                    .rect
                    .iter()
                    .zip(diag_rect)
                    .all(|(a, b)| (a - b).abs() <= 0.001),
            "Stale PowerPoint bounds or diagnostics (frame differs from PPTX)"
        );
        ensure!(
            output["text"].as_str() == Some(shape.text.as_str()),
            "Diagnostic/PPTX text mismatch"
        );
        maxima[0] = maxima[0].max(
            (bound.frame[0] - bound.ink[0])
                .max(bound.ink[2] - bound.frame[2])
                .max(0.0),
        );
        maxima[1] = maxima[1].max(overflow(bound.ink, slide));
        if !output["placement"].is_null() {
            let allowed: [f64; 4] =
                serde_json::from_value(output["placement"]["allowed_rect_pt"].clone())?;
            valid_rect(allowed)?;
            maxima[2] = maxima[2].max(overflow(bound.ink, allowed));
        }
    }
    Ok((
        json!({"text_boxes":shapes.len(),"max_frame_horizontal_overflow_pt":maxima[0],"max_slide_overflow_pt":maxima[1],"max_allowed_region_overflow_pt":maxima[2],"source":"saved_powerpoint_measurements"}),
        maxima.iter().all(|v| *v <= tolerance),
    ))
}

fn valid_rect(r: [f64; 4]) -> Result<()> {
    ensure!(
        r.iter().all(|v| v.is_finite()) && r[0] < r[2] && r[1] < r[3],
        "Invalid measured rectangle"
    );
    Ok(())
}
fn overflow(ink: [f64; 4], allowed: [f64; 4]) -> f64 {
    (allowed[0] - ink[0])
        .max(allowed[1] - ink[1])
        .max(ink[2] - allowed[2])
        .max(ink[3] - allowed[3])
        .max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use n2p_core::RgbImage;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static ID: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "n2p-quality-{}-{}",
                std::process::id(),
                ID.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn image(&self, name: &str, image: &RgbImage) {
            image.save(self.0.join(name)).unwrap();
        }
        fn deck(&self, text: &str, bold: bool) -> Deck {
            use n2p_ooxml::{emu::Rect, opc::Package, shape::TextBox};
            let mut deck = Deck::from_package(
                Package::from_bytes(include_bytes!("../../crates/n2p-build/assets/blank.pptx"))
                    .unwrap(),
            )
            .unwrap();
            deck.push_text_box(
                0,
                &TextBox::new("test", text, Rect::ltrb(10.0, 10.0, 90.0, 40.0)).bold(bold),
            )
            .unwrap();
            deck.save(self.0.join("actual.pptx")).unwrap();
            deck
        }
        fn check(&self, metric: Value, region: Value) -> Result<(Value, bool)> {
            let check: Check = serde_json::from_value(json!({"name":"test", "category":"background", "page":1, "region":region, "metric":metric})).unwrap();
            measure(&check, &self.0)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn one_damaged_pixel_in_a_small_region_fails_even_when_global_diff_passes() {
        let f = Fixture::new();
        let expected = RgbImage::filled(100, 100, [0; 3]);
        let mut actual = expected.clone();
        actual.set_pixel(50, 50, [255; 3]);
        assert!(
            crate::compare::diff(&actual, &expected, 24)
                .unwrap()
                .is_within(&crate::compare::Thresholds::default())
        );
        f.image("actual.png", &actual);
        f.image("expected.png", &expected);
        let (v,pass)=f.check(json!({"kind":"image_diff","actual":"actual.png","expected":"expected.png","pixel_threshold":0,"max_changed_pixels":0}),json!({"space":"image_px","canvas":[100,100],"rect":[50,50,51,51]})).unwrap();
        assert!(!pass);
        assert_eq!(v["changed_pixels"], 1);
    }

    #[test]
    fn protected_pixel_changes_and_mask_overlap_are_detected() {
        let f = Fixture::new();
        let source = RgbImage::filled(4, 4, [80; 3]);
        let mut actual = source.clone();
        actual.set_pixel(1, 1, [81; 3]);
        let mut mask = RgbImage::filled(4, 4, [0; 3]);
        mask.set_pixel(1, 1, [255; 3]);
        f.image("source.png", &source);
        f.image("actual.png", &actual);
        f.image("mask.png", &mask);
        assert!(!f.check(json!({"kind":"image_diff","actual":"actual.png","expected":"source.png","mask":"mask.png","pixel_threshold":0,"max_changed_pixels":0}),Value::Null).unwrap().1);
        assert!(!f.check(json!({"kind":"mask_overlap","preserved":"mask.png","erasure":"mask.png","max_pixels":0}),Value::Null).unwrap().1);
    }

    #[test]
    fn empty_protection_mask_is_not_a_success() {
        let f = Fixture::new();
        let img = RgbImage::filled(2, 2, [0; 3]);
        f.image("empty.png", &img);
        assert!(f.check(json!({"kind":"image_diff","actual":"empty.png","expected":"empty.png","mask":"empty.png","pixel_threshold":0,"max_changed_pixels":0}),Value::Null).is_err());
    }

    #[test]
    fn image_regions_scale_from_the_declared_reference_canvas() {
        let f = Fixture::new();
        let mut img = RgbImage::filled(20, 20, [0; 3]);
        for y in 10..12 {
            for x in 10..12 {
                img.set_pixel(x, y, [255; 3]);
            }
        }
        f.image("image.png", &img);
        let (v, pass) = f
            .check(
                json!({"kind":"luminance","image":"image.png","above":160,"max_pixels":3}),
                json!({"space":"image_px","canvas":[10,10],"rect":[5,5,6,6]}),
            )
            .unwrap();
        assert!(!pass);
        assert_eq!(v["matching_pixels"], 4);
    }

    #[test]
    fn finite_large_coordinates_do_not_overflow_the_image() {
        let f = Fixture::new();
        f.image("image.png", &RgbImage::filled(2, 2, [255; 3]));
        let (values, passed) = f
            .check(
                json!({"kind":"luminance","image":"image.png","above":160,"max_pixels":4}),
                json!({"space":"image_px","canvas":[1e308,1e308],"rect":[0,0,1e308,1e308]}),
            )
            .unwrap();
        assert!(passed);
        assert_eq!(values["selected_pixels"], 4);
    }

    #[test]
    fn native_pixel_coordinates_do_not_expand_due_to_rounding() {
        let image = RgbImage::filled(100, 100, [0; 3]);
        for x in 0..100 {
            let region = Region {
                space: Space::ImagePx,
                canvas: [100.0, 100.0],
                rect: [x as f64, 0.0, (x + 1) as f64, 100.0],
            };
            assert_eq!(
                pixel_rect(Some(&region), &image).unwrap(),
                [x, 0, x + 1, 100]
            );
        }
    }

    #[test]
    fn invalid_and_out_of_canvas_regions_are_rejected() {
        let f = Fixture::new();
        f.image("image.png", &RgbImage::filled(2, 2, [255; 3]));
        for rect in [
            json!([0, 0, 0, 1]),
            json!([-1, 0, 1, 1]),
            json!([0, 0, 3, 1]),
        ] {
            assert!(
                f.check(
                    json!({"kind":"luminance","image":"image.png","above":160,"max_pixels":10}),
                    json!({"space":"image_px","canvas":[2,2],"rect":rect})
                )
                .is_err()
            );
        }
    }

    #[test]
    fn a_missing_input_is_reported_as_unmeasured_and_fails_required_checks() {
        let f = Fixture::new();
        let path = f.0.join("config.json");
        let report = f.0.join("report.json");
        std::fs::write(&path,serde_json::to_vec(&json!({"version":1,"provenance":{},"checks":[{"name":"missing","category":"text","metric":{"kind":"json_equal","actual":"missing.json","pointer":"/text","expected":"correct"}},{"name":"manual","category":"formatting","required":false,"metric":{"kind":"unmeasured","reason":"visual review pending"}}]})).unwrap()).unwrap();
        assert_eq!(run(&path, &report).unwrap(), 1);
        let v: Value = serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap();
        assert_eq!(v["checks"][0]["status"], "unmeasured");
        assert_eq!(v["checks"][1]["status"], "unmeasured");
        assert_eq!(v["summary"]["passed"], 0);
    }

    #[test]
    fn empty_or_duplicate_check_names_are_rejected() {
        let f = Fixture::new();
        let path = f.0.join("config.json");
        for checks in [
            json!([]),
            json!([{"name":"same","category":"text","metric":{"kind":"unmeasured","reason":"pending"}},{"name":"same","category":"text","metric":{"kind":"unmeasured","reason":"pending"}}]),
        ] {
            std::fs::write(
                &path,
                serde_json::to_vec(&json!({"version":1,"provenance":{},"checks":checks})).unwrap(),
            )
            .unwrap();
            assert!(run(&path, &f.0.join("report.json")).is_err());
        }
    }

    #[test]
    fn text_checks_preserve_case_underscores_and_line_breaks() {
        let f = Fixture::new();
        f.deck("Leave_Balance__c\nリソース", false);
        let expected = json!(["Leave_Balance__c\nリソース"]);
        assert!(
            f.check(
                json!({"kind":"text","pptx":"actual.pptx","expected":expected}),
                Value::Null
            )
            .unwrap()
            .1
        );
        for text in [
            "Leave_Balance__C\nリソース",
            "Leave_Balance_c\nリソース",
            "Leave_Balance__cリソース",
        ] {
            assert!(
                !f.check(
                    json!({"kind":"text","pptx":"actual.pptx","expected":[text]}),
                    Value::Null
                )
                .unwrap()
                .1
            );
        }
    }

    #[test]
    fn changed_run_weight_fails_even_if_text_matches() {
        let f = Fixture::new();
        f.deck("Label: body", true);
        let run = json!({"text":"Label: body","bold":false,"color":"000000","latin_font":"Calibri","east_asian_font":null});
        assert!(
            !f.check(
                json!({"kind":"runs","pptx":"actual.pptx","expected":[[run]]}),
                Value::Null
            )
            .unwrap()
            .1
        );
    }

    #[test]
    fn raw_ink_overflow_is_recomputed_and_incomplete_or_stale_bounds_are_rejected() {
        let f = Fixture::new();
        let deck = f.deck("text", false);
        let mut diagnostics = json!({"report":{"text_blocks":[{"output_text_boxes":[{"text":"text","rect_pt":[10,10,90,40],"placement":null}]}]}});
        let mut measurements = Measurements {
            text_bounds: vec![Bound {
                page: 1,
                index: 1,
                frame: [10.0, 10.0, 90.0, 40.0],
                ink: [11.0, 11.0, 93.0, 39.0],
            }],
        };
        assert!(
            !measure_bounds(&deck, &measurements, &diagnostics, 1, 2.0)
                .unwrap()
                .1
        );
        measurements.text_bounds[0].ink = [11.0, 11.0, 89.0, 39.0];
        assert!(
            measure_bounds(&deck, &measurements, &diagnostics, 1, 2.0)
                .unwrap()
                .1
        );
        diagnostics["report"]["text_blocks"][0]["output_text_boxes"][0]["placement"] =
            json!({"allowed_rect_pt":[10,10,80,40]});
        assert!(
            !measure_bounds(&deck, &measurements, &diagnostics, 1, 2.0)
                .unwrap()
                .1
        );
        measurements.text_bounds[0].frame[0] = 12.0;
        assert!(measure_bounds(&deck, &measurements, &diagnostics, 1, 2.0).is_err());
        measurements.text_bounds.clear();
        assert!(measure_bounds(&deck, &measurements, &diagnostics, 1, 2.0).is_err());
    }
}
