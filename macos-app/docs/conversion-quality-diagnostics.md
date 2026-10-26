# 文字消去の診断

`--work-dir` を指定すると、ページ画像に加えて OCR と背景補間の診断を保存する。
通常の変換と GUI の表示には診断情報を追加しない。

```sh
cargo run --release -p n2p-cli -- convert input.pdf \
  --output output.pptx --pages 5,12,13 \
  --layout vision --lang ja-JP,en-US \
  --high-res-background --erase lama --work-dir /tmp/n2p-diagnostics
```

フォルダー名と JSON の `source_page` は、選択後の連番ではなく元 PDF のページ番号を使う。
既存の `page_0005.png` なども引き続き保存する。

| ファイル | 内容と座標 |
| --- | --- |
| `conversion.json` | DPI、選択ページ、消去モード、背景解像度、変換ライブラリの版 |
| `page_0005/source.png` | PDF の描画結果。透かし除去と幅調整の前 |
| `page_0005.png` | 透かし除去と幅調整の後。OCR と文字消去の入力 |
| `page_0005/ocr.json` | Vision の設定、選別前の観測、信頼度、採否。座標は左上原点の正規化値 |
| `page_0005/layout.json` | 採用したブロックと行、OCR 信頼度、回転補正前の観測矩形。座標は幅調整後のページの pt |
| `page_0005/build.json` | 編集対象の判定、文字色、マスク、採色範囲と色、局所背景判定、実際の補間分岐、モデルエラー |
| `page_0005/mask-initial.png` | 行ごとの文字マスクの和集合。OCR 入力画像と同じ解像度 |
| `page_0005/mask-erasure.png` | 補間対象の和集合。PPTX の背景画像と同じ解像度 |
| `page_0005/mask-preserved.png` | 元画素を保持する記号と UI 領域。白が保護対象で、PPTX の背景画像と同じ解像度 |
| `page_0005/background.png` | 完成した背景。編集可能な文字は重ねていない |

`ocr.json` は Vision の成功時に保存する。
他のレイアウトソースでは作成しない。
再現時は空の作業ディレクトリを使い、過去の診断と混在させない。

`build.json` の `block_index` は編集対象の `text_blocks` の索引で、消去処理の `line_index` は対応する行の索引になる。
保護対象を切り離すとブロックや行の数が変わるため、元の `layout.json` との対応は `source_block_index` と `source_line_indices` でたどる。
これらの索引は 0 始まり。
回転文字や行の座標を持たないブロックでは、ブロック全体を処理して `line_index` を `null` にする。
`bbox_px` と `sampling_bbox_px` は背景画像の画素座標で、右端と下端を含まない。
`mask-initial.png` と `mask-erasure.png` の大きさは、背景を縮小する設定では異なる。

## 記号と画面キャプチャの保護

OCR 信頼度は `confidence_milli` (0〜1000、不明は `null`) として行ごとに保持する。
`observation_bbox_pt` は回転補正前の外接矩形で、傾斜文字のインク高を表す `bbox_pt` と区別する。
`natural_font_size_pt` は最小字級に引き上げる前の推定値で、スライドの縮尺を適用した pt。

`report.preserved_text` は画像に残した OCR 文字列、領域、元の観測索引、信頼度、理由を含む。
`report.protected_boxes_pt` は、画面内の観測を囲む領域も含めた保護範囲になる。

| `reason` | 表示方針 |
| --- | --- |
| `graphic_symbol` | 人物、チェック、時計、矢印などの独立した記号を原画のまま保持 |
| `graphic_prefix` | 本文先頭と結合された記号だけを画像に残し、本文は編集可能にする |
| `inline_symbol` | 本文中のチェックだけを画像に残し、前後の文字は編集可能にする |
| `small_ui_text` | 画面内の小さい文字を画像に残し、可視テキストを重ねない |
| `uncertain_graphic` | 信頼度が低い短い観測と色付きの図が重なる場合に原画を保持 |
| `adjacent_graphic` | 近接する文字の消去から図形を保護。OCR の文字を除外していないため `text` は空文字列 |

保護画素は消去マスクと背景の採色候補から除外する。
矩形で消去する旧モードでも、最後に元画素を復元する。
通常の見出しや本文、画面外の孤立した細字を一律に画像へ戻す処理は行わない。
判定は画素の色と形、観測の大きさ、近接関係を使うため、すべての資料の図形や UI を検出できるとは限らない。

`ConvertReport.preserved_text` にも元 PDF のページ番号と保護理由を返す。
このレポートは `--work-dir` を指定しない場合も利用できる。

局所補間は元画像から背景を推定し、全対象文字を候補から除外する。
背景候補の色の制約は初期補間と平滑化で共用する。
`ring_flat` はマスク拡張に使った従来の輝度判定、`local_flat` は文字枠内の RGB 分布による判定であり、補間分岐は後者を使う。
`lama_status` でモデルの読み込みを確認し、各領域の `method` でモデルが実際に使われたかを確認する。
背景候補も利用可能なモデルもない場合は元画像を保持し、`source-preserved-no-background` と記録する。
白色で埋め合わせる処理は行わない。

診断には原稿の画像と OCR テキストが含まれる。
再配布できない資料の診断ファイルはリポジトリへ追加しない。
