# Sampler / SM-810

DTM 用サンプルマネージャー。仕様は [SPEC.md](SPEC.md)、現在の実装状況と計測値は [PROGRESS.md](PROGRESS.md)、設計判断は [DECISIONS.md](DECISIONS.md) を参照してください。

現在は Rust workspace と読み取り専用ライブラリ走査・検索 CLI を実装しています。UI と音声解析は後続マイルストーンです。

## 開発

Rust のバージョンは `rust-toolchain.toml` で固定しています。

```sh
cargo build --release
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
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
- 同一 DB に対する変更は1つの CLI プロセスから実行してください。
