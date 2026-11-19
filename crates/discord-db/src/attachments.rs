//! Attachment ledger for the offline download pipeline (F6).
//!
//! Ported from langkurt `storage/attachments.go` (MIT, `.tmp/`): rows are
//! upserted at sync time (INSERT OR IGNORE), and the download command lists
//! "pending" rows (local_path IS NULL — review#17) with filters, then marks
//! them downloaded.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

/// One attachment row in the ledger.
#[derive(Debug, Clone)]
pub struct AttachmentRow {
    pub id: String,
    pub message_id: String,
    pub channel_id: String,
    pub url: String,
    pub filename: String,
    pub content_type: Option<String>,
    pub size: Option<i64>,
    pub local_path: Option<String>,
}

/// Filters for listing pending attachments (all optional).
#[derive(Debug, Clone, Default)]
pub struct AttachmentFilter {
    pub guild_id: Option<String>,
    pub channel_id: Option<String>,
    /// RFC3339 timestamp cutoff (messages.timestamp >= since).
    pub since: Option<String>,
    /// Only rows from messages with at least this many reactions.
    pub min_reactions: Option<i64>,
    /// image | gif | video | all (content_type based; None = all).
    pub media_type: Option<String>,
    /// Max rows (0 = unlimited).
    pub limit: i64,
}

/// New attachment to insert (idempotent). `id` = md5(msg_id|url) hex — the
/// caller computes it (keeps this crate free of a md5 dep).
#[derive(Debug, Clone)]
pub struct NewAttachment {
    pub id: String,
    pub message_id: String,
    pub channel_id: String,
    pub url: String,
    pub filename: String,
    pub content_type: Option<String>,
    pub size: Option<i64>,
}

const UPSERT_ATTACHMENT_SQL: &str = r#"
    INSERT OR IGNORE INTO attachments
        (id, message_id, channel_id, url, filename, content_type, size)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
    "#;

fn upsert_attachment_with(stmt: &mut rusqlite::Statement<'_>, a: &NewAttachment) -> Result<()> {
    stmt.execute(params![
        a.id,
        a.message_id,
        a.channel_id,
        a.url,
        a.filename,
        a.content_type,
        a.size
    ])
    .context("upsert attachment")?;
    Ok(())
}

/// Insert an attachment row (INSERT OR IGNORE — idempotent).
pub fn upsert_attachment(conn: &Connection, a: &NewAttachment) -> Result<()> {
    let mut stmt = conn
        .prepare(UPSERT_ATTACHMENT_SQL)
        .context("prepare upsert attachment")?;
    upsert_attachment_with(&mut stmt, a)
}

/// Batch variant: prepares the statement ONCE and loops — rusqlite
/// `execute` re-prepares per call, which dominated sync runs with many
/// attachments per channel.
pub fn upsert_attachments(conn: &Connection, rows: &[NewAttachment]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut stmt = conn
        .prepare(UPSERT_ATTACHMENT_SQL)
        .context("prepare upsert attachments")?;
    for a in rows {
        upsert_attachment_with(&mut stmt, a)?;
    }
    Ok(())
}

/// List pending (not-yet-downloaded) attachments with filters.
/// `local_path IS NULL` is always applied (review#17) so re-runs skip done
/// files. Guild/channel filters JOIN messages (+ channels for guild name
/// lookup, review#5 — channel_id is authoritative).
pub fn list_pending_attachments(
    conn: &Connection,
    f: &AttachmentFilter,
) -> Result<Vec<AttachmentRow>> {
    let mut sql = String::from(
        "SELECT a.id, a.message_id, a.channel_id, a.url, a.filename, \
         a.content_type, a.size, a.local_path \
         FROM attachments a JOIN messages m ON a.message_id = m.id \
         WHERE a.local_path IS NULL",
    );
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(g) = &f.guild_id {
        sql.push_str(" AND m.guild_id = ?");
        params_vec.push(Box::new(g.clone()));
    }
    if let Some(c) = &f.channel_id {
        sql.push_str(" AND a.channel_id = ?");
        params_vec.push(Box::new(c.clone()));
    }
    if let Some(s) = &f.since {
        sql.push_str(" AND m.timestamp >= ?");
        params_vec.push(Box::new(s.clone()));
    }
    if let Some(r) = f.min_reactions {
        sql.push_str(" AND m.reaction_count >= ?");
        params_vec.push(Box::new(r));
    }
    match f.media_type.as_deref() {
        Some("gif") => sql.push_str(" AND a.content_type = 'image/gif'"),
        Some("image") => {
            sql.push_str(" AND a.content_type LIKE 'image/%' AND a.content_type != 'image/gif'")
        }
        Some("video") => sql.push_str(" AND a.content_type LIKE 'video/%'"),
        _ => {}
    }
    sql.push_str(" ORDER BY a.id");
    if f.limit > 0 {
        sql.push_str(" LIMIT ?");
        params_vec.push(Box::new(f.limit));
    }

    let mut stmt = conn
        .prepare(&sql)
        .context("prepare list_pending_attachments")?;
    let rows = stmt
        .query_map(
            rusqlite::params_from_iter(params_vec.iter().map(|p| p.as_ref())),
            |r| {
                Ok(AttachmentRow {
                    id: r.get(0)?,
                    message_id: r.get(1)?,
                    channel_id: r.get(2)?,
                    url: r.get(3)?,
                    filename: r.get(4)?,
                    content_type: r.get(5)?,
                    size: r.get(6)?,
                    local_path: r.get(7)?,
                })
            },
        )
        .context("query attachments")?
        .collect::<Result<Vec<_>, _>>()
        .context("collect attachments")?;
    Ok(rows)
}

/// Mark an attachment as downloaded (set local_path).
pub fn mark_downloaded(conn: &Connection, id: &str, local_path: &str) -> Result<()> {
    conn.execute(
        "UPDATE attachments SET local_path = ?1 WHERE id = ?2",
        params![local_path, id],
    )
    .context("mark attachment downloaded")?;
    Ok(())
}

/// Batch variant of [`mark_downloaded`]: one prepared statement for the
/// whole download run instead of a prepare per file.
/// `marks` is `(attachment_id, local_path)` pairs.
pub fn mark_downloaded_batch(conn: &Connection, marks: &[(String, String)]) -> Result<()> {
    if marks.is_empty() {
        return Ok(());
    }
    let mut stmt = conn
        .prepare("UPDATE attachments SET local_path = ?1 WHERE id = ?2")
        .context("prepare mark_downloaded batch")?;
    for (id, path) in marks {
        stmt.execute(params![path, id])
            .context("mark attachment downloaded")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE messages (id TEXT PRIMARY KEY, channel_id TEXT, guild_id TEXT, \
             author_id TEXT, author_name TEXT, content TEXT, timestamp TEXT, \
             edited INTEGER DEFAULT 0, reaction_count INTEGER DEFAULT 0); \
             CREATE TABLE attachments (id TEXT PRIMARY KEY, message_id TEXT NOT NULL \
             REFERENCES messages(id) ON DELETE CASCADE, channel_id TEXT, url TEXT, \
             filename TEXT, content_type TEXT, size INTEGER, local_path TEXT);",
        )
        .unwrap();
        c
    }

    fn msg(c: &Connection, id: &str, guild: &str, ts: &str, reactions: i64) {
        c.execute(
            "INSERT INTO messages (id, channel_id, guild_id, author_id, author_name, content, timestamp, reaction_count) \
             VALUES (?1, 'ch', ?2, 'a', 'n', 'x', ?3, ?4)",
            params![id, guild, ts, reactions],
        )
        .unwrap();
    }

    fn att(c: &Connection, id: &str, msg: &str, path: Option<&str>) {
        c.execute(
            "INSERT INTO attachments (id, message_id, channel_id, url, filename, content_type, size, local_path) \
             VALUES (?1, ?2, 'ch', 'u', 'f', 'image/png', 1, ?3)",
            params![id, msg, path],
        )
        .unwrap();
    }

    #[test]
    fn pending_excludes_downloaded() {
        let c = conn();
        msg(&c, "1", "g1", "2026-01-01", 0);
        msg(&c, "2", "g2", "2026-01-02", 5);
        att(&c, "a1", "1", None);
        att(&c, "a2", "2", Some("/tmp/x.png")); // already downloaded
        let rows = list_pending_attachments(&c, &AttachmentFilter::default()).unwrap();
        assert_eq!(rows.len(), 1, "only a1 is pending");
        assert_eq!(rows[0].id, "a1");
    }

    #[test]
    fn filters_guild_and_reactions() {
        let c = conn();
        msg(&c, "1", "g1", "2026-01-01", 0);
        msg(&c, "2", "g2", "2026-01-02", 5);
        att(&c, "a1", "1", None);
        att(&c, "a2", "2", None);
        let f = AttachmentFilter {
            guild_id: Some("g2".into()),
            min_reactions: Some(3),
            ..Default::default()
        };
        let rows = list_pending_attachments(&c, &f).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "a2");
    }

    #[test]
    fn media_type_filters() {
        let c = conn();
        msg(&c, "1", "g", "2026-01-01", 0);
        msg(&c, "2", "g", "2026-01-01", 0);
        c.execute(
            "INSERT INTO attachments (id, message_id, channel_id, url, filename, content_type, size, local_path) \
             VALUES ('g1', '1', 'ch', 'u', 'f', 'image/gif', 1, NULL)",
            params![],
        )
        .unwrap();
        att(&c, "g2", "2", None); // image/png
        let f = AttachmentFilter {
            media_type: Some("image".into()),
            ..Default::default()
        };
        let rows = list_pending_attachments(&c, &f).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "g2", "gif excluded from 'image'");
    }

    #[test]
    fn mark_sets_path() {
        let c = conn();
        msg(&c, "1", "g", "2026-01-01", 0);
        att(&c, "a1", "1", None);
        mark_downloaded(&c, "a1", "/out/f.png").unwrap();
        let rows = list_pending_attachments(&c, &AttachmentFilter::default()).unwrap();
        assert!(rows.is_empty(), "marked row no longer pending");
    }
}
