//! Two-phase incremental sync to SQLite (langkurt pattern).
//!
//! Phase A (new, forward): if a last_message_id exists, fetch messages after it.
//! Phase B (backward, history): resume from oldest_message_id, paginate backward.
//! Persist max/min cursors via `sync_state`.

use anyhow::Result;
use discord_core::client::ApiClient;
use discord_core::config;
use discord_db::attachments as datt;
use discord_db::db as ddb;
use discord_db::MessageRow;

/// md5 hex of `msg_id|url` — the attachment ledger key (langkurt
/// fetchlinks.go:37-39). Uses the `md-5` crate (review#13 added).
fn attachment_id(message_id: &str, url: &str) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(message_id.as_bytes());
    h.update(b"|");
    h.update(url.as_bytes());
    format!("{:x}", h.finalize())
}

/// Sync one channel into SQLite. Returns message count written.
pub async fn sync_channel(client: &mut ApiClient, channel_id: &str, limit: usize) -> Result<usize> {
    let db_path = config::db_path()?;
    let conn = ddb::open(db_path.to_str().unwrap_or("discord.db"))?;

    // Ensure the channel exists in the DB first (foreign-key requirement).
    // Resolve real name/guild_id/type via the API so archive queries show the
    // channel name instead of its ID (bug: was `name=channel_id, guild=None`).
    let (ch_name, ch_guild, ch_type) = match client.get_channel(channel_id).await {
        Ok(ch) => (
            if ch.name.is_empty() {
                channel_id.to_string()
            } else {
                ch.name
            },
            ch.guild_id,
            ch.channel_type,
        ),
        Err(_) => (channel_id.to_string(), None, 0),
    };
    // Populate the guilds table FIRST so the channels FK (guild_id →
    // guilds.id) is satisfied when upserting the channel (bug: guilds were
    // never inserted, and the channel carried `guild_id = None`).
    if let Some(gid) = &ch_guild {
        if let Ok(info) = client.guild_info(gid).await {
            let _ = ddb::upsert_guild(&conn, gid, &info.name, None);
        }
    }
    ddb::upsert_channel(
        &conn,
        channel_id,
        ch_guild.as_deref(),
        &ch_name,
        ch_type,
        None,
        None,
    )?;

    let (last_id, oldest_id) = ddb::get_sync_state(&conn, channel_id)?;
    let mut total = 0usize;
    let mut new_msgs: Vec<discord_core::types::Message> = Vec::new();

    // Phase B (history backward) — always runs to backfill.
    let before = if oldest_id.is_empty() {
        None
    } else {
        oldest_id.parse().ok()
    };
    let msgs = client
        .fetch_messages(channel_id, limit, before, None)
        .await?;
    for m in &msgs {
        ddb::upsert_message(&conn, &row_from_msg(m, channel_id, ch_guild.as_deref()))?;
        upsert_attachments(&conn, m, channel_id)?;
    }
    total += msgs.len();

    // Phase A (new, forward) — only when we have a last cursor.
    if !last_id.is_empty() {
        let after: Option<u64> = last_id.parse().ok();
        new_msgs = client
            .fetch_messages(channel_id, limit, None, after)
            .await?;
        for m in &new_msgs {
            ddb::upsert_message(&conn, &row_from_msg(m, channel_id, ch_guild.as_deref()))?;
            upsert_attachments(&conn, m, channel_id)?;
        }
        total += new_msgs.len();
    }

    // Compute new cursors from BOTH phases: newest = max id seen across
    // Phase B (history) and Phase A (new), oldest = min id seen. Previously
    // only `msgs` (Phase B) was used, so last_message_id regressed whenever
    // Phase A found newer messages, causing re-fetch of already-archived
    // messages on the next sync (wasted requests + rate-limit risk).
    let mut all_ids: Vec<&str> = msgs.iter().map(|m| m.message_id.as_str()).collect();
    all_ids.extend(new_msgs.iter().map(|m| m.message_id.as_str()));
    let newest = all_ids.iter().map(|s| *s).max().unwrap_or_default().to_string();
    let oldest = all_ids.iter().map(|s| *s).min().unwrap_or_default().to_string();
    if !newest.is_empty() {
        ddb::update_sync_state(&conn, channel_id, &newest, &oldest)?;
    }
    Ok(total)
}

/// Convert a core Message to a db MessageRow.
fn row_from_msg(
    m: &discord_core::types::Message,
    channel_id: &str,
    guild_id: Option<&str>,
) -> MessageRow {
    MessageRow {
        id: m.message_id.clone(),
        channel_id: channel_id.to_string(),
        // The Message type from fetch_messages carries no guild_id (only the
        // channel does), so we thread it through from the resolved channel —
        // otherwise archive joins show "DM" for every guild (bug).
        guild_id: guild_id.map(str::to_string).or_else(|| m.guild_id.clone()),
        author_id: m.author_id.clone().unwrap_or_default(),
        author_name: m.author.clone(),
        content: m.content.clone(),
        timestamp: m.timestamp.clone(),
        edited: false,
        // F8: sum of reaction counts (langkurt upsertMsg sync.go:259-263).
        reaction_count: m
            .reactions
            .as_ref()
            .map(|r| r.iter().map(|x| x.count as u32).sum())
            .unwrap_or(0),
    }
}

/// Upsert attachment ledger rows for a message (F6). Idempotent
/// (INSERT OR IGNORE keyed by md5(msg_id|url)).
fn upsert_attachments(
    conn: &discord_db::Connection,
    m: &discord_core::types::Message,
    channel_id: &str,
) -> anyhow::Result<()> {
    if let Some(details) = &m.attachment_details {
        for a in details {
            datt::upsert_attachment(
                conn,
                &datt::NewAttachment {
                    id: attachment_id(&m.message_id, &a.url),
                    message_id: m.message_id.clone(),
                    channel_id: channel_id.to_string(),
                    url: a.url.clone(),
                    filename: a.filename.clone(),
                    content_type: a.content_type.clone(),
                    size: a.size,
                },
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use discord_core::types::{AttachmentInfo, Message, ReactionInfo};

    fn msg_with(
        reactions: Option<Vec<ReactionInfo>>,
        atts: Option<Vec<AttachmentInfo>>,
    ) -> Message {
        Message {
            message_id: "42".into(),
            channel_id: "c".into(),
            guild_id: None,
            author_id: Some("u".into()),
            author: "alice".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            content: "x".into(),
            attachments: None,
            attachment_details: atts,
            reactions,
        }
    }

    #[test]
    fn reaction_count_is_sum() {
        let m = msg_with(
            Some(vec![ReactionInfo { count: 2 }, ReactionInfo { count: 5 }]),
            None,
        );
        assert_eq!(row_from_msg(&m, "c", None).reaction_count, 7);
    }

    #[test]
    fn reaction_count_zero_without_reactions() {
        let m = msg_with(None, None);
        assert_eq!(row_from_msg(&m, "c", None).reaction_count, 0);
    }

    #[test]
    fn attachment_id_is_md5_msgid_url() {
        // md5("42|https://x.png") — stable, deterministic.
        let id = attachment_id("42", "https://x.png");
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        // Same input -> same id (idempotent upsert key).
        assert_eq!(attachment_id("42", "https://x.png"), id);
        // Different url -> different id.
        assert_ne!(attachment_id("42", "https://y.png"), id);
    }
}
