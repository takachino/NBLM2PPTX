use n2p_build::{BackgroundResolution, BuildOptions, PageInput, build_deck};
use n2p_core::{BBox, RgbImage};
use n2p_layout::{Block, BlockKind, LayoutPage, Line, Span};

fn block(text: &str, bbox: BBox) -> Block {
    let mut block = Block::new(BlockKind::Text, bbox);
    block.text = text.to_owned();
    block.lines = vec![Line {
        spans: vec![Span {
            text: Some(text.to_owned()),
            image_path: None,
        }],
        bbox: Some(bbox),
        confidence_milli: Some(1000),
        observation_bbox: Some(bbox),
    }];
    block
}

fn line(image: &mut RgbImage, from: (i64, i64), to: (i64, i64), radius: i64, color: [u8; 3]) {
    let steps = (to.0 - from.0).abs().max((to.1 - from.1).abs()).max(1);
    for step in 0..=steps {
        let x = from.0 + (to.0 - from.0) * step / steps;
        let y = from.1 + (to.1 - from.1) * step / steps;
        for py in y - radius..=y + radius {
            for px in x - radius..=x + radius {
                if px >= 0 && py >= 0 && px < image.width as i64 && py < image.height as i64 {
                    image.set_pixel(px as u32, py as u32, color);
                }
            }
        }
    }
}

fn ellipse(image: &mut RgbImage, bbox: BBox, color: [u8; 3]) {
    let cx = (bbox.left + bbox.right) as f64 * 0.5;
    let cy = (bbox.top + bbox.bottom) as f64 * 0.5;
    let rx = bbox.width() as f64 * 0.5;
    let ry = bbox.height() as f64 * 0.5;
    for y in bbox.top..bbox.bottom {
        for x in bbox.left..bbox.right {
            let distance = ((x as f64 - cx) / rx).powi(2) + ((y as f64 - cy) / ry).powi(2);
            if (0.55..=1.05).contains(&distance) {
                image.set_pixel(x as u32, y as u32, color);
            }
        }
    }
}

fn strokes(image: &mut RgbImage, bbox: BBox, color: [u8; 3]) {
    for y in bbox.top..bbox.bottom {
        for x in bbox.left..bbox.right {
            if (x - bbox.left) % 7 < 2 {
                image.set_pixel(x as u32, y as u32, color);
            }
        }
    }
}

fn convert(image: &RgbImage, blocks: Vec<Block>) -> (RgbImage, Vec<String>) {
    convert_with_options(
        image,
        blocks,
        BuildOptions {
            background_resolution: BackgroundResolution::FullImage,
            ..Default::default()
        },
    )
}

fn convert_with_options(
    image: &RgbImage,
    blocks: Vec<Block>,
    options: BuildOptions,
) -> (RgbImage, Vec<String>) {
    let layout = LayoutPage {
        size_pt: (960.0, 540.0),
        blocks,
    };
    let (deck, _) = build_deck(
        &[PageInput {
            layout: &layout,
            image,
        }],
        &options,
    )
    .unwrap();
    let media = deck
        .package()
        .part_names()
        .find(|p| p.starts_with("ppt/media/"))
        .unwrap();
    let background = RgbImage::from_png_bytes(deck.package().part(media).unwrap()).unwrap();
    let texts = n2p_ooxml::slide::text_shapes(&deck.slide(0).unwrap())
        .unwrap()
        .into_iter()
        .map(|shape| shape.text)
        .collect();
    (background, texts)
}

fn assert_region_unchanged(actual: &RgbImage, source: &RgbImage, bbox: BBox) {
    for y in bbox.top..bbox.bottom {
        for x in bbox.left..bbox.right {
            assert_eq!(
                actual.pixel(x as u32, y as u32),
                source.pixel(x as u32, y as u32),
                "({x}, {y})"
            );
        }
    }
}

#[test]
fn people_icon_misread_as_zeroes_stays_in_the_image() {
    let mut image = RgbImage::filled(960, 540, [245, 248, 250]);
    let blue = [30, 135, 160];
    for left in [105, 128] {
        ellipse(&mut image, BBox::new(left, 100, left + 16, 116), blue);
        line(&mut image, (left - 4, 132), (left - 4, 125), 1, blue);
        line(&mut image, (left - 4, 125), (left + 4, 120), 1, blue);
        line(&mut image, (left + 4, 120), (left + 12, 120), 1, blue);
        line(&mut image, (left + 12, 120), (left + 20, 125), 1, blue);
        line(&mut image, (left + 20, 125), (left + 20, 132), 1, blue);
        line(&mut image, (left - 4, 132), (left + 20, 132), 1, blue);
    }
    let icon = BBox::new(100, 96, 152, 138);
    let body = BBox::new(200, 104, 330, 130);
    strokes(&mut image, body, [10; 3]);
    let (background, texts) = convert(&image, vec![block("000", icon), block("Contact", body)]);
    assert_eq!(texts, ["Contact"]);
    assert_region_unchanged(&background, &image, icon);
}

#[test]
fn green_check_misread_as_kana_is_preserved_beside_editable_body() {
    let mut image = RgbImage::filled(960, 540, [250; 3]);
    line(&mut image, (105, 115), (118, 128), 4, [10, 80, 35]);
    line(&mut image, (118, 128), (146, 97), 4, [10, 80, 35]);
    line(&mut image, (105, 115), (118, 128), 2, [20, 180, 80]);
    line(&mut image, (118, 128), (146, 97), 2, [20, 180, 80]);
    let icon = BBox::new(98, 90, 154, 137);
    let body = BBox::new(180, 105, 270, 131);
    strokes(&mut image, body, [15; 3]);
    let (background, texts) = convert(&image, vec![block("く", icon), block("変更可能", body)]);
    assert_eq!(texts, ["変更可能"]);
    assert_region_unchanged(&background, &image, icon);
}

#[test]
fn a_clock_joined_to_a_heading_does_not_become_a_leading_zero() {
    let mut image = RgbImage::filled(960, 540, [18, 40, 80]);
    let blue = [80, 170, 210];
    ellipse(&mut image, BBox::new(100, 100, 130, 132), blue);
    line(&mut image, (115, 105), (115, 117), 1, blue);
    line(&mut image, (115, 117), (121, 120), 1, blue);
    line(&mut image, (111, 96), (119, 96), 1, blue);
    let body = BBox::new(150, 101, 370, 129);
    strokes(&mut image, body, [250; 3]);
    let (background, texts) = convert(
        &image,
        vec![block("0パフォーマンスの担保", BBox::new(96, 94, 375, 135))],
    );
    assert_eq!(texts, ["パフォーマンスの担保"]);
    assert_region_unchanged(&background, &image, BBox::new(95, 93, 135, 136));
}

#[test]
fn real_zeroes_short_english_labels_and_kana_remain_editable() {
    let mut image = RgbImage::filled(960, 540, [250; 3]);
    let number = BBox::new(100, 90, 172, 124);
    for x in [100, 125, 150] {
        ellipse(&mut image, BBox::new(x, 90, x + 20, 124), [20; 3]);
    }
    let mut blocks = vec![block("000", number)];
    for (i, text) in ["42", "OK", "API", "く"].into_iter().enumerate() {
        let bbox = BBox::new(260 + i as i64 * 100, 90, 310 + i as i64 * 100, 124);
        strokes(&mut image, bbox, [20; 3]);
        blocks.push(block(text, bbox));
    }
    let (_, texts) = convert(&image, blocks);
    assert_eq!(texts, ["000", "42", "OK", "API", "く"]);
}

#[test]
fn small_screen_text_is_preserved_without_losing_the_surrounding_body() {
    let mut image = RgbImage::filled(960, 540, [250; 3]);
    let screen = BBox::new(100, 160, 490, 365);
    for y in screen.top..screen.bottom {
        for x in screen.left..screen.right {
            image.set_pixel(x as u32, y as u32, [232, 240, 248]);
        }
    }
    let body = BBox::new(100, 70, 360, 105);
    strokes(&mut image, body, [10; 3]);
    let mut blocks = vec![block("画面の操作を説明する本文", body)];
    for (text, bbox) in [
        ("Contacts", BBox::new(120, 190, 142, 196)),
        ("Search", BBox::new(155, 190, 173, 196)),
        ("Records", BBox::new(190, 190, 210, 196)),
        ("Settings", BBox::new(120, 215, 142, 221)),
        ("Open", BBox::new(155, 215, 169, 221)),
    ] {
        strokes(&mut image, bbox, [10; 3]);
        blocks.push(block(text, bbox));
    }
    let (background, texts) = convert(&image, blocks);
    assert_eq!(texts, ["画面の操作を説明する本文"]);
    assert_region_unchanged(&background, &image, screen);
}

#[test]
fn inline_check_keeps_its_pixels_and_the_surrounding_text_is_editable() {
    let mut image = RgbImage::filled(960, 540, [250; 3]);
    strokes(&mut image, BBox::new(100, 100, 172, 132), [15; 3]);
    line(&mut image, (182, 117), (191, 126), 3, [10, 80, 35]);
    line(&mut image, (191, 126), (208, 103), 3, [10, 80, 35]);
    line(&mut image, (182, 117), (191, 126), 1, [20, 180, 80]);
    line(&mut image, (191, 126), (208, 103), 1, [20, 180, 80]);
    line(&mut image, (226, 100), (226, 132), 1, [15; 3]);
    let (background, texts) = convert(
        &image,
        vec![block("可能（<）", BBox::new(100, 98, 232, 135))],
    );
    assert_eq!(texts.concat(), "可能（）");
    assert_region_unchanged(&background, &image, BBox::new(178, 99, 213, 131));
}

#[test]
fn a_real_comparison_operator_and_low_confidence_label_are_not_graphics() {
    let mut image = RgbImage::filled(960, 540, [250; 3]);
    line(&mut image, (145, 95), (115, 110), 2, [20, 160, 60]);
    line(&mut image, (115, 110), (145, 125), 2, [20, 160, 60]);
    strokes(&mut image, BBox::new(160, 100, 200, 130), [20; 3]);
    let mut label = block("42", BBox::new(160, 100, 200, 130));
    label.lines[0].confidence_milli = Some(300);
    let (background, texts) = convert(
        &image,
        vec![block("<", BBox::new(110, 90, 150, 130)), label],
    );
    assert_eq!(texts, ["<", "42"]);
    assert_ne!(
        background.pixel(130, 102),
        image.pixel(130, 102),
        "正しい比較演算子は背景から消して文字として配置する"
    );
}

#[test]
fn a_mixed_block_retains_body_lines_and_an_isolated_fine_print_note() {
    let mut image = RgbImage::filled(960, 540, [250; 3]);
    for y in 160..250 {
        for x in 100..240 {
            image.set_pixel(x, y, [232, 240, 248]);
        }
    }
    let menu = BBox::new(120, 180, 144, 186);
    let body = BBox::new(120, 300, 400, 335);
    strokes(&mut image, menu, [20; 3]);
    strokes(&mut image, body, [20; 3]);
    let mut mixed = block("Tiny menu", menu);
    mixed
        .lines
        .extend(block("通常の本文は編集可能", body).lines);
    mixed.text = "Tiny menu\n通常の本文は編集可能".into();
    mixed.bbox = BBox::new(120, 180, 400, 335);
    let mut blocks = vec![mixed];
    for (text, bbox) in [
        ("Search", BBox::new(160, 180, 178, 186)),
        ("Contacts", BBox::new(120, 205, 143, 211)),
        ("Footnote", BBox::new(720, 480, 744, 486)),
    ] {
        strokes(&mut image, bbox, [20; 3]);
        blocks.push(block(text, bbox));
    }
    let (background, texts) = convert(&image, blocks);
    assert_eq!(texts, ["通常の本文は編集可能", "Footnote"]);
    assert_region_unchanged(&background, &image, menu);
}

#[test]
fn small_ui_ink_is_preserved_even_if_ocr_overestimates_the_font_size() {
    let mut image = RgbImage::filled(960, 540, [250; 3]);
    for y in 150..300 {
        for x in 100..400 {
            image.set_pixel(x, y, [232, 240, 248]);
        }
    }
    let mut blocks = Vec::new();
    for (text, bbox) in [
        ("Name", BBox::new(120, 180, 155, 190)),
        ("Email", BBox::new(170, 180, 205, 190)),
        ("City", BBox::new(120, 210, 148, 220)),
    ] {
        strokes(&mut image, bbox, [20; 3]);
        blocks.push(block(text, bbox));
    }
    let (background, texts) = convert(&image, blocks);
    assert!(texts.is_empty(), "UI の文字が再配置された: {texts:?}");
    assert_region_unchanged(&background, &image, BBox::new(100, 150, 400, 300));
}

#[test]
fn a_plain_list_of_small_numbers_is_not_a_screen_capture() {
    let mut image = RgbImage::filled(960, 540, [250; 3]);
    let mut blocks = Vec::new();
    for (i, text) in ["17", "23", "42"].into_iter().enumerate() {
        let bbox = BBox::new(120, 180 + 20 * i as i64, 128, 186 + 20 * i as i64);
        strokes(&mut image, bbox, [20; 3]);
        blocks.push(block(text, bbox));
    }
    let (_, texts) = convert(&image, blocks);
    assert_eq!(texts, ["17", "23", "42"]);
}

#[test]
fn a_blue_arrow_misread_as_comparison_is_not_erased_by_a_neighbouring_block() {
    let mut image = RgbImage::filled(960, 540, [250; 3]);
    for y in 100..135 {
        for x in 100..160 {
            if (110..125).contains(&y) && x < 140 || x >= 135 && (y - 117i64).abs() < (160 - x) {
                image.set_pixel(x as u32, y as u32, [30, 130, 190]);
            }
        }
    }
    strokes(&mut image, BBox::new(175, 104, 280, 132), [15; 3]);
    for mode in [
        n2p_build::EraseMode::StrokeMask,
        n2p_build::EraseMode::Flat,
        n2p_build::EraseMode::Inpaint,
    ] {
        let (background, texts) = convert_with_options(
            &image,
            vec![
                block(">", BBox::new(98, 98, 162, 137)),
                block("Editable body", BBox::new(145, 100, 285, 135)),
            ],
            BuildOptions {
                erase_mode: mode,
                background_resolution: BackgroundResolution::FullImage,
                ..Default::default()
            },
        );
        assert_eq!(texts, ["Editable body"]);
        assert_region_unchanged(&background, &image, BBox::new(98, 98, 163, 138));
    }
}

#[test]
fn a_standalone_clock_is_preserved_but_a_colored_zero_is_editable() {
    let mut image = RgbImage::filled(960, 540, [250; 3]);
    let blue = [30, 130, 190];
    ellipse(&mut image, BBox::new(100, 100, 132, 134), blue);
    line(&mut image, (116, 105), (116, 118), 1, blue);
    line(&mut image, (116, 118), (123, 122), 1, blue);
    ellipse(&mut image, BBox::new(210, 100, 230, 134), blue);
    let (background, texts) = convert(
        &image,
        vec![
            block("0", BBox::new(98, 98, 135, 137)),
            block("0", BBox::new(208, 98, 233, 137)),
        ],
    );
    assert_eq!(texts, ["0"]);
    assert_region_unchanged(&background, &image, BBox::new(98, 98, 135, 137));
}

#[test]
fn an_unrecognized_check_is_protected_from_an_overlapping_text_observation() {
    let mut image = RgbImage::filled(960, 540, [250; 3]);
    line(&mut image, (105, 115), (118, 128), 4, [10, 80, 35]);
    line(&mut image, (118, 128), (146, 97), 4, [10, 80, 35]);
    line(&mut image, (105, 115), (118, 128), 2, [20, 180, 80]);
    line(&mut image, (118, 128), (146, 97), 2, [20, 180, 80]);
    strokes(&mut image, BBox::new(175, 100, 300, 130), [15; 3]);
    let (background, texts) = convert(
        &image,
        vec![block("変更可能", BBox::new(140, 98, 310, 134))],
    );
    assert_eq!(texts, ["変更可能"]);
    assert_region_unchanged(&background, &image, BBox::new(98, 90, 154, 137));
}

#[test]
fn a_colored_glyph_at_the_end_of_a_heading_is_not_an_adjacent_icon() {
    let mut image = RgbImage::filled(960, 540, [250; 3]);
    let blue = [15, 40, 80];
    strokes(&mut image, BBox::new(100, 100, 290, 134), blue);
    for (from, to) in [
        ((316, 103), (333, 103)),
        ((333, 103), (333, 120)),
        ((333, 120), (316, 120)),
        ((316, 120), (316, 103)),
        ((316, 112), (333, 112)),
    ] {
        line(&mut image, from, to, 0, blue);
    }
    let (background, texts) = convert(
        &image,
        vec![block("説明文の見出し", BBox::new(100, 98, 335, 137))],
    );
    assert_eq!(texts, ["説明文の見出し"]);
    assert_eq!(background.pixel(325, 112), [250; 3]);
}

#[test]
fn tilted_screen_labels_use_the_ink_height_instead_of_the_enclosing_box() {
    let mut image = RgbImage::filled(960, 540, [250; 3]);
    for y in 140..350 {
        for x in 90..460 {
            image.set_pixel(x, y, [220, 235, 245]);
        }
    }
    let mut blocks = Vec::new();
    for (text, bbox) in [
        ("File", BBox::new(110, 170, 136, 180)),
        ("Home", BBox::new(145, 170, 178, 180)),
        ("Data", BBox::new(190, 170, 216, 180)),
        ("City", BBox::new(250, 240, 275, 250)),
    ] {
        strokes(&mut image, bbox, [20; 3]);
        blocks.push(block(text, bbox));
    }
    let mut tilted = block("example@company.test.com", BBox::new(240, 200, 420, 270));
    tilted.rotation_mdeg = -16_000;
    tilted.lines[0].bbox = Some(BBox::new(240, 230, 420, 240));
    line(&mut image, (245, 260), (415, 208), 2, [20; 3]);
    blocks.push(tilted);
    let body = BBox::new(500, 210, 720, 240);
    strokes(&mut image, body, [20; 3]);
    blocks.push(block("通常の説明文", body));
    let (background, texts) = convert(&image, blocks);
    assert_eq!(texts, ["通常の説明文"]);
    assert_region_unchanged(&background, &image, BBox::new(90, 140, 460, 350));
}

#[test]
fn an_isolated_tiny_label_inside_a_screen_is_preserved() {
    let mut image = RgbImage::filled(960, 540, [250; 3]);
    for y in 150..300 {
        for x in 100..400 {
            image.set_pixel(x, y, [225, 238, 246]);
        }
    }
    let tiny = BBox::new(120, 180, 130, 185);
    strokes(&mut image, tiny, [20; 3]);
    let footnote = BBox::new(700, 420, 725, 426);
    strokes(&mut image, footnote, [20; 3]);
    let (background, texts) = convert(&image, vec![block("tiny", tiny), block("Note", footnote)]);
    assert_eq!(texts, ["Note"]);
    assert_region_unchanged(&background, &image, tiny);
}

#[test]
fn an_adjacent_graphic_does_not_shrink_a_rotated_text_frame() {
    let mut image = RgbImage::filled(960, 540, [250; 3]);
    strokes(&mut image, BBox::new(130, 108, 295, 126), [20; 3]);
    let mut label = block("Tilted label", BBox::new(100, 98, 300, 137));
    label.rotation_mdeg = -15_000;
    label.lines[0].bbox = Some(BBox::new(100, 108, 300, 126));
    let layout = LayoutPage {
        size_pt: (960.0, 540.0),
        blocks: vec![label],
    };
    let build = |image: &RgbImage| {
        build_deck(
            &[PageInput {
                layout: &layout,
                image,
            }],
            &BuildOptions::default(),
        )
        .unwrap()
    };
    let (before, _) = build(&image);
    let blue = [30, 130, 190];
    ellipse(&mut image, BBox::new(80, 100, 112, 132), blue);
    line(&mut image, (96, 105), (96, 118), 1, blue);
    line(&mut image, (96, 118), (103, 122), 1, blue);
    let (after, report) = build(&image);
    assert!(
        report.pages[0]
            .preserved_text
            .iter()
            .any(|region| region.reason == n2p_build::PreservationReason::AdjacentGraphic)
    );
    assert_eq!(
        n2p_ooxml::slide::text_shapes(&before.slide(0).unwrap()).unwrap(),
        n2p_ooxml::slide::text_shapes(&after.slide(0).unwrap()).unwrap(),
        "回転文字の配置は水平な記号領域で縮めない"
    );
}
