//! SQLite archive: schema, WAL, upserts, FTS5 search, sync state.
//!
//! Schema ported from langkurt `storage/db.go` (MIT, `.tmp/`) + jackwener
//! `db.py` (Apache-2.0). Verified in plan §6.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

/// Open the database at `path`, apply schema, return the connection.
/// WAL + foreign_keys; single-writer semantics.
pub fn open(path: &str) -> Result<Connection> {
    let conn = Connection::open(path).context("open sqlite")?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .context("pragmas")?;
    migrate(&conn)?;
    Ok(conn)
}

/// Apply schema migrations (idempotent).
fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS guilds (
            id TEXT PRIMARY KEY, name TEXT NOT NULL, icon TEXT
        );
        CREATE TABLE IF NOT EXISTS channels (
            id TEXT PRIMARY KEY, guild_id TEXT REFERENCES guilds(id),
            name TEXT NOT NULL, type INTEGER NOT NULL DEFAULT 0,
            topic TEXT, parent_id TEXT
        );
        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            channel_id TEXT NOT NULL REFERENCES channels(id),
            guild_id TEXT,
            author_id TEXT NOT NULL, author_name TEXT NOT NULL,
            content TEXT NOT NULL,
            timestamp DATETIME NOT NULL,
            edited INTEGER NOT NULL DEFAULT 0,
            reaction_count INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_messages_channel ON messages(channel_id);
        CREATE INDEX IF NOT EXISTS idx_messages_timestamp ON messages(timestamp DESC);
        CREATE INDEX IF NOT EXISTS idx_messages_author ON messages(author_id);
        CREATE INDEX IF NOT EXISTS idx_messages_reactions ON messages(reaction_count DESC);

        -- F6: downloaded-attachment ledger (langkurt attachments.go).
        -- FK ON DELETE CASCADE: purging a channel removes its attachments
        -- rows (review#1: REPLACE + FK broke re-sync; CASCADE keeps purge safe).
        CREATE TABLE IF NOT EXISTS attachments (
            id TEXT PRIMARY KEY,           -- md5(msg_id|url) hex
            message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
            channel_id TEXT NOT NULL,
            url TEXT NOT NULL, filename TEXT NOT NULL,
            content_type TEXT, size INTEGER, local_path TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_attachments_channel ON attachments(channel_id);
        CREATE INDEX IF NOT EXISTS idx_attachments_local_path ON attachments(local_path);

        CREATE TABLE IF NOT EXISTS sync_state (
            channel_id TEXT PRIMARY KEY,
            last_message_id TEXT,
            oldest_message_id TEXT,
            synced_at DATETIME
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
            content, author_name,
            content='messages', content_rowid='rowid',
            tokenize='unicode61 remove_diacritics 1'
        );
        CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
            INSERT INTO messages_fts(rowid, content, author_name) VALUES (new.rowid, new.content, new.author_name);
        END;
        CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
            INSERT INTO messages_fts(messages_fts, rowid, content, author_name) VALUES('delete', old.rowid, old.content, old.author_name);
        END;
        CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
            INSERT INTO messages_fts(messages_fts, rowid, content, author_name) VALUES('delete', old.rowid, old.content, old.author_name);
            INSERT INTO messages_fts(rowid, content, author_name) VALUES (new.rowid, new.content, new.author_name);
        END;
        "#,
    )
    .context("schema migration")?;
    Ok(())
}

/// Upsert a message (ON CONFLICT DO UPDATE — NOT INSERT OR REPLACE).
///
/// Why: with `PRAGMA foreign_keys=ON` and the F6 `attachments` table
/// referencing `messages(id)`, REPLACE (delete+insert) would violate the FK
/// on every re-sync of a channel containing attachments (review#1). UPDATE
/// preserves the row identity and the FTS triggers handle it.
pub fn upsert_message(conn: &Connection, msg: &crate::MessageRow) -> Result<bool> {
    let changed = conn
        .execute(
            r#"
            INSERT INTO messages
                (id, channel_id, guild_id, author_id, author_name, content, timestamp, edited, reaction_count)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(id) DO UPDATE SET
                channel_id = excluded.channel_id,
                guild_id = excluded.guild_id,
                author_id = excluded.author_id,
                author_name = excluded.author_name,
                content = excluded.content,
                timestamp = excluded.timestamp,
                edited = excluded.edited,
                reaction_count = excluded.reaction_count
            "#,
            params![
                msg.id,
                msg.channel_id,
                msg.guild_id,
                msg.author_id,
                msg.author_name,
                msg.content,
                msg.timestamp,
                msg.edited,
                msg.reaction_count,
            ],
        )
        .context("upsert message")?;
    Ok(changed > 0)
}

/// Upsert a guild (INSERT OR REPLACE).
pub fn upsert_guild(conn: &Connection, id: &str, name: &str, icon: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO guilds (id, name, icon) VALUES (?1, ?2, ?3)",
        params![id, name, icon],
    )
    .context("upsert guild")?;
    Ok(())
}

/// Upsert a channel (INSERT OR REPLACE).
pub fn upsert_channel(
    conn: &Connection,
    id: &str,
    guild_id: Option<&str>,
    name: &str,
    channel_type: u8,
    topic: Option<&str>,
    parent_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO channels (id, guild_id, name, type, topic, parent_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, guild_id, name, channel_type, topic, parent_id],
    )
    .context("upsert channel")?;
    Ok(())
}

/// Read sync state for a channel. Returns (last_message_id, oldest_message_id).
pub fn get_sync_state(conn: &Connection, channel_id: &str) -> Result<(String, String)> {
    let mut stmt = conn
        .prepare("SELECT COALESCE(last_message_id,''), COALESCE(oldest_message_id,'') FROM sync_state WHERE channel_id = ?1")
        .context("prepare sync_state")?;
    let mut rows = stmt
        .query(params![channel_id])
        .context("query sync_state")?;
    if let Some(row) = rows.next().context("next sync_state")? {
        let last: String = row.get(0)?;
        let oldest: String = row.get(1)?;
        Ok((last, oldest))
    } else {
        Ok((String::new(), String::new()))
    }
}

/// Update sync state with max/min cursor semantics (langkurt).
pub fn update_sync_state(
    conn: &Connection,
    channel_id: &str,
    newest_id: &str,
    oldest_id: &str,
) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO sync_state (channel_id, last_message_id, oldest_message_id, synced_at)
        VALUES (?1, ?2, ?3, datetime('now'))
        ON CONFLICT(channel_id) DO UPDATE SET
            last_message_id = CASE WHEN excluded.last_message_id > last_message_id
                                   THEN excluded.last_message_id ELSE last_message_id END,
            oldest_message_id = CASE WHEN oldest_message_id='' OR excluded.oldest_message_id < oldest_message_id
                                     THEN excluded.oldest_message_id ELSE oldest_message_id END,
            synced_at = excluded.synced_at
        "#,
        params![channel_id, newest_id, oldest_id],
    )
    .context("update sync_state")?;
    Ok(())
}

/// Recent messages (newest first), optionally filtered by channel + hours.
pub fn recent_messages(
    conn: &Connection,
    channel_name: Option<&str>,
    hours: Option<i64>,
    limit: i64,
) -> Result<Vec<crate::SearchHit>> {
    let mut sql = String::from(
        r#"
        SELECT m.id, m.channel_id, c.name, COALESCE(g.name,'DM'), m.author_name,
               m.content, m.timestamp, 0.0
        FROM messages m
        JOIN channels c ON m.channel_id = c.id
        LEFT JOIN guilds g ON m.guild_id = g.id
        WHERE 1=1
        "#,
    );
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(ch) = channel_name {
        sql.push_str(" AND c.name LIKE ?");
        params.push(Box::new(format!("%{}%", ch)));
    }
    if let Some(h) = hours {
        sql.push_str(" AND m.timestamp >= datetime('now', ?)");
        params.push(Box::new(format!("-{} hours", h)));
    }
    sql.push_str(" ORDER BY m.timestamp DESC LIMIT ?");
    params.push(Box::new(limit));

    let mut stmt = conn.prepare(&sql).context("prepare recent")?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(crate::SearchHit {
            id: row.get(0)?,
            channel_id: row.get(1)?,
            channel_name: row.get(2)?,
            guild_name: row.get(3)?,
            author_name: row.get(4)?,
            content: row.get(5)?,
            timestamp: row.get(6)?,
            rank: row.get(7)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Per-channel message counts (stats).
pub fn channel_stats(conn: &Connection) -> Result<Vec<crate::ChannelStat>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT c.name, COALESCE(g.name,'DM'), COUNT(*)
        FROM messages m
        JOIN channels c ON m.channel_id = c.id
        LEFT JOIN guilds g ON m.guild_id = g.id
        GROUP BY m.channel_id
        ORDER BY COUNT(*) DESC
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(crate::ChannelStat {
            channel_name: row.get(0)?,
            guild_name: row.get(1)?,
            message_count: row.get(2)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Messages since today 00:00 **local** time, grouped by channel.
///
/// The cutoff string is computed in Rust (`Local::now()` date) and passed in
/// — SQLite's `date('now')` is UTC, which would cut the day at 00:00 UTC.
pub fn today_messages(
    conn: &Connection,
    cutoff: &str,
    limit: i64,
) -> Result<Vec<crate::TodayStat>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT c.name, COALESCE(g.name,'DM'), COUNT(*)
        FROM messages m
        JOIN channels c ON m.channel_id = c.id
        LEFT JOIN guilds g ON m.guild_id = g.id
        WHERE m.timestamp >= ?1
        GROUP BY m.channel_id
        ORDER BY COUNT(*) DESC
        LIMIT ?2
        "#,
    )?;
    let rows = stmt.query_map(params![cutoff, limit], |row| {
        Ok(crate::TodayStat {
            channel_name: row.get(0)?,
            guild_name: row.get(1)?,
            message_count: row.get(2)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Messages per bucket: `day` = `YYYY-MM-DD`, `hour` = `HH:00`.
///
/// Bucketing uses `substr(timestamp, ...)` — timestamps are stored as
/// zero-padded RFC3339 UTC strings, so the fixed-width prefix groups them
/// without SQLite datetime conversion.
pub fn timeline(conn: &Connection, by: &str) -> Result<Vec<crate::TimelineBucket>> {
    let expr = match by {
        "day" => "substr(timestamp, 1, 10)",
        "hour" => "substr(timestamp, 12, 2) || ':00'",
        other => anyhow::bail!("invalid timeline granularity: \"{other}\" (use day|hour)"),
    };
    let sql = format!(
        "SELECT {expr} AS bucket, COUNT(*) FROM messages \
         GROUP BY bucket ORDER BY bucket"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok(crate::TimelineBucket {
            bucket: row.get(0)?,
            count: row.get(1)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Top senders in a channel (or globally).
pub fn top_senders(
    conn: &Connection,
    channel_name: Option<&str>,
    limit: i64,
) -> Result<Vec<crate::TopSender>> {
    let mut sql = String::from(
        r#"
        SELECT author_name, COUNT(*) AS cnt
        FROM messages m
        JOIN channels c ON m.channel_id = c.id
        WHERE 1=1
        "#,
    );
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(ch) = channel_name {
        sql.push_str(" AND c.name LIKE ?");
        params.push(Box::new(format!("%{}%", ch)));
    }
    sql.push_str(" GROUP BY author_name ORDER BY cnt DESC LIMIT ?");
    params.push(Box::new(limit));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(crate::TopSender {
            author_name: row.get(0)?,
            message_count: row.get(1)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Export messages for a channel (id, author, content, timestamp).
pub fn channel_messages(
    conn: &Connection,
    channel_id: &str,
    limit: i64,
) -> Result<Vec<crate::ExportRow>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, author_name, content, timestamp
        FROM messages
        WHERE channel_id = ?1
        ORDER BY timestamp
        LIMIT ?2
        "#,
    )?;
    let rows = stmt.query_map(params![channel_id, limit], |row| {
        Ok(crate::ExportRow {
            id: row.get(0)?,
            author_name: row.get(1)?,
            content: row.get(2)?,
            timestamp: row.get(3)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Delete all stored messages for a channel. Returns rows deleted.
pub fn purge_channel(conn: &Connection, channel_id: &str) -> Result<usize> {
    let n = conn
        .execute(
            "DELETE FROM messages WHERE channel_id = ?1",
            params![channel_id],
        )
        .context("purge channel")?;
    Ok(n)
}

/// Find a channel ID by name or ID (used by download --channel).
pub fn find_channel_id(conn: &Connection, name: &str) -> Result<Option<String>> {
    // Exact ID/name first, then substring (channel names carry emoji/decoration
    // suffixes like "chit-chat┊💬", so "chit-chat" must still resolve).
    let exact = conn
        .query_row(
            "SELECT id FROM channels WHERE id = ?1 OR name = ?2 LIMIT 1",
            params![name, name],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = exact {
        return Ok(Some(id));
    }
    let mut stmt = conn
        .prepare("SELECT id FROM channels WHERE name LIKE ?1 LIMIT 1")
        .context("prepare find_channel_id (fuzzy)")?;
    let mut rows = stmt
        .query(params![format!("%{}%", name)])
        .context("query find_channel_id (fuzzy)")?;
    Ok(rows.next()?.map(|r| r.get(0)).transpose()?)
}

/// Find a guild ID by name or ID (used by download --guild).
pub fn find_guild_id(conn: &Connection, name: &str) -> Result<Option<String>> {
    let mut stmt = conn
        .prepare("SELECT id FROM guilds WHERE id = ?1 OR name = ?2 LIMIT 1")
        .context("prepare find_guild_id")?;
    let mut rows = stmt
        .query(params![name, name])
        .context("query find_guild_id")?;
    Ok(rows.next()?.map(|r| r.get(0)).transpose()?)
}

/// Guild name for a channel (via channels.guild_id JOIN).
pub fn guild_name_for_channel(conn: &Connection, channel_id: &str) -> Result<String> {
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(g.name, 'unknown') FROM channels c \
             LEFT JOIN guilds g ON c.guild_id = g.id WHERE c.id = ?1",
        )
        .context("prepare guild_name_for_channel")?;
    let mut rows = stmt
        .query(params![channel_id])
        .context("query guild_name_for_channel")?;
    Ok(rows
        .next()?
        .map(|r| r.get(0))
        .transpose()?
        .unwrap_or_else(|| "unknown".into()))
}

/// Channel name (falls back to the raw ID).
pub fn channel_name(conn: &Connection, channel_id: &str) -> Result<String> {
    let mut stmt = conn
        .prepare("SELECT COALESCE(name, ?2) FROM channels WHERE id = ?1")
        .context("prepare channel_name")?;
    let mut rows = stmt
        .query(params![channel_id, channel_id])
        .context("query channel_name")?;
    Ok(rows
        .next()?
        .map(|r| r.get(0))
        .transpose()?
        .unwrap_or_else(|| channel_id.into()))
}

/// Top-reacted messages (F8). JOINs channels/guilds so filters take names
/// (review#6 — flags pass names, not IDs).
pub fn top_reacted(
    conn: &Connection,
    guild_name: Option<&str>,
    channel_name: Option<&str>,
    limit: i64,
) -> Result<Vec<crate::TopReaction>> {
    let mut sql = String::from(
        r#"
        SELECT m.id, COALESCE(c.name, m.channel_id), COALESCE(g.name, 'DM'),
               m.author_name, m.content, m.reaction_count, m.timestamp
        FROM messages m
        JOIN channels c ON m.channel_id = c.id
        LEFT JOIN guilds g ON m.guild_id = g.id
        WHERE m.reaction_count > 0
        "#,
    );
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(g) = guild_name {
        sql.push_str(" AND g.name = ?");
        params.push(Box::new(g.to_string()));
    }
    if let Some(c) = channel_name {
        sql.push_str(" AND c.name LIKE ?");
        params.push(Box::new(format!("%{}%", c)));
    }
    sql.push_str(" ORDER BY m.reaction_count DESC, m.id DESC LIMIT ?");
    params.push(Box::new(limit));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(crate::TopReaction {
            message_id: row.get(0)?,
            channel_name: row.get(1)?,
            guild_name: row.get(2)?,
            author_name: row.get(3)?,
            content: row.get(4)?,
            reaction_count: row.get(5)?,
            timestamp: row.get(6)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("collect top_reacted")
}

/// FTS5 full-text search over stored messages (langkurt SQL, bind verbatim).
pub fn search_messages(
    conn: &Connection,
    query: &str,
    limit: i64,
) -> Result<Vec<crate::SearchHit>> {
    let mut stmt = conn
        .prepare(
            r#"
            SELECT m.id, m.channel_id, c.name AS channel_name,
                   COALESCE(g.name,'DM') AS guild_name, m.author_name, m.content,
                   m.timestamp, rank
            FROM messages_fts
            JOIN messages m ON messages_fts.rowid = m.rowid
            JOIN channels c ON m.channel_id = c.id
            LEFT JOIN guilds g ON m.guild_id = g.id
            WHERE messages_fts MATCH ?1
            ORDER BY rank
            LIMIT ?2
            "#,
        )
        .context("prepare search")?;
    let rows = stmt
        .query_map(params![query, limit], |row| {
            Ok(crate::SearchHit {
                id: row.get(0)?,
                channel_id: row.get(1)?,
                channel_name: row.get(2)?,
                guild_name: row.get(3)?,
                author_name: row.get(4)?,
                content: row.get(5)?,
                timestamp: row.get(6)?,
                rank: row.get(7)?,
            })
        })
        .context("query search")?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.context("search row")?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn schema_migrates_clean() {
        let conn = temp_db();
        // Verify key tables exist.
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for t in [
            "messages",
            "channels",
            "guilds",
            "sync_state",
            "messages_fts",
        ] {
            assert!(
                tables.contains(&t.to_string()),
                "missing table {t}: {tables:?}"
            );
        }
    }

    #[test]
    fn upsert_message_dedups() {
        let conn = temp_db();
        upsert_guild(&conn, "g1", "Test Guild", None).unwrap();
        upsert_channel(&conn, "c1", Some("g1"), "general", 0, None, None).unwrap();
        let msg = crate::MessageRow {
            id: "1".into(),
            channel_id: "c1".into(),
            guild_id: Some("g1".into()),
            author_id: "u1".into(),
            author_name: "alice".into(),
            content: "hello".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            edited: false,
            reaction_count: 0,
        };
        upsert_message(&conn, &msg).unwrap();
        // Insert same id again — should replace, count still 1.
        upsert_message(&conn, &msg).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn fts_search_after_insert() {
        let conn = temp_db();
        upsert_guild(&conn, "g1", "Test Guild", None).unwrap();
        upsert_channel(&conn, "c1", Some("g1"), "general", 0, None, None).unwrap();
        let msg = crate::MessageRow {
            id: "1".into(),
            channel_id: "c1".into(),
            guild_id: Some("g1".into()),
            author_id: "u1".into(),
            author_name: "alice".into(),
            content: "the quick brown fox".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            edited: false,
            reaction_count: 0,
        };
        upsert_message(&conn, &msg).unwrap();
        let hits = search_messages(&conn, "quick", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].channel_name, "general");
        assert_eq!(hits[0].guild_name, "Test Guild");
    }

    #[test]
    fn sync_state_max_min_cursors() {
        let conn = temp_db();
        update_sync_state(&conn, "c1", "500", "100").unwrap();
        // newer last, older oldest
        update_sync_state(&conn, "c1", "700", "050").unwrap();
        let (last, oldest) = get_sync_state(&conn, "c1").unwrap();
        assert_eq!(last, "700");
        assert_eq!(oldest, "050");
    }

    // Seed helper for today/timeline aggregation tests.
    fn seed_message(conn: &Connection, id: &str, author: &str, ts: &str) {
        conn.execute(
            "INSERT INTO messages (id,channel_id,guild_id,author_id,author_name,content,timestamp,reaction_count) \
             VALUES (?1,'c1','g1',?2,?3,'x',?4,0)",
            params![id, author, author, ts],
        )
        .unwrap();
    }

    #[test]
    fn today_messages_counts_channel_today_only() {
        let conn = temp_db();
        upsert_guild(&conn, "g1", "Test Guild", None).unwrap();
        upsert_channel(&conn, "c1", Some("g1"), "general", 0, None, None).unwrap();
        seed_message(&conn, "1", "alice", "2026-08-09T09:00:00Z");
        seed_message(&conn, "2", "bob", "2026-08-09T09:05:00Z");
        // Yesterday — excluded by the cutoff.
        seed_message(&conn, "3", "bob", "2026-08-08T23:59:00Z");
        // A second channel's today message — grouped separately.
        conn.execute(
            "INSERT INTO channels VALUES ('c2','g1','other',0,NULL,NULL)",
            [],
        )
        .unwrap();
        seed_message(&conn, "4", "bob", "2026-08-09T10:00:00Z");
        conn.execute("UPDATE messages SET channel_id='c2' WHERE id='4'", [])
            .unwrap();

        let stats = today_messages(&conn, "2026-08-09T00:00:00", 10).unwrap();
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].channel_name, "general");
        assert_eq!(stats[0].guild_name, "Test Guild");
        assert_eq!(stats[0].message_count, 2);
        assert_eq!(stats[1].channel_name, "other");
        assert_eq!(stats[1].message_count, 1);
    }

    #[test]
    fn timeline_buckets_day_and_hour() {
        let conn = temp_db();
        upsert_channel(&conn, "c1", None, "general", 0, None, None).unwrap();
        seed_message(&conn, "1", "a", "2026-08-09T09:00:00Z");
        seed_message(&conn, "2", "a", "2026-08-09T09:30:00Z");
        seed_message(&conn, "3", "b", "2026-08-09T14:00:00Z");
        seed_message(&conn, "4", "b", "2026-08-08T22:00:00Z");

        let days = timeline(&conn, "day").unwrap();
        assert_eq!(
            days,
            vec![
                crate::TimelineBucket {
                    bucket: "2026-08-08".into(),
                    count: 1
                },
                crate::TimelineBucket {
                    bucket: "2026-08-09".into(),
                    count: 3
                },
            ]
        );

        let hours = timeline(&conn, "hour").unwrap();
        assert_eq!(
            hours,
            vec![
                crate::TimelineBucket {
                    bucket: "09:00".into(),
                    count: 2
                },
                crate::TimelineBucket {
                    bucket: "14:00".into(),
                    count: 1
                },
                crate::TimelineBucket {
                    bucket: "22:00".into(),
                    count: 1
                },
            ]
        );
    }

    #[test]
    fn timeline_rejects_unknown_granularity() {
        let conn = temp_db();
        upsert_channel(&conn, "c1", None, "general", 0, None, None).unwrap();
        assert!(timeline(&conn, "week").is_err());
    }
}

#[cfg(test)]
mod top_reacted_tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE guilds (id TEXT PRIMARY KEY, name TEXT NOT NULL, icon TEXT);
             CREATE TABLE channels (id TEXT PRIMARY KEY, guild_id TEXT REFERENCES guilds(id),
                name TEXT NOT NULL, type INTEGER DEFAULT 0, topic TEXT, parent_id TEXT);
             CREATE TABLE messages (id TEXT PRIMARY KEY, channel_id TEXT NOT NULL,
                guild_id TEXT, author_id TEXT, author_name TEXT, content TEXT,
                timestamp TEXT, edited INTEGER DEFAULT 0, reaction_count INTEGER DEFAULT 0);
             CREATE INDEX idx_messages_reactions ON messages(reaction_count DESC);",
        )
        .unwrap();
        c
    }

    fn seed(c: &Connection) {
        c.execute("INSERT INTO guilds VALUES ('g1','TestGuild',NULL)", [])
            .unwrap();
        c.execute(
            "INSERT INTO channels VALUES ('c1','g1','general',0,NULL,NULL)",
            [],
        )
        .unwrap();
        let msgs = [
            ("m1", "hi", "2"),
            ("m2", "hot", "9"),
            ("m3", "meh", "0"),
            ("m4", "warm", "4"),
        ];
        for (id, content, rc) in msgs {
            c.execute(
                "INSERT INTO messages (id,channel_id,guild_id,author_id,author_name,content,timestamp,reaction_count) \
                 VALUES (?1,'c1','g1','a','alice',?2,'2026-01-01',?3)",
                params![id, content, rc],
            ).unwrap();
        }
    }

    #[test]
    fn top_reacted_orders_desc() {
        let c = conn();
        seed(&c);
        let top = top_reacted(&c, None, None, 10).unwrap();
        assert_eq!(top.len(), 3); // excludes reaction_count=0
        assert_eq!(top[0].content, "hot");
        assert_eq!(top[0].reaction_count, 9);
        assert_eq!(top[2].content, "hi");
    }

    #[test]
    fn top_reacted_filters_by_guild_and_channel() {
        let c = conn();
        seed(&c);
        let by_guild = top_reacted(&c, Some("TestGuild"), None, 10).unwrap();
        assert_eq!(by_guild.len(), 3);
        let by_channel = top_reacted(&c, None, Some("general"), 10).unwrap();
        assert_eq!(by_channel.len(), 3);
        let miss = top_reacted(&c, Some("Nope"), None, 10).unwrap();
        assert!(miss.is_empty());
    }
}
