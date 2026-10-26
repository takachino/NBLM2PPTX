//! 作業ディレクトリ向けの診断情報。通常の変換では画像を保持しない。

use n2p_core::{BBox, RgbImage};
use n2p_image::Mask;
use serde::Serialize;

use crate::Result;

pub type DiagnosticSink<'a> = dyn FnMut(usize, &PageDiagnostics) -> Result<()> + 'a;

pub struct PageDiagnostics {
    pub report: PageDiagnosticReport,
    pub initial_mask: Mask,
    pub erasure_mask: Mask,
    pub preserved_mask: Mask,
    pub background: Option<RgbImage>,
}

impl PageDiagnostics {
    pub(crate) fn new(image: &RgbImage) -> Self {
        Self {
            report: PageDiagnosticReport::default(),
            initial_mask: Mask::new(image.width, image.height),
            erasure_mask: Mask::new(image.width, image.height),
            preserved_mask: Mask::new(image.width, image.height),
            background: None,
        }
    }
}

#[derive(Default, Serialize)]
pub struct PageDiagnosticReport {
    pub text_blocks: Vec<TextDiagnostic>,
    pub erasures: Vec<EraseDiagnostic>,
    pub lama_status: String,
    pub preserved_text: Vec<crate::PreservedText>,
    pub protected_boxes_pt: Vec<[i64; 4]>,
}

#[derive(Serialize)]
pub struct TextDiagnostic {
    pub block_index: usize,
    pub source_block_index: usize,
    pub source_line_indices: Vec<usize>,
    pub confidence_milli: Vec<Option<u16>>,
    pub natural_font_size_pt: f64,
    pub text: String,
    pub bbox_pt: [i64; 4],
    pub text_region_pt: [i64; 4],
    pub line_boxes_pt: Vec<Option<[i64; 4]>>,
    pub rotation_mdeg: i32,
    pub decision: &'static str,
    pub color: Option<[u8; 3]>,
    pub density: Option<f64>,
    pub output_text_boxes: Vec<OutputTextDiagnostic>,
}

#[derive(Serialize)]
pub struct OutputTextDiagnostic {
    pub text: String,
    pub rect_pt: [f64; 4],
    pub font_size_pt: f64,
    pub bold: bool,
    pub color: [u8; 3],
    pub latin_font: String,
    pub east_asian_font: Option<String>,
    pub emphasis_runs: Vec<(String, bool)>,
}

#[derive(Serialize)]
pub struct EraseDiagnostic {
    pub block_index: usize,
    pub line_index: Option<usize>,
    pub bbox_px: [i64; 4],
    pub expand_px: u32,
    pub mask_bbox_px: Option<[i64; 4]>,
    pub mask_pixels: usize,
    pub ring_flat: bool,
    pub sampling_bbox_px: Option<[i64; 4]>,
    pub background_color: Option<[u8; 3]>,
    pub background_samples: usize,
    pub local_flat: bool,
    pub method: &'static str,
    pub model_error: Option<String>,
}

pub fn bbox_array(bbox: BBox) -> [i64; 4] {
    [bbox.left, bbox.top, bbox.right, bbox.bottom]
}
