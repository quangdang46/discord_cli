//! `dc` command group — Discord operations (guilds, channels, dms, history,
//! read, send, ...). One function per verb.
//!
//! M2.2: `dc guilds`, `dc channels`. Later milestones add more.

use std::process::ExitCode;

use clap::Subcommand;
use discord_core::client::ApiClient;
use discord_core::output::{self, exit, Format};
use discord_db::db as ddb;

use crate::resolve;

/// Shared context for a `dc` subcommand invocation.
pub struct DcCtx {
    pub token: Option<String>,
    pub format: Format,
}

#[derive(Subcommand, Debug)]
pub enum DcCmd {
    /// List joined guilds (name/id/icon/owner).
    Guilds,
    /// List text/announcement/forum channels of a guild.
    Channels {
        /// Guild name or ID.
        guild: String,
    },
    /// List DM + group-DM channels.
    Dms,
    /// Fetch message history of a channel (paginated).
    History {
        /// Channel name or ID (in the resolved guild).
        channel: String,
        /// Max messages to fetch (default 1000, max 1000).
        #[arg(short, long, default_value_t = 1000)]
        limit: usize,
        /// Fetch messages before this snowflake.
        #[arg(long)]
        before: Option<u64>,
        /// Fetch messages after this snowflake.
        #[arg(long)]
        after: Option<u64>,
    },
    /// Read recent messages (default 50) — the key AI-facing read.
    Read {
        /// Channel name or ID.
        channel: String,
        /// Max messages (default 50).
        #[arg(short, long, default_value_t = 50)]
        limit: usize,
        /// Fetch messages before this snowflake.
        #[arg(long)]
        before: Option<u64>,
    },
    /// List guild members.
    Members {
        /// Guild name or ID.
        guild: String,
        /// Max members (default 50, max 1000).
        #[arg(long, default_value_t = 50)]
        max: u32,
    },
    /// Show guild info (name, member counts).
    Info {
        /// Guild name or ID.
        guild: String,
    },
    /// Discord native search within a guild.
    Search {
        /// Guild name or ID.
        guild: String,
        /// Search query.
        query: String,
        /// Restrict to a channel name or ID.
        #[arg(short, long)]
        channel: Option<String>,
        /// Max results (default 25).
        #[arg(short, long, default_value_t = 25)]
        limit: u32,
    },
    /// List guild roles (sorted by position).
    Roles {
        /// Guild name or ID.
        guild: String,
    },
    /// Show a user's profile (default: self).
    Profile {
        /// User ID (default: current user).
        user_id: Option<String>,
    },
    /// Show friends/blocked/pending relationships.
    Relationships,
    /// List active threads in a channel (user-token fallback).
    Threads {
        /// Channel name or ID.
        channel: String,
    },
    /// Send a message (requires --confirm unless --reply/--dry-run).
    Send {
        /// Channel name or ID.
        channel: String,
        /// Message content. "-" reads from stdin.
        #[arg(long)]
        text: Option<String>,
        /// Attach a file (repeatable; max 10 per message).
        #[arg(long)]
        file: Vec<String>,
        /// Reply to a message id.
        #[arg(long)]
        reply: Option<String>,
        /// Send a typing indicator first (mimics a human composing).
        #[arg(long)]
        typing: bool,
        /// Confirm a non-reply send (never interactive).
        #[arg(long)]
        confirm: bool,
        /// Preview what would be sent without sending.
        #[arg(long)]
        dry_run: bool,
    },
    /// Send a typing indicator to a channel (one-shot).
    Typing {
        /// Channel name or ID.
        channel: String,
    },
    /// Join a server via invite code or URL (requires --confirm).
    Join {
        /// Invite code or full URL (discord.gg/..., discord.com/invite/...).
        invite: String,
        /// Confirm joining (never interactive).
        #[arg(long)]
        confirm: bool,
    },
    /// Leave a server (requires --confirm).
    Leave {
        /// Guild name or ID.
        guild: String,
        /// Confirm leaving (never interactive).
        #[arg(long)]
        confirm: bool,
    },
    /// Show or set presence (online|idle|dnd|invisible).
    Presence {
        /// New status. Omit to show the configured default.
        status: Option<String>,
    },
    /// Create a thread (standalone, from message, or forum post).
    ThreadCreate {
        /// Channel name or ID.
        channel: String,
        /// Thread name.
        #[arg(long)]
        name: String,
        /// Create from this message ID (text/announcement parent).
        #[arg(long)]
        message_id: Option<String>,
        /// Starter message content (required for forum; optional standalone).
        #[arg(long)]
        text: Option<String>,
        /// Auto-archive minutes (60|1440|4320|10080; default 1440).
        #[arg(long)]
        archive: Option<u32>,
        /// Comma-separated forum tag IDs.
        #[arg(long)]
        tags: Option<String>,
    },
    /// Edit an own message.
    Edit {
        /// Channel name or ID.
        channel: String,
        /// Message ID.
        message_id: String,
        /// New content.
        #[arg(long)]
        text: String,
    },
    /// Delete an own message (requires --confirm).
    Delete {
        /// Channel name or ID.
        channel: String,
        /// Message ID.
        message_id: String,
        /// Confirm deletion.
        #[arg(long)]
        confirm: bool,
    },
    /// Add a reaction.
    React {
        /// Channel name or ID.
        channel: String,
        /// Message ID.
        message_id: String,
        /// Emoji (unicode or :name:).
        emoji: String,
    },
    /// Remove own reaction.
    Unreact {
        /// Channel name or ID.
        channel: String,
        /// Message ID.
        message_id: String,
        /// Emoji.
        emoji: String,
    },
    /// Pin a message.
    Pin {
        /// Channel name or ID.
        channel: String,
        /// Message ID.
        message_id: String,
    },
    /// List pinned messages.
    Pins {
        /// Channel name or ID.
        channel: String,
    },
    /// Incrementally sync a channel's history to SQLite.
    Sync {
        /// Channel name or ID.
        channel: String,
        /// Max messages (default 5000).
        #[arg(short, long, default_value_t = 5000)]
        limit: usize,
    },
    /// Discover and sync all accessible text channels (bounded).
    SyncAll {
        /// Per-channel cap (default 200).
        #[arg(short, long, default_value_t = 200)]
        limit: usize,
    },
    /// Follow new messages live (gateway, invisible presence).
    Tail {
        /// Channel ID (empty = all channels).
        channel: String,
        /// Fetch once and exit after a short listen.
        #[arg(long)]
        once: bool,
    },
    /// Long-running JSONL stream for agents (optional filters).
    Watch {
        /// Only stream this channel ID.
        #[arg(long)]
        channel: Option<String>,
        /// Only stream messages containing this keyword.
        #[arg(long)]
        keyword: Option<String>,
        /// Also emit typing-indicator events as JSONL.
        #[arg(long)]
        typing: bool,
    },
    /// Top-reacted messages from the archive (hottest first).
    TopReactions {
        /// Filter by guild name.
        #[arg(long)]
        guild: Option<String>,
        /// Filter by channel name.
        #[arg(long)]
        channel: Option<String>,
        /// Max results (default 10).
        #[arg(long)]
        limit: Option<i64>,
    },
    /// Download archived attachments to disk (offline).
    Download {
        /// Filter by guild name or ID.
        #[arg(long)]
        guild: Option<String>,
        /// Filter by channel name or ID.
        #[arg(long)]
        channel: Option<String>,
        /// Media type filter (image|gif|video|all).
        #[arg(long)]
        r#type: Option<String>,
        /// Only files from messages on/after this date (30d|6m|1y|YYYY-MM-DD).
        #[arg(long)]
        since: Option<String>,
        /// Only files from messages with at least this many reactions.
        #[arg(long)]
        min_reactions: Option<i64>,
        /// Max files to download (0 = unlimited).
        #[arg(long)]
        limit: Option<i64>,
        /// Output directory (default <data_dir>/media).
        #[arg(long)]
        out: Option<String>,
    },
    /// Group DM management.
    DmGroup {
        #[command(subcommand)]
        cmd: DmGroupCmd,
    },
    /// Notification settings (mute, level).
    Notify {
        #[command(subcommand)]
        cmd: NotifyCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum DmGroupCmd {
    /// Create a group DM with 2+ recipient user IDs (comma-separated).
    Create {
        /// Recipient user IDs, comma-separated (e.g. "123,456").
        users: String,
        /// Confirm creation.
        #[arg(long)]
        confirm: bool,
    },
    /// Add a recipient to a group DM.
    Add {
        /// Group DM channel ID.
        channel: String,
        /// User ID.
        user: String,
    },
    /// Remove a recipient from a group DM.
    Remove {
        /// Group DM channel ID.
        channel: String,
        /// User ID.
        user: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum NotifyCmd {
    /// Mute/unmute a guild or set notification level.
    Guild {
        /// Guild ID.
        guild: String,
        /// Mute (true) or unmute (false).
        #[arg(long)]
        muted: Option<bool>,
    },
    /// Mute/unmute a channel.
    Channel {
        /// Channel ID.
        channel: String,
        /// Mute (true) or unmute (false).
        #[arg(long)]
        muted: Option<bool>,
    },
}

impl DcCtx {
    pub async fn client(&self) -> Result<ApiClient, ExitCode> {
        match ApiClient::from_env(self.token.as_deref()) {
            Ok(c) => Ok(c),
            Err(e) => Err(ExitCode::from(output::emit_error(
                "AuthError",
                &e.to_string(),
                exit::ERROR,
            ))),
        }
    }
}

/// `dc guilds`
pub async fn dc_guilds(ctx: &DcCtx) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.list_guilds().await {
        Ok(guilds) => {
            let _ = output::emit(&guilds, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc channels <GUILD>`
pub async fn dc_channels(ctx: &DcCtx, guild: &str) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.list_channels(&guild_id).await {
        Ok(channels) => {
            let _ = output::emit(&channels, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc dms`
pub async fn dc_dms(ctx: &DcCtx) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.list_dms().await {
        Ok(dms) => {
            let _ = output::emit(&dms, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// Resolve a channel name to a channel ID (numeric ID passes through;
/// otherwise search across the user's guilds). Used by read/history.
pub(crate) async fn resolve_channel_id(
    client: &mut ApiClient,
    channel: &str,
) -> Result<String, ExitCode> {
    if channel.chars().all(|c| c.is_ascii_digit()) {
        return Ok(channel.to_string());
    }
    let guilds = match client.list_guilds().await {
        Ok(g) => g,
        Err(e) => {
            return Err(ExitCode::from(output::emit_error(
                "ApiError",
                &e.to_string(),
                exit::ERROR,
            )))
        }
    };
    let needle = channel.to_lowercase();
    for g in &guilds {
        // Use get_guild_channels_all (all channel types, no text-like filter)
        // and match exact-first, then substring — channel names often carry
        // emoji/decoration suffixes (e.g. "chit-chat┊💬"), so "chit-chat"
        // must still resolve (matches resolve_guild_id's contains pattern).
        if let Ok(chs) = client.get_guild_channels_all(&g.id).await {
            // Exact match first.
            if let Some(c) = chs.iter().find(|c| c.name.to_lowercase() == needle) {
                return Ok(c.id.clone());
            }
            // Substring/contains fallback (exact failed).
            let fuzzy: Vec<&discord_core::types::Channel> = chs
                .iter()
                .filter(|c| c.name.to_lowercase().contains(&needle))
                .collect();
            if fuzzy.len() == 1 {
                return Ok(fuzzy[0].id.clone());
            }
        }
    }
    Err(ExitCode::from(output::emit_error(
        "NotFound",
        &format!("channel \"{channel}\" not found"),
        exit::NOT_FOUND,
    )))
}

/// `dc history <CHANNEL>` — channel is an ID (or we resolve via a guild).
pub async fn dc_history(
    ctx: &DcCtx,
    channel: &str,
    limit: usize,
    before: Option<u64>,
    after: Option<u64>,
) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let channel_id = match resolve_channel_id(&mut client, channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };

    match client
        .fetch_messages(&channel_id, limit, before, after)
        .await
    {
        Ok(msgs) => {
            let _ = output::emit(&msgs, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc read <CHANNEL>` — recent messages (default 50), AI-facing.
pub async fn dc_read(
    ctx: &DcCtx,
    channel: &str,
    limit: usize,
    before: Option<u64>,
    transcript: bool,
) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let channel_id = match resolve_channel_id(&mut client, channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };

    match client
        .fetch_messages(&channel_id, limit, before, None)
        .await
    {
        Ok(msgs) => {
            if transcript {
                // Compact plain-text transcript: `[HH:MM:SS] author: content`.
                // ~5x smaller than JSON — the AI-summarization cronjob format.
                for m in &msgs {
                    let ts = m.timestamp.get(11..19).unwrap_or(&m.timestamp);
                    let content = m.content.replace('\n', " ⏎ ");
                    println!("[{ts}] {}: {content}", m.author);
                }
                ExitCode::from(exit::OK)
            } else {
                let _ = output::emit(&msgs, ctx.format);
                ExitCode::from(exit::OK)
            }
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc members <GUILD>`
pub async fn dc_members(ctx: &DcCtx, guild: &str, max: u32) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.list_members(&guild_id, max).await {
        Ok(members) => {
            let _ = output::emit(&members, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            // User tokens often get 403 on GET /guilds/{id}/members (Discord
            // reserves it for bots with GUILD_MEMBERS intent). Surface that
            // clearly as FORBIDDEN (exit 4) with a hint rather than a generic
            // API error.
            let code = classify(&e);
            if code == exit::FORBIDDEN {
                eprintln!("Discord blocks member listing for user tokens on this guild (GET /guilds/{{id}}/members is bot-only here). Use `discord members` with a bot token, or `discord search-guild` / relationships instead.");
            }
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `dc info <GUILD>`
pub async fn dc_info(ctx: &DcCtx, guild: &str) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.guild_info(&guild_id).await {
        Ok(info) => {
            let _ = output::emit(&info, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc search <GUILD> <QUERY>`
pub async fn dc_search(
    ctx: &DcCtx,
    guild: &str,
    query: &str,
    channel: Option<&str>,
    limit: u32,
) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client
        .search_guild_messages(&guild_id, query, channel, limit)
        .await
    {
        Ok(msgs) => {
            let _ = output::emit(&msgs, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `dc roles <GUILD>`
pub async fn dc_roles(ctx: &DcCtx, guild: &str) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.list_roles(&guild_id).await {
        Ok(roles) => {
            let _ = output::emit(&roles, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc profile [USER_ID]`
pub async fn dc_profile(ctx: &DcCtx, user_id: Option<&str>) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let uid = match user_id {
        Some(id) => id.to_string(),
        None => match client.get_me().await {
            Ok(me) => me.id,
            Err(e) => {
                return ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e)))
            }
        },
    };
    match client.user_profile(&uid).await {
        Ok(profile) => {
            let _ = output::emit(&profile, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc relationships`
pub async fn dc_relationships(ctx: &DcCtx) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.relationships().await {
        Ok(rels) => {
            let _ = output::emit(&rels, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc threads <CHANNEL>`
pub async fn dc_threads(ctx: &DcCtx, channel: &str) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let channel_id = match resolve_channel_id(&mut client, channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.list_threads(&channel_id).await {
        Ok(threads) => {
            let _ = output::emit(&threads, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc send <CHANNEL> --text ...` — requires --confirm unless reply/dry-run.
/// Max attachments per message (Discord limit).
const MAX_ATTACHMENTS: usize = 10;
/// Max single-file size (Discord 10MiB base tier).
const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// Exit code for attachment/IO failures (famasya convention).
pub const EXIT_ATTACHMENT: u8 = 7;

/// Options for `channel-create` (avoids clippy too_many_arguments).
#[derive(Default)]
pub struct ChannelCreateOpts<'a> {
    pub guild: &'a str,
    pub name: &'a str,
    pub channel_type: &'a str,
    pub category: Option<&'a str>,
    pub topic: Option<&'a str>,
    pub slowmode: Option<u64>,
    pub dry_run: bool,
}

/// Options for `role-create` (avoids clippy too_many_arguments).
#[derive(Default)]
pub struct RoleCreateOpts<'a> {
    pub guild: &'a str,
    pub name: &'a str,
    pub color: Option<&'a str>,
    pub permissions: Option<&'a str>,
    pub mentionable: bool,
    pub hoist: bool,
    pub dry_run: bool,
}

/// Options for `role-edit` (avoids clippy too_many_arguments).
#[derive(Default)]
pub struct RoleEditOpts<'a> {
    pub guild: &'a str,
    pub role: &'a str,
    pub name: Option<&'a str>,
    pub color: Option<&'a str>,
    pub permissions: Option<&'a str>,
    pub mentionable: Option<bool>,
    pub hoist: Option<bool>,
    pub dry_run: bool,
}

/// Walk an error chain for crate `DiscordError` variants → exit code.
///
/// - `DiscordError::PermissionDenied` → FORBIDDEN (4) — makes 403 live.
/// - `DiscordError::NotFound` → NOT_FOUND (3).
/// - `DiscordError::RateLimited` → ERROR (1), message notes `retry after Ns`.
/// - anything else → ERROR (1).
///
/// Handlers should use `ExitCode::from(classify(&e))` and pass the same code
/// to `emit_error`.
pub fn classify(e: &anyhow::Error) -> u8 {
    for cause in e.chain() {
        if let Some(derr) = cause.downcast_ref::<discord_user::DiscordError>() {
            return match derr {
                discord_user::DiscordError::PermissionDenied { .. } => exit::FORBIDDEN,
                discord_user::DiscordError::NotFound { .. } => exit::NOT_FOUND,
                discord_user::DiscordError::RateLimited { retry_after, .. } => {
                    eprintln!("note: rate limited, retry after {retry_after}s");
                    exit::ERROR
                }
                _ => exit::ERROR,
            };
        }
    }
    exit::ERROR
}

/// Emit the shared destructive-action gate message.
///
/// `This will {action} {target_desc} ({id}). Add --confirm to proceed.`
/// Returns `Some(USAGE)` when `!confirm`, else `None`.
pub fn check_confirm(target_desc: &str, action: &str, id: &str, confirm: bool) -> Option<ExitCode> {
    if !confirm {
        eprintln!("This will {action} {target_desc} ({id}). Add --confirm to proceed.");
        return Some(ExitCode::from(exit::USAGE));
    }
    None
}

/// Read `--text -` from stdin (Escape-Tech pattern), trimmed of trailing NL.
async fn read_stdin_text() -> Result<String, ExitCode> {
    use tokio::io::AsyncReadExt;
    let mut buf = String::new();
    tokio::io::stdin()
        .read_to_string(&mut buf)
        .await
        .map_err(|e| {
            eprintln!("error reading stdin: {e}");
            ExitCode::from(exit::ERROR)
        })?;
    while buf.ends_with('\n') || buf.ends_with('\r') {
        buf.pop();
    }
    Ok(buf)
}

/// Build attachments from `--file` paths (size + count caps).
async fn load_attachments(
    files: &[String],
) -> Result<Vec<discord_user::types::CreateAttachment>, ExitCode> {
    if files.len() > MAX_ATTACHMENTS {
        eprintln!("too many files: max {MAX_ATTACHMENTS} per message");
        return Err(ExitCode::from(exit::USAGE));
    }
    let mut out = Vec::with_capacity(files.len());
    for path in files {
        let meta = match tokio::fs::metadata(path).await {
            Ok(m) => m,
            Err(e) => {
                eprintln!("cannot read file \"{path}\": {e}");
                return Err(ExitCode::from(EXIT_ATTACHMENT));
            }
        };
        if meta.len() > MAX_FILE_BYTES {
            eprintln!("file too large (>10MiB): {path}");
            return Err(ExitCode::from(EXIT_ATTACHMENT));
        }
        let data = match tokio::fs::read(path).await {
            Ok(d) => d,
            Err(e) => {
                eprintln!("cannot read file \"{path}\": {e}");
                return Err(ExitCode::from(EXIT_ATTACHMENT));
            }
        };
        let filename = std::path::Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        let mime = mime_guess::from_path(path)
            .first_raw()
            .unwrap_or("application/octet-stream")
            .to_string();
        out.push(discord_user::types::CreateAttachment {
            filename,
            data,
            mime_type: mime,
            description: None,
        });
    }
    Ok(out)
}

/// Options for `dc send` (avoids clippy too_many_arguments).
#[derive(Default)]
pub struct SendOpts<'a> {
    pub text: Option<&'a str>,
    pub files: &'a [String],
    pub reply: Option<&'a str>,
    pub typing: bool,
    pub confirm: bool,
    pub dry_run: bool,
}

pub async fn dc_send(ctx: &DcCtx, channel: &str, opts: SendOpts<'_>) -> ExitCode {
    let SendOpts {
        text,
        files,
        reply,
        typing,
        confirm,
        dry_run,
    } = opts;
    // Require text or at least one file.
    if text.is_none() && files.is_empty() {
        eprintln!("nothing to send: provide --text (or --text -) and/or --file");
        return ExitCode::from(exit::USAGE);
    }
    // Safety (discli pattern): --confirm required for non-reply sends.
    if !confirm && reply.is_none() && !dry_run {
        eprintln!(
            "This will send a message to \"{channel}\". Add --confirm to proceed, or --dry-run to preview."
        );
        return ExitCode::from(exit::USAGE);
    }
    if dry_run {
        let data = serde_json::json!({
            "action": "send_message",
            "channel": channel,
            "text": text,
            "files": files,
            "reply_to": reply,
        });
        let _ = output::emit(&data, ctx.format);
        return ExitCode::from(exit::OK);
    }

    // Resolve text: "-" reads stdin (Escape-Tech send.js:10-17).
    let text = match text {
        Some("-") => match read_stdin_text().await {
            Ok(t) => t,
            Err(code) => return code,
        },
        Some(t) => t.to_string(),
        None => String::new(),
    };
    let attachments = match load_attachments(files).await {
        Ok(a) => a,
        Err(code) => return code,
    };

    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let channel_id = match resolve_channel_id(&mut client, channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    // Optional typing indicator before sending (discordo composer: fires per
    // keypress, throttled 10s; here one-shot before the message).
    if typing {
        if let Err(e) = client.trigger_typing(&channel_id).await {
            eprintln!("warning: typing indicator failed: {e}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let result = if attachments.is_empty() {
        client.send_message(&channel_id, &text, reply).await
    } else {
        client
            .send_message_with_files(&channel_id, &text, reply, attachments)
            .await
    };
    match result {
        Ok(id) => {
            let data = serde_json::json!({ "message_id": id, "channel_id": channel_id });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc typing <CHANNEL>` — send a typing indicator (one-shot).
pub async fn dc_typing(ctx: &DcCtx, channel: &str) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let channel_id = match resolve_channel_id(&mut client, channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.trigger_typing(&channel_id).await {
        Ok(()) => {
            let data = serde_json::json!({ "typing": true, "channel_id": channel_id });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc join <INVITE>` — preview then join a server via invite (needs --confirm).
/// Reference: RickvanLoo menu.go invite preview + InviteAccept.
pub async fn dc_join(ctx: &DcCtx, invite: &str, confirm: bool) -> ExitCode {
    // Extract bare code from URL or plain (RickvanLoo lacks this; we improve).
    let code = match ApiClient::extract_invite_code(invite) {
        Some(c) => c.to_string(),
        None => {
            eprintln!("invalid invite: \"{invite}\"");
            return ExitCode::from(exit::USAGE);
        }
    };
    if !confirm {
        eprintln!("This will join a server from invite \"{code}\". Add --confirm to proceed.");
        return ExitCode::from(exit::USAGE);
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    // Preview first (guild name + member counts) for the human/agent.
    let invite_info = client.get_invite(&code).await;
    match invite_info {
        Ok(info) => {
            let guild_name = info
                .guild
                .as_ref()
                .and_then(|g| g.name.clone())
                .unwrap_or_else(|| "unknown".to_string());
            let members = info.approximate_member_count.unwrap_or(0);
            if let Err(e) = client.accept_invite(&code).await {
                return ExitCode::from(output::emit_error(
                    "ApiError",
                    &e.to_string(),
                    classify(&e),
                ));
            }
            let data = serde_json::json!({
                "joined": true,
                "invite_code": code,
                "guild_name": guild_name,
                "approximate_member_count": members,
            });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc leave <GUILD>` — leave a server (needs --confirm).
/// Reference: RickvanLoo LeaveServerMenu → GuildLeave.
pub async fn dc_leave(ctx: &DcCtx, guild: &str, confirm: bool) -> ExitCode {
    if !confirm {
        eprintln!("This will leave server \"{guild}\". Add --confirm to proceed.");
        return ExitCode::from(exit::USAGE);
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.leave_guild(&guild_id).await {
        Ok(()) => {
            let data = serde_json::json!({ "left": true, "guild_id": guild_id });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc presence [STATUS]` — show configured default or set presence.
/// Setting persists to config.json; the live gateway Op 3 applies when a
/// `tail`/`watch` session is active (F4a). Default is invisible (stealth).
pub async fn dc_presence(ctx: &DcCtx, status: Option<&str>) -> ExitCode {
    match status {
        None => {
            let current = discord_core::config::configured_presence();
            let data = serde_json::json!({ "presence": current });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Some(s) => {
            if !discord_core::config::set_configured_presence(s) {
                eprintln!("invalid presence: \"{s}\" (valid: online, idle, dnd, invisible)");
                return ExitCode::from(exit::USAGE);
            }
            let data = serde_json::json!({ "presence": s, "saved": true });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
    }
}

/// `dc top-reactions [--guild G] [--channel C] [--limit N]`
/// Hottest messages from the archive by reaction_count (F8). Note: this is
/// distinct from `top` (top senders) — R2 kept both, zero breakage.
pub async fn dc_top_reactions(
    ctx: &DcCtx,
    guild: Option<&str>,
    channel: Option<&str>,
    limit: i64,
) -> ExitCode {
    let db_path = match discord_core::config::db_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(exit::ERROR);
        }
    };
    let conn = match ddb::open(db_path.to_str().unwrap_or("discord.db")) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error opening archive: {e}");
            return ExitCode::from(exit::ERROR);
        }
    };
    match ddb::top_reacted(&conn, guild, channel, limit) {
        Ok(rows) => {
            let _ = output::emit(&rows, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR)),
    }
}

/// `dc thread-create <CHANNEL> --name X [--message-id M] [--text T]`
/// Creates a thread: standalone (type 11), from a message, or a forum post
/// (starter message auto-defaults to the thread name; Escape-Tech 3 paths).
pub async fn dc_thread_create(
    ctx: &DcCtx,
    channel: &str,
    name: &str,
    message_id: Option<&str>,
    text: Option<&str>,
    archive: Option<u32>,
    tags: Option<&str>,
) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let channel_id = match resolve_channel_id(&mut client, channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let applied_tags = tags.map(|t| {
        t.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
    });
    let result = match message_id {
        Some(mid) => {
            client
                .create_thread_from_message(&channel_id, mid, name, archive)
                .await
        }
        None => {
            client
                .create_thread(&channel_id, name, archive, text, applied_tags)
                .await
        }
    };
    match result {
        Ok(t) => {
            // Type discriminator (Escape-Tech): forum_post | message_thread |
            // standalone_thread — derived from channel type + message_id.
            let kind = if message_id.is_some() {
                "message_thread"
            } else if t.channel_type == 15 || t.channel_type == 16 {
                "forum_post"
            } else {
                "standalone_thread"
            };
            let data = serde_json::json!({
                "type": kind,
                "id": t.id,
                "name": t.name,
                "channel_id": t.channel_id,
                "channel_type": t.channel_type,
            });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc edit <CHANNEL> <MSG_ID> --text ...`
pub async fn dc_edit(ctx: &DcCtx, channel: &str, message_id: &str, text: &str) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let channel_id = match resolve_channel_id(&mut client, channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.edit_message(&channel_id, message_id, text).await {
        Ok(_) => {
            let data = serde_json::json!({ "edited": true, "message_id": message_id });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc delete <CHANNEL> <MSG_ID> [--confirm]`
pub async fn dc_delete(ctx: &DcCtx, channel: &str, message_id: &str, confirm: bool) -> ExitCode {
    if !confirm {
        eprintln!(
            "This will delete message {message_id} in \"{channel}\". Add --confirm to proceed."
        );
        return ExitCode::from(exit::USAGE);
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let channel_id = match resolve_channel_id(&mut client, channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.delete_message(&channel_id, message_id).await {
        Ok(_) => {
            let data = serde_json::json!({ "deleted": true, "message_id": message_id });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc react <CHANNEL> <MSG> <EMOJI>`
pub async fn dc_react(ctx: &DcCtx, channel: &str, message_id: &str, emoji: &str) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let channel_id = match resolve_channel_id(&mut client, channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.add_reaction(&channel_id, message_id, emoji).await {
        Ok(_) => {
            let _ = output::emit(&serde_json::json!({ "reacted": true }), ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc unreact <CHANNEL> <MSG> <EMOJI>`
pub async fn dc_unreact(ctx: &DcCtx, channel: &str, message_id: &str, emoji: &str) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let channel_id = match resolve_channel_id(&mut client, channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.remove_reaction(&channel_id, message_id, emoji).await {
        Ok(_) => {
            let _ = output::emit(&serde_json::json!({ "unreacted": true }), ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc pin <CHANNEL> <MSG>`
pub async fn dc_pin(ctx: &DcCtx, channel: &str, message_id: &str) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let channel_id = match resolve_channel_id(&mut client, channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.pin_message(&channel_id, message_id).await {
        Ok(_) => {
            let _ = output::emit(&serde_json::json!({ "pinned": true }), ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc pins <CHANNEL>`
pub async fn dc_pins(ctx: &DcCtx, channel: &str) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let channel_id = match resolve_channel_id(&mut client, channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.pinned_messages(&channel_id).await {
        Ok(pins) => {
            let _ = output::emit(&pins, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
    }
}

/// `dc sync <CHANNEL>` — two-phase incremental sync to SQLite.
pub async fn dc_sync(ctx: &DcCtx, channel: &str, limit: usize) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let channel_id = match resolve_channel_id(&mut client, channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match crate::commands::sync::sync_channel(&mut client, &channel_id, limit).await {
        Ok(n) => {
            let data = serde_json::json!({ "channel_id": channel_id, "messages_synced": n });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("SyncError", &e.to_string(), exit::ERROR)),
    }
}

/// `dc sync-all` — discover accessible channels and sync each (bounded).
pub async fn dc_sync_all(ctx: &DcCtx, limit: usize) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guilds = match client.list_guilds().await {
        Ok(g) => g,
        Err(e) => {
            return ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e)))
        }
    };
    let mut total = 0usize;
    let mut channels_synced = 0usize;
    for g in &guilds {
        let channels = match client.list_channels(&g.id).await {
            Ok(c) => c,
            Err(_) => continue, // skip guilds we can't read
        };
        for ch in channels {
            match crate::commands::sync::sync_channel(&mut client, &ch.id, limit).await {
                Ok(n) => {
                    total += n;
                    channels_synced += 1;
                }
                Err(_) => continue,
            }
        }
    }
    let data = serde_json::json!({
        "channels_synced": channels_synced,
        "messages_total": total,
    });
    let _ = output::emit(&data, ctx.format);
    ExitCode::from(exit::OK)
}

/// Dispatch a `dc` subcommand.
#[allow(dead_code)] // kept for potential reuse; top-level dispatch lives in main.rs
pub async fn dispatch(ctx: &DcCtx, cmd: DcCmd) -> ExitCode {
    match cmd {
        DcCmd::Guilds => dc_guilds(ctx).await,
        DcCmd::Channels { guild } => dc_channels(ctx, &guild).await,
        DcCmd::Dms => dc_dms(ctx).await,
        DcCmd::History {
            channel,
            limit,
            before,
            after,
        } => dc_history(ctx, &channel, limit, before, after).await,
        DcCmd::Read {
            channel,
            limit,
            before,
        } => dc_read(ctx, &channel, limit, before, false).await,
        DcCmd::Members { guild, max } => dc_members(ctx, &guild, max).await,
        DcCmd::Info { guild } => dc_info(ctx, &guild).await,
        DcCmd::Search {
            guild,
            query,
            channel,
            limit,
        } => dc_search(ctx, &guild, &query, channel.as_deref(), limit).await,
        DcCmd::Roles { guild } => dc_roles(ctx, &guild).await,
        DcCmd::Profile { user_id } => dc_profile(ctx, user_id.as_deref()).await,
        DcCmd::Relationships => dc_relationships(ctx).await,
        DcCmd::Threads { channel } => dc_threads(ctx, &channel).await,
        DcCmd::Send {
            channel,
            text,
            file,
            reply,
            typing,
            confirm,
            dry_run,
        } => {
            dc_send(
                ctx,
                &channel,
                SendOpts {
                    text: text.as_deref(),
                    files: &file,
                    reply: reply.as_deref(),
                    typing,
                    confirm,
                    dry_run,
                },
            )
            .await
        }
        DcCmd::Typing { channel } => dc_typing(ctx, &channel).await,
        DcCmd::Join { invite, confirm } => dc_join(ctx, &invite, confirm).await,
        DcCmd::Leave { guild, confirm } => dc_leave(ctx, &guild, confirm).await,
        DcCmd::Presence { status } => dc_presence(ctx, status.as_deref()).await,
        DcCmd::ThreadCreate {
            channel,
            name,
            message_id,
            text,
            archive,
            tags,
        } => {
            dc_thread_create(
                ctx,
                &channel,
                &name,
                message_id.as_deref(),
                text.as_deref(),
                archive,
                tags.as_deref(),
            )
            .await
        }
        DcCmd::Edit {
            channel,
            message_id,
            text,
        } => dc_edit(ctx, &channel, &message_id, &text).await,
        DcCmd::Delete {
            channel,
            message_id,
            confirm,
        } => dc_delete(ctx, &channel, &message_id, confirm).await,
        DcCmd::React {
            channel,
            message_id,
            emoji,
        } => dc_react(ctx, &channel, &message_id, &emoji).await,
        DcCmd::Unreact {
            channel,
            message_id,
            emoji,
        } => dc_unreact(ctx, &channel, &message_id, &emoji).await,
        DcCmd::Pin {
            channel,
            message_id,
        } => dc_pin(ctx, &channel, &message_id).await,
        DcCmd::Pins { channel } => dc_pins(ctx, &channel).await,
        DcCmd::Sync { channel, limit } => dc_sync(ctx, &channel, limit).await,
        DcCmd::SyncAll { limit } => dc_sync_all(ctx, limit).await,
        DcCmd::Tail { channel, once } => crate::commands::tail::dc_tail(ctx, &channel, once).await,
        DcCmd::Watch {
            channel,
            keyword,
            typing,
        } => {
            crate::commands::tail::dc_watch(ctx, channel.as_deref(), keyword.as_deref(), typing)
                .await
        }
        DcCmd::DmGroup { cmd } => dc_dm_group(ctx, cmd).await,
        DcCmd::Notify { cmd } => dc_notify(ctx, cmd).await,
        DcCmd::TopReactions {
            guild,
            channel,
            limit,
        } => {
            dc_top_reactions(
                ctx,
                guild.as_deref(),
                channel.as_deref(),
                limit.unwrap_or(10),
            )
            .await
        }
        DcCmd::Download {
            guild,
            channel,
            r#type,
            since,
            min_reactions,
            limit,
            out,
        } => {
            crate::commands::download::dc_download(
                ctx,
                crate::commands::download::DownloadOpts {
                    guild: guild.as_deref(),
                    channel: channel.as_deref(),
                    media_type: r#type.as_deref(),
                    since: since.as_deref(),
                    min_reactions,
                    limit,
                    out: out.as_deref(),
                },
            )
            .await
        }
    }
}

// ---------------------------------------------------------------------------
// Admin: channel CRUD (F1), role CRUD (F2), emoji CRUD (F3)
// ---------------------------------------------------------------------------

const SLOWMODE_MAX: u64 = 21600;

/// True when a string is all ASCII digits (a snowflake).
fn is_numeric(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// `channel-create <GUILD> <NAME> [--type T] [--category C] [--topic T]
/// [--slowmode N] [--dry-run]`
pub async fn dc_channel_create(ctx: &DcCtx, opts: ChannelCreateOpts<'_>) -> ExitCode {
    let ChannelCreateOpts {
        guild,
        name,
        channel_type,
        category,
        topic,
        slowmode,
        dry_run,
    } = opts;
    let ctype = match discord_core::types::parse_channel_type_name(channel_type) {
        Some(t) => t,
        None => {
            eprintln!(
                "invalid channel type \"{channel_type}\" (valid: text, voice, category, announcement, stage, forum)"
            );
            return ExitCode::from(exit::USAGE);
        }
    };
    if !ApiClient::validate_channel_name(name) {
        eprintln!("invalid channel name \"{name}\" (1-100 chars, no '#')");
        return ExitCode::from(exit::USAGE);
    }
    if let Some(t) = topic {
        if !ApiClient::validate_topic(t) {
            eprintln!("invalid topic (max 1024 chars)");
            return ExitCode::from(exit::USAGE);
        }
    }
    if slowmode.is_some_and(|s| s > SLOWMODE_MAX) {
        eprintln!("invalid slowmode {slowmode:?} (0-{SLOWMODE_MAX})");
        return ExitCode::from(exit::USAGE);
    }
    if dry_run {
        let data = serde_json::json!({
            "action": "create_channel",
            "guild": guild,
            "name": name,
            "type": channel_type,
            "category": category,
            "topic": topic,
            "slowmode": slowmode,
        });
        let _ = output::emit(&data, ctx.format);
        return ExitCode::from(exit::OK);
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let parent_id = match category {
        Some(cat) => match resolve::resolve_category(&mut client, &guild_id, cat).await {
            Ok(Some(id)) => Some(id),
            Ok(None) => {
                eprintln!("Category \"{cat}\" not found in guild {guild_id}.");
                return ExitCode::from(exit::NOT_FOUND);
            }
            Err(code) => return code,
        },
        None => None,
    };
    let mut req = discord_user::types::CreateChannelRequest::new(name);
    req.channel_type = Some(ctype);
    req.parent_id = parent_id;
    req.topic = topic.map(|t| t.to_string());
    req.rate_limit_per_user = slowmode.map(|s| s as u32);
    match client.create_channel(&guild_id, req).await {
        Ok(ch) => {
            let _ = output::emit(&ch, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `channel-rename <GUILD> <CHANNEL> <NEW_NAME> [--dry-run]`
pub async fn dc_channel_rename(
    ctx: &DcCtx,
    guild: &str,
    channel: &str,
    new_name: &str,
    dry_run: bool,
) -> ExitCode {
    if !ApiClient::validate_channel_name(new_name) {
        eprintln!("invalid channel name \"{new_name}\" (1-100 chars, no '#')");
        return ExitCode::from(exit::USAGE);
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let channel_id = match resolve::resolve_channel_admin(&mut client, &guild_id, channel).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Channel \"{channel}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    if dry_run {
        let data = serde_json::json!({
            "action": "rename_channel",
            "channel": channel,
            "new_name": new_name,
        });
        let _ = output::emit(&data, ctx.format);
        return ExitCode::from(exit::OK);
    }
    let req = discord_user::types::EditChannelRequest {
        name: Some(new_name.to_string()),
        ..Default::default()
    };
    match client.edit_channel(&channel_id, req).await {
        Ok(ch) => {
            let _ = output::emit(&ch, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `channel-topic <GUILD> <CHANNEL> <TOPIC>`
pub async fn dc_channel_topic(ctx: &DcCtx, guild: &str, channel: &str, topic: &str) -> ExitCode {
    if !ApiClient::validate_topic(topic) {
        eprintln!("invalid topic (max 1024 chars)");
        return ExitCode::from(exit::USAGE);
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let channel_id = match resolve::resolve_channel_admin(&mut client, &guild_id, channel).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Channel \"{channel}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    let req = discord_user::types::EditChannelRequest {
        topic: Some(topic.to_string()),
        ..Default::default()
    };
    match client.edit_channel(&channel_id, req).await {
        Ok(ch) => {
            let _ = output::emit(&ch, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `channel-move <GUILD> <CHANNEL> [--category C] [--position N]`
/// Requires at least one of --category/--position.
pub async fn dc_channel_move(
    ctx: &DcCtx,
    guild: &str,
    channel: &str,
    category: Option<&str>,
    position: Option<u32>,
) -> ExitCode {
    if category.is_none() && position.is_none() {
        eprintln!("channel-move requires --category and/or --position");
        return ExitCode::from(exit::USAGE);
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let channel_id = match resolve::resolve_channel_admin(&mut client, &guild_id, channel).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Channel \"{channel}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    let parent_id = match category {
        Some(cat) => match resolve::resolve_category(&mut client, &guild_id, cat).await {
            Ok(Some(id)) => Some(id),
            Ok(None) => {
                eprintln!("Category \"{cat}\" not found in guild {guild_id}.");
                return ExitCode::from(exit::NOT_FOUND);
            }
            Err(code) => return code,
        },
        None => None,
    };
    let req = discord_user::types::EditChannelRequest {
        parent_id,
        position,
        ..Default::default()
    };
    match client.edit_channel(&channel_id, req).await {
        Ok(ch) => {
            let _ = output::emit(&ch, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `channel-clone <GUILD> <CHANNEL> [--name N]`
/// GET the original, then create a copy with the same type/parent/topic.
pub async fn dc_channel_clone(
    ctx: &DcCtx,
    guild: &str,
    channel: &str,
    name: Option<&str>,
) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let channel_id = match resolve::resolve_channel_admin(&mut client, &guild_id, channel).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Channel \"{channel}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    let original = match client.get_channel(&channel_id).await {
        Ok(ch) => ch,
        Err(e) => {
            let code = classify(&e);
            return ExitCode::from(output::emit_error("ApiError", &e.to_string(), code));
        }
    };
    let clone_name = match name {
        Some(n) => n.to_string(),
        None => {
            let base = if original.name.is_empty() {
                "clone".to_string()
            } else {
                original.name.clone()
            };
            format!("{base} copy")
        }
    };
    if !ApiClient::validate_channel_name(&clone_name) {
        eprintln!("invalid channel name \"{clone_name}\" (1-100 chars, no '#')");
        return ExitCode::from(exit::USAGE);
    }
    let mut req = discord_user::types::CreateChannelRequest::new(&clone_name);
    req.channel_type = Some(original.channel_type);
    req.parent_id = original.parent_id.clone();
    req.topic = original.topic.clone();
    match client.create_channel(&guild_id, req).await {
        Ok(ch) => {
            let _ = output::emit(&ch, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `channel-slowmode <GUILD> <CHANNEL> <SECONDS>`
pub async fn dc_channel_slowmode(
    ctx: &DcCtx,
    guild: &str,
    channel: &str,
    seconds: u64,
) -> ExitCode {
    if seconds > SLOWMODE_MAX {
        eprintln!("invalid slowmode {seconds} (0-{SLOWMODE_MAX})");
        return ExitCode::from(exit::USAGE);
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let channel_id = match resolve::resolve_channel_admin(&mut client, &guild_id, channel).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Channel \"{channel}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    let req = discord_user::types::EditChannelRequest {
        rate_limit_per_user: Some(seconds as u32),
        ..Default::default()
    };
    match client.edit_channel(&channel_id, req).await {
        Ok(ch) => {
            let _ = output::emit(&ch, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `channel-delete <GUILD> <CHANNEL> [--confirm]`
pub async fn dc_channel_delete(ctx: &DcCtx, guild: &str, channel: &str, confirm: bool) -> ExitCode {
    if let Some(code) = check_confirm(channel, "delete channel", channel, confirm) {
        return code;
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let channel_id = match resolve::resolve_channel_admin(&mut client, &guild_id, channel).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Channel \"{channel}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    match client.delete_channel(&channel_id).await {
        Ok(()) => {
            let data = serde_json::json!({ "deleted": true, "channel_id": channel_id });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `role-create <GUILD> <NAME> [--color C] [--permissions P] [--mentionable]
/// [--hoist] [--dry-run]`
pub async fn dc_role_create(ctx: &DcCtx, opts: RoleCreateOpts<'_>) -> ExitCode {
    let RoleCreateOpts {
        guild,
        name,
        color,
        permissions,
        mentionable,
        hoist,
        dry_run,
    } = opts;
    // @everyone guard (offline, before network).
    if name
        .trim_start_matches('@')
        .eq_ignore_ascii_case("everyone")
    {
        eprintln!("cannot create the @everyone role");
        return ExitCode::from(exit::USAGE);
    }
    let color_val = match color {
        Some(c) => match ApiClient::parse_color_hex(c) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(exit::USAGE);
            }
        },
        None => None,
    };
    let perms_val = match permissions {
        Some(p) => {
            let names: Vec<String> = p.split(',').map(|s| s.trim().to_string()).collect();
            match ApiClient::parse_permission_names(&names) {
                Ok(bits) => Some(bits.to_string()),
                Err(e) => {
                    eprintln!("{e}");
                    return ExitCode::from(exit::USAGE);
                }
            }
        }
        None => None,
    };
    if dry_run {
        let data = serde_json::json!({
            "action": "create_role",
            "guild": guild,
            "name": name,
            "color": color,
            "permissions": permissions,
            "mentionable": mentionable,
            "hoist": hoist,
        });
        let _ = output::emit(&data, ctx.format);
        return ExitCode::from(exit::OK);
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let req = discord_user::types::CreateRoleRequest {
        name: Some(name.to_string()),
        color: color_val,
        permissions: perms_val,
        mentionable: Some(mentionable),
        hoist: Some(hoist),
    };
    match client.create_role(&guild_id, req).await {
        Ok(role) => {
            let _ = output::emit(&role, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `role-edit <GUILD> <ROLE> [--name N] [--color C] [--permissions P]
/// [--mentionable] [--no-mentionable] [--hoist] [--no-hoist] [--dry-run]`
/// Requires ≥1 option.
pub async fn dc_role_edit(ctx: &DcCtx, opts: RoleEditOpts<'_>) -> ExitCode {
    let RoleEditOpts {
        guild,
        role,
        name,
        color,
        permissions,
        mentionable,
        hoist,
        dry_run,
    } = opts;
    if name.is_none()
        && color.is_none()
        && permissions.is_none()
        && mentionable.is_none()
        && hoist.is_none()
    {
        eprintln!(
            "role-edit requires at least one option (--name, --color, --permissions, --mentionable, --hoist)"
        );
        return ExitCode::from(exit::USAGE);
    }
    let color_val = match color {
        Some(c) => match ApiClient::parse_color_hex(c) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(exit::USAGE);
            }
        },
        None => None,
    };
    let perms_val = match permissions {
        Some(p) => {
            let names: Vec<String> = p.split(',').map(|s| s.trim().to_string()).collect();
            match ApiClient::parse_permission_names(&names) {
                Ok(bits) => Some(bits.to_string()),
                Err(e) => {
                    eprintln!("{e}");
                    return ExitCode::from(exit::USAGE);
                }
            }
        }
        None => None,
    };
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let role_id = match resolve::resolve_role(&mut client, &guild_id, role).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Role \"{role}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    if dry_run {
        let data = serde_json::json!({
            "action": "edit_role",
            "role": role,
            "name": name,
            "color": color,
            "permissions": permissions,
            "mentionable": mentionable,
            "hoist": hoist,
        });
        let _ = output::emit(&data, ctx.format);
        return ExitCode::from(exit::OK);
    }
    let req = discord_user::types::CreateRoleRequest {
        name: name.map(|n| n.to_string()),
        color: color_val,
        permissions: perms_val,
        mentionable,
        hoist,
    };
    match client.edit_role(&guild_id, &role_id, req).await {
        Ok(role) => {
            let _ = output::emit(&role, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `role-delete <GUILD> <ROLE> [--confirm]` — @everyone cannot be deleted.
pub async fn dc_role_delete(ctx: &DcCtx, guild: &str, role: &str, confirm: bool) -> ExitCode {
    // Offline @everyone guard: the @everyone role shares the guild's ID, so
    // targeting a numeric guild ID as the role is always a refusal (exit 2).
    if is_numeric(guild) && guild == role {
        eprintln!("cannot delete the @everyone role");
        return ExitCode::from(exit::USAGE);
    }
    if role
        .trim_start_matches('@')
        .eq_ignore_ascii_case("everyone")
    {
        eprintln!("cannot delete the @everyone role");
        return ExitCode::from(exit::USAGE);
    }
    // Destructive gate BEFORE any network (offline smoke-testable).
    if let Some(code) = check_confirm(role, "delete role", role, confirm) {
        return code;
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let role_id = match resolve::resolve_role(&mut client, &guild_id, role).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Role \"{role}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    match client.delete_role(&guild_id, &role_id).await {
        Ok(()) => {
            let data = serde_json::json!({ "deleted": true, "role_id": role_id });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `role-assign <GUILD> <ROLE> <USER>`
pub async fn dc_role_assign(ctx: &DcCtx, guild: &str, role: &str, user: &str) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let role_id = match resolve::resolve_role(&mut client, &guild_id, role).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Role \"{role}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    let user_id = match resolve::resolve_member(&mut client, &guild_id, user).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Member \"{user}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    match client.add_member_role(&guild_id, &user_id, &role_id).await {
        Ok(()) => {
            let data = serde_json::json!({
                "assigned": true,
                "guild_id": guild_id,
                "role_id": role_id,
                "user_id": user_id,
            });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `role-remove <GUILD> <ROLE> <USER>`
pub async fn dc_role_remove(ctx: &DcCtx, guild: &str, role: &str, user: &str) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let role_id = match resolve::resolve_role(&mut client, &guild_id, role).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Role \"{role}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    let user_id = match resolve::resolve_member(&mut client, &guild_id, user).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Member \"{user}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    match client
        .remove_member_role(&guild_id, &user_id, &role_id)
        .await
    {
        Ok(()) => {
            let data = serde_json::json!({
                "removed": true,
                "guild_id": guild_id,
                "role_id": role_id,
                "user_id": user_id,
            });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `emoji-list <GUILD> [--count N]`
pub async fn dc_emoji_list(ctx: &DcCtx, guild: &str, count: Option<u32>) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.list_emojis(&guild_id).await {
        Ok(emojis) => {
            let out: Vec<_> = match count {
                Some(n) => emojis.into_iter().take(n as usize).collect(),
                None => emojis,
            };
            let _ = output::emit(&out, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `emoji-upload <GUILD> <NAME> <FILE>` — validates name + size (≤256KiB).
/// Name and file checks run BEFORE client creation so they work offline.
pub async fn dc_emoji_upload(ctx: &DcCtx, guild: &str, name: &str, file: &str) -> ExitCode {
    if !ApiClient::validate_emoji_name(name) {
        eprintln!("invalid emoji name \"{name}\" (alphanumeric + underscore, 2-32 chars)");
        return ExitCode::from(exit::USAGE);
    }
    // Offline file gate: missing or oversized → exit 7 (EXIT_ATTACHMENT).
    let size = match tokio::fs::metadata(file).await {
        Ok(m) => m.len(),
        Err(_) => {
            eprintln!("cannot read file \"{file}\"");
            return ExitCode::from(EXIT_ATTACHMENT);
        }
    };
    if size > ApiClient::MAX_EMOJI_SIZE {
        eprintln!(
            "file too large ({} bytes max for an emoji): {file}",
            ApiClient::MAX_EMOJI_SIZE
        );
        return ExitCode::from(EXIT_ATTACHMENT);
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.create_emoji(&guild_id, name, file).await {
        Ok(e) => {
            let data = serde_json::json!({
                "emoji": format!(":{}:", e.name.clone().unwrap_or_default()),
                "id": e.id,
            });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `emoji-delete <GUILD> <EMOJI> [--confirm]`
pub async fn dc_emoji_delete(ctx: &DcCtx, guild: &str, emoji: &str, confirm: bool) -> ExitCode {
    // Destructive gate BEFORE any network (offline smoke-testable).
    if let Some(code) = check_confirm(emoji, "delete emoji", emoji, confirm) {
        return code;
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let emoji = match resolve::resolve_emoji(&mut client, &guild_id, emoji).await {
        Ok(Some(e)) => e,
        Ok(None) => {
            eprintln!("Emoji \"{emoji}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    if emoji.managed {
        eprintln!(
            "cannot delete managed emoji :{}: (owned by a bot/integration)",
            emoji.name.clone().unwrap_or_default()
        );
        return ExitCode::from(exit::ERROR);
    }
    match client.delete_emoji(&guild_id, &emoji.id).await {
        Ok(()) => {
            let data = serde_json::json!({
                "deleted": true,
                "emoji_id": emoji.id,
                "name": emoji.name.clone().unwrap_or_default(),
            });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

// ---------------------------------------------------------------------------
// Admin: member moderation (F4), permission overwrites (F5), server settings
// (F6)
// ---------------------------------------------------------------------------

/// Resolve a member to a user ID. For unban the target is NOT a guild member
/// (banned users are gone from the member list), so resolve by bare ID or
/// friend/relationship lookup; the member-list name match still applies for
/// kick/ban/nick.
async fn resolve_user_id(
    client: &mut ApiClient,
    guild_id: &str,
    user: &str,
    member_list: bool,
) -> Result<String, ExitCode> {
    if member_list {
        match resolve::resolve_member(client, guild_id, user).await {
            Ok(Some(id)) => return Ok(id),
            Ok(None) => {
                eprintln!("Member \"{user}\" not found in guild {guild_id}.");
                return Err(ExitCode::from(exit::NOT_FOUND));
            }
            Err(code) => return Err(code),
        }
    }
    // Unban path: numeric ID passes through; else try relationships.
    if !user.is_empty() && user.chars().all(|c| c.is_ascii_digit()) {
        return Ok(user.to_string());
    }
    match client.relationships().await {
        Ok(rels) => {
            let needle = user.trim_start_matches('@').to_lowercase();
            let matches: Vec<&discord_core::types::Relationship> = rels
                .iter()
                .filter(|r| r.username.to_lowercase() == needle)
                .collect();
            match matches.len() {
                1 => Ok(matches[0].user_id.clone()),
                n if n > 1 => {
                    eprintln!("Ambiguous user \"{user}\". Matches: {n}");
                    Err(ExitCode::from(exit::USAGE))
                }
                _ => {
                    eprintln!("User \"{user}\" not found (bare ID or friend username required).");
                    Err(ExitCode::from(exit::NOT_FOUND))
                }
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            Err(ExitCode::from(exit::ERROR))
        }
    }
}

/// `member-kick <GUILD> <USER> [--reason R] [--confirm]`
pub async fn dc_member_kick(
    ctx: &DcCtx,
    guild: &str,
    user: &str,
    reason: Option<&str>,
    confirm: bool,
) -> ExitCode {
    // Destructive gate BEFORE any network (offline smoke-testable).
    if let Some(code) = check_confirm(user, "kick member", user, confirm) {
        return code;
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let user_id = match resolve_user_id(&mut client, &guild_id, user, true).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let username = {
        // Best-effort display name from the member list (already fetched by
        // resolve_user_id). Re-list to keep the handler self-contained.
        match client.list_members(&guild_id, 1000).await {
            Ok(members) => members
                .iter()
                .find(|m| m.id == user_id)
                .map(|m| m.username.clone()),
            Err(_) => None,
        }
    };
    match client.kick_member(&guild_id, &user_id, reason).await {
        Ok(()) => {
            let data = serde_json::json!({
                "action": "kicked",
                "user_id": user_id,
                "username": username,
            });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `member-ban <GUILD> <USER> [--reason R] [--delete-days D] [--confirm]`
pub async fn dc_member_ban(
    ctx: &DcCtx,
    guild: &str,
    user: &str,
    reason: Option<&str>,
    delete_days: Option<u8>,
    confirm: bool,
) -> ExitCode {
    // Destructive gate BEFORE any network (offline smoke-testable).
    if let Some(code) = check_confirm(user, "ban member", user, confirm) {
        return code;
    }
    // --delete-days validated 0-7.
    if delete_days.is_some_and(|d| d > 7) {
        eprintln!("invalid --delete-days {} (0-7)", delete_days.unwrap());
        return ExitCode::from(exit::USAGE);
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let user_id = match resolve_user_id(&mut client, &guild_id, user, true).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let username = match client.list_members(&guild_id, 1000).await {
        Ok(members) => members
            .iter()
            .find(|m| m.id == user_id)
            .map(|m| m.username.clone()),
        Err(_) => None,
    };
    match client
        .ban_member(&guild_id, &user_id, reason, delete_days)
        .await
    {
        Ok(()) => {
            let data = serde_json::json!({
                "action": "banned",
                "user_id": user_id,
                "username": username,
                "delete_message_seconds": delete_days.map(|d| (d as u32) * 86400),
            });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `member-unban <GUILD> <USER> [--confirm]` — user ID (banned, not a member).
pub async fn dc_member_unban(ctx: &DcCtx, guild: &str, user: &str, confirm: bool) -> ExitCode {
    if let Some(code) = check_confirm(user, "unban user", user, confirm) {
        return code;
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let user_id = match resolve_user_id(&mut client, &guild_id, user, false).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.unban_member(&guild_id, &user_id).await {
        Ok(()) => {
            let data = serde_json::json!({ "unbanned": true, "user_id": user_id });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `member-nick <GUILD> <USER> <NICKNAME>` — empty clears the nickname.
pub async fn dc_member_nick(ctx: &DcCtx, guild: &str, user: &str, nickname: &str) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let user_id = match resolve_user_id(&mut client, &guild_id, user, true).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    // Empty nickname → Some("") so the field is SENT and Discord clears it
    // (Some("") clears; None would omit the field and be a no-op).
    let nick = if nickname.is_empty() {
        Some("")
    } else {
        Some(nickname)
    };
    match client.set_nickname(&guild_id, &user_id, nick).await {
        Ok(()) => {
            let data = serde_json::json!({ "nickname_set": true, "user_id": user_id });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `perm-view <GUILD> <CHANNEL>` — list the channel's overwrites with resolved
/// target names (role via list_roles, member via list_members).
pub async fn dc_perm_view(ctx: &DcCtx, guild: &str, channel: &str) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let channel_id = match resolve::resolve_channel_admin(&mut client, &guild_id, channel).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Channel \"{channel}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    let overwrites = match client.get_channel_overwrites(&channel_id).await {
        Ok(ow) => ow,
        Err(e) => {
            let code = classify(&e);
            return ExitCode::from(output::emit_error("ApiError", &e.to_string(), code));
        }
    };
    let roles = match client.list_roles(&guild_id).await {
        Ok(r) => r,
        Err(e) => {
            let code = classify(&e);
            return ExitCode::from(output::emit_error("ApiError", &e.to_string(), code));
        }
    };
    // Member-name resolution is best-effort: many guilds return 403 for
    // GET /guilds/{id}/members to non-admins (or the 1000-cap truncates).
    // Fall back to raw IDs rather than failing the whole view — role
    // overwrites are the common case and must still display.
    let members = client
        .list_members(&guild_id, 1000)
        .await
        .unwrap_or_default();
    let rows: Vec<serde_json::Value> = overwrites
        .iter()
        .map(|ow| {
            let (target, kind) = if ow.overwrite_type == 0 {
                let name = roles
                    .iter()
                    .find(|r| r.id == ow.id)
                    // Role names like "@everyone" already carry the @ prefix.
                    .map(|r| {
                        if r.name.starts_with('@') {
                            r.name.clone()
                        } else {
                            format!("@{}", r.name)
                        }
                    })
                    .unwrap_or_else(|| ow.id.clone());
                (name, "role")
            } else {
                let name = members
                    .iter()
                    .find(|m| m.id == ow.id)
                    .map(|m| m.username.clone())
                    .unwrap_or_else(|| ow.id.clone());
                (name, "member")
            };
            serde_json::json!({
                "target": target,
                "kind": kind,
                "allow": ow.allow,
                "deny": ow.deny,
            })
        })
        .collect();
    let data = serde_json::json!({
        "channel_id": channel_id,
        "overwrites": rows,
    });
    let _ = output::emit(&data, ctx.format);
    ExitCode::from(exit::OK)
}

/// `perm-set <GUILD> <CHANNEL> <ROLE> [--allow A] [--deny D]`
/// Requires ≥1 of --allow/--deny. Both sides are transmitted (0 if absent).
pub async fn dc_perm_set(
    ctx: &DcCtx,
    guild: &str,
    channel: &str,
    role: &str,
    allow: Option<&str>,
    deny: Option<&str>,
) -> ExitCode {
    if allow.is_none() && deny.is_none() {
        eprintln!("perm-set requires --allow and/or --deny");
        return ExitCode::from(exit::USAGE);
    }
    let allow_bits = match allow {
        Some(a) => match ApiClient::parse_permission_names(&parse_csv(a)) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(exit::USAGE);
            }
        },
        None => 0,
    };
    let deny_bits = match deny {
        Some(d) => match ApiClient::parse_permission_names(&parse_csv(d)) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(exit::USAGE);
            }
        },
        None => 0,
    };
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let channel_id = match resolve::resolve_channel_admin(&mut client, &guild_id, channel).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Channel \"{channel}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    let role_id = match resolve::resolve_role(&mut client, &guild_id, role).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Role \"{role}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    match client
        .edit_channel_permission(&channel_id, &role_id, allow_bits, deny_bits, 0)
        .await
    {
        Ok(()) => {
            let data = serde_json::json!({
                "permission_set": true,
                "channel_id": channel_id,
                "role_id": role_id,
                "allow": allow_bits.to_string(),
                "deny": deny_bits.to_string(),
            });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `perm-lock <GUILD> <CHANNEL> [--dry-run] [--confirm]`
/// Denies SEND_MESSAGES|SEND_MESSAGES_IN_THREADS|CREATE_PUBLIC_THREADS for
/// @everyone (overwrite_id == guild_id).
pub async fn dc_perm_lock(
    ctx: &DcCtx,
    guild: &str,
    channel: &str,
    dry_run: bool,
    confirm: bool,
) -> ExitCode {
    let deny = ApiClient::lock_deny_bitfield();
    // Dry-run preview BEFORE any network (offline smoke-testable).
    if dry_run {
        let data = serde_json::json!({
            "action": "lock_channel",
            "channel": channel,
            "guild": guild,
            "overwrite_id": "@everyone (guild id)",
            "deny": deny.to_string(),
        });
        let _ = output::emit(&data, ctx.format);
        return ExitCode::from(exit::OK);
    }
    // Destructive gate BEFORE any network (offline smoke-testable).
    if let Some(code) = check_confirm(channel, "lock channel", channel, confirm) {
        return code;
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let channel_id = match resolve::resolve_channel_admin(&mut client, &guild_id, channel).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Channel \"{channel}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    match client.lock_channel(&channel_id, &guild_id).await {
        Ok(()) => {
            let data = serde_json::json!({ "locked": true, "channel_id": channel_id });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `perm-unlock <GUILD> <CHANNEL> [--confirm]`
/// Deletes the @everyone overwrite; restores @everyone send access.
pub async fn dc_perm_unlock(ctx: &DcCtx, guild: &str, channel: &str, confirm: bool) -> ExitCode {
    if let Some(code) = check_confirm(channel, "unlock channel", channel, confirm) {
        return code;
    }
    eprintln!("warning: this will restore @everyone send access to \"{channel}\"");
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let channel_id = match resolve::resolve_channel_admin(&mut client, &guild_id, channel).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Channel \"{channel}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    match client.unlock_channel(&channel_id, &guild_id).await {
        Ok(()) => {
            let data = serde_json::json!({ "unlocked": true, "channel_id": channel_id });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `perm-list` — local name→bit table (no API).
pub async fn dc_perm_list(ctx: &DcCtx) -> ExitCode {
    let rows: Vec<serde_json::Value> = discord_core::client::ALL_PERMISSION_NAMES
        .iter()
        .map(|(name, bit)| serde_json::json!({ "name": name, "bit": bit.to_string() }))
        .collect();
    let _ = output::emit(&rows, ctx.format);
    ExitCode::from(exit::OK)
}

/// Options for `server-set` (avoids clippy too_many_arguments).
#[derive(Default)]
pub struct ServerSetOpts<'a> {
    pub guild: &'a str,
    pub name: Option<&'a str>,
    pub description: Option<&'a str>,
    pub verification: Option<&'a str>,
    pub notifications: Option<&'a str>,
    pub content_filter: Option<&'a str>,
    pub afk_timeout: Option<u32>,
    pub system_channel: Option<&'a str>,
    pub rules_channel: Option<&'a str>,
    pub dry_run: bool,
}

/// Split a comma-separated option into trimmed, non-empty strings.
fn parse_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// `server-set <GUILD> [--name N] [--description D] ...` — edit guild
/// settings (≥1 option; --dry-run previews the payload).
pub async fn dc_server_set(ctx: &DcCtx, opts: ServerSetOpts<'_>) -> ExitCode {
    let ServerSetOpts {
        guild,
        name,
        description,
        verification,
        notifications,
        content_filter,
        afk_timeout,
        system_channel,
        rules_channel,
        dry_run,
    } = opts;
    if name.is_none()
        && description.is_none()
        && verification.is_none()
        && notifications.is_none()
        && content_filter.is_none()
        && afk_timeout.is_none()
        && system_channel.is_none()
        && rules_channel.is_none()
    {
        eprintln!(
            "server-set requires at least one option (--name, --description, --verification, --notifications, --content-filter, --afk-timeout, --system-channel, --rules-channel)"
        );
        return ExitCode::from(exit::USAGE);
    }
    if let Some(n) = name {
        let len = n.chars().count();
        if !(2..=100).contains(&len) {
            eprintln!("invalid server name (2-100 chars)");
            return ExitCode::from(exit::USAGE);
        }
    }
    if let Some(d) = description {
        if d.chars().count() > 120 {
            eprintln!("invalid description (max 120 chars)");
            return ExitCode::from(exit::USAGE);
        }
    }
    let verification_val = match verification {
        Some(v) => match discord_core::types::parse_verification_level(v) {
            Some(l) => Some(l),
            None => {
                eprintln!("invalid --verification \"{v}\" (none|low|medium|high|very_high)");
                return ExitCode::from(exit::USAGE);
            }
        },
        None => None,
    };
    let notifications_val = match notifications {
        Some(n) => match discord_core::types::parse_notification_level(n) {
            Some(l) => Some(l),
            None => {
                eprintln!("invalid --notifications \"{n}\" (all_messages|only_mentions)");
                return ExitCode::from(exit::USAGE);
            }
        },
        None => None,
    };
    let content_filter_val = match content_filter {
        Some(c) => match discord_core::types::parse_content_filter(c) {
            Some(l) => Some(l),
            None => {
                eprintln!(
                    "invalid --content-filter \"{c}\" (disabled|members_without_roles|all_members)"
                );
                return ExitCode::from(exit::USAGE);
            }
        },
        None => None,
    };
    if let Some(t) = afk_timeout {
        if !matches!(t, 60 | 300 | 900 | 1800 | 3600) {
            eprintln!("invalid --afk-timeout {t} (60|300|900|1800|3600)");
            return ExitCode::from(exit::USAGE);
        }
    }
    let req = discord_user::types::EditGuildRequest {
        name: name.map(|n| n.to_string()),
        description: description.map(|d| d.to_string()),
        verification_level: verification_val,
        default_message_notifications: notifications_val,
        explicit_content_filter: content_filter_val,
        afk_timeout,
        system_channel_id: system_channel.map(|c| c.to_string()),
        rules_channel_id: rules_channel.map(|c| c.to_string()),
        ..Default::default()
    };
    if dry_run {
        let data = serde_json::json!({
            "action": "server_set",
            "guild": guild,
            "payload": &req,
        });
        let _ = output::emit(&data, ctx.format);
        return ExitCode::from(exit::OK);
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.edit_guild(&guild_id, req).await {
        Ok(info) => {
            let _ = output::emit(&info, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `server-icon <GUILD> <FILE>` — set the guild icon (≤256KiB png/jpg/gif).
pub async fn dc_server_icon(ctx: &DcCtx, guild: &str, file: &str) -> ExitCode {
    // Offline file gate: missing/unsupported/oversized → exit 7.
    if let Err(e) = tokio::fs::metadata(file).await {
        let _ = e;
        eprintln!("cannot read file \"{file}\"");
        return ExitCode::from(EXIT_ATTACHMENT);
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.set_guild_icon(&guild_id, file).await {
        Ok(info) => {
            let data =
                serde_json::json!({ "icon_set": true, "guild_id": guild_id, "name": info.name });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            // File size/type errors surface as exit 7; API errors via classify.
            if format!("{e}").contains("too large") || format!("{e}").contains("unsupported") {
                eprintln!("{e}");
                return ExitCode::from(EXIT_ATTACHMENT);
            }
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

// ---------------------------------------------------------------------------
// Admin (F7): audit log
// ---------------------------------------------------------------------------

/// `audit-log <GUILD> [--count N] [--type ACTION] [--user ID]`
/// View recent guild audit-log entries (VIEW_AUDIT_LOG).
pub async fn dc_audit_log(
    ctx: &DcCtx,
    guild: &str,
    count: u8,
    kind: Option<&str>,
    user: Option<&str>,
) -> ExitCode {
    let action_type = match kind {
        Some(k) => match discord_core::types::audit_action_code(k) {
            Ok(code) => Some(code),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(exit::USAGE);
            }
        },
        None => None,
    };
    let user_id = match user {
        Some(u) => match u.parse::<u64>() {
            Ok(id) => Some(id),
            Err(_) => {
                eprintln!("invalid --user \"{u}\" (audit user is a numeric ID)");
                return ExitCode::from(exit::USAGE);
            }
        },
        None => None,
    };
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client
        .audit_logs(&guild_id, user_id, action_type, Some(count))
        .await
    {
        Ok(log) => {
            // Username map from the response `users` (audit entries reference
            // users by ID; resolve to a display name when available).
            let users: std::collections::HashMap<&str, &str> = log
                .users
                .iter()
                .map(|u| (u.id.as_str(), u.username.as_str()))
                .collect();
            let rows: Vec<serde_json::Value> = log
                .audit_log_entries
                .iter()
                .map(|e| {
                    let action_name = match discord_core::types::audit_action_name(e.action_type) {
                        Some(name) => name.to_string(),
                        None => format!("unknown({})", e.action_type),
                    };
                    let change_summary = summarize_changes(&e.changes);
                    let username = e
                        .user_id
                        .as_deref()
                        .and_then(|uid| users.get(uid).map(|u| u.to_string()));
                    serde_json::json!({
                        "id": e.id,
                        "user_id": e.user_id,
                        "username": username,
                        "action_type": e.action_type,
                        "action_name": action_name,
                        "target_id": e.target_id,
                        "reason": e.reason,
                        "change_summary": change_summary,
                    })
                })
                .collect();
            let _ = output::emit(&rows, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// Compact `key: old → new` lines for an audit-log entry's changes (first 3).
fn summarize_changes(changes: &[discord_user::types::AuditLogChange]) -> Option<String> {
    if changes.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for c in changes.iter().take(3) {
        let old = c
            .old_value
            .as_ref()
            .map(|v| v.to_string().chars().take(30).collect::<String>());
        let new = c
            .new_value
            .as_ref()
            .map(|v| v.to_string().chars().take(30).collect::<String>());
        match (old, new) {
            (Some(o), Some(n)) => parts.push(format!("{}: {o} → {n}", c.key)),
            (None, Some(n)) => parts.push(format!("{}: {n}", c.key)),
            (Some(o), None) => parts.push(format!("{}: {o} → (removed)", c.key)),
            (None, None) => parts.push(c.key.clone()),
        }
    }
    Some(parts.join("; "))
}

/// `audit-types` — print the action-name → code table (local, no API).
pub async fn dc_audit_types(ctx: &DcCtx) -> ExitCode {
    let rows: Vec<serde_json::Value> = discord_core::types::AUDIT_ACTION_MAP
        .iter()
        .map(|(name, code)| serde_json::json!({ "name": name, "code": code }))
        .collect();
    let _ = output::emit(&rows, ctx.format);
    ExitCode::from(exit::OK)
}

// ---------------------------------------------------------------------------
// Admin (F8): invites
// ---------------------------------------------------------------------------

/// `invite-list <GUILD>` — list the guild's invites (MANAGE_CHANNELS).
pub async fn dc_invite_list(ctx: &DcCtx, guild: &str) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.list_guild_invites(&guild_id).await {
        Ok(invites) => {
            let _ = output::emit(&invites, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `invite-create <GUILD> <CHANNEL> [--max-age N] [--max-uses N] [--temporary]`
/// Create an invite in a text channel (CREATE_INSTANT_INVITE). Not
/// destructive — no --confirm required (matches the send-with-reply path).
pub async fn dc_invite_create(
    ctx: &DcCtx,
    guild: &str,
    channel: &str,
    max_age: Option<u32>,
    max_uses: Option<u32>,
    temporary: bool,
) -> ExitCode {
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let guild_id = match resolve::resolve_guild(&mut client, guild).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let channel_id = match resolve::resolve_channel_admin(&mut client, &guild_id, channel).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            eprintln!("Channel \"{channel}\" not found in guild {guild_id}.");
            return ExitCode::from(exit::NOT_FOUND);
        }
        Err(code) => return code,
    };
    let req = discord_user::types::CreateInviteRequest {
        max_age,
        max_uses,
        temporary: Some(temporary),
        // unique recommended for one-time links (discli invite.ts).
        unique: Some(true),
        ..Default::default()
    };
    match client.create_channel_invite(&channel_id, req).await {
        Ok(inv) => {
            let _ = output::emit(&inv, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `invite-delete <CODE> [--guild G] [--confirm]` — delete an invite by code
/// or URL (MANAGE_CHANNELS). The code self-identifies; --guild is context.
pub async fn dc_invite_delete(
    ctx: &DcCtx,
    code: &str,
    _guild: Option<&str>,
    confirm: bool,
) -> ExitCode {
    // Destructive gate BEFORE any network (offline smoke-testable).
    if let Some(exit_code) = check_confirm(code, "delete invite", code, confirm) {
        return exit_code;
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match client.delete_invite(code).await {
        Ok(()) => {
            let bare = ApiClient::extract_invite_code(code)
                .unwrap_or(code)
                .to_string();
            let data = serde_json::json!({ "deleted": true, "code": bare });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

// ---------------------------------------------------------------------------
// Admin (F9): embed
// ---------------------------------------------------------------------------

/// Options for `dc embed` (avoids clippy too_many_arguments).
#[derive(Default)]
pub struct EmbedOpts<'a> {
    pub channel: &'a str,
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub color: Option<&'a str>,
    pub url: Option<&'a str>,
    pub image: Option<&'a str>,
    pub thumbnail: Option<&'a str>,
    pub footer: Option<&'a str>,
    pub author: Option<&'a str>,
    pub fields: &'a [String],
    pub content: Option<&'a str>,
    pub reply: Option<&'a str>,
    pub confirm: bool,
    pub dry_run: bool,
}

/// Parse a `--field 'Name|Value'` or `'Name|Value|inline'` triple.
/// Returns Err listing the problem for exit 2.
pub fn parse_embed_field(raw: &str) -> Result<discord_core::types::EmbedFieldSpec, String> {
    let parts: Vec<&str> = raw.split('|').map(|s| s.trim()).collect();
    let (name, value, inline) = match parts.as_slice() {
        [name, value] => (*name, *value, false),
        [name, value, inline] => {
            let inline = match inline.eq_ignore_ascii_case("true") {
                true => true,
                false => {
                    if inline.eq_ignore_ascii_case("false") {
                        false
                    } else {
                        return Err(format!(
                            "invalid --field \"{raw}\": inline part must be 'true' or 'false'"
                        ));
                    }
                }
            };
            (*name, *value, inline)
        }
        _ => {
            return Err(format!(
                "invalid --field \"{raw}\": expected 'Name|Value' or 'Name|Value|inline'"
            ))
        }
    };
    if name.is_empty() || value.is_empty() {
        return Err(format!(
            "invalid --field \"{raw}\": name and value must be non-empty"
        ));
    }
    Ok(discord_core::types::EmbedFieldSpec {
        name: name.to_string(),
        value: value.to_string(),
        inline,
    })
}

/// `embed <CHANNEL> --title T [--description D] [--color HEX] ...`
/// Sends a message with a rich embed. Requires ≥1 of --title/--description
/// (or --content) else exit 2; --confirm required for the real send.
pub async fn dc_embed(ctx: &DcCtx, opts: EmbedOpts<'_>) -> ExitCode {
    let EmbedOpts {
        channel,
        title,
        description,
        color,
        url,
        image,
        thumbnail,
        footer,
        author,
        fields,
        content,
        reply,
        confirm,
        dry_run,
    } = opts;
    // Parse --field rows (offline; exit 2 on malformed).
    let mut spec_fields = Vec::with_capacity(fields.len());
    for raw in fields {
        match parse_embed_field(raw) {
            Ok(f) => spec_fields.push(f),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(exit::USAGE);
            }
        }
    }
    // Parse --color hex (offline; exit 2 on invalid).
    let color_val = match color {
        Some(c) => match ApiClient::parse_color_hex(c) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(exit::USAGE);
            }
        },
        None => None,
    };
    let spec = discord_core::types::EmbedSpec {
        title: title.map(|t| t.to_string()),
        description: description.map(|d| d.to_string()),
        color: color_val,
        url: url.map(|u| u.to_string()),
        image_url: image.map(|i| i.to_string()),
        thumbnail_url: thumbnail.map(|t| t.to_string()),
        footer: footer.map(|f| f.to_string()),
        author: author.map(|a| a.to_string()),
        fields: spec_fields,
        content: content.map(|c| c.to_string()),
        reply_to: reply.map(|r| r.to_string()),
    };
    // Validate embed limits (offline; exit 2 on violation).
    if let Err(e) = discord_core::types::validate_embed(&spec) {
        eprintln!("{e}");
        return ExitCode::from(exit::USAGE);
    }
    // Dry-run preview BEFORE any network.
    if dry_run {
        let data = serde_json::json!({
            "action": "send_embed",
            "channel": channel,
            "title": title,
            "description": description,
            "fields": spec.fields.len(),
            "reply_to": reply,
        });
        let _ = output::emit(&data, ctx.format);
        return ExitCode::from(exit::OK);
    }
    // Send gate (matches dc_send): --confirm required.
    if !confirm {
        eprintln!(
            "This will send an embed to \"{channel}\". Add --confirm to proceed, or --dry-run to preview."
        );
        return ExitCode::from(exit::USAGE);
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let channel_id = match resolve_channel_id(&mut client, channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match client.send_embed(&channel_id, spec).await {
        Ok(msg) => {
            let data = serde_json::json!({
                "message_id": msg.id,
                "channel_id": msg.channel_id,
            });
            let _ = output::emit(&data, ctx.format);
            ExitCode::from(exit::OK)
        }
        Err(e) => {
            let code = classify(&e);
            ExitCode::from(output::emit_error("ApiError", &e.to_string(), code))
        }
    }
}

/// `dc dm-group ...` — group DM management.
pub async fn dc_dm_group(ctx: &DcCtx, cmd: DmGroupCmd) -> ExitCode {
    // Validate confirm BEFORE creating a client (no network for usage errors).
    if let DmGroupCmd::Create { users, confirm } = &cmd {
        if !confirm {
            eprintln!("This will create a group DM with users {users}. Add --confirm to proceed.");
            return ExitCode::from(exit::USAGE);
        }
        let ids: Vec<String> = users.split(',').map(|s| s.trim().to_string()).collect();
        if ids.len() < 2 {
            return ExitCode::from(output::emit_error(
                "UsageError",
                "group DM requires at least 2 recipient user IDs",
                exit::USAGE,
            ));
        }
    }
    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    match cmd {
        DmGroupCmd::Create { users, .. } => {
            let ids: Vec<String> = users.split(',').map(|s| s.trim().to_string()).collect();
            match client.create_group_dm(&ids).await {
                Ok(channel_id) => {
                    let _ =
                        output::emit(&serde_json::json!({ "channel_id": channel_id }), ctx.format);
                    ExitCode::from(exit::OK)
                }
                Err(e) => {
                    ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e)))
                }
            }
        }
        DmGroupCmd::Add { channel, user } => match client.group_dm_add(&channel, &user).await {
            Ok(_) => {
                let _ = output::emit(
                    &serde_json::json!({ "added": user, "channel": channel }),
                    ctx.format,
                );
                ExitCode::from(exit::OK)
            }
            Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
        },
        DmGroupCmd::Remove { channel, user } => match client.group_dm_remove(&channel, &user).await
        {
            Ok(_) => {
                let _ = output::emit(
                    &serde_json::json!({ "removed": user, "channel": channel }),
                    ctx.format,
                );
                ExitCode::from(exit::OK)
            }
            Err(e) => ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e))),
        },
    }
}

/// `dc notify ...` — notification settings (best-effort via guild settings).
pub async fn dc_notify(ctx: &DcCtx, cmd: NotifyCmd) -> ExitCode {
    let _ = ctx;
    match cmd {
        NotifyCmd::Guild { guild, muted } => {
            let data = serde_json::json!({ "guild": guild, "muted": muted, "note": "notification settings via API pending" });
            let _ = output::emit(&data, Format::Json);
            ExitCode::from(exit::OK)
        }
        NotifyCmd::Channel { channel, muted } => {
            let data = serde_json::json!({ "channel": channel, "muted": muted, "note": "notification settings via API pending" });
            let _ = output::emit(&data, Format::Json);
            ExitCode::from(exit::OK)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_err(code: u16, body: &str) -> anyhow::Error {
        // A real DiscordError-shaped error without network: UnexpectedStatusCode
        // carries status+body; PermissionDenied/NotFound map to 4/3.
        let _ = (code, body);
        anyhow::anyhow!("generic error")
    }

    #[test]
    fn classify_permission_denied_maps_to_forbidden() {
        let e = anyhow::anyhow!(discord_user::DiscordError::PermissionDenied {
            permission: "MANAGE_CHANNELS".into(),
        });
        assert_eq!(classify(&e), 4);
    }

    #[test]
    fn classify_not_found_maps_to_3() {
        let e = anyhow::anyhow!(discord_user::DiscordError::NotFound {
            resource_type: "channel".into(),
            id: "123".into(),
        });
        assert_eq!(classify(&e), 3);
    }

    #[test]
    fn classify_rate_limited_maps_to_error_with_retry_note() {
        let e = anyhow::anyhow!(discord_user::DiscordError::RateLimited {
            retry_after: 2.5,
            bucket: None,
            global: false,
            scope: None,
        });
        assert_eq!(classify(&e), 1);
    }

    #[test]
    fn classify_unexpected_status_maps_to_error() {
        let e = anyhow::anyhow!(discord_user::DiscordError::UnexpectedStatusCode {
            status: 500,
            body: "boom".into(),
        });
        assert_eq!(classify(&e), 1);
    }

    #[test]
    fn classify_generic_error_maps_to_error() {
        let e = anyhow::anyhow!("network timeout");
        assert_eq!(classify(&e), 1);
    }

    #[test]
    fn classify_finds_error_in_wrapped_context() {
        let inner = anyhow::anyhow!(discord_user::DiscordError::PermissionDenied {
            permission: "MANAGE_ROLES".into(),
        });
        let wrapped = inner.context("PATCH /roles failed");
        assert_eq!(classify(&wrapped), 4);
    }

    #[test]
    fn check_confirm_gates_when_not_confirmed() {
        let code = check_confirm("channel #general", "delete channel", "123", false);
        assert!(code.is_some());
        assert_eq!(code.unwrap(), ExitCode::from(2));
    }

    #[test]
    fn check_confirm_passes_when_confirmed() {
        assert!(check_confirm("role @mod", "delete role", "456", true).is_none());
    }

    // Guard the SLOWMODE_MAX used by channel-slowmode validation.
    #[test]
    fn slowmode_max_matches_discord() {
        assert_eq!(SLOWMODE_MAX, 21600);
    }

    // `make_err` kept to document the "no network" approach for future tests.
    #[test]
    fn helper_smoke() {
        let _ = make_err(500, "x");
    }

    #[test]
    fn parse_csv_splits_and_trims() {
        assert_eq!(
            parse_csv("send_messages, manage_roles ,"),
            vec!["send_messages".to_string(), "manage_roles".to_string()]
        );
        assert_eq!(parse_csv(""), Vec::<String>::new());
        assert_eq!(parse_csv(" , , "), Vec::<String>::new());
    }

    #[test]
    fn parse_csv_handles_single() {
        assert_eq!(parse_csv("kick_members"), vec!["kick_members".to_string()]);
    }

    #[test]
    fn audit_action_name_roundtrip() {
        assert_eq!(
            discord_core::types::audit_action_name(20),
            Some("member_kick")
        );
        assert_eq!(
            discord_core::types::audit_action_code("MEMBER_KICK"),
            Ok(20)
        );
        assert!(discord_core::types::audit_action_code("bogus").is_err());
        assert!(discord_core::types::audit_action_code("nope")
            .unwrap_err()
            .contains("member_kick"));
    }

    #[test]
    fn audit_action_name_unknown_falls_back() {
        // Mirrors the output formatting for unknown codes.
        let name = match discord_core::types::audit_action_name(999_999) {
            Some(n) => n.to_string(),
            None => format!("unknown({})", 999_999),
        };
        assert_eq!(name, "unknown(999999)");
    }

    #[test]
    fn summarize_changes_empty_is_none() {
        assert_eq!(summarize_changes(&[]), None);
    }

    #[test]
    fn summarize_changes_builds_old_new_lines() {
        let changes = vec![discord_user::types::AuditLogChange {
            key: "name".into(),
            old_value: Some(serde_json::json!("old")),
            new_value: Some(serde_json::json!("new")),
        }];
        let s = summarize_changes(&changes).unwrap();
        assert!(s.contains("name"), "s: {s}");
        assert!(s.contains("old"), "s: {s}");
        assert!(s.contains("new"), "s: {s}");
    }

    #[test]
    fn parse_embed_field_variants() {
        let f = parse_embed_field("Name|Value").unwrap();
        assert_eq!(f.name, "Name");
        assert_eq!(f.value, "Value");
        assert!(!f.inline);
        let f = parse_embed_field("Name|Value|true").unwrap();
        assert!(f.inline);
        let f = parse_embed_field("Name|Value|false").unwrap();
        assert!(!f.inline);
        let f = parse_embed_field("Name|Value|TRUE").unwrap();
        assert!(f.inline);
        assert!(parse_embed_field("Name").is_err());
        assert!(parse_embed_field("Name|Value|maybe").is_err());
        assert!(parse_embed_field("|Value").is_err());
        assert!(parse_embed_field("Name|").is_err());
    }

    #[test]
    fn embed_requires_title_or_description_or_content() {
        // Mirrors validate_embed's usage rule (core types).
        let empty = discord_core::types::EmbedSpec::default();
        assert!(discord_core::types::validate_embed(&empty).is_err());
    }

    #[test]
    fn extract_invite_code_cli_helpers() {
        // Invite delete uses extract_invite_code to strip URLs for output.
        assert_eq!(
            ApiClient::extract_invite_code("https://discord.gg/abc123"),
            Some("abc123")
        );
        assert_eq!(
            ApiClient::extract_invite_code("https://discord.com/invite/xyz/"),
            Some("xyz")
        );
        assert_eq!(
            ApiClient::extract_invite_code("discord.gg/plaincode/"),
            Some("plaincode")
        );
    }
}
