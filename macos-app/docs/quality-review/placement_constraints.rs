//! n2p-build/examples/placement_constraints.rs へコピーして実行する。
//! 第 1 引数のディレクトリに合成入力と 3 ページの PPTX を保存する。

use std::{error::Error, fs, path::Path};

use n2p_build::{BuildOptions, PageInput, build_deck};
use n2p_core::{BBox, RgbImage};
use n2p_layout::{Block, BlockKind, LayoutPage, Line, Span};

fn block(rows: &[(&str, BBox)]) -> Block {
    let bbox = rows
        .iter()
        .map(|(_, b)| *b)
        .reduce(|a, b| {
            BBox::new(
                a.left.min(b.left),
                a.top.min(b.top),
                a.right.max(b.right),
                a.bottom.max(b.bottom),
            )
        })
        .unwrap();
    let mut result = Block::new(BlockKind::Text, bbox);
    result.text = rows.iter().map(|(t, _)| *t).collect::<Vec<_>>().join("\n");
    result.lines = rows
        .iter()
        .map(|(text, bbox)| Line {
            spans: vec![Span {
                text: Some((*text).to_owned()),
                image_path: None,
            }],
            bbox: Some(*bbox),
            observation_bbox: Some(*bbox),
            confidence_milli: Some(950),
        })
        .collect();
    result
}

fn main() -> Result<(), Box<dyn Error>> {
    let dir = std::env::args()
        .nth(1)
        .ok_or("output directory is required")?;
    let dir = Path::new(&dir);
    fs::create_dir_all(dir)?;
    let cases = [
        (
            BBox::new(40, 50, 205, 90),
            false,
            vec![("Cell contents", BBox::new(50, 58, 195, 82))],
        ),
        (
            BBox::new(40, 50, 230, 110),
            true,
            vec![
                ("A long description", BBox::new(50, 58, 220, 76)),
                ("short", BBox::new(50, 84, 110, 102)),
            ],
        ),
        (
            BBox::new(200, 50, 260, 100),
            true,
            vec![("Adjacent text", BBox::new(50, 58, 195, 82))],
        ),
    ];
    let mut images = Vec::new();
    let mut pages = Vec::new();
    for (index, (bbox, filled, rows)) in cases.iter().enumerate() {
        let mut image = RgbImage::filled(960, 540, [255; 3]);
        for y in bbox.top..bbox.bottom {
            for x in bbox.left..bbox.right {
                if *filled
                    || x < bbox.left + 2
                    || x >= bbox.right - 2
                    || y < bbox.top + 2
                    || y >= bbox.bottom - 2
                {
                    image.set_pixel(x as u32, y as u32, [180, 200, 220]);
                }
            }
        }
        fs::write(
            dir.join(format!("source-{}.png", index + 1)),
            image.to_png()?,
        )?;
        images.push(image);
        pages.push(LayoutPage {
            size_pt: (960.0, 540.0),
            blocks: vec![block(rows)],
        });
    }
    let inputs: Vec<_> = pages
        .iter()
        .zip(&images)
        .map(|(layout, image)| PageInput { layout, image })
        .collect();
    let (deck, _) = build_deck(
        &inputs,
        &BuildOptions {
            erase_background_regions: false,
            font_name: "Arial".into(),
            ..Default::default()
        },
    )?;
    deck.save(dir.join("constraints.pptx"))?;
    Ok(())
}
