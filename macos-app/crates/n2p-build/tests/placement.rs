use n2p_build::{BuildOptions, PageInput, build_deck};
use n2p_core::{BBox, RgbImage};
use n2p_layout::{Block, BlockKind, LayoutPage, Line, Span};

fn observed_block(rows: &[(&str, BBox)]) -> Block {
    let bbox = rows.iter().map(|(_, bbox)| *bbox).reduce(|a, b| {
        BBox::new(
            a.left.min(b.left),
            a.top.min(b.top),
            a.right.max(b.right),
            a.bottom.max(b.bottom),
        )
    });
    let mut block = Block::new(BlockKind::Text, bbox.unwrap());
    block.text = rows
        .iter()
        .map(|(text, _)| *text)
        .collect::<Vec<_>>()
        .join("\n");
    block.lines = rows
        .iter()
        .map(|(text, bbox)| Line {
            spans: vec![Span {
                text: Some((*text).to_owned()),
                image_path: None,
            }],
            bbox: Some(*bbox),
            observation_bbox: Some(*bbox),
            confidence_milli: Some(1000),
        })
        .collect();
    block
}

fn text_frames(block: Block) -> Vec<(n2p_ooxml::slide::TextBoxInfo, f64)> {
    let image = RgbImage::filled(1376, 768, [245; 3]);
    let layout = LayoutPage {
        size_pt: (1376.0, 768.0),
        blocks: vec![block],
    };
    let (deck, _) = build_deck(
        &[PageInput {
            image: &image,
            layout: &layout,
        }],
        &BuildOptions {
            erase_background_regions: false,
            ..Default::default()
        },
    )
    .unwrap();
    let slide = deck.slide(0).unwrap();
    let sizes: Vec<f64> = slide
        .root
        .path(&["p:cSld", "p:spTree"])
        .unwrap()
        .elements("p:sp")
        .filter_map(|sp| sp.path(&["p:txBody", "a:p", "a:r", "a:rPr"]))
        .map(|r| r.get_attr("sz").unwrap().parse::<f64>().unwrap() / 100.0)
        .collect();
    n2p_ooxml::slide::text_shapes(&slide)
        .unwrap()
        .into_iter()
        .zip(sizes)
        .collect()
}

#[test]
fn long_two_line_footer_keeps_its_original_horizontal_bounds() {
    let block = observed_block(&[
        (
            "【ミニクイズ】次の中でカスタムオブジェクトはどれ？",
            BBox::new(342, 687, 1042, 723),
        ),
        (
            "（a） CampaignMember （b） Leave_Balance__C （c） Order__x （d） Lead → 正解：（b）",
            BBox::new(118, 726, 1254, 765),
        ),
    ]);
    let (shape, font_size) = text_frames(block).remove(0);
    let left = shape.x_emu as f64 / 12700.0;
    let right = (shape.x_emu + shape.cx_emu) as f64 / 12700.0;
    assert!(left >= 60.0, "文字枠が左端へ広がった: {left}");
    assert!(right <= 900.0, "文字枠が右端へ広がった: {right}");
    assert!(
        font_size <= 19.0,
        "フッターの文字が枠幅を超える: {font_size}"
    );
}

#[test]
fn broad_title_does_not_shift_to_the_slide_edge() {
    let block = observed_block(&[(
        "Lightning Experience ： ワークスペースの解剖図",
        BBox::new(35, 27, 907, 75),
    )]);
    let (shape, font_size) = text_frames(block).remove(0);
    let left = shape.x_emu as f64 / 12700.0;
    assert!(left >= 15.0, "原稿の左端より離れている: {left}");
    assert!(
        font_size <= 27.0,
        "タイトルの文字が枠幅を超える: {font_size}"
    );
}

#[test]
fn observed_body_lines_keep_their_own_positions_and_widths() {
    let rows = [
        ("説明の見出し Heading", BBox::new(100, 100, 400, 130)),
        (
            "行間を維持する長い説明の本文",
            BBox::new(100, 140, 510, 162),
        ),
        ("短い最終行", BBox::new(100, 170, 230, 192)),
    ];
    let frames = text_frames(observed_block(&rows));
    assert_eq!(
        frames.len(),
        rows.len(),
        "観測行を一つの固定行間へ押し込んでいる"
    );
    let scale = 960.0 / 1376.0;
    for ((frame, size), (text, bbox)) in frames.iter().zip(rows) {
        assert_eq!(frame.text, text);
        assert!((frame.x_emu as f64 / 12700.0 - bbox.left as f64 * scale).abs() < 1.0);
        let center = (frame.y_emu as f64 + frame.cy_emu as f64 / 2.0) / 12700.0;
        assert!((center - (bbox.top + bbox.bottom) as f64 / 2.0 * scale - 2.0).abs() < 1.0);
        assert!(*size <= 22.0, "短い行が拡大された: {size}");
        assert!(frame.cx_emu as f64 / 12700.0 <= bbox.width() as f64 * scale + 1.0);
    }
}

#[test]
fn unobserved_paragraph_remains_one_editable_shape() {
    let mut block = observed_block(&[
        ("長い説明の一行目", BBox::new(100, 100, 400, 125)),
        ("短い二行目", BBox::new(100, 135, 230, 160)),
    ]);
    for line in &mut block.lines {
        line.observation_bbox = None;
    }
    assert_eq!(text_frames(block).len(), 1);
}
