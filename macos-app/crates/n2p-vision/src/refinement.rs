//! 候補の不一致を画像で再確認する。資料固有の単語への置換は行わない。

use n2p_core::RgbImage;

use crate::TextCandidate;

fn compact(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

pub(crate) fn ambiguous_prefix(candidates: &[TextCandidate]) -> bool {
    let Some(best) = candidates.first() else {
        return false;
    };
    let text = compact(&best.text);
    if text.chars().count() < 8 || best.confidence < 0.9 {
        return false;
    }
    candidates.iter().skip(1).any(|candidate| {
        let other = compact(&candidate.text);
        candidate.confidence >= best.confidence - 0.05
            && other.ends_with(&text)
            && (1..=2).contains(&(other.chars().count().saturating_sub(text.chars().count())))
    })
}

pub(crate) fn restored_prefix(original: &str, retry: &str) -> bool {
    let original = compact(original);
    let retry = compact(retry);
    let Some(prefix) = retry.strip_suffix(&original) else {
        return false;
    };
    (1..=2).contains(&prefix.chars().count())
        && prefix
            .chars()
            .all(|c| matches!(c as u32, 0x3041..=0x30ff | 0x3400..=0x9fff))
}

/// 同じ単語に大文字と小文字の基準があり、各文字を画素の列で分離できる場合だけ判定する。
pub(crate) fn case_from_word_image(word: &str, index: usize, image: &RgbImage) -> Option<bool> {
    if !word.is_ascii() || image.width < 3 || image.height < 3 {
        return None;
    }
    let chars: Vec<char> = word.chars().collect();
    if !"cosuvwxz".contains(chars.get(index)?.to_ascii_lowercase()) {
        return None;
    }
    let mut corners = [
        image.pixel(0, 0),
        image.pixel(image.width - 1, 0),
        image.pixel(0, image.height - 1),
        image.pixel(image.width - 1, image.height - 1),
    ];
    let background: [u8; 3] = std::array::from_fn(|channel| {
        corners.sort_unstable_by_key(|p| p[channel]);
        corners[1][channel]
    });
    if corners.iter().any(|pixel| {
        pixel
            .iter()
            .zip(background)
            .any(|(a, b)| a.abs_diff(b) > 35)
    }) {
        return None;
    }
    let mut heights = Vec::new();
    let (mut top, mut bottom, mut width) = (image.height, 0, 0);
    for x in 0..=image.width {
        let ys: Vec<u32> = (0..image.height)
            .filter(|&y| {
                x < image.width
                    && image
                        .pixel(x, y)
                        .iter()
                        .zip(background)
                        .any(|(a, b)| a.abs_diff(b) > 140)
            })
            .collect();
        if ys.len() >= 2 {
            top = top.min(ys[0]);
            bottom = bottom.max(ys[ys.len() - 1] + 1);
            width += 1;
        } else if width > 0 {
            if width < 2 {
                return None;
            }
            heights.push((bottom - top) as f64);
            (top, bottom, width) = (image.height, 0, 0);
        }
    }
    if heights.len() != chars.len() {
        return None;
    }
    let samples = |uppercase: bool| -> Vec<f64> {
        chars
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                *i != index
                    && if uppercase {
                        c.is_ascii_uppercase()
                    } else {
                        "acemnorsuvwxz".contains(**c)
                    }
            })
            .map(|(i, _)| heights[i])
            .collect()
    };
    let median = |mut values: Vec<f64>| -> Option<f64> {
        values.sort_by(f64::total_cmp);
        let middle = *values.get(values.len() / 2)?;
        values
            .iter()
            .all(|v| (v / middle - 1.0).abs() < 0.15)
            .then_some(middle)
    };
    let upper = samples(true);
    let lower = samples(false);
    if upper.len() < 2 || lower.len() < 3 {
        return None;
    }
    let cap_height = median(upper)?;
    let x_height = median(lower)?;
    if cap_height / x_height < 1.15 {
        return None;
    }
    let cap_error = (heights[index] / cap_height - 1.0).abs();
    let x_error = (heights[index] / x_height - 1.0).abs();
    if cap_error < 0.1 && x_error > 0.15 {
        Some(true)
    } else if x_error < 0.1 && cap_error > 0.15 {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(text: &str, confidence: f32) -> TextCandidate {
        TextCandidate {
            text: text.into(),
            confidence,
        }
    }

    #[test]
    fn retries_a_confident_but_ambiguous_prefix() {
        assert!(ambiguous_prefix(&[
            candidate("ソース（CPU、メモリ）", 1.0),
            candidate("1ソース（CPU、メモリ）", 1.0),
        ]));
        assert!(!ambiguous_prefix(&[
            candidate("見出しと本文の文字列", 1.0),
            candidate("見出しと文の文字列", 1.0),
            candidate("1見出しと本文の文字列", 0.5),
        ]));
    }

    #[test]
    fn accepts_only_a_short_prefix_recovered_from_the_same_line() {
        assert!(restored_prefix(
            "ソース（CPU、メモリ）",
            "リソース （CPU、メモリ）"
        ));
        assert!(!restored_prefix("ソース（CPU、メモリ）", "別の説明文"));
        assert!(!restored_prefix(
            "ソース（CPU、メモリ）",
            "1ソース（CPU、メモリ）"
        ));
        assert!(!restored_prefix(
            "ソース（CPU、メモリ）",
            "リソース（GPU、メモリ）"
        ));
    }

    fn word_image(word: &str, lower_last: bool) -> RgbImage {
        let mut image = RgbImage::filled(word.len() as u32 * 8 + 2, 26, [15, 35, 55]);
        for (i, c) in word.chars().enumerate() {
            let height = if c == '_' {
                2
            } else if c.is_ascii_lowercase() || (lower_last && i == word.len() - 1) {
                12
            } else {
                18
            };
            let bottom = if c == '_' { 24 } else { 20 };
            for x in (i as u32 * 8 + 2)..(i as u32 * 8 + 7) {
                for y in bottom - height..bottom {
                    image.set_pixel(x, y, [245; 3]);
                }
            }
        }
        image
    }

    #[test]
    fn resolves_case_by_cap_height_and_x_height_without_a_word_dictionary() {
        let word = "Example_Data__C";
        assert_eq!(
            case_from_word_image(word, word.len() - 1, &word_image(word, true)),
            Some(false)
        );
        assert_eq!(
            case_from_word_image(word, word.len() - 1, &word_image(word, false)),
            Some(true)
        );
        assert_eq!(
            case_from_word_image("ABC", 2, &word_image("ABC", false)),
            None
        );
        assert_eq!(
            case_from_word_image(word, word.len() - 1, &RgbImage::filled(100, 30, [245; 3])),
            None
        );
    }

    #[test]
    fn faint_edges_between_underscores_and_letters_do_not_join_the_glyphs() {
        let word = "Example_Data__C";
        let mut image = word_image(word, true);
        for x in ((word.len() - 3) * 8) as u32..image.width {
            for y in 18..25 {
                if image.pixel(x, y)[0] < 100 {
                    image.set_pixel(x, y, [125; 3]);
                }
            }
        }
        assert_eq!(
            case_from_word_image(word, word.len() - 1, &image),
            Some(false)
        );
    }
}
