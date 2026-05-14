-- Primary table of captured frames.
CREATE TABLE IF NOT EXISTS frames (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    ts                 INTEGER NOT NULL,        -- unix milliseconds (first capture of this visual)
    path               TEXT    NOT NULL,        -- absolute path on disk (original frame or video path)
    phash              INTEGER NOT NULL,        -- 64-bit perceptual hash
    app                TEXT,                    -- active process name
    window_title       TEXT,                    -- active window title
    monitor_id         INTEGER NOT NULL,
    ocr_done           INTEGER NOT NULL DEFAULT 0,
    embed_done         INTEGER NOT NULL DEFAULT 0,
    static_until_ms    INTEGER NOT NULL,        -- last tick this still matched (>= ts); dedupe extends this
    video_path         TEXT,                    -- path to archived video segment (null = still on disk as frame file)
    video_offset_ms    INTEGER                  -- ms offset into video_path where this frame lives (null if not archived)
);
CREATE INDEX IF NOT EXISTS idx_frames_ts ON frames(ts);
CREATE INDEX IF NOT EXISTS idx_frames_monitor_id ON frames(monitor_id, id);
CREATE INDEX IF NOT EXISTS idx_frames_ocr_done ON frames(ocr_done);
CREATE INDEX IF NOT EXISTS idx_frames_embed_done ON frames(embed_done);

-- FTS5 virtual table for OCR text.
-- `frame_id` is stored as an UNINDEXED column so we can filter by it.
CREATE VIRTUAL TABLE IF NOT EXISTS ocr_text USING fts5(
    text,
    frame_id UNINDEXED,
    tokenize = 'porter unicode61'
);

-- Ensure there is only one row per frame_id in ocr_text (FTS5 doesn't
-- support UNIQUE directly, so we manage it in code via ON CONFLICT pattern
-- with a shadow table).
-- For a simple v1 we use a regular table for the unique constraint:
-- (we stored the FTS row above for search; keep both in sync.)

-- Embeddings, one per frame, dense f32 vector stored as little-endian bytes.
CREATE TABLE IF NOT EXISTS embeddings (
    frame_id INTEGER PRIMARY KEY REFERENCES frames(id) ON DELETE CASCADE,
    dim      INTEGER NOT NULL,
    vector   BLOB    NOT NULL
);

-- Meta table for future migrations / arbitrary KV.
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT OR IGNORE INTO meta(key, value) VALUES ('schema_version', '2');
