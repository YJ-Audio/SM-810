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

## マップの性能と操作

10万点描画の再計測は、ビルド後に通常のSamplerを終了してから行う。専用ページは合成点であることを画面に明示し、ライブラリDBには保存しない。

```sh
SAMPLER_MAP_BENCH=1 SAMPLER_DB=/tmp/sampler-map-benchmark.sqlite3 \
  target/debug/bundle/macos/Sampler.app/Contents/MacOS/sampler-desktop
```

60フレームのウォームアップ後、600フレームを測定する。本体と同じRenderer／Gridで、ズーム0.2–1.0、パン、12pxのヒット判定、100msごとの80px範囲の先読み候補とリング更新を実行する。fps、p95、25ms超のフレーム数、JS処理時間、描画サイズとDPRを画面から記録する。ブラウザのリフレッシュレート、前面／背面、並行ビルドなどの条件も記録する。

なぞり試聴の計測を有効にする場合:

```sh
SAMPLER_TRACE_LATENCY=1 \
  target/debug/bundle/macos/Sampler.app/Contents/MacOS/sampler-desktop
```

実ライブラリのMapで点をクリック／なぞり、標準エラーの `onset_probe` を記録する。UIのヒット時刻→Rustへ到達するまでの壁時計差と、デコード・キュー・最初のPCMコールバックまでの単調時計差に、CPALのplayback−callbackを足す。これはドライバの再生予定時刻までの値で、5msフェードが可聴になる位置や録音経路・DAC以降を含む実音の測定ではない。物理ループバック測定と聴感確認は別に行う。

1. 点のクリックで選択・再生し、なぞって離すと最後の音がインスペクタと波形に出ることを確認する。
2. なぞっている途中のトランスポートは元の選択音を保ち、マップ外へ押したまま出ると停止する。
3. Shift投げ縄で複数選択し、タグをまとめて付ける。コレクション追加はマイルストーン6で確認する。
4. ホイール／ピンチはカーソル中心にズーム、右／中ドラッグ・2本指スクロールはパンする。Fit viewは表示範囲だけを合わせる。
5. フィルタ不一致点は薄く残り、解除しても座標が変わらない。全体再計算のときだけアニメーションし、provisional数とrevisionが更新される。
6. 再計算を中止しても既存配置が変わらず、再起動後も配置とrevisionが残ることを確認する。
