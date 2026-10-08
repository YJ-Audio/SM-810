# マイルストーン3の実機受け入れ

単体テスト・ビルドの成功と、DAWに実際に取り込めることは別々に記録する。現状は後者の検証が未完了。Windows環境はないとのユーザー回答（2026-10-08）により、Windowsは未検証として記録する。

## 検証用ライブラリ

実ライブラリの代わりに、専用フォルダへコピーしたWAV／AIFF／FLAC／MP3と、別のDBを使う。DAWが解析用の補助ファイルを音声の隣へ作る場合があるため、元のサンプルパックで試さない。

macOS（リポジトリルートで実行）:

```sh
SAMPLER_DB="$HOME/Library/Application Support/studio.poti.sampler/acceptance/library.sqlite3" \
  target/debug/bundle/macos/Sampler.app/Contents/MacOS/sampler-desktop
```

Windows（PowerShell、リポジトリルートで実行）:

```powershell
$env:SAMPLER_DB = "$env:LOCALAPPDATA\studio.poti.sampler\acceptance\library.sqlite3"
.\target\debug\sampler-desktop.exe
```

Sourcesから検証用フォルダを追加してAnalyzeを実行する。アプリの試聴設定とDAWの出力音量を通常の作業時の設定にする。

## チェック手順

1. List画面でWAV／AIFF／FLAC／MP3をそれぞれ選択し、波形・長さ・再生終了を確認する。
2. ↑↓を連続して押し、選択された音へ切り替わること、クリックノイズや予期しない途切れがないことを聴く。短いワンショットと長いループの両方で試す。
3. 大きなファイルを選んだ直後にStopや別のサンプルを選び、前のデコード結果が遅れて鳴り始めないことを確認する。
4. Match LUFSと移調を変更し、再試聴で反映されることを確認する。元ファイルは変更されない。
5. Liveで空の検証用セットを開く。Samplerの行をドラッグし、オーディオクリップとして取り込めること、全体の長さが一致することを確認する。
6. Samplerの下部波形で開始・終了を調整する。ハンドルを離したらDrag sliceになるのを待ち、Liveへドラッグする。範囲の長さと内容が一致することを確認する。
7. 検証用Liveセットを保存し、Samplerを終了・再起動する。アプリデータ領域の `slices/` にWAVが残り、Liveから再び参照できることを確認する。
8. WindowsとmacOSの両方で手順を実施し、下の表にOS、Live、ビルドのコミット、結果を記録する。

## 記録

| 環境                                  | 全ファイルのドロップ | スライスのドロップ | 再起動後の参照             | 連続試聴の聴感 |
| ------------------------------------- | -------------------- | ------------------ | -------------------------- | -------------- |
| macOS / Apple Silicon / Live 12 Suite | 未検証               | 未検証             | エンジン単体テストのみ通過 | 未検証         |
| Windows                               | 未検証               | 未検証             | エンジン単体テストのみ通過 | 未検証         |

アプリのビルド、実ライブラリ検索、選択操作、再生状態、波形、WAV書き出し、元ファイル不変の確認結果は `PROGRESS.md` を参照。
