# Sampler / SM-810

サンプル音源を探す・試聴する・整理するための、Windows／macOS向けデスクトップアプリです。似た音が並ぶ2Dマップをなぞって聴き比べ、気に入った音や切り出した範囲をDAWへドラッグできます。

音声ファイルは元の場所に置いたまま管理します。元ファイルの編集・移動・削除は行いません。

## 主な機能

- **音を探す** — ファイル名やタグで絞り込み、選択した音に似たサンプルを検索。`~a deep punchy kick drum`のような文章でも探せます。
- **マップで聴き比べる** — 音の類似度で配置された2Dマップ上をなぞって試聴できます。
- **ライブラリを整理する** — 階層タグ、ファイル名・フォルダに応じた自動タグ、手動コレクション、条件で更新されるスマートコレクションに対応します。
- **試聴を調整する** — 音量合わせ、移調、波形表示、BPM・キーの確認ができます。Ableton Linkでループの再生開始を小節頭に合わせられます。
- **DAWへ持ち込む** — 元ファイルをそのまま渡すほか、波形で選んだ範囲をWAVとして書き出して渡せます。

対応形式: WAV、AIFF、FLAC、MP3、OGG。

## 使い始める

1. **Sources**の「+」から音声フォルダを追加します。
2. **Analyze**で波形や音声情報を解析します。
3. 名前やタグで検索し、行をクリックするか↑↓キーで試聴します。Spaceで再生・停止できます。
4. 気に入った音をDAWへドラッグします。一部分だけ使う場合は、下部の波形で範囲を選び、**Drag slice**から渡します。

類似検索と2Dマップを使うには、**Similarity index → Enable**で約622MBのモデルを取得し、**Index sounds**を実行します。取得後の推論は端末内で行います。処理は一時停止でき、続きから再開できます。

## ソースから起動する

Node.js 22.12以上、npm 11、Rust、[各OSのTauri開発環境](https://v2.tauri.app/start/prerequisites/)、CMake、libclangが必要です。Rustのバージョンは`rust-toolchain.toml`で指定しています。

WindowsではVisual Studio Build Toolsの「C++によるデスクトップ開発」をインストールし、libclang.dllのあるディレクトリを設定してください。LLVMを標準の場所にインストールした場合:

```powershell
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
```

リポジトリのルートで実行します。

```sh
npx --yes npm@11 --prefix ui ci
npm --prefix ui run tauri -- dev
```

## ビルドする

Windows 10／11（x64）のインストーラー:

```powershell
npm --prefix ui run build:windows
```

`target/release/bundle/nsis/`に生成されます。必要なランタイムを同梱し、現在のユーザー向けにインストールします。WebView2がない場合はセットアップ中に取得します。インストーラーは未署名です。

macOSのアプリバンドル:

```sh
npm --prefix ui run tauri -- build --debug --bundles app
```

`target/debug/bundle/macos/Sampler.app`に生成されます。開発用ビルドで、署名・公証は行っていません。

## 開発者向け資料

- [仕様](SPEC.md)
- [設計判断](DECISIONS.md)
- [実機検証の手順](ACCEPTANCE.md)

CLIの使い方は`cargo run -- --help`で確認できます。
