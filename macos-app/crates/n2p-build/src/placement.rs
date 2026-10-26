//! 文字の観測位置を保ち、容器と隣接図形で出力枠の拡張を制限する。

use n2p_core::{BBox, RgbImage};
use n2p_layout::LayoutPage;
use n2p_ooxml::{Align, AutoFit, Rect, TextBox};

use crate::diagnostics::{PlacementBoundaryDiagnostic, PlacementDiagnostic, bbox_array};

struct Boundary {
    bbox: BBox,
    kind: &'static str,
    confidence: u16,
}

pub(crate) struct PlacementMap {
    boundaries: Vec<Boundary>,
    scale: f64,
    page: Rect,
}

impl PlacementMap {
    pub(crate) fn new(
        layout: &LayoutPage,
        image: &RgbImage,
        protected: &[BBox],
        scale: f64,
        width: f64,
        height: f64,
    ) -> Self {
        let image_scale = image.width as f64 / layout.size_pt.0;
        let excluded: Vec<_> = layout
            .text_blocks()
            .map(|b| b.bbox.scale(image_scale))
            .collect();
        let mut boundaries: Vec<_> =
            n2p_image::rectangles::find_rectangular_regions(image, &excluded)
                .into_iter()
                .map(|r| Boundary {
                    bbox: r.bbox.scale(1.0 / image_scale),
                    kind: "framed_region",
                    confidence: r.confidence_milli,
                })
                .collect();
        boundaries.extend(layout.foreground_blocks().map(|b| Boundary {
            bbox: b.bbox,
            kind: "layout_graphic",
            confidence: 1000,
        }));
        boundaries.extend(protected.iter().map(|&bbox| Boundary {
            bbox,
            kind: "protected_graphic",
            confidence: 1000,
        }));
        Self {
            boundaries,
            scale,
            page: Rect::ltrb(0.0, 0.0, width, height),
        }
    }

    fn diagnostic(&self, boundary: &Boundary) -> PlacementBoundaryDiagnostic {
        let b = boundary.bbox;
        PlacementBoundaryDiagnostic {
            kind: boundary.kind,
            confidence_milli: boundary.confidence,
            bbox_pt: bbox_array(b),
            slide_rect_pt: [
                b.left as f64 * self.scale,
                b.top as f64 * self.scale,
                b.right as f64 * self.scale,
                b.bottom as f64 * self.scale,
            ],
        }
    }

    pub(crate) fn diagnostics(&self) -> Vec<PlacementBoundaryDiagnostic> {
        self.boundaries.iter().map(|b| self.diagnostic(b)).collect()
    }

    /// 元の観測が境界を越える場合は推測で切らない。回転文字も対象外。
    pub(crate) fn apply(
        &self,
        source: BBox,
        rotation_mdeg: i32,
        text: &mut TextBox,
    ) -> Option<PlacementDiagnostic> {
        if source.is_empty() || rotation_mdeg.abs() >= 3_000 {
            return None;
        }
        let original = rect_array(text.rect);
        let original_size = text.font_size_pt;
        let mut bounds = self.page;
        let mut evidence = Vec::new();
        let margin = 2.0 / self.scale;
        if let Some(container) = self
            .boundaries
            .iter()
            .filter(|b| b.kind == "framed_region" && contains(b.bbox, source))
            .min_by_key(|b| b.bbox.width() * b.bbox.height())
        {
            let b = container.bbox;
            bounds = Rect::ltrb(
                (b.left as f64 + margin.min((source.left - b.left) as f64 / 2.0)) * self.scale,
                (b.top as f64 + margin.min((source.top - b.top) as f64 / 2.0)) * self.scale,
                (b.right as f64 - margin.min((b.right - source.right) as f64 / 2.0)) * self.scale,
                (b.bottom as f64 - margin.min((b.bottom - source.bottom) as f64 / 2.0))
                    * self.scale,
            );
            evidence.push(self.diagnostic(container));
        }
        for boundary in &self.boundaries {
            let b = boundary.bbox;
            let overlap_x = (b.right.min(source.right) - b.left.max(source.left)).max(0);
            let overlap_y = (b.bottom.min(source.bottom) - b.top.max(source.top)).max(0);
            // 含む枠と、元の文字に重なる図形は障害物として扱わない。
            if overlap_x > 0 && overlap_y > 0 {
                continue;
            }
            let before = rect_array(bounds);
            if overlap_y * 2 >= source.height() {
                if b.left >= source.right {
                    bounds.right_pt = bounds.right_pt.min(
                        (b.left as f64 - margin.min((b.left - source.right) as f64 / 2.0))
                            * self.scale,
                    );
                } else if b.right <= source.left {
                    bounds.left_pt = bounds.left_pt.max(
                        (b.right as f64 + margin.min((source.left - b.right) as f64 / 2.0))
                            * self.scale,
                    );
                }
            }
            if overlap_x * 2 >= source.width() {
                if b.top >= source.bottom {
                    bounds.bottom_pt = bounds.bottom_pt.min(
                        (b.top as f64 - margin.min((b.top - source.bottom) as f64 / 2.0))
                            * self.scale,
                    );
                } else if b.bottom <= source.top {
                    bounds.top_pt = bounds.top_pt.max(
                        (b.bottom as f64 + margin.min((source.top - b.bottom) as f64 / 2.0))
                            * self.scale,
                    );
                }
            }
            if before != rect_array(bounds) {
                evidence.push(self.diagnostic(boundary));
            }
        }
        if evidence.is_empty() {
            return None;
        }
        bounds.left_pt = bounds.left_pt.max(self.page.left_pt);
        bounds.top_pt = bounds.top_pt.max(self.page.top_pt);
        bounds.right_pt = bounds.right_pt.min(self.page.right_pt);
        bounds.bottom_pt = bounds.bottom_pt.min(self.page.bottom_pt);
        let old = text.rect;
        let mut rect = Rect::ltrb(
            old.left_pt.max(bounds.left_pt),
            old.top_pt.max(bounds.top_pt),
            old.right_pt.min(bounds.right_pt),
            old.bottom_pt.min(bounds.bottom_pt),
        );
        // 中央揃えの文字を、片側だけ狭めた枠の新しい中心へ移動させない。
        if text.align == Align::Center {
            let cx = (old.left_pt + old.right_pt) / 2.0;
            let half = (old.width_pt() / 2.0)
                .min(cx - bounds.left_pt)
                .min(bounds.right_pt - cx);
            rect.left_pt = cx - half;
            rect.right_pt = cx + half;
        }
        let cy = (old.top_pt + old.bottom_pt) / 2.0;
        let half = (old.height_pt() / 2.0)
            .min(cy - bounds.top_pt)
            .min(bounds.bottom_pt - cy);
        rect.top_pt = cy - half;
        rect.bottom_pt = cy + half;
        if rect.width_pt() <= 0.0 || rect.height_pt() <= 0.0 {
            return None;
        }
        // 制約に触れていない文字のサイズは変えない。
        if rect_array(rect)
            .into_iter()
            .zip(original)
            .any(|(a, b)| (a - b).abs() > 0.01)
        {
            let units = text
                .text
                .lines()
                .map(crate::fontsize::full_width_units)
                .fold(0.0_f64, f64::max);
            let lines = text.text.lines().count().max(1) as f64;
            if units > 0.0 {
                text.font_size_pt = text.font_size_pt.min(rect.width_pt() * 0.9 / units);
            }
            // OCR 枠の見積もりに足した余裕を、実描画の行高へ重ねて足さない。
            let pitch = if text.zero_insets {
                lines * crate::LINE_PITCH_RATIO
            } else {
                lines * crate::LINE_SPACING_RATIO + crate::TEXT_SLACK_LINES
            };
            text.font_size_pt = text.font_size_pt.min(rect.height_pt() / pitch);
            text.rect = rect;
            text.autofit = AutoFit::Normal;
        }
        Some(PlacementDiagnostic {
            source_bbox_pt: bbox_array(source),
            boundaries: evidence,
            original_rect_pt: original,
            original_font_size_pt: original_size,
            allowed_rect_pt: rect_array(bounds),
        })
    }
}

fn rect_array(rect: Rect) -> [f64; 4] {
    [rect.left_pt, rect.top_pt, rect.right_pt, rect.bottom_pt]
}

fn contains(outer: BBox, inner: BBox) -> bool {
    outer.left <= inner.left
        && outer.top <= inner.top
        && outer.right >= inner.right
        && outer.bottom >= inner.bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(boxes: &[(BBox, &'static str)]) -> PlacementMap {
        PlacementMap {
            boundaries: boxes
                .iter()
                .map(|&(bbox, kind)| Boundary {
                    bbox,
                    kind,
                    confidence: 1000,
                })
                .collect(),
            scale: 1.0,
            page: Rect::ltrb(0.0, 0.0, 960.0, 540.0),
        }
    }

    #[test]
    fn uncertain_membership_and_rotated_text_keep_their_original_geometry() {
        let placement = map(&[(BBox::new(100, 50, 200, 150), "framed_region")]);
        for (source, rotation) in [
            (BBox::new(90, 80, 160, 100), 0),
            (BBox::new(120, 80, 180, 100), 12000),
        ] {
            let rect = Rect::ltrb(80.0, 75.0, 220.0, 120.0);
            let mut text = TextBox::new("body", "ordinary text", rect).font("Arial", 20.0);
            assert!(placement.apply(source, rotation, &mut text).is_none());
            assert_eq!(rect_array(text.rect), rect_array(rect));
            assert_eq!(text.font_size_pt, 20.0);
        }
    }

    #[test]
    fn closest_container_preserves_the_original_text_center() {
        let placement = map(&[
            (BBox::new(20, 20, 400, 300), "framed_region"),
            (BBox::new(80, 60, 220, 120), "framed_region"),
        ]);
        let mut text = TextBox::new(
            "body",
            "An explanation",
            Rect::ltrb(60.0, 50.0, 240.0, 130.0),
        )
        .font("Arial", 24.0)
        .align(Align::Center);
        let diagnostic = placement
            .apply(BBox::new(100, 75, 200, 105), 0, &mut text)
            .unwrap();
        assert_eq!(diagnostic.boundaries[0].bbox_pt, [80, 60, 220, 120]);
        assert!((text.rect.left_pt + text.rect.right_pt - 300.0).abs() < 0.01);
        assert!((text.rect.top_pt + text.rect.bottom_pt - 180.0).abs() < 0.01);
        assert!(text.rect.left_pt >= 82.0 && text.rect.right_pt <= 218.0);
        assert!(text.rect.top_pt >= 62.0 && text.rect.bottom_pt <= 118.0);
        assert!(text.font_size_pt < 24.0);
    }

    #[test]
    fn known_adjacent_graphics_limit_both_axes_without_clipping_the_observation() {
        let source = BBox::new(100, 100, 200, 120);
        for kind in ["layout_graphic", "protected_graphic"] {
            let placement = map(&[
                (BBox::new(205, 80, 260, 150), kind),
                (BBox::new(80, 124, 220, 200), kind),
            ]);
            let mut text = TextBox::new(
                "body",
                "An explanation",
                Rect::ltrb(85.0, 90.0, 230.0, 140.0),
            )
            .font("Arial", 24.0);
            let diagnostic = placement.apply(source, 0, &mut text).unwrap();
            assert_eq!(diagnostic.boundaries.len(), 2);
            assert!(text.rect.right_pt <= 203.0 && text.rect.bottom_pt <= 122.0);
            assert!(
                text.rect.right_pt >= source.right as f64
                    && text.rect.bottom_pt >= source.bottom as f64
            );
            assert_eq!(text.text, "An explanation");
        }
    }

    #[test]
    fn distant_boundaries_do_not_shrink_unconstrained_body_text() {
        let placement = map(&[(BBox::new(600, 80, 700, 200), "framed_region")]);
        let rect = Rect::ltrb(80.0, 80.0, 250.0, 140.0);
        let mut text = TextBox::new("body", "ordinary text", rect).font("Arial", 20.0);
        placement.apply(BBox::new(100, 100, 200, 120), 0, &mut text);
        assert_eq!(rect_array(text.rect), rect_array(rect));
        assert_eq!(text.font_size_pt, 20.0);
    }

    #[test]
    fn roundoff_does_not_shrink_a_title_above_a_distant_frame() {
        let placement = map(&[(BBox::new(70, 268, 454, 292), "framed_region")]);
        let rect = Rect::ltrb(
            20.264767441860556,
            19.873837209302323,
            777.8747674418605,
            74.82383720930233,
        );
        let mut text = TextBox::new(
            "title",
            "進化するプラットフォーム：CRMからマルチクラウドへ",
            rect,
        )
        .font("Arial", 29.0)
        .align(Align::Center);
        placement.apply(BBox::new(40, 31, 758, 60), 0, &mut text);
        assert_eq!(text.font_size_pt, 29.0);
        assert_eq!(rect_array(text.rect), rect_array(rect));
    }

    #[test]
    fn height_fitting_uses_text_pitch_without_extra_observation_slack() {
        let placement = map(&[(BBox::new(0, 0, 200, 100), "framed_region")]);
        let mut text = TextBox::new("body", "A short line", Rect::ltrb(10.0, 70.0, 180.0, 110.0))
            .font("Arial", 16.0);
        placement.apply(BBox::new(20, 76, 150, 99), 0, &mut text);
        assert!(text.font_size_pt >= 15.0, "{}", text.font_size_pt);
        assert!(text.font_size_pt * crate::LINE_PITCH_RATIO <= text.rect.height_pt() + 0.01);
    }
}
