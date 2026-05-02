//! Thin wrapper around a `rusqlite` connection protected by a `Mutex`.
//!
//! SQLite handles concurrent reads/writes fine for our workload since
//! everything goes through a single writer (the capture / indexer tasks)
//! and short read-only queries from commands.

use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

const SCHEMA: &str = include_str!("schema.sql");

#[derive(Clone, Debug, Serialize)]
pub struct Frame {
    pub id: i64,
    pub ts: i64, // unix ms
    pub path: String,
    pub app: Option<String>,
    pub window_title: Option<String>,
    pub monitor_id: i32,
    pub ocr_done: bool,
    pub embed_done: bool,
    /// `true` when a row exists in `embeddings` (actual vector stored).
    pub has_embedding: bool,
    /// Last time (unix ms) this still matched; equals `ts` if never deduped. Held ≈ `static_until_ms - ts`.
    pub static_until_ms: i64,
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path).context("open sqlite")?;
        // Good defaults for a local indexing workload.
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA temp_store=MEMORY;
             -- Embedding blobs make the DB file huge;SQLite's default mmap can map GBs → RSS blows up.
             PRAGMA mmap_size=67108864;
             -- Page cache (~8 MiB) — enough for indexer/hot-path without holding entire DB RAM.
             PRAGMA cache_size=-8192;",
        )?;
        conn.execute_batch(SCHEMA)?;
        migrate_frames_static_until(&conn)?;
        backfill_embed_done_for_empty_ocr(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn insert_frame(
        &self,
        ts: i64,
        path: &str,
        phash: u64,
        app: Option<&str>,
        window_title: Option<&str>,
        monitor_id: i32,
    ) -> Result<i64> {
        let guard = self.conn.lock().unwrap();
        guard.execute(
            "INSERT INTO frames(ts, path, phash, app, window_title, monitor_id, ocr_done, embed_done, static_until_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, ?7)",
            params![ts, path, phash as i64, app, window_title, monitor_id, ts],
        )?;
        Ok(guard.last_insert_rowid())
    }

    /// Latest stored frame for a monitor: `(id, phash)`.
    pub fn last_frame_fingerprint(
        &self,
        monitor_id: i32,
    ) -> Result<Option<(i64, u64)>> {
        let guard = self.conn.lock().unwrap();
        let row = guard
            .query_row(
                "SELECT id, phash FROM frames WHERE monitor_id = ?1 ORDER BY id DESC LIMIT 1",
                params![monitor_id],
                |row| {
                    let id: i64 = row.get(0)?;
                    let p: i64 = row.get(1)?;
                    Ok((id, p as u64))
                },
            )
            .optional()?;
        Ok(row)
    }

    /// When a new capture is deduped (same dHash as last), extend how long the prior frame stayed.
    pub fn extend_frame_static_until(&self, frame_id: i64, until_ms: i64) -> Result<()> {
        let guard = self.conn.lock().unwrap();
        guard.execute(
            "UPDATE frames SET static_until_ms = MAX(COALESCE(static_until_ms, ts), ?1) WHERE id = ?2",
            params![until_ms, frame_id],
        )?;
        Ok(())
    }

    pub fn list_frames(&self, limit: i64, before_ts: Option<i64>) -> Result<Vec<Frame>> {
        let guard = self.conn.lock().unwrap();
        let mut stmt = guard.prepare(
            "SELECT f.id, f.ts, f.path, f.app, f.window_title, f.monitor_id, f.ocr_done, f.embed_done,
                    COALESCE(f.static_until_ms, f.ts) AS static_until_ms,
                    EXISTS(SELECT 1 FROM embeddings e WHERE e.frame_id = f.id) AS has_embedding
             FROM frames f
             WHERE (?1 IS NULL OR f.ts < ?1)
             ORDER BY f.ts DESC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![before_ts, limit], row_to_frame)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_frame(&self, id: i64) -> Result<Option<Frame>> {
        let guard = self.conn.lock().unwrap();
        let f = guard
            .query_row(
                "SELECT f.id, f.ts, f.path, f.app, f.window_title, f.monitor_id, f.ocr_done, f.embed_done,
                        COALESCE(f.static_until_ms, f.ts) AS static_until_ms,
                        EXISTS(SELECT 1 FROM embeddings e WHERE e.frame_id = f.id) AS has_embedding
                 FROM frames f WHERE f.id = ?1",
                params![id],
                row_to_frame,
            )
            .optional()?;
        Ok(f)
    }

    pub fn set_ocr_text(&self, frame_id: i64, text: &str) -> Result<()> {
        let guard = self.conn.lock().unwrap();
        // FTS5 virtual tables don't support ON CONFLICT, so delete+insert.
        guard.execute(
            "DELETE FROM ocr_text WHERE frame_id = ?1",
            params![frame_id],
        )?;
        guard.execute(
            "INSERT INTO ocr_text(text, frame_id) VALUES (?1, ?2)",
            params![text, frame_id],
        )?;
        guard.execute(
            "UPDATE frames SET ocr_done = 1 WHERE id = ?1",
            params![frame_id],
        )?;
        Ok(())
    }

    pub fn get_ocr_text(&self, frame_id: i64) -> Result<Option<String>> {
        let guard = self.conn.lock().unwrap();
        // Read raw UTF-8 bytes: rare invalid sequences from Tesseract/FTS would make `get::<String>` fail;
        // lossy decode always returns a string for the UI/IPC.
        let t: Option<String> = guard
            .query_row(
                "SELECT text FROM ocr_text WHERE frame_id = ?1",
                params![frame_id],
                |row| {
                    let b = row.get_ref(0)?.as_bytes()?;
                    let s = String::from_utf8_lossy(b).replace('\0', "");
                    Ok(s)
                },
            )
            .optional()?;
        Ok(t)
    }

    /// Read one frame embedding (if present). Returns `(dim, preview_values)` where
    /// `preview_values` is capped at `max_values` for UI inspection.
    pub fn get_embedding_preview(
        &self,
        frame_id: i64,
        max_values: usize,
    ) -> Result<Option<(usize, Vec<f32>)>> {
        use byteorder::{LittleEndian, ReadBytesExt};
        use std::io::Cursor;

        let guard = self.conn.lock().unwrap();
        let row: Option<(i64, Vec<u8>)> = guard
            .query_row(
                "SELECT dim, vector FROM embeddings WHERE frame_id = ?1",
                params![frame_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((dim_i64, blob)) = row else {
            return Ok(None);
        };
        let dim = dim_i64.max(0) as usize;
        let take = dim.min(max_values.max(1));
        let mut cur = Cursor::new(&blob);
        let mut vals = Vec::with_capacity(take);
        for _ in 0..take {
            vals.push(cur.read_f32::<LittleEndian>().unwrap_or(0.0));
        }
        Ok(Some((dim, vals)))
    }

    /// Full-text search on OCR text. Returns (frame, snippet, score) tuples.
    pub fn fts_search(&self, query: &str, limit: i64) -> Result<Vec<(Frame, String, f32)>> {
        let guard = self.conn.lock().unwrap();
        let mut stmt = guard.prepare(
            "SELECT f.id, f.ts, f.path, f.app, f.window_title, f.monitor_id, f.ocr_done, f.embed_done,
                    COALESCE(f.static_until_ms, f.ts) AS s_until,
                    EXISTS(SELECT 1 FROM embeddings e WHERE e.frame_id = f.id) AS has_embedding,
                    snippet(ocr_text, 0, '<<', '>>', '…', 16) AS snip,
                    bm25(ocr_text) AS score
             FROM ocr_text
             JOIN frames f ON f.id = ocr_text.frame_id
             WHERE ocr_text.text MATCH ?1
             ORDER BY score ASC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![query, limit], |row| {
                let static_until_ms: i64 = row.get(8)?;
                let has_embedding: i32 = row.get(9)?;
                let frame = Frame {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    path: row.get(2)?,
                    app: row.get(3)?,
                    window_title: row.get(4)?,
                    monitor_id: row.get(5)?,
                    ocr_done: row.get::<_, i32>(6)? != 0,
                    embed_done: row.get::<_, i32>(7)? != 0,
                    has_embedding: has_embedding != 0,
                    static_until_ms,
                };
                let snippet: String = row.get(10)?;
                // bm25 returns negative-ish scores where lower is better; map to [0,1].
                let raw: f64 = row.get(11)?;
                let score = 1.0 / (1.0 + (raw.abs() as f32));
                Ok((frame, snippet, score))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_embedding(&self, frame_id: i64, vec: &[f32]) -> Result<()> {
        use byteorder::{LittleEndian, WriteBytesExt};
        let mut buf = Vec::with_capacity(vec.len() * 4);
        for v in vec {
            buf.write_f32::<LittleEndian>(*v)?;
        }
        let guard = self.conn.lock().unwrap();
        guard.execute(
            "INSERT INTO embeddings(frame_id, dim, vector) VALUES (?1, ?2, ?3)
             ON CONFLICT(frame_id) DO UPDATE SET dim = excluded.dim, vector = excluded.vector",
            params![frame_id, vec.len() as i64, buf],
        )?;
        guard.execute(
            "UPDATE frames SET embed_done = 1 WHERE id = ?1",
            params![frame_id],
        )?;
        Ok(())
    }

    /// Walk every embedding and compute cosine similarity with `query`.
    /// Returns top-`limit` frames with scores.
    pub fn semantic_search(
        &self,
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<(Frame, f32)>> {
        use byteorder::{LittleEndian, ReadBytesExt};
        use std::io::Cursor;

        let q_norm = norm(query);
        if q_norm == 0.0 {
            return Ok(vec![]);
        }

        let guard = self.conn.lock().unwrap();
        let mut stmt = guard.prepare(
            "SELECT f.id, f.ts, f.path, f.app, f.window_title, f.monitor_id, f.ocr_done, f.embed_done,
                    COALESCE(f.static_until_ms, f.ts) AS s_until, e.dim, e.vector
             FROM embeddings e
             JOIN frames f ON f.id = e.frame_id",
        )?;
        let rows = stmt.query_map([], |row| {
            let static_until_ms: i64 = row.get(8)?;
            let frame = Frame {
                id: row.get(0)?,
                ts: row.get(1)?,
                path: row.get(2)?,
                app: row.get(3)?,
                window_title: row.get(4)?,
                monitor_id: row.get(5)?,
                ocr_done: row.get::<_, i32>(6)? != 0,
                embed_done: row.get::<_, i32>(7)? != 0,
                has_embedding: true,
                static_until_ms,
            };
            let dim: i64 = row.get(9)?;
            let blob: Vec<u8> = row.get(10)?;
            Ok((frame, dim, blob))
        })?;

        let mut scored: Vec<(Frame, f32)> = Vec::new();
        for row in rows {
            let (frame, dim, blob) = row?;
            if dim as usize != query.len() {
                continue;
            }
            let mut v = Vec::with_capacity(query.len());
            let mut cur = Cursor::new(&blob);
            let mut dot = 0f32;
            let mut vn = 0f32;
            for i in 0..query.len() {
                let x = cur.read_f32::<LittleEndian>().unwrap_or(0.0);
                v.push(x);
                dot += x * query[i];
                vn += x * x;
            }
            let denom = q_norm * vn.sqrt();
            if denom == 0.0 {
                continue;
            }
            scored.push((frame, dot / denom));
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        Ok(scored)
    }

    pub fn pending_ocr(&self, limit: i64) -> Result<Vec<i64>> {
        let guard = self.conn.lock().unwrap();
        let mut stmt =
            guard.prepare("SELECT id FROM frames WHERE ocr_done = 0 ORDER BY id ASC LIMIT ?1")?;
        let rows = stmt
            .query_map(params![limit], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn pending_embed(&self, limit: i64) -> Result<Vec<i64>> {
        let guard = self.conn.lock().unwrap();
        // Must include rows where OCR is done but text is empty; `embed::process_one` calls
        // `set_embed_done_skipped` in that case. (If we filter empty text out, those frames
        // never get `embed_done` set after a race where embed ran before OCR finished.)
        let mut stmt = guard.prepare(
            "SELECT id FROM frames
             WHERE ocr_done = 1 AND embed_done = 0
             ORDER BY id ASC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Mark embedding as complete for this frame without storing a vector (OCR was empty / no text
    /// to embed). Keeps the embed worker from waiting forever on `embed_done = 0`.
    pub fn set_embed_done_skipped(&self, frame_id: i64) -> Result<()> {
        let guard = self.conn.lock().unwrap();
        guard.execute(
            "UPDATE frames SET embed_done = 1 WHERE id = ?1",
            params![frame_id],
        )?;
        Ok(())
    }

    /// Resets frames that never got useful OCR (pending OCR, or finished with empty / missing
    /// text) so the OCR worker can run again. Returns affected frame ids.
    pub fn requeue_ocr_rerun(&self) -> Result<Vec<i64>> {
        let guard = self.conn.lock().unwrap();
        let mut stmt = guard.prepare(
            "SELECT id FROM frames
             WHERE ocr_done = 0
                OR (
                  ocr_done = 1
                  AND length(trim(COALESCE((SELECT text FROM ocr_text WHERE frame_id = frames.id), ''))) = 0
                )
             ORDER BY id ASC",
        )?;
        let ids: Vec<i64> = stmt
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        for id in &ids {
            guard.execute("DELETE FROM embeddings WHERE frame_id = ?1", params![id])?;
            guard.execute("DELETE FROM ocr_text WHERE frame_id = ?1", params![id])?;
            guard.execute(
                "UPDATE frames SET ocr_done = 0, embed_done = 0 WHERE id = ?1",
                params![id],
            )?;
        }
        Ok(ids)
    }

    /// Requeue embeddings for frames that already have non-empty OCR text but no stored vector.
    /// Useful after model/server issues or accidental `embed_done = 1` without vectors.
    pub fn requeue_embed_rerun(&self) -> Result<Vec<i64>> {
        let guard = self.conn.lock().unwrap();
        let mut stmt = guard.prepare(
            "SELECT id FROM frames
             WHERE ocr_done = 1
               AND length(trim(COALESCE((SELECT text FROM ocr_text WHERE frame_id = frames.id), ''))) > 0
               AND id NOT IN (SELECT frame_id FROM embeddings)
             ORDER BY id ASC",
        )?;
        let ids: Vec<i64> = stmt
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        for id in &ids {
            guard.execute("UPDATE frames SET embed_done = 0 WHERE id = ?1", params![id])?;
        }
        Ok(ids)
    }

    /// Count of frames still waiting for OCR (`ocr_done = 0`).
    pub fn count_pending_ocr(&self) -> Result<i64> {
        let guard = self.conn.lock().unwrap();
        let n: i64 = guard.query_row(
            "SELECT COUNT(*) FROM frames WHERE ocr_done = 0",
            [],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    /// Count of frames with OCR done but embed not done (includes empty-OCR "skip" path when running).
    pub fn count_pending_embed(&self) -> Result<i64> {
        let guard = self.conn.lock().unwrap();
        let n: i64 = guard.query_row(
            "SELECT COUNT(*) FROM frames WHERE ocr_done = 1 AND embed_done = 0",
            [],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    pub fn stats(&self) -> Result<(i64, i64)> {
        let guard = self.conn.lock().unwrap();
        let frames: i64 =
            guard.query_row("SELECT COUNT(*) FROM frames", [], |r| r.get(0))?;
        let indexed: i64 = guard.query_row(
            "SELECT COUNT(*) FROM frames WHERE embed_done = 1",
            [],
            |r| r.get(0),
        )?;
        Ok((frames, indexed))
    }

    pub fn delete_all(&self) -> Result<()> {
        let guard = self.conn.lock().unwrap();
        guard.execute_batch(
            "DELETE FROM embeddings; DELETE FROM ocr_text; DELETE FROM frames;",
        )?;
        Ok(())
    }
}

fn row_to_frame(row: &rusqlite::Row<'_>) -> rusqlite::Result<Frame> {
    Ok(Frame {
        id: row.get(0)?,
        ts: row.get(1)?,
        path: row.get(2)?,
        app: row.get(3)?,
        window_title: row.get(4)?,
        monitor_id: row.get(5)?,
        ocr_done: row.get::<_, i32>(6)? != 0,
        embed_done: row.get::<_, i32>(7)? != 0,
        static_until_ms: row.get(8)?,
        has_embedding: row.get::<_, i32>(9)? != 0,
    })
}

fn backfill_embed_done_for_empty_ocr(conn: &Connection) -> Result<()> {
    // Frames where OCR produced nothing (or missing row) do not get embeddings; mark embed pipeline done.
    conn.execute(
        "UPDATE frames SET embed_done = 1
         WHERE ocr_done = 1
           AND embed_done = 0
           AND id NOT IN (SELECT frame_id FROM embeddings)
           AND length(trim(COALESCE((SELECT text FROM ocr_text WHERE frame_id = frames.id), ''))) = 0",
        [],
    )?;
    Ok(())
}

fn migrate_frames_static_until(conn: &Connection) -> Result<()> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('frames') WHERE name = 'static_until_ms'",
        [],
        |r| r.get(0),
    )?;
    if n == 0 {
        conn.execute("ALTER TABLE frames ADD COLUMN static_until_ms INTEGER", [])
            .context("add static_until_ms to frames")?;
        conn.execute(
            "UPDATE frames SET static_until_ms = ts WHERE static_until_ms IS NULL",
            [],
        )
        .context("backfill static_until_ms")?;
    }
    Ok(())
}

fn norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}
