use n2p_ooxml::{
    NewlineMode, Rect, TextBox,
    shape::TextRun,
    xml::{Element, Node},
};
fn content(e: &Element) -> String {
    e.children
        .iter()
        .filter_map(|n| {
            if let Node::Text(text) = n {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn mixed_emphasis_keeps_every_character_and_explicitly_resets_bold() {
    let mut box_info = TextBox::new(
        "mixed",
        "見出し：通常の説明",
        Rect::ltrb(10.0, 10.0, 300.0, 60.0),
    )
    .bold(true);
    box_info.styled_runs = vec![
        TextRun {
            text: "見出し：".into(),
            bold: true,
        },
        TextRun {
            text: "通常の説明".into(),
            bold: false,
        },
    ];
    let shape = box_info.build(2);
    let paragraph = shape.path(&["p:txBody", "a:p"]).unwrap();
    let runs: Vec<_> = paragraph.elements("a:r").collect();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].child_el("a:rPr").unwrap().get_attr("b"), Some("1"));
    assert_eq!(runs[1].child_el("a:rPr").unwrap().get_attr("b"), Some("0"));
    assert_eq!(
        runs.iter()
            .map(|r| content(r.child_el("a:t").unwrap()))
            .collect::<String>(),
        "見出し：通常の説明"
    );
}

#[test]
fn stale_run_text_falls_back_without_losing_edits() {
    let mut text = TextBox::new("edited", "修正後", Rect::ltrb(0.0, 0.0, 100.0, 40.0));
    text.styled_runs = vec![TextRun {
        text: "修正前".into(),
        bold: true,
    }];
    let shape = text.build(2);
    let paragraph = shape.path(&["p:txBody", "a:p"]).unwrap();
    assert_eq!(
        content(paragraph.child_el("a:r").unwrap().child_el("a:t").unwrap()),
        "修正後"
    );
}

#[test]
fn mixed_runs_preserve_line_breaks_and_paragraphs() {
    for mode in [NewlineMode::LineBreak, NewlineMode::Paragraph] {
        let mut text =
            TextBox::new("lines", "太字\n通常", Rect::ltrb(0.0, 0.0, 100.0, 80.0)).newline(mode);
        text.styled_runs = vec![
            TextRun {
                text: "太字\n".into(),
                bold: true,
            },
            TextRun {
                text: "通常".into(),
                bold: false,
            },
        ];
        let shape = text.build(2);
        let body = shape.child_el("p:txBody").unwrap();
        assert_eq!(
            body.elements("a:p").count(),
            if mode == NewlineMode::Paragraph { 2 } else { 1 }
        );
        assert_eq!(
            body.elements("a:p").flat_map(|p| p.elements("a:r")).count(),
            2
        );
    }
}
