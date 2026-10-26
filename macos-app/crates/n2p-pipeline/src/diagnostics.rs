//! 元 PDF のページ番号を保った診断ファイルの保存。

use std::path::{Path, PathBuf};

use n2p_build::diagnostics::{PageDiagnostics, bbox_array};
use n2p_core::RgbImage;
use n2p_image::Mask;
use n2p_layout::LayoutPage;
use n2p_vision::{VisionConfig, VisionDocument};
use serde_json::{Value, json};

use crate::{Error, Result};

pub(crate) fn page_dir(root: &Path, page: usize) -> PathBuf {
    root.join(format!("page_{page:04}"))
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| Error::Image(e.to_string()))?;
    std::fs::write(path, bytes)?;
    Ok(())
}

fn mask_image(mask: &Mask) -> RgbImage {
    let mut image = RgbImage::new(mask.width, mask.height);
    for y in 0..mask.height {
        for x in 0..mask.width {
            if mask.get(x, y) {
                image.set_pixel(x, y, [255; 3]);
            }
        }
    }
    image
}

pub(crate) fn save_page(root: &Path, page: usize, diagnostics: &PageDiagnostics) -> Result<()> {
    let dir = page_dir(root, page);
    std::fs::create_dir_all(&dir)?;
    for (name, image) in [
        ("mask-initial.png", mask_image(&diagnostics.initial_mask)),
        ("mask-erasure.png", mask_image(&diagnostics.erasure_mask)),
    ] {
        image
            .save(dir.join(name))
            .map_err(|e| Error::Image(e.to_string()))?;
    }
    if let Some(background) = &diagnostics.background {
        background
            .save(dir.join("background.png"))
            .map_err(|e| Error::Image(e.to_string()))?;
    }
    write_json(
        &dir.join("build.json"),
        &json!({
            "source_page": page,
            "initial_mask_size_px": [diagnostics.initial_mask.width, diagnostics.initial_mask.height],
            "erasure_mask_size_px": [diagnostics.erasure_mask.width, diagnostics.erasure_mask.height],
            "report": diagnostics.report,
        }),
    )
}

pub(crate) fn save_layout(root: &Path, page: usize, layout: &LayoutPage) -> Result<()> {
    let dir = page_dir(root, page);
    std::fs::create_dir_all(&dir)?;
    write_json(
        &dir.join("layout.json"),
        &json!({
            "source_page": page,
            "size_pt": [layout.size_pt.0, layout.size_pt.1],
            "blocks": layout.blocks.iter().map(|block| json!({
                "kind": block.kind.label(),
                "text": block.text,
                "bbox_pt": bbox_array(block.bbox),
                "rotation_mdeg": block.rotation_mdeg,
                "lines": block.lines.iter().map(|line| json!({
                    "text": line.text(),
                    "bbox_pt": line.bbox.map(bbox_array),
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        }),
    )
}

pub(crate) fn save_ocr(
    root: &Path,
    selected: &[usize],
    document: &VisionDocument,
    config: &VisionConfig,
) -> Result<()> {
    for (page, observations) in selected.iter().zip(document.observations()) {
        let dir = page_dir(root, *page);
        std::fs::create_dir_all(&dir)?;
        write_json(
            &dir.join("ocr.json"),
            &json!({
                "source_page": page,
                "config": {
                    "languages": config.languages,
                    "accurate": config.accurate,
                    "language_correction": config.language_correction,
                    "minimum_text_height": config.minimum_text_height,
                    "min_confidence": config.min_confidence,
                    "group_lines": config.group_lines,
                },
                "observations": observations.iter().map(|observation| json!({
                    "text": observation.text,
                    "confidence": observation.confidence,
                    "bbox_norm": observation.bbox_norm,
                    "corners_norm": observation.corners_norm,
                    "accepted": observation.confidence >= config.min_confidence
                        && !observation.text.trim().is_empty(),
                })).collect::<Vec<_>>(),
            }),
        )?;
    }
    Ok(())
}
