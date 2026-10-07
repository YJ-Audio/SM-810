# 実装進捗

## 2026-10-07 — マイルストーン1

- [x] Cargo workspace、SQLite マイグレーション、scan、engine、開発用 CLI。
- [x] 実ライブラリ `~/Assets/Sample Packs/` の 15,100 ファイルを初回 7.106 秒で走査。暫定サンプル数 14,532、エラー 0。macOS / Apple Silicon、release ビルド、OS キャッシュ状態は制御していない。
- [x] 再スキャン 7.292 秒、全 file ID・sample ID・相対パスの対応を比較し一致。
- [x] 移動・リネーム・重複・削除・root 切断・再接続による ID / タグ保持を一時ディレクトリで検証。実ライブラリは変更していない。
- [x] FTS5 trigram 部分一致。`kick` の CLI 起動込み検索 15.8ms（上位5件）。短いクエリ・引用符・日本語・ワイルドカード文字・ページングもテスト。
- [x] 簡易ハッシュ衝突を合成し、全体ハッシュで分割、タグ複製、再走査後の ID 維持を確認。
- [x] `cargo test --workspace`（8件）、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check`。

DB: OS アプリデータ領域の `studio.poti.sampler/library.sqlite3`。元ファイルに書き込む API は使用していない。

## 次の作業

マイルストーン2: symphonia / rubato の独立 decode クレート、解析、波形パック、再開可能なジョブワーカー。

## 未検証・後続段階

- Windows でのビルドと DAW ドロップ、macOS の DAW ドロップは未検証。
- マイルストーン3〜6（Tauri / Svelte UI、試聴、CLAP、WebGL2、整理機能）は未実装。
- UI は提供された List mode / Map mode 画像の4ペイン、ダーク配色、Geist / Geist Mono を基準にする。
