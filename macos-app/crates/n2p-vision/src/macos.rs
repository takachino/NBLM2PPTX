//! macOS Vision framework 的實際呼叫。
//!
//! 走 `VNImageRequestHandler initWithData:options:` —— 直接餵 PNG bytes，
//! 省掉 CGImage 的整條管線（也因此不需要 objc2-core-graphics）。

use n2p_core::{BBox, RgbImage};
use objc2::AllocAnyThread;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSError, NSRange, NSString};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedTextObservation, VNRequest,
    VNRequestTextRecognitionLevel,
};

use crate::{Error, Observation, Result, TextCandidate, TextRefinement, VisionConfig, refinement};

/// 讀取 macOS 系統偏好語言（`NSLocale.preferredLanguages`）。
pub fn preferred_languages() -> Vec<String> {
    let Some(cls) = AnyClass::get(c"NSLocale") else {
        return Vec::new();
    };
    let langs: Retained<NSArray<NSString>> = unsafe { msg_send![cls, preferredLanguages] };
    (0..langs.count())
        .map(|i| langs.objectAtIndex(i).to_string())
        .collect()
}

pub fn recognize_png(png: &[u8], config: &VisionConfig) -> Result<Vec<Observation>> {
    let image = RgbImage::from_png_bytes(png).map_err(|e| Error::Encode(e.to_string()))?;
    let mut observations = recognize_raw(png, config, Some(&image))?;
    // 再認識の費用を抑え、曖昧な先頭だけを確認する。
    for observation in observations
        .iter_mut()
        .filter(|o| config.accurate && refinement::ambiguous_prefix(&o.candidates))
        .take(8)
    {
        let (x, y, w, h) = observation.bbox_norm;
        if observation
            .corners_norm
            .is_some_and(|[tl, tr, _, _]| (tr.1 - tl.1).abs() > h * 0.1)
        {
            continue;
        }
        let pad = h * image.height as f64;
        let crop_box = BBox::new(
            (x * image.width as f64 - pad).floor() as i64,
            (y * image.height as f64 - pad * 0.25).floor() as i64,
            ((x + w) * image.width as f64 + pad).ceil() as i64,
            ((y + h) * image.height as f64 + pad * 0.25).ceil() as i64,
        )
        .clamp(image.width as i64, image.height as i64);
        let crop = image.crop(crop_box);
        if crop.width == 0 || crop.height == 0 {
            continue;
        }
        let mut enlarged = RgbImage::new(crop.width * 2, crop.height * 2);
        for cy in 0..enlarged.height {
            for cx in 0..enlarged.width {
                enlarged.set_pixel(cx, cy, crop.pixel(cx / 2, cy / 2));
            }
        }
        let Ok(bytes) = enlarged.to_png() else {
            continue;
        };
        let Ok(retries) = recognize_raw(&bytes, config, None) else {
            continue;
        };
        // 一つの行として再認識でき、残りの文字列が一致した場合に限る。
        if retries.len() != 1 {
            continue;
        }
        let retry = &retries[0];
        if retry.confidence < 0.95
            || retry.confidence < observation.confidence - 0.05
            || !refinement::restored_prefix(&observation.text, &retry.text)
        {
            continue;
        }
        let rx =
            (crop_box.left as f64 + retry.bbox_norm.0 * crop.width as f64) / image.width as f64;
        // 欠落文字を含む左端まで消去対象を広げる。他の行の幾何は維持する。
        observation.bbox_norm = (rx.min(x), y, (x + w) - rx.min(x), h);
        observation.corners_norm = None;
        observation.refinement = Some(TextRefinement {
            original_text: observation.text.clone(),
            reason: "cropped-prefix-recovery",
        });
        observation.text = retry.text.clone();
    }
    if config.accurate {
        refine_low_confidence(&image, &mut observations, config);
    }
    Ok(observations)
}

fn refine_low_confidence(
    image: &RgbImage,
    observations: &mut [Observation],
    config: &VisionConfig,
) {
    let mut targets: Vec<usize> = observations
        .iter()
        .enumerate()
        .filter(|(_, o)| {
            o.confidence >= config.min_confidence
                && o.confidence < 0.8
                && o.text.chars().count() >= 3
                && o.text.chars().any(char::is_alphabetic)
                && o.bbox_norm.3 >= 0.018
                && !o
                    .corners_norm
                    .is_some_and(|[tl, tr, _, _]| (tr.1 - tl.1).abs() > o.bbox_norm.3 * 0.1)
        })
        .map(|(i, _)| i)
        .collect();
    targets.sort_by(|a, b| {
        observations[*a]
            .confidence
            .total_cmp(&observations[*b].confidence)
            .then_with(|| {
                observations[*b]
                    .bbox_norm
                    .3
                    .total_cmp(&observations[*a].bbox_norm.3)
            })
    });
    for index in targets.into_iter().take(12) {
        let o = &mut observations[index];
        let (x, y, w, h) = o.bbox_norm;
        let pad = h * image.height as f64;
        let bounds = BBox::new(
            (x * image.width as f64 - pad * 0.35).floor() as i64,
            (y * image.height as f64 - pad * 0.1).floor() as i64,
            ((x + w) * image.width as f64 + pad * 0.35).floor() as i64,
            ((y + h) * image.height as f64 + pad * 0.1).floor() as i64,
        )
        .clamp(image.width as i64, image.height as i64);
        let crop = image.crop(bounds);
        if crop.width == 0 || crop.height == 0 {
            continue;
        }
        let mut retries = Vec::new();
        for factor in [2, 3] {
            let mut enlarged = RgbImage::new(crop.width * factor, crop.height * factor);
            for cy in 0..enlarged.height {
                for cx in 0..enlarged.width {
                    enlarged.set_pixel(cx, cy, crop.pixel(cx / factor, cy / factor));
                }
            }
            let result = enlarged
                .to_png()
                .ok()
                .and_then(|bytes| recognize_raw(&bytes, config, None).ok());
            let Some(rows) = result else {
                o.retry_outcome = Some("recognition-failed");
                break;
            };
            if rows.len() != 1 {
                o.retry_outcome = Some("line-split-or-neighbour");
                break;
            }
            let row = &rows[0];
            let expected_y =
                (y * image.height as f64 - bounds.top as f64 + pad / 2.0) / crop.height as f64;
            let actual_y = row.bbox_norm.1 + row.bbox_norm.3 / 2.0;
            if (expected_y - actual_y).abs() > pad / crop.height as f64 * 0.3 {
                o.retry_outcome = Some("different-line-position");
                break;
            }
            retries.push(TextCandidate {
                text: row.text.clone(),
                confidence: row.confidence,
            });
        }
        if retries.len() == 2 {
            if let Some(candidate) =
                refinement::retry_consensus(&o.text, o.confidence, &retries[0], &retries[1])
            {
                o.refinement = Some(TextRefinement {
                    original_text: o.text.clone(),
                    reason: "multiscale-line-consensus",
                });
                o.text = candidate.text;
                o.confidence = candidate.confidence;
                o.retry_outcome = Some("accepted-consensus");
            } else {
                o.retry_outcome = Some("uncertain-or-disagreeing");
            }
        }
        o.retry_candidates = retries;
    }
}

fn recognize_raw(
    png: &[u8],
    config: &VisionConfig,
    image: Option<&RgbImage>,
) -> Result<Vec<Observation>> {
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(if config.accurate {
        VNRequestTextRecognitionLevel::Accurate
    } else {
        VNRequestTextRecognitionLevel::Fast
    });
    request.setUsesLanguageCorrection(config.language_correction);
    request.setMinimumTextHeight(config.minimum_text_height);

    let languages: Vec<Retained<NSString>> = config
        .languages
        .iter()
        .map(|language| NSString::from_str(language))
        .collect();
    if !languages.is_empty() {
        request.setRecognitionLanguages(&NSArray::from_retained_slice(&languages));
    }

    let data = NSData::with_bytes(png);
    let options = NSDictionary::new();
    // SAFETY: `initWithData:options:` 只讀取我們持有的 data 與 options，且兩者存活到呼叫結束。
    let handler = VNImageRequestHandler::initWithData_options(
        VNImageRequestHandler::alloc(),
        &data,
        &options,
    );

    let requests: Retained<NSArray<VNRequest>> = {
        let upstream: &VNRequest = &request;
        NSArray::from_slice(&[upstream])
    };

    handler
        .performRequests_error(&requests)
        .map_err(|error: Retained<NSError>| Error::Vision(error.to_string()))?;

    let results = request
        .results()
        .ok_or_else(|| Error::Vision("Vision 沒有回傳結果".into()))?;

    let mut observations = Vec::with_capacity(results.len());
    for index in 0..results.len() {
        let observation: Retained<VNRecognizedTextObservation> = results.objectAtIndex(index);
        let candidates = observation.topCandidates(10);
        if candidates.is_empty() {
            continue;
        }
        let best = candidates.objectAtIndex(0);
        let mut text = best.string().to_string();
        let confidence = best.confidence();
        let rect = unsafe { observation.boundingBox() };
        let tl = unsafe { observation.topLeft() };
        let tr = unsafe { observation.topRight() };
        let br = unsafe { observation.bottomRight() };
        let bl = unsafe { observation.bottomLeft() };

        // Vision 的原點在左下、y 向上；轉成左上、y 向下。
        let x = rect.origin.x;
        let width = rect.size.width;
        let height = rect.size.height;
        let y_top = 1.0 - (rect.origin.y + height);

        let candidates: Vec<TextCandidate> = (0..candidates.len())
            .map(|i| {
                let candidate = candidates.objectAtIndex(i);
                TextCandidate {
                    text: candidate.string().to_string(),
                    confidence: candidate.confidence(),
                }
            })
            .collect();
        let mut refinement = None;
        if let Some(image) = image {
            for candidate in &candidates {
                if candidate.confidence < confidence - 0.05
                    || !text.eq_ignore_ascii_case(&candidate.text)
                {
                    continue;
                }
                let differences: Vec<usize> = text
                    .bytes()
                    .zip(candidate.text.bytes())
                    .enumerate()
                    .filter_map(|(i, (a, b))| (a != b).then_some(i))
                    .collect();
                if differences.len() != 1 {
                    continue;
                }
                let index = differences[0];
                let bytes = text.as_bytes();
                let mut start = index;
                let mut end = index + 1;
                while start > 0
                    && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_')
                {
                    start -= 1;
                }
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                {
                    end += 1;
                }
                let range = NSRange::new(
                    text[..start].encode_utf16().count(),
                    text[start..end].encode_utf16().count(),
                );
                // Vision の単語領域は概算。文字ごとの列を分離できない場合、画素の判定は棄却する。
                let Ok(word_box) = (unsafe { best.boundingBoxForRange_error(range) }) else {
                    continue;
                };
                let r = unsafe { word_box.boundingBox() };
                let bbox = BBox::new(
                    (r.origin.x * image.width as f64).floor() as i64,
                    ((1.0 - r.origin.y - r.size.height) * image.height as f64).floor() as i64,
                    ((r.origin.x + r.size.width) * image.width as f64).ceil() as i64,
                    ((1.0 - r.origin.y) * image.height as f64).ceil() as i64,
                );
                let crop = image.crop(bbox);
                let Some(uppercase) =
                    refinement::case_from_word_image(&text[start..end], index - start, &crop)
                else {
                    continue;
                };
                if uppercase != candidate.text.as_bytes()[index].is_ascii_uppercase() {
                    continue;
                }
                refinement = Some(TextRefinement {
                    original_text: text.clone(),
                    reason: "candidate-case-ink-height",
                });
                text = candidate.text.clone();
                break;
            }
        }
        observations.push(Observation {
            text,
            confidence,
            bbox_norm: (x, y_top, width, height),
            corners_norm: Some([
                (tl.x, 1.0 - tl.y),
                (tr.x, 1.0 - tr.y),
                (br.x, 1.0 - br.y),
                (bl.x, 1.0 - bl.y),
            ]),
            candidates,
            refinement,
            retry_candidates: Vec::new(),
            retry_outcome: None,
        });
    }

    Ok(observations)
}
