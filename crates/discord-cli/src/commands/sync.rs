//! Two-phase incremental sync to SQLite (langkurt pattern).
//!
//! Phase A (new, forward): if a last_message_id exists, fetch messages after it.
//! Phase B (backward, history): resume from oldest_message_id, paginate backward.
//! Persist max/min cursors via `sync_state`.
//!
//! `dc_sync_follow` extends this with a gateway tail (invisible presence):
//! after the backfill, MESSAGE_CREATE events are persisted as they arrive
//! (same `upsert_message` dedup, id-keyed) — `sync <CH> --follow`.

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

/// Open the archive DB connection.
pub(crate) fn open_db() -> Result<discord_db::Connection> {
    let db_path = config::db_path()?;
    ddb::open(db_path.to_str().unwrap_or("discord.db"))
}

/// True when the error chain contains a Discord 404 (`NotFound`).
fn is_not_found(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        matches!(
            c.downcast_ref::<discord_user::DiscordError>(),
            Some(discord_user::DiscordError::NotFound { .. })
        )
    })
}

/// Ensure the channel (+ its guild) exists in the DB — foreign-key
/// requirement before any message upsert. Resolve real name/guild_id/type
/// via the API so archive queries show the channel name instead of its ID
/// (bug: was `name=channel_id, guild=None`).
pub(crate) async fn ensure_channel_row(
    conn: &discord_db::Connection,
    client: &mut ApiClient,
    channel_id: &str,
) -> Result<Option<String>> {
    // Propagate get_channel failures: the old `Err(_) =>` fallback upserted
    // the channel with `guild_id=None, type=0`, so permission/network errors
    // silently mislabeled guild channels as DMs and wasted fetches.
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
        Err(e) => return Err(e.context(format!("get_channel {channel_id}"))),
    };
    // Populate the guilds table FIRST so the channels FK (guild_id →
    // guilds.id) is satisfied when upserting the channel (bug: guilds were
    // never inserted, and the channel carried `guild_id = None`).
    if let Some(gid) = &ch_guild {
        match client.guild_info(gid).await {
            Ok(info) => {
                let _ = ddb::upsert_guild(conn, gid, &info.name, None);
            }
            Err(e) if is_not_found(&e) => {
                // Guild truly absent (left/deleted): keep the FK satisfied
                // with an id-named placeholder rather than skipping the row
                // (a missing guilds row would FK-violate the channel upsert).
                let _ = ddb::upsert_guild(conn, gid, gid, None);
            }
            Err(e) => return Err(e.context(format!("guild_info {gid}"))),
        }
    }
    ddb::upsert_channel(
        conn,
        channel_id,
        ch_guild.as_deref(),
        &ch_name,
        ch_type,
        None,
        None,
    )?;
    Ok(ch_guild)
}

/// Channel metadata already fetched via `list_channels`/`list_guilds` —
/// lets `sync-all` skip the redundant `get_channel` + `guild_info`
/// round-trips per channel (the guild name comes from the guilds list,
/// fetched once for the whole run).
pub(crate) struct ChannelMeta<'a> {
    pub name: &'a str,
    pub guild_id: Option<&'a str>,
    pub guild_name: Option<&'a str>,
    pub channel_type: u8,
}

/// Write-side counterpart of `ensure_channel_row` for already-known
/// metadata: same upserts, no API lookups. The guild name falls back to
/// the id when unknown (keeps the channels FK satisfied).
fn ensure_channel_row_meta(
    conn: &discord_db::Connection,
    channel_id: &str,
    meta: &ChannelMeta<'_>,
) -> Result<Option<String>> {
    let ch_name = if meta.name.is_empty() {
        channel_id.to_string()
    } else {
        meta.name.to_string()
    };
    if let Some(gid) = meta.guild_id {
        let _ = ddb::upsert_guild(conn, gid, meta.guild_name.unwrap_or(gid), None);
    }
    ddb::upsert_channel(
        conn,
        channel_id,
        meta.guild_id,
        &ch_name,
        meta.channel_type,
        None,
        None,
    )?;
    Ok(meta.guild_id.map(str::to_string))
}

/// Sync one channel into SQLite. Returns message count written.
pub async fn sync_channel(client: &mut ApiClient, channel_id: &str, limit: usize) -> Result<usize> {
    let conn = open_db()?;
    let ch_guild = ensure_channel_row(&conn, client, channel_id).await?;
    sync_channel_fetched(client, conn, channel_id, &ch_guild, limit).await
}

/// Sync one channel whose metadata is already known (sync-all path) —
/// identical to `sync_channel` minus the `get_channel`/`guild_info`
/// lookups.
pub async fn sync_channel_meta(
    client: &mut ApiClient,
    channel_id: &str,
    limit: usize,
    meta: &ChannelMeta<'_>,
) -> Result<usize> {
    let conn = open_db()?;
    let ch_guild = ensure_channel_row_meta(&conn, channel_id, meta)?;
    sync_channel_fetched(client, conn, channel_id, &ch_guild, limit).await
}

/// Shared sync body: fetch (Phase A ∥ Phase B) then write in one
/// transaction. `ch_guild` is the resolved guild id (or None for DMs).
async fn sync_channel_fetched(
    client: &mut ApiClient,
    mut conn: discord_db::Connection,
    channel_id: &str,
    ch_guild: &Option<String>,
    limit: usize,
) -> Result<usize> {
    let (last_id, oldest_id) = ddb::get_sync_state(&conn, channel_id)?;

    // Phase B (history backward) always runs to backfill; Phase A (new,
    // forward) only when we have a last cursor. The two fetches are
    // independent, so run them concurrently on a peer client — exactly 2
    // in-flight requests per channel, still rate-limit friendly.
    let before = if oldest_id.is_empty() {
        None
    } else {
        oldest_id.parse().ok()
    };
    let mut peer = client.peer();
    let fetch_forward = async {
        if last_id.is_empty() {
            return Ok(Vec::new());
        }
        let after: Option<u64> = last_id.parse().ok();
        peer.fetch_messages(channel_id, limit, None, after).await
    };
    let (msgs_res, new_msgs_res) = tokio::join!(
        client.fetch_messages(channel_id, limit, before, None),
        fetch_forward
    );
    let msgs = msgs_res?;
    let new_msgs = new_msgs_res?;
    let total = msgs.len() + new_msgs.len();

    // All writes in ONE transaction: hundreds of autocommit upserts were
    // fsync-per-row. Fetches already completed, so the write lock is held
    // only for the (fast) local writes. Statements are prepared once per
    // batch — rusqlite `execute` re-prepares the SQL every call.
    let tx = conn.transaction()?;
    let rows: Vec<MessageRow> = msgs
        .iter()
        .chain(new_msgs.iter())
        .map(|m| row_from_msg(m, channel_id, ch_guild.as_deref()))
        .collect();
    ddb::upsert_messages(&tx, &rows)?;
    let atts: Vec<datt::NewAttachment> = msgs
        .iter()
        .chain(new_msgs.iter())
        .flat_map(|m| attachment_rows(m, channel_id))
        .collect();
    datt::upsert_attachments(&tx, &atts)?;

    // Compute new cursors from BOTH phases: newest = max id seen across
    // Phase B (history) and Phase A (new), oldest = min id seen. Previously
    // only `msgs` (Phase B) was used, so last_message_id regressed whenever
    // Phase A found newer messages, causing re-fetch of already-archived
    // messages on the next sync (wasted requests + rate-limit risk).
    let mut all_ids: Vec<&str> = msgs.iter().map(|m| m.message_id.as_str()).collect();
    all_ids.extend(new_msgs.iter().map(|m| m.message_id.as_str()));
    // Snowflakes compare NUMERICALLY — lexical order breaks across digit
    // counts ("9" > "10" as strings). Non-numeric falls back to lexical.
    fn snow_ord(a: &str, b: &str) -> std::cmp::Ordering {
        match (a.parse::<u64>(), b.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            _ => a.cmp(b),
        }
    }
    let newest = all_ids
        .iter()
        .copied()
        .max_by(|a, b| snow_ord(a, b))
        .unwrap_or_default()
        .to_string();
    let oldest = all_ids
        .iter()
        .copied()
        .min_by(|a, b| snow_ord(a, b))
        .unwrap_or_default()
        .to_string();
    if !newest.is_empty() {
        ddb::update_sync_state(&tx, channel_id, &newest, &oldest)?;
    }
    tx.commit()?;
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

/// Build the attachment ledger rows for a message (F6). Idempotent
/// (INSERT OR IGNORE keyed by md5(msg_id|url)). Shared by the batch sync
/// path and the single-message gateway path.
fn attachment_rows(m: &discord_core::types::Message, channel_id: &str) -> Vec<datt::NewAttachment> {
    m.attachment_details
        .as_ref()
        .map(|details| {
            details
                .iter()
                .map(|a| datt::NewAttachment {
                    id: attachment_id(&m.message_id, &a.url),
                    message_id: m.message_id.clone(),
                    channel_id: channel_id.to_string(),
                    url: a.url.clone(),
                    filename: a.filename.clone(),
                    content_type: a.content_type.clone(),
                    size: a.size,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Convert a gateway MESSAGE_CREATE message to a db MessageRow, sharing the
/// same field mapping as the REST `row_from_msg` (author, reaction sum, ...)
/// so archived rows look identical regardless of ingestion path.
fn row_from_gateway_message(
    m: &discord_user::types::Message,
    channel_id: &str,
    guild_id: Option<&str>,
) -> MessageRow {
    MessageRow {
        id: m.id.clone(),
        channel_id: channel_id.to_string(),
        // The gateway Message carries guild_id directly (REST fetch_messages
        // does not) — prefer the threaded-through resolved channel for
        // consistency with the sync path (bug: archive joins showed "DM").
        guild_id: guild_id.map(str::to_string).or_else(|| m.guild_id.clone()),
        author_id: m.author.id.clone(),
        author_name: m.author.username.clone(),
        content: m.content.clone(),
        timestamp: m.timestamp.clone(),
        edited: m.edited_timestamp.is_some(),
        reaction_count: m.reactions.iter().map(|r| r.count).sum(),
    }
}

/// Persist one gateway message into SQLite. Idempotent (upsert keyed by
/// message id, same as the REST sync path).
pub(crate) fn persist_gateway_message(
    conn: &discord_db::Connection,
    m: &discord_user::types::Message,
    channel_id: &str,
    guild_id: Option<&str>,
) -> anyhow::Result<()> {
    ddb::upsert_message(conn, &row_from_gateway_message(m, channel_id, guild_id))?;
    // Attachment ledger (F6): gateway attachments carry the same fields as
    // the REST shape — mirror the sync path's md5(msg_id|url) keying.
    if !m.attachments.is_empty() {
        let rows: Vec<datt::NewAttachment> = m
            .attachments
            .iter()
            .map(|a| datt::NewAttachment {
                id: attachment_id(&m.id, &a.url),
                message_id: m.id.clone(),
                channel_id: channel_id.to_string(),
                url: a.url.clone(),
                filename: a.filename.clone(),
                content_type: a.content_type.clone(),
                size: Some(a.size as i64),
            })
            .collect();
        datt::upsert_attachments(conn, &rows)?;
    }
    Ok(())
}

/// `discord sync <CH> --follow [--max-duration SECS]` — backfill via
/// `sync_channel`, then keep persisting new messages into SQLite through the
/// gateway (invisible presence) until Ctrl-C or the optional deadline.
pub async fn dc_sync_follow(
    ctx: &crate::commands::dc::DcCtx,
    channel: &str,
    limit: usize,
    max_duration_secs: Option<u64>,
) -> std::process::ExitCode {
    use discord_core::output::{self, exit};
    use std::process::ExitCode;

    let token = match discord_core::config::resolve_token(ctx.token.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            return ExitCode::from(output::emit_error("AuthError", &e.to_string(), exit::ERROR))
        }
    };

    let mut api = ApiClient::with_token(token.clone());
    let channel_id = match super::dc::resolve_channel_id(&mut api, channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };

    // Phase 1: normal incremental sync (backfill) — same as `sync <CH>`.
    match sync_channel(&mut api, &channel_id, limit).await {
        Ok(n) => {
            let line = serde_json::json!({
                "type": "synced",
                "channel_id": channel_id,
                "messages_synced": n,
            });
            output::out_line(&serde_json::to_string(&line).unwrap_or_default());
        }
        Err(e) => {
            return ExitCode::from(output::emit_error("SyncError", &e.to_string(), exit::ERROR))
        }
    }

    // Resolve the guild once (needed for the channel FK on new rows); the
    // backfill already wrote the channel/guild rows, but a follow run on a
    // fresh DB still needs the guild_id for gateway messages.
    let conn = match open_db() {
        Ok(c) => c,
        Err(e) => {
            return ExitCode::from(output::emit_error("SyncError", &e.to_string(), exit::ERROR))
        }
    };
    let ch_guild = match ensure_channel_row(&conn, &mut api, &channel_id).await {
        Ok(g) => g,
        Err(e) => {
            return ExitCode::from(output::emit_error("SyncError", &e.to_string(), exit::ERROR))
        }
    };

    // Phase 2: gateway follow. Handler is sync (Fn, not async), so events go
    // through an mpsc queue and a single loop does the DB writes — the DB
    // connection (rusqlite, !Sync) never enters the callback.
    let mut client = discord_user::DiscordUser::new(token.clone()).with_status(
        match discord_core::config::configured_presence().as_str() {
            "online" => discord_user::UserStatus::Online,
            "idle" => discord_user::UserStatus::Idle,
            "dnd" => discord_user::UserStatus::DoNotDisturb,
            _ => discord_user::UserStatus::Invisible, // stealth default
        },
    );
    if let Err(e) = client.init().await {
        return ExitCode::from(output::emit_error(
            "GatewayError",
            &e.to_string(),
            exit::ERROR,
        ));
    }

    let target = channel_id.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<discord_user::types::Message>();
    let _sub = client
        .on_message_create(move |event| {
            if event.message.channel_id.as_str() != target {
                return;
            }
            let _ = tx.send(event.message);
        })
        .await;

    let start = std::time::Instant::now();
    let mut synced = 0usize;
    let deadline = max_duration_secs.map(|s| start + std::time::Duration::from_secs(s));
    let result: Result<()> = async {
        loop {
            match deadline {
                Some(d) => {
                    match tokio::time::timeout_at(tokio::time::Instant::from_std(d), rx.recv())
                        .await
                    {
                        Ok(Some(m)) => {
                            persist_gateway_message(&conn, &m, &channel_id, ch_guild.as_deref())?;
                            synced += 1;
                        }
                        // Stream closed or deadline reached: graceful exit.
                        Ok(None) | Err(_) => break,
                    }
                }
                None => match rx.recv().await {
                    Some(m) => {
                        persist_gateway_message(&conn, &m, &channel_id, ch_guild.as_deref())?;
                        synced += 1;
                    }
                    None => break,
                },
            }
        }
        Ok(())
    }
    .await;
    let _ = client.disconnect().await;

    // Summary line (JSONL, stdout stays machine-readable).
    let dur = start.elapsed().as_secs();
    let summary = serde_json::json!({
        "messages_synced": synced,
        "followed": true,
        "duration_secs": dur,
    });
    output::out_line(&serde_json::to_string(&summary).unwrap_or_default());
    match result {
        Ok(()) => ExitCode::from(exit::OK),
        Err(e) => ExitCode::from(output::emit_error("SyncError", &e.to_string(), exit::ERROR)),
    }
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
    fn gateway_row_maps_same_fields() {
        // Row conversion from a gateway MESSAGE_CREATE must mirror the REST
        // path: id/channel/guild/author content + summed reaction count.
        // Built via serde from the wire shape (Message has no Default impl,
        // and this exercises the real gateway deserialization path).
        let m: discord_user::types::Message = serde_json::from_value(serde_json::json!({
            "id": "42",
            "channel_id": "c",
            "guild_id": null,
            "author": { "id": "u", "username": "alice" },
            "content": "hello",
            "timestamp": "2026-01-01T00:00:00Z",
            "edited_timestamp": "2026-01-01T00:00:01Z",
            "reactions": [
                { "count": 3, "me": false, "emoji": { "name": "a" } },
                { "count": 2, "me": false, "emoji": { "name": "b" } },
            ],
        }))
        .unwrap();
        let row = row_from_gateway_message(&m, "c", Some("g"));
        assert_eq!(row.id, "42");
        assert_eq!(row.channel_id, "c");
        assert_eq!(row.guild_id.as_deref(), Some("g"));
        assert_eq!(row.author_id, "u");
        assert_eq!(row.author_name, "alice");
        assert_eq!(row.content, "hello");
        assert_eq!(row.timestamp, "2026-01-01T00:00:00Z");
        assert!(row.edited); // edited_timestamp present -> edited
        assert_eq!(row.reaction_count, 5); // sum of reaction counts (F8)
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
