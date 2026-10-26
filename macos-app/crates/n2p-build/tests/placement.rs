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

fn text_frame(block: Block) -> n2p_ooxml::slide::TextBoxInfo {
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
    n2p_ooxml::slide::text_shapes(&deck.slide(0).unwrap())
        .unwrap()
        .remove(0)
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
    let shape = text_frame(block);
    let left = shape.x_emu as f64 / 12700.0;
    let right = (shape.x_emu + shape.cx_emu) as f64 / 12700.0;
    assert!(left >= 60.0, "文字枠が左端へ広がった: {left}");
    assert!(right <= 900.0, "文字枠が右端へ広がった: {right}");
}

#[test]
fn broad_title_does_not_shift_to_the_slide_edge() {
    let block = observed_block(&[(
        "Lightning Experience ： ワークスペースの解剖図",
        BBox::new(35, 27, 907, 75),
    )]);
    let shape = text_frame(block);
    let left = shape.x_emu as f64 / 12700.0;
    assert!(left >= 15.0, "原稿の左端より離れている: {left}");
}
