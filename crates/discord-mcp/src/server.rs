//! MCP server (rmcp stdio) exposing Discord tools to AI agents.
//!
//! Tools return **JSON** (not plaintext — fixing the langkurt gap).
//! Uses rmcp's `#[tool_router]` + `#[tool]` macros (official pattern).

use rmcp::{
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{ServerCapabilities, ServerInfo},
    schemars::JsonSchema,
    tool, tool_handler, tool_router, ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};

use discord_core::client::ApiClient;

/// No-arg parameter type (empty schema).
#[derive(Serialize, Deserialize, JsonSchema, Default)]
pub struct EmptyParams {}

/// Guild ID parameter.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct GuildParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
}

/// Channel read parameter.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ReadParams {
    /// The channel ID (snowflake).
    pub channel_id: String,
    /// Max messages to read (default 50).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Fetch messages before this snowflake.
    #[serde(default)]
    pub before: Option<String>,
}

/// Send message parameter.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SendParams {
    /// The channel ID (snowflake).
    pub channel_id: String,
    /// Message content.
    pub content: String,
    /// Reply to this message ID.
    #[serde(default)]
    pub reply_to: Option<String>,
    /// Local file paths to attach (server-side; max 10, each ≤10MiB).
    #[serde(default)]
    pub files: Option<Vec<String>>,
}

/// Create a thread.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ThreadCreateParams {
    /// The channel ID (snowflake).
    pub channel_id: String,
    /// Thread name.
    pub name: String,
    /// Create from this message ID (text/announcement parent).
    #[serde(default)]
    pub message_id: Option<String>,
    /// Starter message content (required for forum; optional standalone).
    #[serde(default)]
    pub text: Option<String>,
    /// Auto-archive minutes (60|1440|4320|10080).
    #[serde(default)]
    pub archive: Option<u32>,
}

/// Top-reacted messages from the archive.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct TopReactionsParams {
    /// Filter by guild name.
    #[serde(default)]
    pub guild: Option<String>,
    /// Filter by channel name.
    #[serde(default)]
    pub channel: Option<String>,
    /// Max results (default 10).
    #[serde(default)]
    pub limit: Option<i64>,
}

/// Download archived attachments.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct DownloadParams {
    /// Filter by channel name or ID (from archive).
    #[serde(default)]
    pub channel: Option<String>,
    /// Filter by guild name or ID (from archive).
    #[serde(default)]
    pub guild: Option<String>,
    /// Media type (image|gif|video|all).
    #[serde(default)]
    pub media_type: Option<String>,
    /// Max files (0 = unlimited).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Output directory (default <data_dir>/media).
    #[serde(default)]
    pub out_dir: Option<String>,
    /// Only files from messages on/after this date (30d|6m|1y|YYYY-MM-DD).
    #[serde(default)]
    pub since: Option<String>,
}

/// Parse a `since` value (YYYY-MM-DD or <n><d|m|y>) — mirrors the download
/// CLI's parse_since so MCP and CLI behave identically.
fn parse_mcp_since(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return d
            .and_hms_opt(0, 0, 0)
            .map(|t| chrono::DateTime::from_naive_utc_and_offset(t, chrono::Utc));
    }
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = num.parse().ok()?;
    if n <= 0 {
        return None;
    }
    let now = chrono::Utc::now();
    match unit {
        "d" => Some(now - chrono::Duration::days(n)),
        "m" => Some(now - chrono::Duration::days(30 * n)),
        "y" => Some(now - chrono::Duration::days(365 * n)),
        _ => None,
    }
}

/// Set presence (persisted; applies to next tail/watch connect).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct PresenceParams {
    /// online | idle | dnd | invisible.
    pub status: String,
}

/// Join a server via invite.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct JoinParams {
    /// Invite code or full URL (discord.gg/..., discord.com/invite/...).
    pub invite_code: String,
    /// Must be true to actually join (advisory — client-side approval).
    pub confirm: bool,
}

/// Leave a server.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct LeaveParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// Must be true to actually leave (advisory — client-side approval).
    pub confirm: bool,
}

/// Get single message parameter.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct GetMessageParams {
    /// The channel ID (snowflake).
    pub channel_id: String,
    /// The message ID (snowflake).
    pub message_id: String,
}

/// List members parameter.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct MembersParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// Max members (default 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// List threads parameter.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ThreadsParams {
    /// The channel ID (snowflake).
    pub channel_id: String,
}

/// Search message parameter.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// Search query.
    pub query: String,
    /// Max results (default 25).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Create a channel (admin).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CreateChannelParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// Channel name (1-100 chars, no '#').
    pub name: String,
    /// Channel type: text|voice|category|announcement|stage|forum.
    #[serde(default)]
    pub channel_type: Option<String>,
    /// Parent category ID.
    #[serde(default)]
    pub category_id: Option<String>,
    /// Channel topic (≤1024).
    #[serde(default)]
    pub topic: Option<String>,
    /// Slowmode seconds (0-21600).
    #[serde(default)]
    pub slowmode: Option<u64>,
}

/// Edit a channel (admin).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct EditChannelParams {
    /// The channel ID (snowflake).
    pub channel_id: String,
    /// New name.
    #[serde(default)]
    pub name: Option<String>,
    /// New topic (≤1024).
    #[serde(default)]
    pub topic: Option<String>,
    /// New slowmode seconds (0-21600).
    #[serde(default)]
    pub slowmode: Option<u64>,
    /// New parent category ID.
    #[serde(default)]
    pub parent_id: Option<String>,
    /// New sorting position.
    #[serde(default)]
    pub position: Option<u32>,
    /// New channel type (0↔5 conversion only).
    #[serde(default)]
    pub channel_type: Option<String>,
}

/// Delete a channel (admin, gated).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct DeleteChannelParams {
    /// The channel ID (snowflake).
    pub channel_id: String,
    /// Must be true to delete (advisory).
    pub confirm: bool,
}

/// List roles.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ListRolesParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// Max roles (default all).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Create a role (admin).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CreateRoleParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// Role name.
    pub name: String,
    /// Color hex (#RRGGBB or RRGGBB).
    #[serde(default)]
    pub color: Option<String>,
    /// Permission names (comma-separated or list).
    #[serde(default)]
    pub permissions: Option<Vec<String>>,
    /// Allow anyone to mention.
    #[serde(default)]
    pub mentionable: Option<bool>,
    /// Show separately in the member list.
    #[serde(default)]
    pub hoist: Option<bool>,
}

/// Edit a role (admin).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct EditRoleParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// The role ID (snowflake).
    pub role_id: String,
    /// New name.
    #[serde(default)]
    pub name: Option<String>,
    /// New color hex.
    #[serde(default)]
    pub color: Option<String>,
    /// New permission names.
    #[serde(default)]
    pub permissions: Option<Vec<String>>,
    /// New mentionable state.
    #[serde(default)]
    pub mentionable: Option<bool>,
    /// New hoist state.
    #[serde(default)]
    pub hoist: Option<bool>,
}

/// Delete a role (admin, gated).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct DeleteRoleParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// The role ID (snowflake).
    pub role_id: String,
    /// Must be true to delete (advisory).
    pub confirm: bool,
}

/// Assign/remove a role to/from a member.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct RoleMemberParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// The member user ID (snowflake).
    pub user_id: String,
    /// The role ID (snowflake).
    pub role_id: String,
}

/// List custom emojis.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct GuildEmojiParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
}

/// Create a custom emoji (admin).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CreateEmojiParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// Emoji name (alphanumeric + underscore).
    pub name: String,
    /// Local image file path (png/jpg/gif, ≤256KiB).
    pub file_path: String,
}

/// Delete a custom emoji (admin, gated).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct DeleteEmojiParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// The emoji ID (snowflake).
    pub emoji_id: String,
    /// Must be true to delete (advisory).
    pub confirm: bool,
}

/// Kick a member (admin, gated).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct KickMemberParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// The member user ID (snowflake).
    pub user_id: String,
    /// Audit-log reason (X-Audit-Log-Reason header).
    #[serde(default)]
    pub reason: Option<String>,
    /// Must be true to kick (advisory).
    pub confirm: bool,
}

/// Ban a member (admin, gated).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct BanMemberParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// The member user ID (snowflake).
    pub user_id: String,
    /// Audit-log reason (body `reason`).
    #[serde(default)]
    pub reason: Option<String>,
    /// Delete message history up to this many days (0-7).
    #[serde(default)]
    pub delete_message_days: Option<u8>,
    /// Must be true to ban (advisory).
    pub confirm: bool,
}

/// Unban a user (admin, gated).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct UnbanMemberParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// The banned user ID (snowflake).
    pub user_id: String,
    /// Must be true to unban (advisory).
    pub confirm: bool,
}

/// Set a member's nickname (admin; not destructive, no confirm).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SetNicknameParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// The member user ID (snowflake).
    pub user_id: String,
    /// New nickname (empty clears).
    pub nickname: String,
}

/// View a channel's permission overwrites (read-only).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ViewOverwritesParams {
    /// The channel ID (snowflake).
    pub channel_id: String,
}

/// Set a permission overwrite (admin; ≥1 of allow/deny).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SetOverwritesParams {
    /// The channel ID (snowflake).
    pub channel_id: String,
    /// The role ID (exactly one of role_id/user_id required).
    #[serde(default)]
    pub role_id: Option<String>,
    /// The user ID (exactly one of role_id/user_id required).
    #[serde(default)]
    pub user_id: Option<String>,
    /// Permission names to allow.
    #[serde(default)]
    pub allow: Option<Vec<String>>,
    /// Permission names to deny.
    #[serde(default)]
    pub deny: Option<Vec<String>>,
}

/// Lock a channel read-only for @everyone (admin, gated).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct LockChannelParams {
    /// The channel ID (snowflake).
    pub channel_id: String,
    /// The guild ID (snowflake); @everyone overwrite target.
    pub guild_id: String,
    /// Must be true to lock (advisory).
    pub confirm: bool,
}

/// Unlock a channel (admin, gated).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct UnlockChannelParams {
    /// The channel ID (snowflake).
    pub channel_id: String,
    /// The guild ID (snowflake); @everyone overwrite target.
    pub guild_id: String,
    /// Must be true to unlock (advisory).
    pub confirm: bool,
}

/// Fetch a guild's audit log (read-only; VIEW_AUDIT_LOG).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct AuditLogParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// Filter by the user who performed the action (numeric ID).
    #[serde(default)]
    pub user_id: Option<String>,
    /// Filter by action name (e.g. "member_kick", "channel_create"). Resolved
    /// via the audit-action map; unknown names error.
    #[serde(default)]
    pub action_type: Option<String>,
    /// Max entries (default 50, capped 100).
    #[serde(default)]
    pub limit: Option<u8>,
}

/// List a guild's invites (MANAGE_CHANNELS).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ListInvitesParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
}

/// Create a channel invite (CREATE_INSTANT_INVITE; not destructive).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CreateInviteParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// The channel ID (snowflake; text-like).
    pub channel_id: String,
    /// Duration in seconds before expiry (0 = never; default 86400).
    #[serde(default)]
    pub max_age: Option<u32>,
    /// Max uses (0 = unlimited; default 0).
    #[serde(default)]
    pub max_uses: Option<u32>,
    /// Grant temporary membership.
    #[serde(default)]
    pub temporary: Option<bool>,
}

/// Delete an invite (MANAGE_CHANNELS; advisory confirm).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct DeleteInviteParams {
    /// Invite code or full URL (discord.gg/..., discord.com/invite/...).
    pub code: String,
    /// Must be true to delete (advisory — client-side approval).
    pub confirm: bool,
}

/// One embed field for `send_embed` (F9).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct EmbedFieldInput {
    /// Field name (≤256).
    pub name: String,
    /// Field value (≤1024).
    pub value: String,
    /// Whether to render inline (default false).
    #[serde(default)]
    pub inline: bool,
}

/// Send a message with a rich embed (F9; advisory confirm).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SendEmbedParams {
    /// The channel ID (snowflake).
    pub channel_id: String,
    /// Embed title (≤256).
    #[serde(default)]
    pub title: Option<String>,
    /// Embed description (≤4096).
    #[serde(default)]
    pub description: Option<String>,
    /// Embed color hex (#RRGGBB or RRGGBB).
    #[serde(default)]
    pub color: Option<String>,
    /// Clickable title URL.
    #[serde(default)]
    pub url: Option<String>,
    /// Image URL.
    #[serde(default)]
    pub image: Option<String>,
    /// Footer text.
    #[serde(default)]
    pub footer: Option<String>,
    /// Author name.
    #[serde(default)]
    pub author: Option<String>,
    /// Embed fields (max 10).
    #[serde(default)]
    pub fields: Vec<EmbedFieldInput>,
    /// Plain text content alongside the embed.
    #[serde(default)]
    pub content: Option<String>,
    /// Reply to this message ID.
    #[serde(default)]
    pub reply_to: Option<String>,
    /// Must be true to send (advisory — client-side approval).
    pub confirm: bool,
}

/// Edit guild settings (admin; MANAGE_GUILD).
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct EditGuildParams {
    /// The guild ID (snowflake).
    pub guild_id: String,
    /// New server name (2-100 chars).
    #[serde(default)]
    pub name: Option<String>,
    /// New description (≤120; community servers).
    #[serde(default)]
    pub description: Option<String>,
    /// Verification level: none|low|medium|high|very_high.
    #[serde(default)]
    pub verification: Option<String>,
    /// Default notifications: all_messages|only_mentions.
    #[serde(default)]
    pub notifications: Option<String>,
    /// Content filter: disabled|members_without_roles|all_members.
    #[serde(default)]
    pub content_filter: Option<String>,
    /// AFK timeout seconds (60|300|900|1800|3600).
    #[serde(default)]
    pub afk_timeout: Option<u32>,
    /// System channel ID.
    #[serde(default)]
    pub system_channel: Option<String>,
    /// Rules channel ID.
    #[serde(default)]
    pub rules_channel: Option<String>,
}

/// The MCP server.
#[derive(Clone)]
pub struct DiscordMcpServer {
    tool_router: ToolRouter<Self>,
}

impl DiscordMcpServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    fn client(&self) -> Result<ApiClient, String> {
        ApiClient::from_env(None).map_err(|e| e.to_string())
    }
}

impl Default for DiscordMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router(router = tool_router)]
impl DiscordMcpServer {
    /// List all Discord servers the user belongs to.
    #[tool(description = "List all Discord servers (guilds) the logged-in user belongs to.")]
    pub async fn list_guilds(&self, _params: Parameters<EmptyParams>) -> Result<String, String> {
        let mut c = self.client()?;
        let guilds = c.list_guilds().await.map_err(|e| e.to_string())?;
        Ok(serde_json::to_string(&guilds).unwrap_or_else(|_| "[]".into()))
    }

    /// List text channels of a guild.
    #[tool(description = "List text/announcement/forum channels of a guild.")]
    pub async fn list_channels(
        &self,
        Parameters(req): Parameters<GuildParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        let channels = c
            .list_channels(&req.guild_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::to_string(&channels).unwrap_or_else(|_| "[]".into()))
    }

    /// List DM and group-DM channels.
    #[tool(description = "List DM and group-DM channels of the user.")]
    pub async fn list_dms(&self, _params: Parameters<EmptyParams>) -> Result<String, String> {
        let mut c = self.client()?;
        let dms = c.list_dms().await.map_err(|e| e.to_string())?;
        Ok(serde_json::to_string(&dms).unwrap_or_else(|_| "[]".into()))
    }

    /// Read recent messages from a channel.
    #[tool(description = "Read recent messages from a channel (agent-friendly JSON).")]
    pub async fn read_messages(
        &self,
        Parameters(req): Parameters<ReadParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        let limit = req.limit.unwrap_or(50) as usize;
        let before = req.before.as_deref().and_then(|s| s.parse().ok());
        let msgs = c
            .fetch_messages(&req.channel_id, limit, before, None)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::to_string(&msgs).unwrap_or_else(|_| "[]".into()))
    }

    /// Send a message to a channel.
    ///
    /// If `files` is set, each path is read from the MCP server's local
    /// filesystem and attached (max 10 files, each ≤10MiB). Note: the
    /// `--confirm` gate is advisory here — approval is enforced by the MCP
    /// client, not the server.
    #[tool(
        description = "Send a message to a channel, optionally with local file attachments. Gate behind approval in client (advisory server-side)."
    )]
    pub async fn send_message(
        &self,
        Parameters(req): Parameters<SendParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        let id = match req.files.as_deref() {
            None => c
                .send_message(
                    &req.channel_id,
                    &req.content,
                    req.reply_to.as_deref(),
                    0,
                    &[],
                )
                .await
                .map_err(|e| e.to_string())?,
            Some(paths) => {
                // Server-local attachment load (same caps as CLI: 10 files, 10MiB).
                let mut atts = Vec::with_capacity(paths.len());
                for path in paths {
                    let data = std::fs::read(path)
                        .map_err(|e| format!("cannot read file \"{path}\": {e}"))?;
                    if data.len() > 10 * 1024 * 1024 {
                        return Err(format!("file too large (>10MiB): {path}"));
                    }
                    let filename = std::path::Path::new(path)
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.clone());
                    let mime = mime_guess::from_path(path)
                        .first_raw()
                        .unwrap_or("application/octet-stream")
                        .to_string();
                    atts.push(discord_user::types::CreateAttachment {
                        filename,
                        data,
                        mime_type: mime,
                        description: None,
                    });
                }
                if atts.len() > 10 {
                    return Err("too many files: max 10 per message".into());
                }
                c.send_message_with_files(
                    &req.channel_id,
                    &req.content,
                    req.reply_to.as_deref(),
                    atts,
                    0,
                    &[],
                )
                .await
                .map_err(|e| e.to_string())?
            }
        };
        Ok(format!(r#"{{"message_id":"{id}"}}"#))
    }

    /// Search messages in a guild.
    #[tool(description = "Native Discord search within a guild.")]
    pub async fn search_messages(
        &self,
        Parameters(req): Parameters<SearchParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        let limit = req.limit.unwrap_or(25);
        let msgs = c
            .search_guild_messages(&req.guild_id, &req.query, None, limit)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::to_string(&msgs).unwrap_or_else(|_| "[]".into()))
    }

    /// Read a DM channel's recent messages.
    #[tool(description = "Read recent messages from a DM channel (same as read_messages).")]
    pub async fn read_dm(&self, Parameters(req): Parameters<ReadParams>) -> Result<String, String> {
        let mut c = self.client()?;
        let limit = req.limit.unwrap_or(50) as usize;
        let before = req.before.as_deref().and_then(|s| s.parse().ok());
        let msgs = c
            .fetch_messages(&req.channel_id, limit, before, None)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::to_string(&msgs).unwrap_or_else(|_| "[]".into()))
    }

    /// Top-reacted messages from the local archive (sync first).
    #[tool(
        description = "Rank archived messages by reaction count (hottest first). Requires prior sync."
    )]
    pub async fn top_reactions(
        &self,
        Parameters(req): Parameters<TopReactionsParams>,
    ) -> Result<String, String> {
        let db_path = discord_core::config::db_path().map_err(|e| e.to_string())?;
        let conn = discord_db::db::open(db_path.to_str().unwrap_or("discord.db"))
            .map_err(|e| e.to_string())?;
        let rows = discord_db::db::top_reacted(
            &conn,
            req.guild.as_deref(),
            req.channel.as_deref(),
            req.limit.unwrap_or(10),
        )
        .map_err(|e| e.to_string())?;
        Ok(serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into()))
    }

    /// Download archived attachments summary (langkurt MCP mirror).
    ///
    /// Reports pending attachment count + sample filenames from the local
    /// archive; the actual fetch is done via the `discord download` CLI
    /// (the MCP server avoids a binary-only dependency).
    #[tool(
        description = "Report archived attachments pending download (sync first); fetch via the download CLI."
    )]
    pub async fn download_attachments(
        &self,
        Parameters(req): Parameters<DownloadParams>,
    ) -> Result<String, String> {
        let db_path = discord_core::config::db_path().map_err(|e| e.to_string())?;
        let conn = discord_db::db::open(db_path.to_str().unwrap_or("discord.db"))
            .map_err(|e| e.to_string())?;
        let mut filter = discord_db::attachments::AttachmentFilter {
            media_type: req
                .media_type
                .filter(|t| *t != "all")
                .map(|s| s.to_string()),
            limit: req.limit.unwrap_or(0),
            ..Default::default()
        };
        if let Some(s) = &req.since {
            let parsed = match parse_mcp_since(s) {
                Some(t) => t.to_rfc3339(),
                None => return Err(format!("invalid since \"{s}\" (YYYY-MM-DD or 30d/6m/1y)")),
            };
            filter.since = Some(parsed);
        }
        if let Some(c) = &req.channel {
            let id = discord_db::db::find_channel_id(&conn, c)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("channel \"{c}\" not found in archive (sync first)"))?;
            filter.channel_id = Some(id);
        }
        if let Some(g) = &req.guild {
            let id = discord_db::db::find_guild_id(&conn, g)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("guild \"{g}\" not found in archive"))?;
            filter.guild_id = Some(id);
        }
        let rows = discord_db::attachments::list_pending_attachments(&conn, &filter)
            .map_err(|e| e.to_string())?;
        let files: Vec<String> = rows.iter().take(20).map(|a| a.filename.clone()).collect();
        Ok(serde_json::json!({
            "pending": rows.len(),
            "sample_files": files,
            "note": "run `discord download` (CLI) to actually fetch files",
        })
        .to_string())
    }

    /// Create a thread (standalone, from message, or forum post).
    #[tool(
        description = "Create a thread: standalone, from a message, or a forum post (with starter text)."
    )]
    pub async fn create_thread(
        &self,
        Parameters(req): Parameters<ThreadCreateParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        let result = match req.message_id.as_deref() {
            Some(mid) => c
                .create_thread_from_message(&req.channel_id, mid, &req.name, req.archive)
                .await
                .map_err(|e| e.to_string())?,
            None => c
                .create_thread(
                    &req.channel_id,
                    &req.name,
                    req.archive,
                    req.text.as_deref(),
                    None,
                )
                .await
                .map_err(|e| e.to_string())?,
        };
        Ok(serde_json::json!({
            "type": if req.message_id.is_some() { "message_thread" }
                    else if result.channel_type == 15 || result.channel_type == 16 { "forum_post" }
                    else { "standalone_thread" },
            "id": result.id,
            "name": result.name,
            "channel_id": result.channel_id,
        })
        .to_string())
    }

    /// Set presence for future connections (persisted to config.json).
    ///
    /// The MCP server has no live gateway per invocation, so this persists
    /// the status — it takes effect on the next `tail`/`watch` connect.
    #[tool(
        description = "Set presence status (online|idle|dnd|invisible), persisted for future connections."
    )]
    pub async fn set_presence(
        &self,
        Parameters(req): Parameters<PresenceParams>,
    ) -> Result<String, String> {
        if !discord_core::config::set_configured_presence(&req.status) {
            return Err(format!(
                "invalid presence: {} (valid: online, idle, dnd, invisible)",
                req.status
            ));
        }
        Ok(serde_json::json!({ "presence": req.status, "saved": true }).to_string())
    }

    /// Join a server via invite code or URL.
    ///
    /// Previews the invite (guild name, member count) then accepts. The
    /// `confirm` flag is advisory — client-side approval is the real gate.
    #[tool(description = "Join a server via invite code or URL. confirm must be true (advisory).")]
    pub async fn join_guild(
        &self,
        Parameters(req): Parameters<JoinParams>,
    ) -> Result<String, String> {
        if !req.confirm {
            return Err("join_guild requires confirm: true".into());
        }
        let code = match ApiClient::extract_invite_code(&req.invite_code) {
            Some(c) => c.to_string(),
            None => return Err(format!("invalid invite: {}", req.invite_code)),
        };
        let mut c = self.client()?;
        // Preview first, then accept (satisfies the {guild_name,members} contract).
        let info = c.get_invite(&code).await.map_err(|e| e.to_string())?;
        let guild_name = info
            .guild
            .as_ref()
            .and_then(|g| g.name.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let members = info.approximate_member_count.unwrap_or(0);
        c.accept_invite(&code).await.map_err(|e| e.to_string())?;
        Ok(serde_json::json!({
            "joined": true,
            "invite_code": code,
            "guild_name": guild_name,
            "approximate_member_count": members,
        })
        .to_string())
    }

    /// Leave a server.
    ///
    /// The `confirm` flag is advisory — client-side approval is the real gate.
    #[tool(description = "Leave a server by guild_id. confirm must be true (advisory).")]
    pub async fn leave_guild(
        &self,
        Parameters(req): Parameters<LeaveParams>,
    ) -> Result<String, String> {
        if !req.confirm {
            return Err("leave_guild requires confirm: true".into());
        }
        let mut c = self.client()?;
        c.leave_guild(&req.guild_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "left": true, "guild_id": req.guild_id }).to_string())
    }

    /// Get a single message by channel + message ID.
    #[tool(description = "Fetch a single message by channel_id and message_id.")]
    pub async fn get_message(
        &self,
        Parameters(req): Parameters<GetMessageParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        let msg = c
            .get_message(&req.channel_id, &req.message_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::to_string(&msg).unwrap_or_else(|_| "{}".into()))
    }

    /// List guild members.
    #[tool(description = "List members of a guild.")]
    pub async fn list_members(
        &self,
        Parameters(req): Parameters<MembersParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        let members = c
            .list_members(&req.guild_id, req.limit.unwrap_or(50))
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::to_string(&members).unwrap_or_else(|_| "[]".into()))
    }

    /// List threads in a channel (user-token fallback).
    #[tool(description = "List active threads in a channel (handles user-token fallback).")]
    pub async fn list_threads(
        &self,
        Parameters(req): Parameters<ThreadsParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        let threads = c
            .list_threads(&req.channel_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::to_string(&threads).unwrap_or_else(|_| "[]".into()))
    }

    /// Get local archive sync status.
    #[tool(description = "Get per-channel sync status of the local SQLite archive.")]
    pub async fn get_sync_status(
        &self,
        _params: Parameters<EmptyParams>,
    ) -> Result<String, String> {
        // Best-effort: report whether a local DB exists.
        let db_path = discord_core::config::db_path().map_err(|e| e.to_string())?;
        let exists = db_path.exists();
        Ok(serde_json::json!({
            "db_path": db_path.to_string_lossy(),
            "db_exists": exists,
            "note": "run `discord dc sync-all` to populate the archive"
        })
        .to_string())
    }

    /// Create a guild channel (admin; requires MANAGE_CHANNELS).
    #[tool(
        description = "Create a guild channel. channel_type: text|voice|category|announcement|stage|forum."
    )]
    pub async fn create_channel(
        &self,
        Parameters(req): Parameters<CreateChannelParams>,
    ) -> Result<String, String> {
        let ctype: u8 = match req.channel_type.as_deref() {
            None => 0u8,
            Some(t) => discord_core::types::parse_channel_type_name(t).ok_or_else(|| {
                format!(
                    "invalid channel_type \"{t}\" (valid: text, voice, category, announcement, stage, forum)"
                )
            })?,
        };
        if !ApiClient::validate_channel_name(&req.name) {
            return Err(format!(
                "invalid channel name \"{}\" (1-100 chars, no '#')",
                req.name
            ));
        }
        if let Some(t) = &req.topic {
            if !ApiClient::validate_topic(t) {
                return Err("invalid topic (max 1024 chars)".into());
            }
        }
        if req.slowmode.is_some_and(|s| s > 21_600) {
            return Err("invalid slowmode (0-21600)".into());
        }
        let mut c = self.client()?;
        let mut cr = discord_user::types::CreateChannelRequest::new(&req.name);
        cr.channel_type = Some(ctype);
        cr.parent_id = req.category_id.clone();
        cr.topic = req.topic.clone();
        cr.rate_limit_per_user = req.slowmode.map(|s| s as u32);
        let ch = c
            .create_channel(&req.guild_id, cr)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&ch).map_err(|e| e.to_string())
    }

    /// Edit a guild channel (admin).
    #[tool(description = "Edit a guild channel: rename, topic, slowmode, parent, position, type.")]
    pub async fn edit_channel(
        &self,
        Parameters(req): Parameters<EditChannelParams>,
    ) -> Result<String, String> {
        if let Some(n) = &req.name {
            if !ApiClient::validate_channel_name(n) {
                return Err(format!(
                    "invalid channel name \"{n}\" (1-100 chars, no '#')"
                ));
            }
        }
        if let Some(t) = &req.topic {
            if !ApiClient::validate_topic(t) {
                return Err("invalid topic (max 1024 chars)".into());
            }
        }
        let channel_type = match req.channel_type.as_deref() {
            None => None,
            Some(t) => Some(
                discord_core::types::parse_channel_type_name(t).ok_or_else(|| {
                    format!("invalid channel_type \"{t}\" (text|voice|category|announcement|stage|forum)")
                })?,
            ),
        };
        let mut c = self.client()?;
        let er = discord_user::types::EditChannelRequest {
            name: req.name.clone(),
            topic: req.topic.clone(),
            rate_limit_per_user: req.slowmode.map(|s| s as u32),
            parent_id: req.parent_id.clone(),
            position: req.position,
            channel_type,
            ..Default::default()
        };
        let ch = c
            .edit_channel(&req.channel_id, er)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&ch).map_err(|e| e.to_string())
    }

    /// Delete a guild channel (admin; confirm gated).
    #[tool(description = "Delete a guild channel. confirm must be true (advisory).")]
    pub async fn delete_channel(
        &self,
        Parameters(req): Parameters<DeleteChannelParams>,
    ) -> Result<String, String> {
        if !req.confirm {
            return Err("delete_channel requires confirm: true".into());
        }
        let mut c = self.client()?;
        c.delete_channel(&req.channel_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "deleted": true, "channel_id": req.channel_id }).to_string())
    }

    /// List guild roles.
    #[tool(description = "List roles of a guild (sorted by position, desc).")]
    pub async fn list_roles(
        &self,
        Parameters(req): Parameters<ListRolesParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        let roles = c
            .list_roles(&req.guild_id)
            .await
            .map_err(|e| e.to_string())?;
        let out: Vec<_> = match req.limit {
            Some(n) => roles.into_iter().take(n as usize).collect(),
            None => roles,
        };
        serde_json::to_string(&out).map_err(|e| e.to_string())
    }

    /// Create a guild role (admin; MANAGE_ROLES).
    #[tool(description = "Create a guild role with optional color and permission names.")]
    pub async fn create_role(
        &self,
        Parameters(req): Parameters<CreateRoleParams>,
    ) -> Result<String, String> {
        let color = match &req.color {
            Some(c) => Some(ApiClient::parse_color_hex(c).map_err(|e| e.to_string())?),
            None => None,
        };
        let permissions = match &req.permissions {
            Some(names) => Some(
                ApiClient::parse_permission_names(names)
                    .map_err(|e| e.to_string())?
                    .to_string(),
            ),
            None => None,
        };
        let mut c = self.client()?;
        let rr = discord_user::types::CreateRoleRequest {
            name: Some(req.name.clone()),
            color,
            permissions,
            mentionable: req.mentionable,
            hoist: req.hoist,
        };
        let role = c
            .create_role(&req.guild_id, rr)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&role).map_err(|e| e.to_string())
    }

    /// Edit a guild role (admin).
    #[tool(description = "Edit a guild role: name, color, permissions, mentionable, hoist.")]
    pub async fn edit_role(
        &self,
        Parameters(req): Parameters<EditRoleParams>,
    ) -> Result<String, String> {
        let color = match &req.color {
            Some(c) => Some(ApiClient::parse_color_hex(c).map_err(|e| e.to_string())?),
            None => None,
        };
        let permissions = match &req.permissions {
            Some(names) => Some(
                ApiClient::parse_permission_names(names)
                    .map_err(|e| e.to_string())?
                    .to_string(),
            ),
            None => None,
        };
        let mut c = self.client()?;
        let rr = discord_user::types::CreateRoleRequest {
            name: req.name.clone(),
            color,
            permissions,
            mentionable: req.mentionable,
            hoist: req.hoist,
        };
        let role = c
            .edit_role(&req.guild_id, &req.role_id, rr)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&role).map_err(|e| e.to_string())
    }

    /// Delete a guild role (admin; confirm gated).
    #[tool(description = "Delete a guild role. confirm must be true (advisory).")]
    pub async fn delete_role(
        &self,
        Parameters(req): Parameters<DeleteRoleParams>,
    ) -> Result<String, String> {
        if !req.confirm {
            return Err("delete_role requires confirm: true".into());
        }
        let mut c = self.client()?;
        c.delete_role(&req.guild_id, &req.role_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "deleted": true, "role_id": req.role_id }).to_string())
    }

    /// Assign a role to a member (admin).
    #[tool(description = "Assign a role to a member by user_id.")]
    pub async fn assign_role(
        &self,
        Parameters(req): Parameters<RoleMemberParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        c.add_member_role(&req.guild_id, &req.user_id, &req.role_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "assigned": true }).to_string())
    }

    /// Remove a role from a member (admin).
    #[tool(description = "Remove a role from a member by user_id.")]
    pub async fn remove_role(
        &self,
        Parameters(req): Parameters<RoleMemberParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        c.remove_member_role(&req.guild_id, &req.user_id, &req.role_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "removed": true }).to_string())
    }

    /// List custom guild emojis.
    #[tool(description = "List custom emojis of a guild.")]
    pub async fn list_emojis(
        &self,
        Parameters(req): Parameters<GuildEmojiParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        let emojis = c
            .list_emojis(&req.guild_id)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&emojis).map_err(|e| e.to_string())
    }

    /// Create a custom guild emoji (admin; ≤256KiB).
    #[tool(description = "Upload a custom emoji from a local file (png/jpg/gif, ≤256KiB).")]
    pub async fn create_emoji(
        &self,
        Parameters(req): Parameters<CreateEmojiParams>,
    ) -> Result<String, String> {
        if !ApiClient::validate_emoji_name(&req.name) {
            return Err(format!(
                "invalid emoji name \"{}\" (alphanumeric + underscore, 2-32 chars)",
                req.name
            ));
        }
        let mut c = self.client()?;
        let emoji = c
            .create_emoji(&req.guild_id, &req.name, &req.file_path)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&emoji).map_err(|e| e.to_string())
    }

    /// Delete a custom guild emoji (admin; confirm gated).
    #[tool(description = "Delete a custom emoji. confirm must be true (advisory).")]
    pub async fn delete_emoji(
        &self,
        Parameters(req): Parameters<DeleteEmojiParams>,
    ) -> Result<String, String> {
        if !req.confirm {
            return Err("delete_emoji requires confirm: true".into());
        }
        let mut c = self.client()?;
        c.delete_emoji(&req.guild_id, &req.emoji_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "deleted": true, "emoji_id": req.emoji_id }).to_string())
    }

    /// Kick a guild member (admin; KICK_MEMBERS).
    #[tool(
        description = "Kick a member from a guild (requires KICK_MEMBERS). confirm must be true (advisory)."
    )]
    pub async fn kick_member(
        &self,
        Parameters(req): Parameters<KickMemberParams>,
    ) -> Result<String, String> {
        if !req.confirm {
            return Err("kick_member requires confirm: true".into());
        }
        let mut c = self.client()?;
        c.kick_member(&req.guild_id, &req.user_id, req.reason.as_deref())
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "kicked": true, "user_id": req.user_id }).to_string())
    }

    /// Ban a guild member (admin; BAN_MEMBERS).
    #[tool(
        description = "Ban a member from a guild (requires BAN_MEMBERS). delete_message_days 0-7. confirm must be true (advisory)."
    )]
    pub async fn ban_member(
        &self,
        Parameters(req): Parameters<BanMemberParams>,
    ) -> Result<String, String> {
        if !req.confirm {
            return Err("ban_member requires confirm: true".into());
        }
        if req.delete_message_days.is_some_and(|d| d > 7) {
            return Err("delete_message_days must be 0-7".into());
        }
        let mut c = self.client()?;
        c.ban_member(
            &req.guild_id,
            &req.user_id,
            req.reason.as_deref(),
            req.delete_message_days,
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "banned": true, "user_id": req.user_id }).to_string())
    }

    /// Unban a user (admin; BAN_MEMBERS).
    #[tool(
        description = "Unban a user from a guild (banned users are not in the member list). confirm must be true (advisory)."
    )]
    pub async fn unban_member(
        &self,
        Parameters(req): Parameters<UnbanMemberParams>,
    ) -> Result<String, String> {
        if !req.confirm {
            return Err("unban_member requires confirm: true".into());
        }
        let mut c = self.client()?;
        c.unban_member(&req.guild_id, &req.user_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "unbanned": true, "user_id": req.user_id }).to_string())
    }

    /// Set/clear a member's nickname (admin; MANAGE_NICKNAMES).
    #[tool(
        description = "Set a member's server nickname (requires MANAGE_NICKNAMES). Empty clears."
    )]
    pub async fn set_nickname(
        &self,
        Parameters(req): Parameters<SetNicknameParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        // Empty nickname → Some("") so the field is SENT and Discord clears
        // it (Some("") clears; None would omit the field and be a no-op).
        let nick = if req.nickname.is_empty() {
            Some("")
        } else {
            Some(req.nickname.as_str())
        };
        c.set_nickname(&req.guild_id, &req.user_id, nick)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "nickname_set": true, "user_id": req.user_id }).to_string())
    }

    /// View a channel's permission overwrites (read-only).
    #[tool(
        description = "List a channel's permission overwrites (roles/members) with allow/deny bitfields."
    )]
    pub async fn view_overwrites(
        &self,
        Parameters(req): Parameters<ViewOverwritesParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        let overwrites = c
            .get_channel_overwrites(&req.channel_id)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&overwrites).map_err(|e| e.to_string())
    }

    /// Set a permission overwrite (admin; MANAGE_CHANNELS).
    #[tool(
        description = "Set a channel permission overwrite for a role or user. Provide role_id XOR user_id, and ≥1 of allow/deny. Both sides are always transmitted (0 when absent)."
    )]
    pub async fn set_overwrites(
        &self,
        Parameters(req): Parameters<SetOverwritesParams>,
    ) -> Result<String, String> {
        let (overwrite_id, kind) = match (&req.role_id, &req.user_id) {
            (Some(r), None) => (r.clone(), 0u8),
            (None, Some(u)) => (u.clone(), 1u8),
            _ => return Err("set_overwrites requires exactly one of role_id/user_id".into()),
        };
        if req.allow.is_none() && req.deny.is_none() {
            return Err("set_overwrites requires ≥1 of allow/deny".into());
        }
        let allow = match &req.allow {
            Some(names) => ApiClient::parse_permission_names(names).map_err(|e| e.to_string())?,
            None => 0,
        };
        let deny = match &req.deny {
            Some(names) => ApiClient::parse_permission_names(names).map_err(|e| e.to_string())?,
            None => 0,
        };
        let mut c = self.client()?;
        c.edit_channel_permission(&req.channel_id, &overwrite_id, allow, deny, kind)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({
            "permission_set": true,
            "channel_id": req.channel_id,
            "overwrite_id": overwrite_id,
            "allow": allow.to_string(),
            "deny": deny.to_string(),
        })
        .to_string())
    }

    /// Lock a channel read-only for @everyone (admin; gated).
    #[tool(
        description = "Deny SEND_MESSAGES|SEND_MESSAGES_IN_THREADS|CREATE_PUBLIC_THREADS for @everyone (read-only). confirm must be true (advisory)."
    )]
    pub async fn lock_channel(
        &self,
        Parameters(req): Parameters<LockChannelParams>,
    ) -> Result<String, String> {
        if !req.confirm {
            return Err("lock_channel requires confirm: true".into());
        }
        let mut c = self.client()?;
        c.lock_channel(&req.channel_id, &req.guild_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "locked": true, "channel_id": req.channel_id }).to_string())
    }

    /// Unlock a channel (admin; gated).
    #[tool(
        description = "Delete the @everyone overwrite, restoring send access. confirm must be true (advisory)."
    )]
    pub async fn unlock_channel(
        &self,
        Parameters(req): Parameters<UnlockChannelParams>,
    ) -> Result<String, String> {
        if !req.confirm {
            return Err("unlock_channel requires confirm: true".into());
        }
        let mut c = self.client()?;
        c.unlock_channel(&req.channel_id, &req.guild_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "unlocked": true, "channel_id": req.channel_id }).to_string())
    }

    /// Edit guild settings (admin; MANAGE_GUILD).
    #[tool(
        description = "Edit guild settings: name, description, verification, notifications, content-filter, afk-timeout, system/rules channels. Reversible; no confirm. Requires MANAGE_GUILD."
    )]
    pub async fn edit_guild(
        &self,
        Parameters(req): Parameters<EditGuildParams>,
    ) -> Result<String, String> {
        let verification =
            match &req.verification {
                Some(v) => Some(discord_core::types::parse_verification_level(v).ok_or_else(
                    || format!("invalid verification \"{v}\" (none|low|medium|high|very_high)"),
                )?),
                None => None,
            };
        let notifications =
            match &req.notifications {
                Some(n) => Some(discord_core::types::parse_notification_level(n).ok_or_else(
                    || format!("invalid notifications \"{n}\" (all_messages|only_mentions)"),
                )?),
                None => None,
            };
        let content_filter = match &req.content_filter {
            Some(c) => Some(discord_core::types::parse_content_filter(c).ok_or_else(|| {
                format!(
                    "invalid content_filter \"{c}\" (disabled|members_without_roles|all_members)"
                )
            })?),
            None => None,
        };
        if req
            .afk_timeout
            .is_some_and(|t| !matches!(t, 60 | 300 | 900 | 1800 | 3600))
        {
            return Err("afk_timeout must be one of 60|300|900|1800|3600".into());
        }
        let guild_req = discord_user::types::EditGuildRequest {
            name: req.name.clone(),
            description: req.description.clone(),
            verification_level: verification,
            default_message_notifications: notifications,
            explicit_content_filter: content_filter,
            afk_timeout: req.afk_timeout,
            system_channel_id: req.system_channel.clone(),
            rules_channel_id: req.rules_channel.clone(),
            ..Default::default()
        };
        let mut c = self.client()?;
        let guild = c
            .edit_guild(&req.guild_id, guild_req)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&guild).map_err(|e| e.to_string())
    }

    /// Fetch a guild's audit log (read-only; VIEW_AUDIT_LOG).
    #[tool(
        description = "Fetch a guild's audit log. action_type is an action NAME (e.g. member_kick, channel_create) resolved to the numeric code; limit capped at 100."
    )]
    pub async fn get_audit_logs(
        &self,
        Parameters(req): Parameters<AuditLogParams>,
    ) -> Result<String, String> {
        let action_type = match req.action_type.as_deref() {
            Some(name) => {
                Some(discord_core::types::audit_action_code(name).map_err(|e| e.to_string())?)
            }
            None => None,
        };
        let user_id = match req.user_id.as_deref() {
            Some(u) => Some(
                u.parse::<u64>()
                    .map_err(|_| format!("invalid user_id \"{u}\""))?,
            ),
            None => None,
        };
        let mut c = self.client()?;
        let log = c
            .audit_logs(&req.guild_id, user_id, action_type, req.limit)
            .await
            .map_err(|e| e.to_string())?;
        // Render agent-friendly rows: action name + username + change summary.
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
                    Some(n) => n.to_string(),
                    None => format!("unknown({})", e.action_type),
                };
                let username = e
                    .user_id
                    .as_deref()
                    .and_then(|uid| users.get(uid).map(|u| u.to_string()));
                let change_summary = {
                    let mut parts: Vec<String> = Vec::new();
                    for c in e.changes.iter().take(3) {
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
                    if parts.is_empty() {
                        None
                    } else {
                        Some(parts.join("; "))
                    }
                };
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
        serde_json::to_string(&rows).map_err(|e| e.to_string())
    }

    /// List a guild's invites (MANAGE_CHANNELS).
    #[tool(description = "List a guild's invites (requires MANAGE_CHANNELS).")]
    pub async fn list_invites(
        &self,
        Parameters(req): Parameters<ListInvitesParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        let invites = c
            .list_guild_invites(&req.guild_id)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&invites).map_err(|e| e.to_string())
    }

    /// Create a channel invite (CREATE_INSTANT_INVITE; not destructive).
    #[tool(
        description = "Create a guild invite for a text channel. Sets unique=true (one-time link). Not destructive; no confirm needed."
    )]
    pub async fn create_invite(
        &self,
        Parameters(req): Parameters<CreateInviteParams>,
    ) -> Result<String, String> {
        let mut c = self.client()?;
        let inv_req = discord_user::types::CreateInviteRequest {
            max_age: req.max_age,
            max_uses: req.max_uses,
            temporary: req.temporary,
            unique: Some(true),
            ..Default::default()
        };
        let inv = c
            .create_channel_invite(&req.channel_id, inv_req)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&inv).map_err(|e| e.to_string())
    }

    /// Delete an invite by code or URL (MANAGE_CHANNELS; advisory confirm).
    #[tool(description = "Delete an invite by code or full URL. confirm must be true (advisory).")]
    pub async fn delete_invite(
        &self,
        Parameters(req): Parameters<DeleteInviteParams>,
    ) -> Result<String, String> {
        if !req.confirm {
            return Err("delete_invite requires confirm: true".into());
        }
        let mut c = self.client()?;
        c.delete_invite(&req.code)
            .await
            .map_err(|e| e.to_string())?;
        let bare = ApiClient::extract_invite_code(&req.code)
            .unwrap_or(&req.code)
            .to_string();
        Ok(serde_json::json!({ "deleted": true, "code": bare }).to_string())
    }

    /// Send a message with a rich embed (F9; advisory confirm).
    #[tool(
        description = "Send a message with a rich embed. Requires ≥1 of title/description/content. confirm must be true (advisory)."
    )]
    pub async fn send_embed(
        &self,
        Parameters(req): Parameters<SendEmbedParams>,
    ) -> Result<String, String> {
        if !req.confirm {
            return Err("send_embed requires confirm: true".into());
        }
        let color = match &req.color {
            Some(c) => Some(ApiClient::parse_color_hex(c).map_err(|e| e.to_string())?),
            None => None,
        };
        let spec = discord_core::types::EmbedSpec {
            title: req.title.clone(),
            description: req.description.clone(),
            color,
            url: req.url.clone(),
            image_url: req.image.clone(),
            thumbnail_url: None,
            footer: req.footer.clone(),
            author: req.author.clone(),
            fields: req
                .fields
                .iter()
                .map(|f| discord_core::types::EmbedFieldSpec {
                    name: f.name.clone(),
                    value: f.value.clone(),
                    inline: f.inline,
                })
                .collect(),
            content: req.content.clone(),
            reply_to: req.reply_to.clone(),
        };
        discord_core::types::validate_embed(&spec).map_err(|e| e.to_string())?;
        let mut c = self.client()?;
        let msg = c
            .send_embed(&req.channel_id, spec)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&msg).map_err(|e| e.to_string())
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for DiscordMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Discord CLI MCP server. Manage the logged-in Discord user account: \
             list servers/channels/DMs, read and send messages, search, and \
             admin ops (channel/role/emoji/member/permission/server/invite \
             CRUD, audit logs, embeds — destructive ops gated by confirm:true). \
             ToS: automating a user account may violate Discord ToS.",
        )
    }
}

/// Run the server over stdio (called by the `serve` subcommand).
pub async fn serve_stdio() -> anyhow::Result<()> {
    let server = DiscordMcpServer::new()
        .serve(rmcp::transport::stdio())
        .await?;
    server.waiting().await?;
    Ok(())
}
