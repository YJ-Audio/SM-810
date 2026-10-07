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

## 2026-10-07 — マイルストーン2

- [x] symphonia 0.6.1 (gapless)、rubato 5.0.1。全体 / 先頭デコード、モノラル化、指定レートへの変換。
- [x] 長さ・LUFS・ピーク・acid / smpl・ファイル名の BPM / キー、ループ判定、テンポ推定、FFT によるルート / クロマ推定。
- [x] 256ビンの min/max を i8 で peaks.pack に追記し、永続化後にDBへ登録。
- [x] CPUコア数−1の解析ワーカー、単一 writer、古い analyzer_ver の再解析、失敗ジョブの明示的再試行。
- [x] 実ライブラリ解析プロセスを実際に強制終了。running 9件が残り、再起動後 pending に回収され、running 0件になった。累計200件解析済み、解析失敗0件。
- [x] manual BPM / キー / モードの再解析後保持、再起動後の波形パック読み出し、合成 WAV のデコードとリサンプル、短い音のLUFS、RIFF奇数チャンクのパディングと不正長をテスト。
- [x] Rust テスト13件、Clippy warnings-as-errors 通過。

## 次の作業

マイルストーン3: cpal / rtrb の再生、Tauri 2 + Svelte 5 の画像に沿ったリストUI、トランスポート、永続スライス書き出しとネイティブドラッグ。

## 未検証・後続段階

- Windows でのビルドと DAW ドロップ、macOS の DAW ドロップは未検証。
- マイルストーン3〜6（Tauri / Svelte UI、試聴、CLAP、WebGL2、整理機能）は未実装。
- UI は提供された List mode / Map mode 画像の4ペイン、ダーク配色、Geist / Geist Mono を基準にする。
