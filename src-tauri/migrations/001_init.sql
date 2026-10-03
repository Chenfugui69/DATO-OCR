-- DATO COR 初始 schema（规格 07 §7.2）
--
-- 全文索引统一用 contentless FTS5 + 应用层 bigram 分词：
--   unicode61 不切中文，整段中文会被当成一个词。Rust 侧在写入前把 CJK 字符拆成
--   "单字 + 相邻二字"（见 storage/tokenize.rs），搜索词做同样处理。
--   contentless_delete=1 让我们能按 rowid 直接删索引行，不必回填原文。

-- ════════════════════════ 剪贴板 ════════════════════════
CREATE TABLE clipboard_groups (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  name       TEXT NOT NULL UNIQUE,
  color      TEXT,
  sort_order INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

CREATE TABLE clipboard_items (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  type            TEXT NOT NULL,              -- text|link|color|image|files
  content_text    TEXT,
  content_html    TEXT,
  content_rtf     TEXT,
  file_path       TEXT,                       -- 图片相对路径
  thumb_path      TEXT,
  file_list       TEXT,                       -- JSON 数组
  preview         TEXT,
  hash            TEXT NOT NULL,
  char_count      INTEGER,
  size_bytes      INTEGER,
  width           INTEGER,
  height          INTEGER,
  source_app      TEXT,
  source_app_path TEXT,
  source_icon     TEXT,                       -- app-icons 下的相对路径
  truncated       INTEGER NOT NULL DEFAULT 0,
  pinned          INTEGER NOT NULL DEFAULT 0,
  favorite        INTEGER NOT NULL DEFAULT 0,
  note            TEXT,
  group_id        INTEGER REFERENCES clipboard_groups(id) ON DELETE SET NULL,
  created_at      INTEGER NOT NULL,
  last_used_at    INTEGER NOT NULL,
  -- 第二阶段同步预留（规格 09 §6），第一版只建不用
  sync_id         TEXT,
  sync_rev        INTEGER NOT NULL DEFAULT 0,
  sync_state      INTEGER NOT NULL DEFAULT 0,
  device_id       TEXT
);

CREATE INDEX idx_clip_order  ON clipboard_items(pinned DESC, last_used_at DESC, id DESC);
CREATE INDEX idx_clip_hash   ON clipboard_items(hash);
CREATE INDEX idx_clip_type   ON clipboard_items(type, last_used_at DESC);
CREATE INDEX idx_clip_group  ON clipboard_items(group_id, last_used_at DESC);
CREATE INDEX idx_clip_fav    ON clipboard_items(last_used_at DESC) WHERE favorite = 1;
CREATE INDEX idx_clip_sync   ON clipboard_items(sync_state) WHERE sync_state = 1;

CREATE VIRTUAL TABLE clipboard_fts USING fts5(
  tokens,
  content = '',
  contentless_delete = 1,
  tokenize = 'unicode61 remove_diacritics 2'
);

-- ════════════════════════ 截图库 ════════════════════════
CREATE TABLE screenshots (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  file_path       TEXT NOT NULL,
  thumb_path      TEXT,
  width           INTEGER NOT NULL,
  height          INTEGER NOT NULL,
  size_bytes      INTEGER NOT NULL,
  kind            TEXT NOT NULL,              -- normal|longshot
  source_app      TEXT,
  ocr_text        TEXT,
  has_annotations INTEGER NOT NULL DEFAULT 0,
  favorite        INTEGER NOT NULL DEFAULT 0,
  note            TEXT,
  created_at      INTEGER NOT NULL
);
CREATE INDEX idx_shot_created ON screenshots(created_at DESC, id DESC);

CREATE VIRTUAL TABLE screenshots_fts USING fts5(
  tokens,
  content = '',
  contentless_delete = 1,
  tokenize = 'unicode61 remove_diacritics 2'
);

-- ════════════════════════ 识字记录 ════════════════════════
CREATE TABLE ocr_records (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  screenshot_id INTEGER REFERENCES screenshots(id) ON DELETE SET NULL,
  image_path    TEXT,
  width         INTEGER,
  height        INTEGER,
  engine        TEXT NOT NULL,
  raw_blocks    TEXT NOT NULL,
  plain_text    TEXT NOT NULL,
  lang          TEXT,
  elapsed_ms    INTEGER,
  created_at    INTEGER NOT NULL
);
CREATE INDEX idx_ocr_created ON ocr_records(created_at DESC, id DESC);

CREATE VIRTUAL TABLE ocr_fts USING fts5(
  tokens,
  content = '',
  contentless_delete = 1,
  tokenize = 'unicode61 remove_diacritics 2'
);

-- ════════════════════════ 密钥（DPAPI 密文） ════════════════════════
CREATE TABLE secrets (
  key        TEXT PRIMARY KEY,
  cipher     BLOB NOT NULL,
  updated_at INTEGER NOT NULL
);

-- ════════════════════════ 杂项键值 ════════════════════════
CREATE TABLE kv (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
