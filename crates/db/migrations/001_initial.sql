-- UIの読み取りとスキャナ・ジョブの書き込みを同時に走らせたいので WAL にする



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
