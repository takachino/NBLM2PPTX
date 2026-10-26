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
| `page_0005/layout.json` | 採用したブロックと行。座標は幅調整後のページの pt |
| `page_0005/build.json` | 編集対象の判定、文字色、マスク、採色範囲と色、局所背景判定、実際の補間分岐、モデルエラー |
| `page_0005/mask-initial.png` | 行ごとの文字マスクの和集合。OCR 入力画像と同じ解像度 |
| `page_0005/mask-erasure.png` | 補間対象の和集合。PPTX の背景画像と同じ解像度 |
| `page_0005/background.png` | 完成した背景。編集可能な文字は重ねていない |

`ocr.json` は Vision の成功時に保存する。
他のレイアウトソースでは作成しない。
再現時は空の作業ディレクトリを使い、過去の診断と混在させない。

`build.json` の `block_index` は `text_blocks` の索引で、`line_index` は対応する行の索引になる。
回転文字や行の座標を持たないブロックでは、ブロック全体を処理して `line_index` を `null` にする。
`bbox_px` と `sampling_bbox_px` は背景画像の画素座標で、右端と下端を含まない。
`mask-initial.png` と `mask-erasure.png` の大きさは、背景を縮小する設定では異なる。

局所補間は元画像から背景を推定し、全対象文字を候補から除外する。
背景候補の色の制約は初期補間と平滑化で共用する。
`ring_flat` はマスク拡張に使った従来の輝度判定、`local_flat` は文字枠内の RGB 分布による判定であり、補間分岐は後者を使う。
`lama_status` でモデルの読み込みを確認し、各領域の `method` でモデルが実際に使われたかを確認する。
背景候補も利用可能なモデルもない場合は元画像を保持し、`source-preserved-no-background` と記録する。
白色で埋め合わせる処理は行わない。

診断には原稿の画像と OCR テキストが含まれる。
再配布できない資料の診断ファイルはリポジトリへ追加しない。
