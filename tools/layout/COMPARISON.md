# UMAP比較（2026-10-08）

作者判断用の小規模試作。実ライブラリの保存済みCLAP埋め込み153件×512次元を読み取り専用で使用し、元音声にはアクセスしていない。同じ正規化ベクトル、厳密な15近傍、初期座標、500エポック、min_dist=0.1を使用。

| 試作                                     | 元の上位5近傍との平均一致率 |                  処理時間 |
| ---------------------------------------- | --------------------------: | ------------------------: |
| umap-rs 0.4.5 の fit をそのまま使用      |                       7.71% |                   0.278秒 |
| Rust: 同クレートのグラフ＋勾配処理の補完 |                      52.42% |                   0.308秒 |
| Python umap-learn 0.5.12                 |                      54.25% | 初回3.028秒／2回目0.101秒 |

Apple Silicon、Rust release、Python 3.12。Rust時間はプロセス起動・バイナリ読み書き・厳密近傍計算を含む。Python時間はfit_transformだけで、Pythonの起動・import・共有近傍行列の準備を含まない。初回はNumba初期化の影響を含む。153件のみの単一部分集合であり、差の有意性や10万件での性能は未評価。この一致率は音の聴感上の類似度を表さない。

Rust upstreamの最適化コードを調べると、引力・斥力の分母に標準実装より距離二乗が余分に入っていた。補完版ではグラフ構築をクレートに任せ、`1/(1+a*r^(2b))` の勾配、負例サンプリング、固定シードの逐次SGDを実装した。独自部分を保守する負担があるため、この補完箇所は今後もテストと比較で保守する。

導入負担:

- Rust試作: macOS arm64のrelease実行ファイル約812KiB。TauriのexternalBinで同梱でき、利用者のPython導入が不要。umap-rsはBSD-3-Clause。自前の最適化処理の検証・保守が必要。
- Python試作: umap-learnはBSD-3-Clause。Python、NumPy、SciPy、scikit-learn、Numba/llvmlite等が必要。試作用venvは約297MiB（インタプリタは別）。製品で選ぶ場合はOSごとの実行環境の同梱・起動・署名を整備する必要がある。venvをそのまま配布する構成は未実装。
- 両方式とも現試作の厳密近傍構築はO(N²)。明示的な再計算を別プロセスで行い、キャンセル可能にする。大規模再計算の時間短縮は別途計測が必要。

再実行（開発用、モデルの取得は不要）:

```sh
uv venv --python 3.12 tools/layout/.venv
uv pip install --python tools/layout/.venv/bin/python 'umap-learn==0.5.12'
cargo build --release -p sampler-layout
tools/layout/.venv/bin/python tools/layout/compare.py '/absolute/path/library.sqlite3' --output tools/layout/.cache/corrected-gradient
```

`SAMPLER_LAYOUT_UPSTREAM=1` を付けるとupstream版も比較できる。生成データはgitignore対象。参照: [umap-rs](https://github.com/wilsonzlin/umap-rs)、[umap-learn](https://github.com/lmcinnes/umap)、[UMAPパラメータ](https://umap-learn.readthedocs.io/en/latest/parameters.html)。作者がRust補完版の採用を選択（2026-10-08）。単体バイナリでの導入を優先し、独自の勾配処理の保守を受け入れる判断。
