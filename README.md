# Sampler / SM-810

DTM 用サンプルマネージャー。仕様は [SPEC.md](SPEC.md)、現在の実装状況と計測値は [PROGRESS.md](PROGRESS.md)、設計判断は [DECISIONS.md](DECISIONS.md) を参照してください。

現在は Tauri 2 + Svelte 5 のデスクトップ版 Map／List UI を起動できます。提供画像を基準に、ダーク配色のソース／タグ、仮想リスト、インスペクタ、波形トランスポートを実装しています。表示・検索・試聴には SQLite 内の実データを使います。

CLAPによる類似上位5件・自然文検索を実装しています。WebGL2マップとなぞり試聴、Rust UMAPサイドカーによる再計算を実装しています。ファイル名／パスのタグルール、手動順の静的コレクション、共通条件式のスマートコレクションを実装しています。Ableton Linkによるループ開始の小節頭同期を実装しています。外付け／ネットワーク／圧縮音声の先頭i16 PCMキャッシュを実装しています。マイルストーン3のDAWドロップ・聴感の実機受け入れは未検証で、Windows環境はユーザー確認により利用できません。

## デスクトップの起動

Node.js 22.12 以上、npm 11、Rust（固定ツールチェーン）、各 OS の Tauri 開発環境、CMake、libclang（rusty_linkのC++ビルド／バインディング生成）が必要です。

```sh
npx --yes npm@11 --prefix ui ci
npm --prefix ui run tauri -- dev
```

macOS のアプリバンドルを作成する場合:

```sh
npm --prefix ui run tauri -- build --debug --bundles app
```

生成先は `target/debug/bundle/macos/Sampler.app` です。開発用ビルドで、配布用の署名・公証は行っていません。Windows の実行ファイルは同じプロジェクトを Windows 上で `npm --prefix ui run tauri -- build --debug --no-bundle` により生成します。

- Sources の「+」でフォルダを追加し、Analyze で解析します。既存の CLI データベースもそのまま開きます。起動中はフォルダの変更を自動監視し、30秒間隔の再走査で通知漏れや再接続にも対応します。
- 名前の部分一致、`#tag`、ソース／子孫タグを絞り込みに使えます。
- Similarity index の Enable で約622MBの固定CLAPモデルを取得し、Index sounds で埋め込みを作ります。Pause・終了後も保存済みの続きから再開します。モデル取得後の推論は端末内で行います。
- `~a deep punchy kick drum` のような自然文で、埋め込み済みの音を検索できます。インスペクタの類似上位5件はクリックで選択・試聴できます。選択後は類似度順を既定とし、↑↓中は並びを固定します。Sort で名前順にも戻せます。
- クリックまたは ↑↓ で試聴、Space で再生／停止、Escape で選択解除、Cmd/Ctrl+K で検索に移動します。
- Mapでは左ボタンを押してなぞると試聴し、離した最後の音を選択します。Shift+ドラッグは投げ縄、右／中ドラッグと2本指スクロールはパン、ホイール／ピンチはズームです。Fit viewで全点に表示範囲を合わせます。
- MapのRecompute layoutは明示したときだけ全体を再計算します。キャンセル中も既存配置で操作でき、追加点は既存8近傍の周辺へ置きます。カテゴリの「+」で現在の条件から新しいマップを作れます。
- Shift+クリックで範囲選択し、Tag selection でまとめてタグを付けられます。解析済みの BPM／キーはインスペクタで修正できます。
- TagsのTag rulesでファイル名／パスの正規表現を登録できます。再適用しても手動タグを保持します。Collectionsの「+」で選択音の保存、または条件で更新されるスマートコレクションを作成できます。選択バーの+ Collectionから追加し、静的コレクションでは↑↓で順序を変更します。
- Linkをオンにすると、解析上のループを次の4拍の小節頭から試聴します。テンポは設定で変更できます。再生中のタイムストレッチは行いません。
- 設定のLibrary identitiesで全体ハッシュをバックグラウンド検証し、暫定的な重複判定の衝突を分割できます。Pause・再開・失敗の再試行に対応します。
- 設定のFast previewsで対象音声の先頭1秒をローカルのpreview.packへキャッシュできます。Pause・再起動から再開でき、失敗だけ再試行できます。追加時のStorage指定が対象を決めます。
- Match LUFS、目標 LUFS、±24 半音の移調に対応します。波形は元ファイルの時間軸を表示します。
- 行または下部波形のドラッグで元ファイルを渡します。両端ハンドルで範囲を調整し、Drag slice から永続 WAV を渡します。初期の全範囲は Prepare slice を押すと書き出せます。
- オーディオデバイス切断時はエラーを表示します。再接続後はアプリを再起動してください。

デスクトップの DB を分けるには起動環境の `SAMPLER_DB` に絶対パスを設定します。ブラウザだけで UI を開いてもローカルライブラリへの接続や試聴は行えません。

## 開発

Rust のバージョンは `rust-toolchain.toml` で固定しています。

```sh
npm --prefix ui run build
cargo build --release
npm --prefix ui run prepare-layout
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
npm --prefix ui run check
npm --prefix ui run test
cargo bench -p sampler-audio --bench mixer
cargo bench -p sampler-similarity --bench neighbors
```

## CLI

```sh
cargo run --release -- init
cargo run --release -- add-root "$HOME/Assets/Sample Packs" --label "Sample Packs"
cargo run --release -- scan
cargo run --release -- roots
cargo run --release -- search kick --limit 25
cargo run --release -- tag 1 'Drums/kick'
cargo run --release -- verify
cargo run --release -- analyze
cargo run --release -- cache-previews
cargo run --release -- download-model
cargo run --release -- embed --limit 100
cargo run --release -- similar 1
cargo run --release -- search "~a bright metallic hi hat" --limit 10
cargo run --release -- metadata 1
cargo run --release -- set-metadata 1 --bpm 124 --key 8
cargo run --release -- retry-failed
cargo run --release -- jobs
cargo run --release -- watch 1
```

`--db /absolute/path/library.sqlite3` で検証用 DB を指定できます。既定の DB は macOS では `~/Library/Application Support/studio.poti.sampler/library.sqlite3`、Windows では OS のローカルアプリデータ領域に作成します。

- 元音声は読み取りのみ。削除・編集・移動・リネームを行いません。
- 同じ内容の複数パスは同じ sample ID を共有し、タグは sample ID に付きます。
- 消えたファイルの行も保持します。`roots` の `missing`、検索結果の `available` で状態を確認できます。
- ドライブのマウント先が変わった場合は `relocate-root <id> <new-path>` の後に `scan <id>` を実行します。
- `verify` は全体ハッシュを読み、簡易ハッシュ衝突を分割します。初回スキャンでは全体を読まないため別コマンドです。
- `watch` は変更をまとめて再走査し、NAS と再接続のために定期走査も行います。Ctrl+C で終了します。
- 同一 DB は OS のファイルロックで排他制御します。CLI を実行する前にデスクトップを終了してください。強制終了してもロックは OS により解放されます。

CLAPのリビジョン、前処理、参照出力との照合手順は [tools/models/README.md](tools/models/README.md) を参照してください。
