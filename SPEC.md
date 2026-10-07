# サンプルマネージャー 実装書

Oct 7, 2026 · @ぽち。

## 目的とスコープ

DTM用のサンプル管理デスクトップアプリを、まず作者本人が日常的に使うツールとして実装する。中心となる体験は、音の類似度で配置した2Dマップをマウスでなぞって試聴し、気に入った音をそのままDAWへドラッグすることである。

- 対象OS: Windows と macOS。Linux は対象外。
- 規模: 現在1万ファイル超。10万件まで構成を変えずに耐えることを目安にする。
- 解析: 標準レベルで十分（長さ、ラウドネス、ピーク、BPM、ルートキー、ループ判定）。
- 試聴: 切り替えに待ちや無音の隙間を感じさせない。
- 整理: 階層タグ、ファイル名やフォルダからのルールによる自動タグ、条件で決まるスマートコレクション。
- 類似検索: 近傍検索と2Dマップの両方が必須。
- UI: 作り込む。見た目と操作感は妥協しない。

対象外: 配布用インストーラ、オンボーディング、ライセンス管理、クラウド同期、元ファイルの編集・移動・リネーム。

不変条件が2つある。ユーザーの音声ファイルには一切書き込まないこと、そしてタグや解析結果はSQLiteのDBだけを正とすること。サンプルライブラリは長年かけて集めた資産で、アプリのバグで壊れると取り返しがつかないため。

UIのモックは [Sample Manager UI](https://claude.ai/artifact/H8dVn2vzbDtnWiSMEkDtGW) にある。レイアウトと状態の見せ方の参考であり、ピクセル単位で合わせる必要はない。

## 技術スタックとクレート構成

バックエンドはRustのCargo workspace、フロントはTauri 2 + Svelte 5、データはSQLite。凝ったUIをWin/macOSで揃えるためにWebViewを使い、音声と重い処理はすべてRust側に置く。

| 領域 | 採用 | 理由 |
| --- | --- | --- |
| アプリ殻 | Tauri 2 | UIの作り込みが速く、tauri-plugin-drag で両OSのネイティブドラッグアウトが使える |
| UI | Svelte 5 (runes) + TypeScript | 作者が普段使っている |
| マップ描画 | WebGL2 インスタンス描画 | WebGPU は WKWebView 側の対応がOSバージョン依存のため使わない |
| DB | rusqlite (bundled) + rusqlite\_migration | 単一ファイル、FTS5 trigram と式インデックスを使う |
| デコード | symphonia, rubato | WAV/AIFF/FLAC/MP3/OGG を一本化し、必要に応じてリサンプル |
| 音声出力 | cpal, rtrb | 音声スレッドとの受け渡しをロックフリーにする |
| テンポ同期 | rusty\_link (Ableton Link) | ループ試聴をDAWのテンポに合わせる |
| 推論 | ort (ONNX Runtime) | CLAP埋め込み。macOS は CoreML、Windows は DirectML の実行プロバイダを優先 |
| ラウドネス | ebur128 | 試聴の音量合わせとLUFS表示 |
| ハッシュ | xxhash-rust (xxh3) | 重複検出用途なので暗号学的強度は不要 |
| 監視 | notify, ignore | フォルダ監視と走査 |

依存ライブラリは採用時点の最新安定版を使い、バージョンは Cargo.lock で固定する。

```
sampler/
├─ crates/
│  ├─ db          スキーマ、マイグレーション、クエリ
│  ├─ scan        走査、監視、ハッシュ
│  ├─ decode      symphonia + rubato のラッパ
│  ├─ analysis    LUFS、ピーク、RIFFチャンク、BPM、キー、ループ判定
│  ├─ embed       CLAP前処理と推論
│  ├─ similarity  ベクトルストア、近傍検索、マップ配置
│  ├─ audio       再生エンジン
│  └─ engine      ジョブスケジューラと外向きAPI
├─ src-tauri      Tauriコマンド層
├─ ui             Svelte + WebGL2
└─ tools/umap     UMAP再計算のサイドカー（言語は未決、末尾参照）
```

依存の向きには次のルールがある。どれも理由があるので崩さないこと。

- engine は Tauri に依存しない。UIなしでスキャン・解析・検索をテストやベンチで回せるようにするため。src-tauri はコマンドとイベントを中継するだけにする。
- audio は db にも engine にも依存しない。リアルタイムスレッドからブロッキングIOへ到達する経路を、クレート境界の時点で作れなくするため。audio が受け取るのはデコード済みバッファと制御コマンドだけ。
- decode は独立クレートにする。analysis、embed、audio の先読みの3か所が共有するため。
- DBへの書き込みは engine 内の単一 writer スレッドに集約する。SQLite の writer は同時に1本だけなので、各ワーカーが直接書くと SQLITE\_BUSY で詰まるため。

## データモデル

スキーマは以下を初期マイグレーションとしてそのまま使う。核になるのは `samples`（音の中身）と `files`（置き場所）の分離で、移動・リネーム・重複コピー・ドライブの取り外しがあってもタグと解析結果が失われない。

合意済みのスキーマから3点を追加している。`waveform_peaks`（リスト用の波形サムネ）、`slice_exports`（DAWへ渡したスライスの管理）、そして `jobs.kind` への `peaks` の追加。

```sql
-- UIの読み取りとスキャナ・ジョブの書き込みを同時に走らせたいので WAL にする
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

-- 外付けドライブのドライブレターやマウントポイントが変わっても、ここの path を1行直すだけで
-- 全ファイルを復帰させたい。そのため files 側は root からの相対パスで持つ
CREATE TABLE roots (
  id       INTEGER PRIMARY KEY,
  path     TEXT NOT NULL UNIQUE,
  label    TEXT NOT NULL,
  -- 試聴用の先頭キャッシュを作るかどうかは、置き場所の速さで決めたい。
  -- 自動判定は当てにならないので、ユーザーが明示する
  storage  TEXT NOT NULL DEFAULT 'local' CHECK (storage IN ('local', 'external', 'network')),
  enabled  INTEGER NOT NULL DEFAULT 1
);

-- タグや解析結果はパスではなく音の中身に紐づけたい。
-- そうすれば、移動・リネーム・重複コピーがあってもメタデータを失わずに済む
CREATE TABLE samples (
  id          INTEGER PRIMARY KEY,
  size        INTEGER NOT NULL,
  -- 初回スキャンを全読みせずに終わらせるための暫定ID（先頭と末尾の数十KBにかけた xxh3-64）。
  -- 衝突は個人ライブラリの規模なら許容する。full_hash が埋まった時点で食い違いを検出して分割する
  quick_hash  BLOB NOT NULL,
  full_hash   BLOB,
  created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
  UNIQUE (size, quick_hash)
);
CREATE INDEX samples_full_hash ON samples(full_hash) WHERE full_hash IS NOT NULL;

CREATE TABLE files (
  id              INTEGER PRIMARY KEY,
  sample_id       INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
  root_id         INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
  rel_path        TEXT NOT NULL,
  mtime           INTEGER NOT NULL,
  -- 消えたファイルを即削除すると、ドライブを外しただけでタグが飛んでしまう。
  -- スキャン世代で「今は見えていない」状態を表し、実際の削除はユーザー操作に限る
  last_seen_scan  INTEGER NOT NULL,
  UNIQUE (root_id, rel_path)
);
CREATE INDEX files_sample ON files(sample_id);

CREATE TABLE analysis (
  sample_id     INTEGER PRIMARY KEY REFERENCES samples(id) ON DELETE CASCADE,
  -- 解析ロジックを直したときに、古い版で出した結果だけを再解析キューに戻したい
  analyzer_ver  INTEGER NOT NULL,
  duration_ms   INTEGER NOT NULL,
  sample_rate   INTEGER NOT NULL,
  channels      INTEGER NOT NULL,
  lufs          REAL,
  peak_dbfs     REAL,
  is_loop       INTEGER,
  bpm           REAL,
  -- 手で直した値を再解析で上書きしたくないので、値の出どころを持つ。
  -- 優先度は manual > chunk > filename > analysis
  bpm_source    TEXT CHECK (bpm_source IN ('manual', 'chunk', 'filename', 'analysis')),
  key_root      INTEGER CHECK (key_root BETWEEN 0 AND 11),
  key_mode      TEXT CHECK (key_mode IN ('major', 'minor')),
  key_source    TEXT CHECK (key_source IN ('manual', 'chunk', 'filename', 'analysis'))
);

-- モデルを差し替えたときに、異なるベクトル空間が混ざらないようにしたい。
-- モデル単位の一括ロードを範囲スキャンで済ませたいので、model を主キーの先頭に置く
CREATE TABLE embeddings (
  model      TEXT NOT NULL,
  sample_id  INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
  dim        INTEGER NOT NULL,
  -- 起動時に連続配列へ載せ替えて総当たり検索する前提なので、DB上は f32 LE の生バイト列で足りる
  vec        BLOB NOT NULL,
  PRIMARY KEY (model, sample_id)
) WITHOUT ROWID;

CREATE TABLE maps (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL,
  model       TEXT NOT NULL,
  -- マップに載せる対象は、固定のサンプル集合ではなく条件で決めたい。
  -- スマートコレクションと同じクエリ表現（JSON）を使い回す
  query       TEXT NOT NULL,
  -- 全体を再計算したときだけ上げる。UI側はこの値で、配置の移行アニメーションを挟むかどうかを判断する
  layout_rev  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE map_points (
  map_id     INTEGER NOT NULL REFERENCES maps(id) ON DELETE CASCADE,
  sample_id  INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
  x          REAL NOT NULL,
  y          REAL NOT NULL,
  -- 追加時の近傍配置は近似にすぎない。暫定点の数を数えて、
  -- 全体再計算をする価値があるかどうかの目安をUIに出したいので区別しておく
  placed_by  TEXT NOT NULL CHECK (placed_by IN ('projection', 'neighbors')),
  PRIMARY KEY (map_id, sample_id)
) WITHOUT ROWID;

CREATE TABLE tags (
  id         INTEGER PRIMARY KEY,
  parent_id  INTEGER REFERENCES tags(id) ON DELETE CASCADE,
  name       TEXT NOT NULL,
  color      INTEGER
);
-- UNIQUE(parent_id, name) だと NULL 同士が別物として扱われ、トップレベルに同名タグが作れてしまう
CREATE UNIQUE INDEX tags_unique_name ON tags(ifnull(parent_id, 0), name);

-- ルールを後から直して再適用したいので、ルール自体をデータとして持つ
CREATE TABLE tag_rules (
  id       INTEGER PRIMARY KEY,
  tag_id   INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  target   TEXT NOT NULL CHECK (target IN ('filename', 'rel_path')),
  pattern  TEXT NOT NULL,
  enabled  INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE sample_tags (
  sample_id  INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
  tag_id     INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  -- ルールの再適用では、ルール由来の付与だけを消して付け直したい。手で付けたタグは巻き込まない。
  -- 同じタグが手動とルールの両方から来た場合は手動を優先して NULL にする
  rule_id    INTEGER REFERENCES tag_rules(id) ON DELETE CASCADE,
  PRIMARY KEY (sample_id, tag_id)
) WITHOUT ROWID;
CREATE INDEX sample_tags_tag ON sample_tags(tag_id);

CREATE TABLE collections (
  id     INTEGER PRIMARY KEY,
  name   TEXT NOT NULL,
  kind   TEXT NOT NULL CHECK (kind IN ('static', 'smart')),
  query  TEXT,
  CHECK ((kind = 'smart') = (query IS NOT NULL))
);

CREATE TABLE collection_items (
  collection_id  INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
  sample_id      INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
  -- 手で並べた順を保ちたい。間に差し込むたびに全行を振り直さずに済むよう REAL にする
  position       REAL NOT NULL,
  PRIMARY KEY (collection_id, sample_id)
) WITHOUT ROWID;

-- ファイル名は "KCK_dark_03" のように区切りの不揃いな文字列が多く、単語単位では部分一致を拾えない。
-- そのため trigram トークナイザを使う。
-- 複数のパスやタグ名をサンプル単位の1文書にまとめたいので、トリガではなくアプリ側で組み立てて書き込む
CREATE VIRTUAL TABLE search_fts USING fts5(name, paths, tags, tokenize = 'trigram');

CREATE TABLE jobs (
  sample_id  INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
  kind       TEXT NOT NULL CHECK (kind IN ('full_hash', 'analyze', 'embed', 'peaks', 'preview_cache')),
  -- 途中で落ちても続きから再開したい。起動時に running を pending へ戻す運用にする
  state      TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'running', 'done', 'failed')),
  -- 今マップで見ているカテゴリを先に処理したい。UIの操作に応じて後から書き換える前提
  priority   INTEGER NOT NULL DEFAULT 0,
  attempts   INTEGER NOT NULL DEFAULT 0,
  error      TEXT,
  PRIMARY KEY (sample_id, kind)
) WITHOUT ROWID;
CREATE INDEX jobs_pending ON jobs(priority DESC) WHERE state = 'pending';

-- 先頭キャッシュが必要なのは、圧縮フォーマットや遅いストレージ上のファイルだけ。
-- 先頭部分を i16 PCM で1つのパックファイルに詰めて mmap し、ページキャッシュの管理はOSに任せる
CREATE TABLE preview_cache (
  sample_id    INTEGER PRIMARY KEY REFERENCES samples(id) ON DELETE CASCADE,
  pack_offset  INTEGER NOT NULL,
  frames       INTEGER NOT NULL,
  sample_rate  INTEGER NOT NULL,
  channels     INTEGER NOT NULL
);

-- リストのスクロールに波形サムネを追従させたいので、デコードせずに描ける粗いピーク列を事前に持つ。
-- 固定ビン数の (min, max) を i8 で詰め、preview_cache と同様にパックファイルへ置く
CREATE TABLE waveform_peaks (
  sample_id    INTEGER PRIMARY KEY REFERENCES samples(id) ON DELETE CASCADE,
  pack_offset  INTEGER NOT NULL,
  bins         INTEGER NOT NULL
);

-- DAWはドロップされたファイルをパスで参照し続けるので、切り出したスライスは一時ファイルにできない。
-- 同じ範囲を何度ドラッグしても同じファイルを再利用したいので、範囲で引けるようにする
CREATE TABLE slice_exports (
  sample_id    INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
  start_frame  INTEGER NOT NULL,
  end_frame    INTEGER NOT NULL,
  rel_path     TEXT NOT NULL,
  created_at   INTEGER NOT NULL DEFAULT (unixepoch()),
  PRIMARY KEY (sample_id, start_frame, end_frame)
) WITHOUT ROWID;
```

パックファイル（`preview.pack`、`peaks.pack`）とスライスの書き出し先（`slices/`）は、OS標準のアプリデータディレクトリに置く。パックファイルは追記のみで運用し、断片化の回収は当面行わない。

## バックエンド仕様

各クレートの責務と、実装時に外してほしくない判断をまとめる。ここに書いていない細部は実装側で決めてよいが、決めた内容は `DECISIONS.md` に1行ずつ残すこと（後述）。

### scan

- 対象拡張子は wav / aif / aiff / flac / mp3 / ogg。
- `quick_hash` は、ファイルサイズ（u64 LE）、先頭64KiB、末尾64KiBを連結して xxh3-64 にかける。128KiB以下のファイルは全体をかける。初回スキャンで全バイトを読まずに済ませるのが目的。
- スキャンごとに世代番号を進め、見えたファイルの `last_seen_scan` を更新する。古い世代のままのファイルは「見えていない」としてUIに出すだけで、行は削除しない。
- 新しいパスで既存の `(size, quick_hash)` が見つかったら、新しい `files` 行を既存 `samples` に紐づける。これで移動やリネームにタグが追従する。
- `full_hash` はバックグラウンドジョブで xxh3-128 を全体にかける。同じ sample に属する files の間で full\_hash が食い違ったら、sample を分割して新しい sample にタグをコピーし、解析ジョブを積み直す。
- notify でフォルダを監視する。大量コピーでイベントが溢れたら、そのディレクトリの再走査に切り替える。
- 新規 sample には full\_hash / analyze / embed / peaks のジョブを積む。preview\_cache は、root の storage が local 以外、または圧縮フォーマットのときだけ積む。

### decode

- 全フォーマットを f32 インターリーブで返す。全体のデコード、先頭N秒だけのデコード、モノラル化と指定レートへのリサンプルの3つの入口を持つ。
- symphonia の gapless を有効にする。MP3 のエンコーダ遅延が残ると、ワンショットの頭がずれて試聴の体感が悪くなるため。

### analysis

- 長さ、サンプルレート、チャンネル数はコンテナから取る。
- ラウドネスは ebur128 の統合ラウドネスを使う。0.4秒未満のファイルは統合ラウドネスが定義できないため、K特性フィルタ後のRMSで代替する。ピークはサンプルピークでよい。
- WAV の acid チャンク（テンポ、ルートノート、ワンショットフラグ）と smpl チャンク（ユニティノート、ループ点）を最初に読む。値があれば source は `chunk` にする。AIFF の Apple Loops メタデータは後回しでよい。
- ファイル名から BPM（例: `120bpm`、`_120_`）とキー（例: `Am`、`F#min`）を拾い、source を `filename` にする。誤検出しやすいので、正規表現はテストケースと一緒に育てる。
- 信号解析は、チャンクとファイル名で埋まらなかった値に対してだけ行う。BPM はループと判定したものに限る。キーは調性のある素材にはクロマ＋キープロファイル、ワンショットにはピッチ推定によるルート音を使う。
- ループ判定は、acid のワンショットフラグがあればそれに従う。なければ、BPMから見た長さが小節数として整数に近いこと、かつ末尾が無音まで減衰していないことで判定する。
- `manual` の値は再解析で上書きしない。`analyzer_ver` は定数で持ち、ロジックを変えたら上げて、古い版の結果を再解析キューに戻す。

### embed

- CLAP の音声エンコーダを ort で回す。`embeddings.model` にはモデル名とリビジョンを含む識別子を入れる。
- 前処理（モノラル化、レート、窓長、メル化）は、モデルの学習時の前処理と厳密に一致させる。数本のフィクスチャで、参照実装の出力とのコサイン類似度が 0.99 以上になることをテストで確認する。ここがずれると、検索結果がそれらしく見えるまま静かに劣化するため。
- ワンショットは窓長に満たないので、パディングの方式もテストに含める。
- テキストエンコーダも同じモデルで用意し、検索欄の自然文クエリに使う。トークナイザは tokenizers クレートで読む。
- 実行プロバイダは macOS が CoreML、Windows が DirectML。使えなければ CPU にフォールバックする。

### similarity

- 起動時に、モデルごとの全ベクトルを連続した `Vec<f32>` に載せ、L2正規化しておく。これで内積だけでコサイン類似度が出る。
- 近傍検索は全件の総当たりにする。候補集合（マップのクエリ結果など）で絞り込めるようにする。目安は10万件で10ms以内。
- マップに新しい点を足すときは、同じマップ内の既存点から埋め込み空間で近い8件を取り、2D座標を距離の逆数で重み付き平均した位置に置く。完全な重なりを避けるため、小さなジッタを加える。`placed_by` は `neighbors`。
- 全体の再計算は、ユーザーが明示的に実行したときだけ行う。結果は \[0,1\] に正規化する。そのうえで、前回の配置に Procrustes 解析（回転・反転・スケール）で合わせてから保存する。UMAP の出力は向きが任意なので、そのままだと再計算のたびにマップが裏返り、覚えた位置が無駄になるため。保存時に `layout_rev` を上げる。
- クラスタラベルは、2D座標でクラスタリングして各クラスタの最頻タグを使う。手法は任せる。

### audio

- cpal で出力デバイスのネイティブレートを使う。リサンプルはバッファを読み込む段階で済ませ、コールバックの中ではやらない。
- 制御は rtrb のコマンドキューで渡す。コールバック内ではロック、アロケーション、解放、IOをしない。不要になったバッファは別のリングでGCスレッドに返し、解放はそこで行う。
- 再生中に別の音へ切り替えるときは、約5msのイコールパワー・クロスフェードを入れて、クリックノイズを消す。
- Match LUFS が有効なら、目標ラウドネスとの差でゲインを決める。ただし、ピークが -1 dBFS を超えない範囲に抑える。
- UI は、カーソル付近の近傍IDを移動中に間引いて送ってくる。engine はその先頭部分をデコードし、容量上限つきのLRUに保持する。キャッシュにない音は、最初のブロックがデコードできた時点で鳴らし始める。
- 移調は、ワンショットについてはリサンプルによるピッチ変更（長さも変わる）でよい。サンプラーと同じ挙動になるため。
- Link 同期は、まずループの再生開始を Link の小節頭に揃えるところまで。テンポに合わせた伸縮は未決事項とする。

### engine

- 単一の writer スレッド、読み取り用のコネクション群、ジョブワーカー（コア数−1）、メモリ上のベクトルストアを持つ。writer はキューに溜まった書き込みを1トランザクションにまとめて書く。
- 起動時に `running` のジョブを `pending` に戻す。
- 表示中のマップが変わったら、そのマップに含まれる sample のジョブの priority を上げる。
- スマートコレクションとマップが共有するクエリはJSONで表す。条件の種類は、タグ（子孫タグを含む）、テキスト（FTS）、フィールド比較（duration\_ms, bpm, key\_root, lufs, is\_loop）、root、`similar_to`（sample\_id と件数）、自然文（CLAPテキスト埋め込み）。これを all / any / not で組み合わせる。
- UIへは、ジョブの進捗、sample の更新、マップ配置の変更をイベントで通知する。

### src-tauri

- コマンドは engine への中継に徹する。
- マップの点データは `tauri::ipc::Response` で1回のバイナリとして返す。中身は1点あたり固定長の little-endian レコード（id, x, y, 色インデックス, フラグ, サイズ）にし、UI側でそのまま TypedArray として読む。JSONは経由しない。
- 再生コマンドは結果を待たない投げっぱなしにする。ホバーやなぞりの経路でIPCの往復を待たないため。
- DAWへのドラッグは tauri-plugin-drag を使う。スライスは `slice_exports` を引き、未書き出しなら WAV に書き出してからそのファイルをドラッグする。

## UI仕様

マップを主役にした4ペイン構成にする。リストはマップと同じフィルタ状態を共有する、もう一つの見え方という位置づけ。

### レイアウト

- ヘッダー: Map / List の切り替え、検索バー（Ctrl/Cmd+K でフォーカス）、試聴設定（Link のオンオフとテンポ表示、Match LUFS、半音単位の移調）。
- 左サイドバー: Sources（root ごとの件数、EXT や NET の表示、オフライン状態）、Tags（階層ツリーと件数）、Collections、最下部に解析ジョブの進捗。
- 中央: マップまたはリスト。その下にトランスポートを置く。
- 右インスペクタ: 種類、名前、パス、メタデータ（長さ、ルート、BPM、LUFS、ピーク、フォーマット）、タグ編集、類似上位5件。BPM とキーはここで手修正でき、source は `manual` になる。
- テーマはダーク。書体はモックに合わせて Geist と Geist Mono。種類ごとの色は色相だけでなく明度でも見分けられるようにする。

### マップの操作

- 左ボタンを押しながらなぞると試聴する。ヒット半径内で最も近い点を鳴らし、点が変わるたびにクロスフェードで切り替える。マウスを離すと、最後に鳴っていた点が選択になる。「なぞる → 離す → トランスポートからDAWへ投げる」を途切れずにつなぐため。
- 点を単にクリックすると、選択して再生する。
- 押したままマップの外に出たら、すぐ停止する。
- Shift+ドラッグで投げ縄選択。選んだ点に対して、一括タグ付けとコレクション追加のバーを出す。
- ホイールやピンチで、カーソル位置を中心にズームする。右ドラッグ、中ドラッグ、トラックパッドの2本指スクロールでパンする。
- 鳴っている音の名前と再生位置を、カーソルの横に小さなラベルで出す。
- 選択中の点は白いリング、鳴っている点はアクセント色のリング、類似上位5件は細いリングで示す。
- フィルタに当てはまらない点は消さず、不透明度を約0.15に落とす。点の座標は全体再計算のとき以外は動かさない。`layout_rev` が変わったときだけ、新しい配置へアニメーションで移行する。空間の記憶を壊さないことがマップの価値なので、これは崩さないこと。
- 引いた状態ではクラスタラベルを出し、寄ると消す。
- 色の割り当て（種類 / キー / 長さ）と、サイズの割り当て（ラウドネス）を切り替えられるようにする。
- マップはカテゴリごとにタブで分ける。`+` でクエリから新しいマップを作れる。
- 暫定配置の点の数と「Recompute layout」ボタンを、マップ右下に出す。
- 描画は WebGL2 のインスタンス描画で行う。ヒット判定はJS側の一様グリッドで済ませ、Rust には再生コマンドと近傍IDだけを送る。10万点でも60fpsを保つこと。

### 検索

検索欄は1つにまとめる。`#tag` はタグのチップ、`~` で始まる文は自然文検索（CLAP）のチップになる。それ以外のテキストはファイル名やパスの部分一致検索（FTS）に使う。この記法は暫定なので、変えやすいように作ること。ヒットした点はマップ上で強調する。

### リスト

- 見えている行だけを描画する仮想スクロールにする。
- 列は、波形サムネ（`waveform_peaks` から描画）、名前、タグ、ルート、長さ、LUFS、BPM。
- 選択がある場合の既定の並び順は、その音との類似度順。選択がなければ名前順。
- ↑↓ で選択を移動すると、そのまま再生する。Shift+クリックで範囲選択。行をドラッグするとファイル丸ごとをDAWに渡す。

### トランスポート

- 表示するのは選択中のサンプル。いま鳴っている音（なぞり中の音）ではない。なぞっている間にここが切り替わると、スライス作業の文脈が飛ぶため。
- 横幅いっぱいの波形にスライス範囲を重ねる。範囲の端はドラッグで動かし、ゼロクロスとトランジェントに吸着させる。選択中の音が鳴っているときだけ再生位置を出す。
- ドラッグハンドルを掴むとスライスを、波形の本体を掴むとファイル丸ごとを、DAWへ渡す。

### キーボード

Space で選択中のサンプルを再生・停止、Esc で選択解除、Ctrl/Cmd+K で検索。Alt/Option、Ctrl、Cmd は、OSのドラッグ操作でコピー・移動・リンクの切り替えに使われる。そのため、ドラッグ中のアプリ独自の修飾キーには割り当てないこと。

## マイルストーン

下から順に積み上げる。各マイルストーンは受け入れ条件をすべて満たしてから次へ進むこと。UIより先にデータの土台を固めるのは、識別子やジョブの設計ミスが後から一番高くつくため。

1. **土台とスキャン**: workspace、db（マイグレーション込み）、scan、そしてUIなしで走査・検索できる開発用CLI。
   - 実ライブラリ（1万件超）を初回スキャンでき、所要時間を記録している。
   - フォルダを移動・リネームして再スキャンしても、sample id とタグが保たれる。
   - root のドライブを外して再スキャンすると、行は消えずにオフライン扱いになる。
   - ファイル名の部分一致検索が FTS trigram で引ける。
2. **デコードと解析**: decode、analysis、peaks、ジョブキュー。
   - 解析の途中でプロセスを落としても、再起動すれば続きから進む。
   - `manual` の値が再解析で上書きされない。
   - ファイル名からのBPM・キー抽出と、RIFF チャンク読みにテストケースがある。
3. **試聴とリスト**: audio、Tauri の殻、リスト画面、トランスポート、DAWへのドラッグ。
   - ↑↓ で連続して試聴しても、クリックノイズや途切れが出ない。
   - Windows と macOS の両方で、Ableton Live にファイル丸ごととスライスの両方をドロップできる。
   - ドロップしたスライスのファイルが、アプリを再起動しても残っている。
4. **類似検索**: embed、similarity、インスペクタの類似一覧、`~` による自然文検索。
   - 参照実装との埋め込みのコサイン類似度テストが通る。
   - 10万件相当のダミーベクトルで、top-k 検索のベンチが10ms以内に収まる。
5. **マップ**: WebGL2 マップ、全操作、近傍配置、全体再計算（サイドカー）と Procrustes による位置合わせ。
   - 10万点で60fpsを保つ。
   - なぞり試聴で、点に入ってから音が出るまでの遅延を計測して記録している。
   - 全体再計算をしても、マップが反転したり回転したりしない。
6. **整理と仕上げ**: タグルール、スマートコレクション、投げ縄での一括操作、Link による開始タイミングの同期、LUFS 合わせ、遅いストレージ向けの先頭キャッシュ。
   - ルールを編集して再適用すると、ルール由来のタグだけが付け直される。
   - NAS 上の圧縮ファイルでも、キャッシュ済みであれば即座に鳴る。

## 規約と進め方

### コメント

コメントには、コードを読めば分かることを書かない。書くのは、そのコードを書く前に実装者が持っていた意図、理由、制約、前提、非自明な判断である。向きは「意図 → コード」で書く。完成したコードを第三者の視点で言い換える「コード → 説明」にはしない。

```rust
// 悪い例: コードの言い換え
// クロスフェード時間を5msに設定する
const XFADE_MS: f32 = 5.0;

// 良い例: 書く前の意図
// 切り替え時のクリックは消したいが、なぞり試聴の追従感は落としたくないので5msに留める
const XFADE_MS: f32 = 5.0;
```

書くべき意図がない行には、コメントを付けない。データモデル節のSQLコメントがこの方針の実例になっている。

### Rust

- ライブラリクレートのエラーは thiserror で型を定義する。anyhow は src-tauri と開発用CLIに限る。
- `unwrap` / `expect` は、テストと「破れたらバグ」という不変条件にだけ使う。後者には、その不変条件を説明するメッセージを付ける。
- 音声コールバックは panic しない作りにする。panic すると出力ストリームごと止まり、アプリを再起動するまで音が出なくなるため。
- OS ごとの分岐は `cfg` で局所化し、両OSでビルドが通る状態を常に保つ。
- `cargo fmt` と `cargo clippy -- -D warnings` が通ること。

### テスト

- テスト用の音声は、テストの中で合成して生成する（サイン波、ノイズバースト、減衰エンベロープ、acid / smpl チャンク付きのWAV）。大きな音声ファイルはリポジトリに入れない。
- scan は一時ディレクトリを使った結合テストで、移動・リネーム・削除・root の取り外しを再現する。
- similarity の検索と audio のミキサーには criterion でベンチを書く。マイルストーンの数値条件はこのベンチで確認する。
- 両OSでしか確かめられない項目（DAWへのドロップ、CoreML / DirectML）は、手順と結果を `PROGRESS.md` に記録する。

### 進捗の残し方

作業は長期にわたり、途中でコンテキストがリセットされる前提で進める。そのため、状態は会話ではなくリポジトリに残す。

- `PROGRESS.md`: マイルストーンごとの状態、次にやること、詰まっている点。作業の区切りごとに更新する。
- `DECISIONS.md`: この実装書で決まっていない細部について下した判断を、「判断 — 理由」の1行で追記する。
- 受け入れ条件を1つ満たすごとにコミットする。

新しいセッションで再開するときは、`PROGRESS.md`、`DECISIONS.md`、`git log` で現在地を把握してから続ける。

### 判断に迷ったとき

この実装書と矛盾しない選択肢のうち、最も単純なものを選ぶ。選んだら `DECISIONS.md` に記録して、そのまま先へ進んでよい。ただし、次の2つに触れる場合は、手を止めて確認を求めること。

- 不変条件（ユーザーの音声ファイルに書き込まない、DBを唯一の正とする）に関わる変更。
- 次節の未決事項。

## 未決事項

以下は作者が決める。実装側の役目は、比較できる材料をそろえて `PROGRESS.md` に報告するところまで。それまでは「当面の扱い」で進めてよい。

| 項目 | 選択肢 | 当面の扱い |
| --- | --- | --- |
| 類似度の特徴量 | CLAP の埋め込み / 手作り特徴量（MFCC＋スペクトル統計）/ 両者の併用 | CLAP で実装する。既存の手動タグを正解ラベルにして、k近傍のタグ一致率を両方式で測り、報告する |
| UMAP の実装 | Rust のクレート / Python の umap-learn をサイドカーで呼ぶ | 小さく試作して、結果の質と導入の手間を報告する |
| ループのテンポ同期 | リサンプルによる変速（ピッチも変わる）/ タイムストレッチのライブラリ | 開始タイミングを小節頭に揃えるところまで実装する。Rubber Band は GPL か商用ライセンス、signalsmith-stretch は MIT なので、ライセンスも比較に含める |
| 試聴の目標ラウドネス | 具体的な値 | 仮に -16 LUFS とし、設定から変えられるようにする |
| 検索の記法 | `~` を接頭辞にする現案 / 別の記法 | 現案で実装し、パーサを差し替えやすくしておく |
| アプリ名 | 未定 | リポジトリ名は `sampler` のままでよい |
