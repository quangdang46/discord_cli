//! `discord` CLI entry point.
//!
//! Global flags: `--token`, `--no-color`, `--json`, `--yaml`, `--format`.
//! Commands: `status`, `whoami` (M1.4); later `dc`, `search`, `serve`, `auth`.

use std::process::ExitCode;

mod commands;
mod resolve;

use clap::{CommandFactory, Parser, Subcommand};
use discord_core::client::ApiClient;
use discord_core::config::load_env;
use discord_core::output::{self, exit, Format};

use commands::dc::{DcCtx, DmGroupCmd, NotifyCmd};

#[derive(Parser, Debug)]
#[command(
    name = "discord",
    version,
    about = "Discord CLI + MCP server for AI agents (user-token/selfbot style)",
    long_about = "Read/send/search Discord as the logged-in user, for AI agents.
WARNING: automating a user account may violate Discord ToS — use only on accounts you control."
)]
struct Cli {
    /// Discord token (overrides env/.env/keyring).
    #[arg(long, global = true)]
    token: Option<String>,

    /// Disable ANSI color (also honored via NO_COLOR).
    #[arg(long, global = true)]
    no_color: bool,

    /// Force JSON envelope output.
    #[arg(long, global = true)]
    json: bool,

    /// Force YAML envelope output.
    #[arg(long, global = true)]
    yaml: bool,

    /// Output format override: json|jsonl|yaml|rich|auto.
    #[arg(long, global = true, value_name = "FMT")]
    format: Option<String>,

    /// Reserved: TLS ClientHello (JA3) spoofing is not yet implemented.
    #[arg(long, global = true)]
    tls_chrome: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Validate the configured token (exit 1 on failure).
    Status,
    /// Show the authenticated user's profile.
    Whoami,
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
        /// Channel name or ID.
        channel: String,
        /// Max messages to fetch (default 1000).
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
        /// Fetch messages around this message ID (limit/2 each side).
        #[arg(long, conflicts_with_all = ["before"])]
        around: Option<u64>,
        /// Only messages on/after this time (12h|30d|YYYY-MM-DD).
        #[arg(long)]
        since: Option<String>,
        /// Compact plain-text transcript `[HH:MM:SS] author: content`
        /// — ~5x smaller than JSON, ideal for AI summarization.
        #[arg(long)]
        transcript: bool,
    },
    /// List guild members.
    Members {
        /// Guild name or ID.
        guild: String,
        /// Max members (default 50).
        #[arg(long, default_value_t = 50)]
        max: u32,
    },
    /// Show guild info (name, member counts).
    Info {
        /// Guild name or ID.
        guild: String,
    },
    /// Discord native search within a guild.
    GuildSearch {
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
    /// Public info for any user (username, badges, avatar, created_at).
    Userinfo {
        /// User ID.
        user_id: String,
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
        /// Suppress link embeds in this message (SUPPRESS_EMBEDS flag).
        #[arg(long)]
        suppress_embeds: bool,
        /// Allow mentioning this role id (repeatable; @everyone/@here stay off).
        #[arg(long)]
        mention_roles: Vec<String>,
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
    /// Download archived attachments to disk (offline).
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
    /// Download images linked in messages via the Discord CDN proxy.
    FetchLinks {
        /// Channel name or ID.
        channel: String,
        /// Only links from messages on/after this date (30d|6m|1y|YYYY-MM-DD).
        #[arg(long)]
        since: Option<String>,
        /// Max messages to scan (default 100).
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Output directory (default <data_dir>/media-links).
        #[arg(long)]
        out: Option<String>,
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
        /// After backfill, keep tailing new messages into SQLite via the
        /// gateway (invisible presence) until Ctrl-C.
        #[arg(long)]
        follow: bool,
        /// With --follow: exit gracefully after this many seconds.
        #[arg(long)]
        max_duration: Option<u64>,
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
    /// FTS5 search of the local SQLite archive.
    Search {
        /// Keyword to search.
        keyword: String,
        /// Filter by channel name.
        #[arg(short, long)]
        channel: Option<String>,
        /// Author name contains.
        #[arg(long)]
        author: Option<String>,
        /// Only messages on/after (12h|30d|YYYY-MM-DD).
        #[arg(long)]
        since: Option<String>,
        /// Max results (default 50).
        #[arg(short, long, default_value_t = 50)]
        limit: usize,
    },
    /// Recent stored messages.
    Recent {
        /// Filter by channel name.
        #[arg(short, long)]
        channel: Option<String>,
        /// Only messages from the last N hours.
        #[arg(long)]
        hours: Option<i64>,
        /// Only messages on/after (12h|30d|YYYY-MM-DD).
        #[arg(long)]
        since: Option<String>,
        /// Max results (default 50).
        #[arg(short, long, default_value_t = 50)]
        limit: usize,
    },
    /// Per-channel message counts.
    Stats,
    /// Per-channel message counts since 00:00 local time.
    Today,
    /// Message volume per day or per hour (ASCII bars on TTY).
    Timeline {
        /// Granularity: day|hour (default hour).
        #[arg(long, default_value = "hour")]
        by: String,
    },
    /// Top senders.
    Top {
        /// Filter by channel name.
        #[arg(short, long)]
        channel: Option<String>,
        /// Max senders (default 10).
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
    },
    /// Export stored messages for a channel.
    Export {
        /// Channel ID.
        channel: String,
        /// Output as JSON (default text).
        #[arg(long)]
        json: bool,
        /// Output file path.
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Delete stored messages for a channel (requires -y).
    Purge {
        /// Channel ID.
        channel: String,
        /// Confirm purge.
        #[arg(short, long)]
        yes: bool,
    },
    /// Auth: auto-detect token, paste, or QR scan; validate, save.
    Auth {
        /// Save the detected/pasted token to .env.
        #[arg(long)]
        save: bool,
        /// Paste the token manually instead of auto-detect.
        #[arg(long)]
        paste: bool,
        /// Authenticate by scanning a QR code with the Discord mobile app.
        #[arg(long)]
        qr: bool,
    },
    /// Start the MCP server (stdio) for AI agents.
    Serve,
    /// Create a channel (admin; MANAGE_CHANNELS).
    ChannelCreate {
        /// Guild name or ID.
        guild: String,
        /// Channel name (1-100 chars, no '#').
        name: String,
        /// Channel type (text|voice|category|announcement|stage|forum).
        #[arg(long, default_value = "text")]
        r#type: String,
        /// Parent category name or ID.
        #[arg(long)]
        category: Option<String>,
        /// Channel topic (≤1024 chars).
        #[arg(long)]
        topic: Option<String>,
        /// Slowmode seconds (0-21600).
        #[arg(long)]
        slowmode: Option<u64>,
        /// Preview without creating.
        #[arg(long)]
        dry_run: bool,
    },
    /// Rename a channel (admin).
    ChannelRename {
        /// Guild name or ID.
        guild: String,
        /// Channel name or ID.
        channel: String,
        /// New name (1-100 chars, no '#').
        new_name: String,
        /// Preview without changing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Set a channel topic (admin).
    ChannelTopic {
        /// Guild name or ID.
        guild: String,
        /// Channel name or ID.
        channel: String,
        /// New topic (≤1024 chars).
        topic: String,
    },
    /// Move a channel to a category and/or position (admin).
    ChannelMove {
        /// Guild name or ID.
        guild: String,
        /// Channel name or ID.
        channel: String,
        /// Target category name or ID.
        #[arg(long)]
        category: Option<String>,
        /// Sorting position.
        #[arg(long)]
        position: Option<u32>,
    },
    /// Clone a channel (same type/parent/topic).
    ChannelClone {
        /// Guild name or ID.
        guild: String,
        /// Source channel name or ID.
        channel: String,
        /// Override the cloned name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Set slowmode on a channel (admin).
    ChannelSlowmode {
        /// Guild name or ID.
        guild: String,
        /// Channel name or ID.
        channel: String,
        /// Slowmode seconds (0-21600).
        seconds: u64,
    },
    /// Delete a channel (admin; requires --confirm).
    ChannelDelete {
        /// Guild name or ID.
        guild: String,
        /// Channel name or ID.
        channel: String,
        /// Confirm deletion.
        #[arg(long)]
        confirm: bool,
    },
    /// Create a role (admin; MANAGE_ROLES).
    RoleCreate {
        /// Guild name or ID.
        guild: String,
        /// Role name.
        name: String,
        /// Color hex (#RRGGBB or RRGGBB).
        #[arg(long)]
        color: Option<String>,
        /// Comma-separated permission names.
        #[arg(long)]
        permissions: Option<String>,
        /// Allow anyone to mention this role.
        #[arg(long)]
        mentionable: bool,
        /// Show separately in the member list.
        #[arg(long)]
        hoist: bool,
        /// Preview without creating.
        #[arg(long)]
        dry_run: bool,
    },
    /// Edit a role (admin; ≥1 option required).
    RoleEdit {
        /// Guild name or ID.
        guild: String,
        /// Role name or ID.
        role: String,
        /// New name.
        #[arg(long)]
        name: Option<String>,
        /// New color hex (#RRGGBB or RRGGBB).
        #[arg(long)]
        color: Option<String>,
        /// New comma-separated permissions.
        #[arg(long)]
        permissions: Option<String>,
        /// Allow anyone to mention (--no-mentionable to disallow).
        #[arg(long)]
        mentionable: Option<bool>,
        /// Show separately (--no-hoist to hide).
        #[arg(long)]
        hoist: Option<bool>,
        /// Preview without changing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Delete a role (admin; requires --confirm).
    RoleDelete {
        /// Guild name or ID.
        guild: String,
        /// Role name or ID.
        role: String,
        /// Confirm deletion.
        #[arg(long)]
        confirm: bool,
    },
    /// Assign a role to a member (admin; MANAGE_ROLES).
    RoleAssign {
        /// Guild name or ID.
        guild: String,
        /// Role name or ID.
        role: String,
        /// Member username or ID.
        user: String,
    },
    /// Remove a role from a member (admin).
    RoleRemove {
        /// Guild name or ID.
        guild: String,
        /// Role name or ID.
        role: String,
        /// Member username or ID.
        user: String,
    },
    /// List custom emojis (admin).
    EmojiList {
        /// Guild name or ID.
        guild: String,
        /// Max emojis to show (default all).
        #[arg(long)]
        count: Option<u32>,
    },
    /// Upload a custom emoji (admin; ≤256KiB png/jpg/gif).
    EmojiUpload {
        /// Guild name or ID.
        guild: String,
        /// Emoji name (alphanumeric + underscore).
        name: String,
        /// Image file path.
        file: String,
    },
    /// Delete a custom emoji (admin; requires --confirm).
    EmojiDelete {
        /// Guild name or ID.
        guild: String,
        /// Emoji name (:name:) or ID.
        emoji: String,
        /// Confirm deletion.
        #[arg(long)]
        confirm: bool,
    },
    /// Kick a member (admin; KICK_MEMBERS; requires --confirm).
    MemberKick {
        /// Guild name or ID.
        guild: String,
        /// Member username, global name, nick, or ID.
        user: String,
        /// Audit-log reason (X-Audit-Log-Reason header).
        #[arg(long)]
        reason: Option<String>,
        /// Confirm the kick.
        #[arg(long)]
        confirm: bool,
    },
    /// Ban a member (admin; BAN_MEMBERS; requires --confirm).
    MemberBan {
        /// Guild name or ID.
        guild: String,
        /// Member username, global name, nick, or ID.
        user: String,
        /// Audit-log reason (body `reason`).
        #[arg(long)]
        reason: Option<String>,
        /// Delete message history up to this many days (0-7).
        #[arg(long, value_name = "DAYS")]
        delete_days: Option<u8>,
        /// Confirm the ban.
        #[arg(long)]
        confirm: bool,
    },
    /// Unban a user (admin; BAN_MEMBERS; requires --confirm).
    MemberUnban {
        /// Guild name or ID.
        guild: String,
        /// Banned user ID (not a guild member).
        user: String,
        /// Confirm the unban.
        #[arg(long)]
        confirm: bool,
    },
    /// Set/clear a member's nickname (admin; MANAGE_NICKNAMES).
    MemberNick {
        /// Guild name or ID.
        guild: String,
        /// Member username, global name, nick, or ID.
        user: String,
        /// New nickname (empty clears).
        nickname: String,
    },
    /// View a channel's permission overwrites (admin).
    PermView {
        /// Guild name or ID.
        guild: String,
        /// Channel name or ID.
        channel: String,
    },
    /// Set a role's channel permission overwrite (admin; MANAGE_CHANNELS).
    PermSet {
        /// Guild name or ID.
        guild: String,
        /// Channel name or ID.
        channel: String,
        /// Role name or ID.
        role: String,
        /// Comma-separated permissions to allow.
        #[arg(long)]
        allow: Option<String>,
        /// Comma-separated permissions to deny.
        #[arg(long)]
        deny: Option<String>,
    },
    /// Lock a channel read-only for @everyone (admin; requires --confirm).
    PermLock {
        /// Guild name or ID.
        guild: String,
        /// Channel name or ID.
        channel: String,
        /// Preview the @everyone deny without applying.
        #[arg(long)]
        dry_run: bool,
        /// Confirm the lock.
        #[arg(long)]
        confirm: bool,
    },
    /// Unlock a channel locked via perm-lock (admin; requires --confirm).
    PermUnlock {
        /// Guild name or ID.
        guild: String,
        /// Channel name or ID.
        channel: String,
        /// Confirm the unlock.
        #[arg(long)]
        confirm: bool,
    },
    /// List available permission names and their bits.
    PermList,
    /// Edit server settings (admin; MANAGE_GUILD; ≥1 option required).
    ServerSet {
        /// Guild name or ID.
        guild: String,
        /// New server name (2-100 chars).
        #[arg(long)]
        name: Option<String>,
        /// New description (≤120; community servers).
        #[arg(long)]
        description: Option<String>,
        /// Verification level: none|low|medium|high|very_high.
        #[arg(long)]
        verification: Option<String>,
        /// Default notifications: all_messages|only_mentions.
        #[arg(long)]
        notifications: Option<String>,
        /// Content filter: disabled|members_without_roles|all_members.
        #[arg(long)]
        content_filter: Option<String>,
        /// AFK timeout seconds (60|300|900|1800|3600).
        #[arg(long)]
        afk_timeout: Option<u32>,
        /// System channel ID.
        #[arg(long)]
        system_channel: Option<String>,
        /// Rules channel ID.
        #[arg(long)]
        rules_channel: Option<String>,
        /// Preview the payload without applying.
        #[arg(long)]
        dry_run: bool,
    },
    /// Set the server icon (admin; MANAGE_GUILD; ≤256KiB png/jpg/gif).
    ServerIcon {
        /// Guild name or ID.
        guild: String,
        /// Image file path.
        file: String,
    },
    /// View the guild audit log (admin; VIEW_AUDIT_LOG).
    AuditLog {
        /// Guild name or ID.
        guild: String,
        /// Max entries (default 50, capped 100).
        #[arg(short, long, default_value_t = 50)]
        count: u8,
        /// Filter by action name (e.g. member_kick, channel_create).
        #[arg(long, value_name = "ACTION")]
        r#type: Option<String>,
        /// Filter by the user who performed the action (numeric ID).
        #[arg(long)]
        user: Option<String>,
    },
    /// List audit-log action type names and codes (local, no API).
    AuditTypes,
    /// List a guild's invites (admin; MANAGE_CHANNELS).
    InviteList {
        /// Guild name or ID.
        guild: String,
    },
    /// Create an invite for a text channel (admin; CREATE_INSTANT_INVITE).
    InviteCreate {
        /// Guild name or ID.
        guild: String,
        /// Channel name or ID (text-like).
        channel: String,
        /// Duration in seconds before expiry (0 = never; default 86400).
        #[arg(long)]
        max_age: Option<u32>,
        /// Max uses (0 = unlimited; default 0).
        #[arg(long)]
        max_uses: Option<u32>,
        /// Grant temporary membership.
        #[arg(long)]
        temporary: bool,
    },
    /// Delete an invite by code or URL (admin; MANAGE_CHANNELS).
    InviteDelete {
        /// Invite code or full URL (discord.gg/..., discord.com/invite/...).
        code: String,
        /// Guild name or ID (context only).
        #[arg(long)]
        guild: Option<String>,
        /// Confirm deletion.
        #[arg(long)]
        confirm: bool,
    },
    /// Send a message with a rich embed (requires --confirm unless --dry-run).
    /// NOTE: user-token accounts cannot render embeds — Discord strips them
    /// server-side (200 with `embeds:[]`). The message sends, but the rich card
    /// will not display. Bot tokens render embeds normally.
    Embed {
        /// Channel name or ID.
        channel: String,
        /// Embed title (≤256).
        #[arg(long)]
        title: Option<String>,
        /// Embed description (≤4096).
        #[arg(long)]
        description: Option<String>,
        /// Embed color hex (#RRGGBB or RRGGBB).
        #[arg(long)]
        color: Option<String>,
        /// Clickable title URL.
        #[arg(long)]
        url: Option<String>,
        /// Image URL.
        #[arg(long)]
        image: Option<String>,
        /// Thumbnail URL.
        #[arg(long)]
        thumbnail: Option<String>,
        /// Footer text.
        #[arg(long)]
        footer: Option<String>,
        /// Author name.
        #[arg(long)]
        author: Option<String>,
        /// Field 'Name|Value' or 'Name|Value|inline' (repeatable).
        #[arg(long)]
        field: Vec<String>,
        /// Plain text content alongside the embed.
        #[arg(long)]
        content: Option<String>,
        /// Reply to a message id.
        #[arg(long)]
        reply: Option<String>,
        /// Confirm sending (never interactive).
        #[arg(long)]
        confirm: bool,
        /// Preview the embed without sending.
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() -> ExitCode {
    // Discord-user-rs's binary uses a roomy stack for clap's recursive debug
    // assertions on Windows (default 1 MiB overflows). Same trick.
    #[cfg(windows)]
    {
        let worker = std::thread::Builder::new()
            .name("discord-cli".into())
            .stack_size(32 * 1024 * 1024)
            .spawn(run)
            .expect("spawn worker");
        worker.join().unwrap_or(ExitCode::from(1))
    }
    #[cfg(not(windows))]
    run()
}

#[tokio::main]
async fn run() -> ExitCode {
    load_env();
    let cli = Cli::parse();
    let format = output::resolve_format(cli.json, cli.yaml, cli.format.as_deref());

    // Reserved --tls-chrome: honest not-implemented (F9; review#9).
    // TLS ClientHello (JA3) spoofing needs unstable rustls APIs or a
    // BoringSSL vendor; reserved for a future feature.
    if cli.tls_chrome {
        eprintln!(
            "--tls-chrome is reserved: TLS ClientHello (JA3) spoofing is not implemented yet."
        );
        return ExitCode::from(exit::USAGE);
    }

    // NO_COLOR honored.
    if cli.no_color || std::env::var("NO_COLOR").is_ok() {
        // Output stays plain; no color lib in core yet.
    }

    // Build a DcCtx for the discord operations (share token + format).
    let dcctx = DcCtx {
        token: cli.token.clone(),
        format,
    };
    let ctx = &dcctx;

    match cli.command {
        Some(Command::Status) => cmd_status(&cli, format).await,
        Some(Command::Whoami) => cmd_whoami(&cli, format).await,
        Some(Command::Guilds) => commands::dc::dc_guilds(ctx).await,
        Some(Command::Channels { guild }) => commands::dc::dc_channels(ctx, &guild).await,
        Some(Command::Dms) => commands::dc::dc_dms(ctx).await,
        Some(Command::History {
            channel,
            limit,
            before,
            after,
        }) => commands::dc::dc_history(ctx, &channel, limit, before, after).await,
        Some(Command::Read {
            channel,
            limit,
            before,
            around,
            since,
            transcript,
        }) => {
            commands::dc::dc_read(
                ctx,
                &channel,
                limit,
                before,
                since.as_deref(),
                around,
                transcript,
            )
            .await
        }
        Some(Command::Members { guild, max }) => commands::dc::dc_members(ctx, &guild, max).await,
        Some(Command::Info { guild }) => commands::dc::dc_info(ctx, &guild).await,
        Some(Command::GuildSearch {
            guild,
            query,
            channel,
            limit,
        }) => commands::dc::dc_search(ctx, &guild, &query, channel.as_deref(), limit).await,
        Some(Command::Roles { guild }) => commands::dc::dc_roles(ctx, &guild).await,
        Some(Command::Profile { user_id }) => {
            commands::dc::dc_profile(ctx, user_id.as_deref()).await
        }
        Some(Command::Userinfo { user_id }) => commands::dc::dc_userinfo(ctx, &user_id).await,
        Some(Command::Relationships) => commands::dc::dc_relationships(ctx).await,
        Some(Command::Threads { channel }) => commands::dc::dc_threads(ctx, &channel).await,
        Some(Command::Send {
            channel,
            text,
            file,
            reply,
            typing,
            confirm,
            dry_run,
            suppress_embeds,
            mention_roles,
        }) => {
            commands::dc::dc_send(
                ctx,
                &channel,
                commands::dc::SendOpts {
                    text: text.as_deref(),
                    files: &file,
                    reply: reply.as_deref(),
                    typing,
                    confirm,
                    dry_run,
                    suppress_embeds,
                    mention_roles,
                },
            )
            .await
        }
        Some(Command::Typing { channel }) => commands::dc::dc_typing(ctx, &channel).await,
        Some(Command::Join { invite, confirm }) => {
            commands::dc::dc_join(ctx, &invite, confirm).await
        }
        Some(Command::Leave { guild, confirm }) => {
            commands::dc::dc_leave(ctx, &guild, confirm).await
        }
        Some(Command::Presence { status }) => {
            commands::dc::dc_presence(ctx, status.as_deref()).await
        }
        Some(Command::TopReactions {
            guild,
            channel,
            limit,
        }) => {
            commands::dc::dc_top_reactions(
                ctx,
                guild.as_deref(),
                channel.as_deref(),
                limit.unwrap_or(10),
            )
            .await
        }
        Some(Command::Download {
            guild,
            channel,
            r#type,
            since,
            min_reactions,
            limit,
            out,
        }) => {
            commands::download::dc_download(
                ctx,
                commands::download::DownloadOpts {
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
        Some(Command::FetchLinks {
            channel,
            since,
            limit,
            out,
        }) => {
            commands::fetchlinks::cmd_fetch_links(
                ctx,
                commands::fetchlinks::FetchLinksOpts {
                    channel: &channel,
                    since: since.as_deref(),
                    limit,
                    out: out.as_deref(),
                },
            )
            .await
        }
        Some(Command::ThreadCreate {
            channel,
            name,
            message_id,
            text,
            archive,
            tags,
        }) => {
            commands::dc::dc_thread_create(
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
        Some(Command::Edit {
            channel,
            message_id,
            text,
        }) => commands::dc::dc_edit(ctx, &channel, &message_id, &text).await,
        Some(Command::Delete {
            channel,
            message_id,
            confirm,
        }) => commands::dc::dc_delete(ctx, &channel, &message_id, confirm).await,
        Some(Command::React {
            channel,
            message_id,
            emoji,
        }) => commands::dc::dc_react(ctx, &channel, &message_id, &emoji).await,
        Some(Command::Unreact {
            channel,
            message_id,
            emoji,
        }) => commands::dc::dc_unreact(ctx, &channel, &message_id, &emoji).await,
        Some(Command::Pin {
            channel,
            message_id,
        }) => commands::dc::dc_pin(ctx, &channel, &message_id).await,
        Some(Command::Pins { channel }) => commands::dc::dc_pins(ctx, &channel).await,
        Some(Command::Sync {
            channel,
            limit,
            follow,
            max_duration,
        }) => {
            if follow {
                commands::sync::dc_sync_follow(ctx, &channel, limit, max_duration).await
            } else {
                commands::dc::dc_sync(ctx, &channel, limit).await
            }
        }
        Some(Command::SyncAll { limit }) => commands::dc::dc_sync_all(ctx, limit).await,
        Some(Command::Tail { channel, once }) => commands::tail::dc_tail(ctx, &channel, once).await,
        Some(Command::Watch {
            channel,
            keyword,
            typing,
        }) => commands::tail::dc_watch(ctx, channel.as_deref(), keyword.as_deref(), typing).await,
        Some(Command::DmGroup { cmd }) => commands::dc::dc_dm_group(ctx, cmd).await,
        Some(Command::Notify { cmd }) => commands::dc::dc_notify(ctx, cmd).await,
        Some(Command::Search {
            keyword,
            channel,
            author,
            since,
            limit,
        }) => commands::local::cmd_search(
            &keyword,
            channel.as_deref(),
            author.as_deref(),
            since.as_deref(),
            limit,
            format,
        ),
        Some(Command::Recent {
            channel,
            hours,
            since,
            limit,
        }) => {
            commands::local::cmd_recent(channel.as_deref(), hours, since.as_deref(), limit, format)
        }
        Some(Command::Stats) => commands::local::cmd_stats(format),
        Some(Command::Today) => commands::local::cmd_today(format),
        Some(Command::Timeline { by }) => commands::local::cmd_timeline(&by, format),
        Some(Command::Top { channel, limit }) => {
            commands::local::cmd_top(channel.as_deref(), limit, format)
        }
        Some(Command::Export {
            channel,
            json,
            output,
        }) => commands::local::cmd_export(&channel, json, output.as_deref(), format),
        Some(Command::Purge { channel, yes }) => commands::local::cmd_purge(&channel, yes, format),
        Some(Command::Auth { save, paste, qr }) => cmd_auth(save, paste, qr, format).await,
        Some(Command::Serve) => cmd_serve().await,
        Some(Command::ChannelCreate {
            guild,
            name,
            r#type,
            category,
            topic,
            slowmode,
            dry_run,
        }) => {
            commands::dc::dc_channel_create(
                ctx,
                commands::dc::ChannelCreateOpts {
                    guild: &guild,
                    name: &name,
                    channel_type: &r#type,
                    category: category.as_deref(),
                    topic: topic.as_deref(),
                    slowmode,
                    dry_run,
                },
            )
            .await
        }
        Some(Command::ChannelRename {
            guild,
            channel,
            new_name,
            dry_run,
        }) => commands::dc::dc_channel_rename(ctx, &guild, &channel, &new_name, dry_run).await,
        Some(Command::ChannelTopic {
            guild,
            channel,
            topic,
        }) => commands::dc::dc_channel_topic(ctx, &guild, &channel, &topic).await,
        Some(Command::ChannelMove {
            guild,
            channel,
            category,
            position,
        }) => {
            commands::dc::dc_channel_move(ctx, &guild, &channel, category.as_deref(), position)
                .await
        }
        Some(Command::ChannelClone {
            guild,
            channel,
            name,
        }) => commands::dc::dc_channel_clone(ctx, &guild, &channel, name.as_deref()).await,
        Some(Command::ChannelSlowmode {
            guild,
            channel,
            seconds,
        }) => commands::dc::dc_channel_slowmode(ctx, &guild, &channel, seconds).await,
        Some(Command::ChannelDelete {
            guild,
            channel,
            confirm,
        }) => commands::dc::dc_channel_delete(ctx, &guild, &channel, confirm).await,
        Some(Command::RoleCreate {
            guild,
            name,
            color,
            permissions,
            mentionable,
            hoist,
            dry_run,
        }) => {
            commands::dc::dc_role_create(
                ctx,
                commands::dc::RoleCreateOpts {
                    guild: &guild,
                    name: &name,
                    color: color.as_deref(),
                    permissions: permissions.as_deref(),
                    mentionable,
                    hoist,
                    dry_run,
                },
            )
            .await
        }
        Some(Command::RoleEdit {
            guild,
            role,
            name,
            color,
            permissions,
            mentionable,
            hoist,
            dry_run,
        }) => {
            commands::dc::dc_role_edit(
                ctx,
                commands::dc::RoleEditOpts {
                    guild: &guild,
                    role: &role,
                    name: name.as_deref(),
                    color: color.as_deref(),
                    permissions: permissions.as_deref(),
                    mentionable,
                    hoist,
                    dry_run,
                },
            )
            .await
        }
        Some(Command::RoleDelete {
            guild,
            role,
            confirm,
        }) => commands::dc::dc_role_delete(ctx, &guild, &role, confirm).await,
        Some(Command::RoleAssign { guild, role, user }) => {
            commands::dc::dc_role_assign(ctx, &guild, &role, &user).await
        }
        Some(Command::RoleRemove { guild, role, user }) => {
            commands::dc::dc_role_remove(ctx, &guild, &role, &user).await
        }
        Some(Command::EmojiList { guild, count }) => {
            commands::dc::dc_emoji_list(ctx, &guild, count).await
        }
        Some(Command::EmojiUpload { guild, name, file }) => {
            commands::dc::dc_emoji_upload(ctx, &guild, &name, &file).await
        }
        Some(Command::EmojiDelete {
            guild,
            emoji,
            confirm,
        }) => commands::dc::dc_emoji_delete(ctx, &guild, &emoji, confirm).await,
        Some(Command::MemberKick {
            guild,
            user,
            reason,
            confirm,
        }) => commands::dc::dc_member_kick(ctx, &guild, &user, reason.as_deref(), confirm).await,
        Some(Command::MemberBan {
            guild,
            user,
            reason,
            delete_days,
            confirm,
        }) => {
            commands::dc::dc_member_ban(ctx, &guild, &user, reason.as_deref(), delete_days, confirm)
                .await
        }
        Some(Command::MemberUnban {
            guild,
            user,
            confirm,
        }) => commands::dc::dc_member_unban(ctx, &guild, &user, confirm).await,
        Some(Command::MemberNick {
            guild,
            user,
            nickname,
        }) => commands::dc::dc_member_nick(ctx, &guild, &user, &nickname).await,
        Some(Command::PermView { guild, channel }) => {
            commands::dc::dc_perm_view(ctx, &guild, &channel).await
        }
        Some(Command::PermSet {
            guild,
            channel,
            role,
            allow,
            deny,
        }) => {
            commands::dc::dc_perm_set(
                ctx,
                &guild,
                &channel,
                &role,
                allow.as_deref(),
                deny.as_deref(),
            )
            .await
        }
        Some(Command::PermLock {
            guild,
            channel,
            dry_run,
            confirm,
        }) => commands::dc::dc_perm_lock(ctx, &guild, &channel, dry_run, confirm).await,
        Some(Command::PermUnlock {
            guild,
            channel,
            confirm,
        }) => commands::dc::dc_perm_unlock(ctx, &guild, &channel, confirm).await,
        Some(Command::PermList) => commands::dc::dc_perm_list(ctx).await,
        Some(Command::ServerSet {
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
        }) => {
            commands::dc::dc_server_set(
                ctx,
                commands::dc::ServerSetOpts {
                    guild: &guild,
                    name: name.as_deref(),
                    description: description.as_deref(),
                    verification: verification.as_deref(),
                    notifications: notifications.as_deref(),
                    content_filter: content_filter.as_deref(),
                    afk_timeout,
                    system_channel: system_channel.as_deref(),
                    rules_channel: rules_channel.as_deref(),
                    dry_run,
                },
            )
            .await
        }
        Some(Command::ServerIcon { guild, file }) => {
            commands::dc::dc_server_icon(ctx, &guild, &file).await
        }
        Some(Command::AuditLog {
            guild,
            count,
            r#type,
            user,
        }) => {
            commands::dc::dc_audit_log(ctx, &guild, count, r#type.as_deref(), user.as_deref()).await
        }
        Some(Command::AuditTypes) => commands::dc::dc_audit_types(ctx).await,
        Some(Command::InviteList { guild }) => commands::dc::dc_invite_list(ctx, &guild).await,
        Some(Command::InviteCreate {
            guild,
            channel,
            max_age,
            max_uses,
            temporary,
        }) => {
            commands::dc::dc_invite_create(ctx, &guild, &channel, max_age, max_uses, temporary)
                .await
        }
        Some(Command::InviteDelete {
            code,
            guild,
            confirm,
        }) => commands::dc::dc_invite_delete(ctx, &code, guild.as_deref(), confirm).await,
        Some(Command::Embed {
            channel,
            title,
            description,
            color,
            url,
            image,
            thumbnail,
            footer,
            author,
            field,
            content,
            reply,
            confirm,
            dry_run,
        }) => {
            commands::dc::dc_embed(
                ctx,
                commands::dc::EmbedOpts {
                    channel: &channel,
                    title: title.as_deref(),
                    description: description.as_deref(),
                    color: color.as_deref(),
                    url: url.as_deref(),
                    image: image.as_deref(),
                    thumbnail: thumbnail.as_deref(),
                    footer: footer.as_deref(),
                    author: author.as_deref(),
                    fields: &field,
                    content: content.as_deref(),
                    reply: reply.as_deref(),
                    confirm,
                    dry_run,
                },
            )
            .await
        }
        None => {
            // No subcommand: print help.
            let mut c = Cli::command();
            let _ = c.print_help();
            println!();
            ExitCode::from(exit::OK)
        }
    }
}

async fn cmd_status(cli: &Cli, format: Format) -> ExitCode {
    let mut client = match ApiClient::from_env(cli.token.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            return ExitCode::from(output::emit_error("AuthError", &e.to_string(), exit::ERROR))
        }
    };

    match client.validate().await {
        Ok(true) => {
            let data = serde_json::json!({ "authenticated": true });
            let _ = output::emit(&data, format);
            ExitCode::from(exit::OK)
        }
        _ => {
            let data = serde_json::json!({ "authenticated": false });
            let _ = output::emit(&data, format);
            ExitCode::from(exit::ERROR)
        }
    }
}

/// `auth [--save] [--paste]` — auto-detect or paste token, validate, save.
async fn cmd_auth(save: bool, paste: bool, qr: bool, format: Format) -> ExitCode {
    // QR flow (opt-in, highest ToS risk — review#18: only on explicit --qr).
    if qr {
        match discord_auth::qr::qr_login(120).await {
            Ok(token) => {
                if save {
                    if let Err(e) = discord_auth::auth::save_token_to_env(&token, None) {
                        return ExitCode::from(output::emit_error(
                            "AuthError",
                            &e.to_string(),
                            exit::ERROR,
                        ));
                    }
                }
                let _ = output::emit(
                    &serde_json::json!({ "authenticated": true, "token_saved": save, "source": "qr" }),
                    format,
                );
                return ExitCode::from(exit::OK);
            }
            Err(e) => {
                return ExitCode::from(output::emit_error("AuthError", &e.to_string(), exit::ERROR))
            }
        }
    }
    // Paste flow.
    if paste {
        match discord_auth::auth::auth_paste(save).await {
            Ok(_token) => {
                let _ = output::emit(
                    &serde_json::json!({ "authenticated": true, "token_saved": save, "token": "***" }),
                    format,
                );
                return ExitCode::from(exit::OK);
            }
            Err(e) => {
                return ExitCode::from(output::emit_error("AuthError", &e.to_string(), exit::ERROR))
            }
        }
    }
    // Auto-detect flow.
    let tokens = discord_auth::auth::find_tokens();
    if tokens.is_empty() {
        return ExitCode::from(output::emit_error(
            "NoTokenFound",
            "no token found in local Discord/browser. Use --paste to enter manually.",
            exit::ERROR,
        ));
    }
    // Validate each candidate, pick first valid.
    for (source, token) in &tokens {
        if let Ok(true) = discord_auth::auth::validate_token(token).await {
            if save {
                let _ = discord_auth::auth::save_token_to_env(token, None);
            }
            let _ = output::emit(
                &serde_json::json!({ "authenticated": true, "source": source, "token_saved": save }),
                format,
            );
            return ExitCode::from(exit::OK);
        }
    }
    ExitCode::from(output::emit_error(
        "InvalidTokens",
        "found token(s) but none validated against Discord",
        exit::ERROR,
    ))
}

/// `serve` — start the MCP server over stdio.
async fn cmd_serve() -> ExitCode {
    match discord_mcp::server::serve_stdio().await {
        Ok(_) => ExitCode::from(exit::OK),
        Err(e) => ExitCode::from(output::emit_error("McpError", &e.to_string(), exit::ERROR)),
    }
}

async fn cmd_whoami(cli: &Cli, format: Format) -> ExitCode {
    let mut client = match ApiClient::from_env(cli.token.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            return ExitCode::from(output::emit_error("AuthError", &e.to_string(), exit::ERROR))
        }
    };

    match client.get_me().await {
        Ok(me) => {
            let _ = output::emit(&me, format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("NotFound", &e.to_string(), exit::ERROR)),
    }
}
