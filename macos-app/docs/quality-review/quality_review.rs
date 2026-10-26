//! 合成入力の背景と文字枠を保存する PR 比較用のサンプル。
//! n2p-build/examples/quality_review.rs へコピーして実行する。

use std::{error::Error, fs, path::Path};

use n2p_build::{BackgroundResolution, BuildOptions, EraseMode, PageInput, build_deck};
use n2p_core::{BBox, RgbImage};
use n2p_layout::{Block, BlockKind, LayoutPage, Line, Span};

fn block(text: &str, rows: &[BBox]) -> Block {
    let bbox = rows
        .iter()
        .copied()
        .reduce(|a, b| {
            BBox::new(
                a.left.min(b.left),
                a.top.min(b.top),
                a.right.max(b.right),
                a.bottom.max(b.bottom),
            )
        })
        .unwrap();
    let mut block = Block::new(BlockKind::Text, bbox);
    block.text = text.to_owned();
    block.lines = text
        .split('\n')
        .zip(rows)
        .map(|(text, bbox)| Line {
            spans: vec![Span {
                text: Some(text.to_owned()),
                image_path: None,
            }],
            bbox: Some(*bbox),
            ..Default::default()
        })
        .collect();
    block
}

fn draw_line(image: &mut RgbImage, a: (i64, i64), b: (i64, i64), color: [u8; 3]) {
    let steps = (b.0 - a.0).abs().max((b.1 - a.1).abs()).max(1);
    for i in 0..=steps {
        let x = a.0 + (b.0 - a.0) * i / steps;
        let y = a.1 + (b.1 - a.1) * i / steps;
        for dy in -1..=1 {
            for dx in -1..=1 {
                image.set_pixel((x + dx) as u32, (y + dy) as u32, color);
            }
        }
    }
}

fn stripes(image: &mut RgbImage, bbox: BBox, color: [u8; 3], period: i64, width: i64) {
    for y in bbox.top..bbox.bottom {
        for x in bbox.left..bbox.right {
            if (x - bbox.left) % period < width {
                image.set_pixel(x as u32, y as u32, color);
            }
        }
    }
}

fn save_case(
    dir: &Path,
    name: &str,
    image: RgbImage,
    blocks: Vec<Block>,
    erase: EraseMode,
) -> Result<(), Box<dyn Error>> {
    let layout = LayoutPage {
        size_pt: (image.width as f64, image.height as f64),
        blocks,
    };
    let (deck, _) = build_deck(
        &[PageInput {
            image: &image,
            layout: &layout,
        }],
        &BuildOptions {
            erase_mode: erase,
            font_name: "Noto Sans JP".to_owned(),
            east_asian_font_name: Some("Noto Sans JP".to_owned()),
            background_resolution: BackgroundResolution::FullImage,
            ..Default::default()
        },
    )?;
    fs::write(dir.join(format!("{name}-source.png")), image.to_png()?)?;
    fs::write(
        dir.join(format!("{name}-background.png")),
        deck.background_image(0)?.unwrap(),
    )?;
    deck.save(dir.join(format!("{name}.pptx")))?;
    let slide = deck.slide(0)?;
    let frames = n2p_ooxml::slide::text_shapes(&slide)?;
    let sizes: Vec<String> = slide
        .root
        .path(&["p:cSld", "p:spTree"])
        .unwrap()
        .elements("p:sp")
        .filter_map(|sp| sp.path(&["p:txBody", "a:p", "a:r", "a:rPr"]))
        .map(|r| r.get_attr("sz").unwrap_or("0").to_owned())
        .collect();
    let mut metrics = String::new();
    for (frame, size) in frames.iter().zip(sizes) {
        metrics.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            frame.text.replace('\n', " | "),
            frame.x_emu,
            frame.y_emu,
            frame.cx_emu,
            frame.cy_emu,
            size
        ));
    }
    fs::write(dir.join(format!("{name}-frames.tsv")), metrics)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let dir = std::env::args()
        .nth(1)
        .ok_or("output directory is required")?;
    let dir = Path::new(&dir);
    fs::create_dir_all(dir)?;

    let mut banner = RgbImage::filled(240, 160, [255; 3]);
    for y in 120..160 {
        for x in 0..240 {
            banner.set_pixel(x, y, [12, 30, 54]);
        }
    }
    let rows = [BBox::new(16, 124, 224, 136), BBox::new(16, 144, 224, 156)];
    for bbox in rows {
        stripes(&mut banner, bbox, [255; 3], 8, 4);
    }
    save_case(
        dir,
        "banner",
        banner,
        vec![block("Editable banner text\nA second editable line", &rows)],
        EraseMode::Lama,
    )?;

    let mut people = RgbImage::filled(960, 540, [245, 248, 250]);
    let blue = [30, 135, 160];
    for left in [105, 128] {
        for y in 100..116 {
            for x in left..left + 16 {
                let dx = (x as f64 - (left + 8) as f64) / 8.0;
                let dy = (y as f64 - 108.0) / 8.0;
                if dx * dx + dy * dy <= 1.0 {
                    people.set_pixel(x as u32, y, blue);
                }
            }
        }
        for (a, b) in [
            ((left - 4, 132), (left - 4, 125)),
            ((left - 4, 125), (left + 4, 120)),
            ((left + 4, 120), (left + 12, 120)),
            ((left + 12, 120), (left + 20, 125)),
            ((left + 20, 125), (left + 20, 132)),
            ((left - 4, 132), (left + 20, 132)),
        ] {
            draw_line(&mut people, a, b, blue);
        }
    }
    let body = BBox::new(200, 104, 330, 130);
    stripes(&mut people, body, [10; 3], 7, 2);
    save_case(
        dir,
        "people",
        people,
        vec![
            block("000", &[BBox::new(100, 96, 152, 138)]),
            block("Contact", &[body]),
        ],
        EraseMode::StrokeMask,
    )?;

    let image = RgbImage::filled(1376, 768, [245; 3]);
    save_case(
        dir,
        "placement",
        image,
        vec![block(
            "Editable Presentation ： レイアウトの検証",
            &[BBox::new(35, 27, 907, 75)],
        )],
        EraseMode::StrokeMask,
    )?;
    Ok(())
}
