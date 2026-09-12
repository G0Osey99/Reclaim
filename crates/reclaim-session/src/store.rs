//! SQLite result store (docs/plan/03 §2.5). One session = one `session.sqlite`
//! with `sources`, `scan_runs`, `entries` (Phase 2+), `carved`, `fragments`
//! (round 2), `proposed_volumes` (Phase 4), `progress` and `events` tables.
//! Deterministic ids make INSERT OR IGNORE idempotent across resume.

use crate::id::result_id;
use crate::SessionError;
use reclaim_carve::{CarvedFile, Metadata, Validity};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::Path;

/// A stored carved result.
#[derive(Clone, Debug, Serialize)]
pub struct CarvedRecord {
    /// Deterministic id.
    pub id: String,
    /// Source identity.
    pub source_id: String,
    /// Producing engine.
    pub engine: String,
    /// Absolute byte offset.
    pub offset: u64,
    /// Length in bytes.
    pub len: u64,
    /// Refined format id.
    pub format: String,
    /// Family.
    pub family: String,
    /// Extension.
    pub ext: String,
    /// `full` / `truncated` / `suspect`.
    pub validity: String,
    /// Confidence.
    pub score: u8,
    /// Header sat on a block boundary.
    pub block_aligned: bool,
    /// Embedded name.
    pub name: Option<String>,
    /// Embedded date (YYYY-MM-DD).
    pub date: Option<String>,
    /// Camera/device model.
    pub model: Option<String>,
    /// Embedded thumbnail offset.
    pub thumb_offset: Option<u64>,
    /// Embedded thumbnail length.
    pub thumb_len: Option<u64>,
}

impl CarvedRecord {
    /// Synthesize the recovery-relative path (docs/plan/05 §6).
    #[must_use]
    pub fn synth_path(&self) -> String {
        let meta = Metadata {
            name: self.name.clone(),
            date: self.date.clone(),
            model: self.model.clone(),
            thumb_offset: self.thumb_offset,
            thumb_len: self.thumb_len,
        };
        let subtype = self.format.rsplit('.').next().unwrap_or(&self.format);
        meta.suggested_path(&self.family, subtype, self.offset, &self.ext)
    }
}

/// Sort key for queries.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Sort {
    /// By offset (default; deterministic).
    Offset,
    /// By size, descending.
    Size,
    /// By embedded date.
    Date,
    /// By score, descending.
    Score,
    /// By synthesized path.
    Path,
}

impl Sort {
    /// Parse a `--sort` argument.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "offset" => Some(Sort::Offset),
            "size" => Some(Sort::Size),
            "date" => Some(Sort::Date),
            "score" => Some(Sort::Score),
            "path" => Some(Sort::Path),
            _ => None,
        }
    }
}

/// Result query filter (docs/plan/07 §2 `results`).
#[derive(Clone, Debug, Default)]
pub struct QueryFilter {
    /// Restrict to a family.
    pub family: Option<String>,
    /// Restrict to these extensions.
    pub exts: Vec<String>,
    /// Minimum size.
    pub min_size: Option<u64>,
    /// Embedded date on/after (YYYY-MM-DD).
    pub after: Option<String>,
    /// Minimum score.
    pub min_score: Option<u8>,
    /// Restrict to an engine.
    pub engine: Option<String>,
    /// Only `full` results.
    pub full_only: bool,
    /// Glob on the synthesized path (applied in Rust).
    pub path_glob: Option<String>,
    /// Explicit id set (recover --ids).
    pub ids: Option<Vec<String>>,
    /// Sort key.
    pub sort: Option<Sort>,
    /// Row limit.
    pub limit: Option<usize>,
}

/// The session's SQLite store.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating the schema if needed) the database at `path`.
    pub fn open(path: &Path) -> Result<Self, SessionError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let store = Store { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), SessionError> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sources (
                id TEXT PRIMARY KEY, size INTEGER, sector_size INTEGER,
                first_mib_hash TEXT, last_mib_hash TEXT, added_at TEXT
            );
            CREATE TABLE IF NOT EXISTS scan_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT, started_at TEXT,
                engines TEXT, options TEXT, status TEXT
            );
            CREATE TABLE IF NOT EXISTS entries (
                id TEXT PRIMARY KEY, source_id TEXT, path TEXT, kind TEXT,
                size INTEGER, state TEXT, confidence INTEGER
            );
            CREATE TABLE IF NOT EXISTS carved (
                id TEXT PRIMARY KEY, source_id TEXT, engine TEXT,
                offset INTEGER, len INTEGER, format TEXT, family TEXT, ext TEXT,
                validity TEXT, score INTEGER, block_aligned INTEGER,
                name TEXT, date TEXT, model TEXT, thumb_offset INTEGER, thumb_len INTEGER
            );
            CREATE INDEX IF NOT EXISTS carved_family ON carved(family);
            CREATE INDEX IF NOT EXISTS carved_offset ON carved(offset);
            CREATE TABLE IF NOT EXISTS fragments (
                result_id TEXT, seq INTEGER, offset INTEGER, len INTEGER, confidence INTEGER
            );
            CREATE TABLE IF NOT EXISTS proposed_volumes (
                id INTEGER PRIMARY KEY AUTOINCREMENT, start INTEGER, len INTEGER,
                fs TEXT, confidence REAL, evidence TEXT
            );
            CREATE TABLE IF NOT EXISTS progress (
                engine TEXT PRIMARY KEY, cursor INTEGER, scanned INTEGER,
                total INTEGER, found INTEGER, complete INTEGER, block_size INTEGER,
                updated_at TEXT
            );
            CREATE TABLE IF NOT EXISTS events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT, kind TEXT, json TEXT
            );
            "#,
        )?;
        Ok(())
    }

    /// Record the source identity.
    pub fn put_source(&self, id: &str, size: u64, sector_size: u32) -> Result<(), SessionError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO sources(id,size,sector_size,added_at) VALUES(?1,?2,?3,datetime('now'))",
            params![id, size as i64, sector_size],
        )?;
        Ok(())
    }

    /// Insert a batch of carved results in one transaction (INSERT OR IGNORE
    /// on the deterministic id). Returns the ids inserted-or-present.
    pub fn insert_carved(
        &mut self,
        source_id: &str,
        engine: &str,
        batch: &[CarvedFile],
    ) -> Result<Vec<String>, SessionError> {
        let tx = self.conn.transaction()?;
        let mut ids = Vec::with_capacity(batch.len());
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO carved(id,source_id,engine,offset,len,format,family,ext,validity,score,block_aligned,name,date,model,thumb_offset,thumb_len) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            )?;
            for cf in batch {
                let id = result_id(source_id, engine, cf.offset, cf.len);
                stmt.execute(params![
                    id,
                    source_id,
                    engine,
                    cf.offset as i64,
                    cf.len as i64,
                    cf.format,
                    cf.family,
                    cf.ext,
                    validity_str(cf.validity),
                    cf.score,
                    cf.block_aligned as i64,
                    cf.meta.name,
                    cf.meta.date,
                    cf.meta.model,
                    cf.meta.thumb_offset.map(|v| v as i64),
                    cf.meta.thumb_len.map(|v| v as i64),
                ])?;
                ids.push(id);
            }
        }
        tx.commit()?;
        Ok(ids)
    }

    /// Save engine progress (checkpoint).
    #[allow(clippy::too_many_arguments)]
    pub fn put_progress(
        &self,
        engine: &str,
        cursor: u64,
        scanned: u64,
        total: u64,
        found: u64,
        complete: bool,
        block_size: u32,
    ) -> Result<(), SessionError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO progress(engine,cursor,scanned,total,found,complete,block_size,updated_at) \
             VALUES(?1,?2,?3,?4,?5,?6,?7,datetime('now'))",
            params![engine, cursor as i64, scanned as i64, total as i64, found as i64, complete as i64, block_size],
        )?;
        Ok(())
    }

    /// Read engine progress: (cursor, complete, block_size) if present.
    pub fn get_progress(&self, engine: &str) -> Result<Option<(u64, bool, u32)>, SessionError> {
        let mut stmt = self
            .conn
            .prepare("SELECT cursor, complete, block_size FROM progress WHERE engine=?1")?;
        let mut rows = stmt.query(params![engine])?;
        if let Some(r) = rows.next()? {
            let cursor: i64 = r.get(0)?;
            let complete: i64 = r.get(1)?;
            let bs: i64 = r.get(2)?;
            Ok(Some((cursor as u64, complete != 0, bs as u32)))
        } else {
            Ok(None)
        }
    }

    /// Append an event row.
    pub fn put_event(&self, kind: &str, json: &str) -> Result<(), SessionError> {
        self.conn.execute(
            "INSERT INTO events(ts,kind,json) VALUES(datetime('now'),?1,?2)",
            params![kind, json],
        )?;
        Ok(())
    }

    /// Total carved rows.
    pub fn count(&self) -> Result<u64, SessionError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM carved", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    /// Fetch one carved result by id.
    pub fn get(&self, id: &str) -> Result<Option<CarvedRecord>, SessionError> {
        let mut stmt = self.conn.prepare(&format!("{SELECT_CARVED} WHERE id=?1"))?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(r) => Ok(Some(row_to_record(r)?)),
            None => Ok(None),
        }
    }

    /// Query carved results with a filter.
    pub fn query(&self, f: &QueryFilter) -> Result<Vec<CarvedRecord>, SessionError> {
        let mut sql = String::from(SELECT_CARVED);
        let mut clauses: Vec<String> = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(fam) = &f.family {
            clauses.push(format!("family=?{}", args.len() + 1));
            args.push(Box::new(fam.clone()));
        }
        if let Some(ms) = f.min_size {
            clauses.push(format!("len>=?{}", args.len() + 1));
            args.push(Box::new(ms as i64));
        }
        if let Some(sc) = f.min_score {
            clauses.push(format!("score>=?{}", args.len() + 1));
            args.push(Box::new(sc as i64));
        }
        if let Some(eng) = &f.engine {
            clauses.push(format!("engine=?{}", args.len() + 1));
            args.push(Box::new(eng.clone()));
        }
        if let Some(after) = &f.after {
            clauses.push(format!("date>=?{}", args.len() + 1));
            args.push(Box::new(after.clone()));
        }
        if f.full_only {
            clauses.push("validity='full'".to_string());
        }
        if !f.exts.is_empty() {
            let ph: Vec<String> = f
                .exts
                .iter()
                .map(|e| {
                    args.push(Box::new(e.clone()));
                    format!("?{}", args.len())
                })
                .collect();
            clauses.push(format!("ext IN ({})", ph.join(",")));
        }
        if let Some(ids) = &f.ids {
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            let ph: Vec<String> = ids
                .iter()
                .map(|i| {
                    args.push(Box::new(i.clone()));
                    format!("?{}", args.len())
                })
                .collect();
            clauses.push(format!("id IN ({})", ph.join(",")));
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(match f.sort.unwrap_or(Sort::Offset) {
            Sort::Offset => " ORDER BY offset ASC",
            Sort::Size => " ORDER BY len DESC",
            Sort::Date => " ORDER BY date ASC",
            Sort::Score => " ORDER BY score DESC",
            Sort::Path => " ORDER BY family ASC, date ASC, offset ASC",
        });

        let mut stmt = self.conn.prepare(&sql)?;
        let arg_refs: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let mut rows = stmt.query(arg_refs.as_slice())?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            let rec = row_to_record(r)?;
            if let Some(glob) = &f.path_glob {
                if !glob_match(glob, &rec.synth_path()) {
                    continue;
                }
            }
            out.push(rec);
            if let Some(lim) = f.limit {
                if out.len() >= lim {
                    break;
                }
            }
        }
        Ok(out)
    }
}

const SELECT_CARVED: &str = "SELECT id,source_id,engine,offset,len,format,family,ext,validity,score,block_aligned,name,date,model,thumb_offset,thumb_len FROM carved";

fn row_to_record(r: &rusqlite::Row) -> Result<CarvedRecord, SessionError> {
    let off: i64 = r.get(3)?;
    let len: i64 = r.get(4)?;
    let ba: i64 = r.get(10)?;
    let thumb_off: Option<i64> = r.get(14)?;
    let thumb_len: Option<i64> = r.get(15)?;
    Ok(CarvedRecord {
        id: r.get(0)?,
        source_id: r.get(1)?,
        engine: r.get(2)?,
        offset: off as u64,
        len: len as u64,
        format: r.get(5)?,
        family: r.get(6)?,
        ext: r.get(7)?,
        validity: r.get(8)?,
        score: r.get::<_, i64>(9)? as u8,
        block_aligned: ba != 0,
        name: r.get(11)?,
        date: r.get(12)?,
        model: r.get(13)?,
        thumb_offset: thumb_off.map(|v| v as u64),
        thumb_len: thumb_len.map(|v| v as u64),
    })
}

fn validity_str(v: Validity) -> &'static str {
    v.label()
}

/// Minimal glob: supports `*` (any run) and `?` (one char), case-sensitive.
fn glob_match(pat: &str, s: &str) -> bool {
    fn m(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) => m(&p[1..], s) || (!s.is_empty() && m(p, &s[1..])),
            (Some(b'?'), Some(_)) => m(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a == b => m(&p[1..], &s[1..]),
            _ => false,
        }
    }
    m(pat.as_bytes(), s.as_bytes())
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use reclaim_carve::Validity;

    fn cf(off: u64, len: u64, fam: &str, ext: &str, val: Validity, score: u8) -> CarvedFile {
        CarvedFile {
            offset: off,
            len,
            format: format!("{fam}.{ext}"),
            family: fam.into(),
            ext: ext.into(),
            validity: val,
            score,
            meta: Metadata::default(),
            block_aligned: true,
        }
    }

    #[test]
    fn insert_query_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("s.sqlite")).unwrap();
        let batch = vec![
            cf(4096, 1000, "image", "png", Validity::Full, 97),
            cf(8192, 500, "doc", "txt", Validity::Truncated, 60),
        ];
        s.insert_carved("src", "carve", &batch).unwrap();
        // Re-insert identical batch → idempotent.
        s.insert_carved("src", "carve", &batch).unwrap();
        assert_eq!(s.count().unwrap(), 2);

        let imgs = s
            .query(&QueryFilter {
                family: Some("image".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(imgs.len(), 1);
        assert_eq!(imgs[0].offset, 4096);

        let full = s
            .query(&QueryFilter {
                full_only: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(full.len(), 1);
    }

    #[test]
    fn glob() {
        assert!(glob_match("image/*", "image/2026/x.jpg"));
        assert!(glob_match("*.jpg", "a/b/c.jpg"));
        assert!(!glob_match("video/*", "image/x.jpg"));
    }
}
