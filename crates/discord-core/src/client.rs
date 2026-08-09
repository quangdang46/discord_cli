//! Client wrapper over `discord-user-rs`'s `DiscordHttpClient`.
//!
//! Provides browser headers + X-Super-Properties (via `set_super_properties_b64`)
//! plus rate-limit handling. Full stealth header set lands in M8; the core here
//! is the thin typed layer commands call.
//!
//! `discord-user-rs` is the MIT core crate (plan §2.2).

use anyhow::{Context, Result};
use discord_user::client::DiscordHttpClient;
use discord_user::route::Route;

use crate::config::{resolve_token, API_BASE};
use crate::types::{Channel, DmChannel, Guild, GuildInfo, Invite, Me, Member};

/// Authenticated API client backed by `discord-user-rs`.
///
/// Holds the token and lazily constructs the underlying `DiscordHttpClient`.
pub struct ApiClient {
    token: String,
    client: Option<DiscordHttpClient>,
}

impl ApiClient {
    /// Set the live gateway presence (Op 3) for an active connection.
    ///
    /// Requires a `DiscordUser` gateway; use `DiscordUserContext::gateway()`
    /// to obtain it (verified: `DiscordContext` is public, `gateway() ->
    /// Option<&Gateway>`, `Gateway::send_presence` exists — crate context.rs:10,
    /// gateway.rs:953). Returns Ok(()) when no gateway is connected (presence
    /// applies on next connect via `with_status` instead).
    pub async fn set_presence(
        client: &discord_user::DiscordUser,
        status: discord_user::UserStatus,
    ) -> anyhow::Result<()> {
        use discord_user::DiscordContext;
        if let Some(gw) = client.gateway() {
            gw.send_presence(status)
                .await
                .map_err(|e| anyhow::anyhow!("gateway send_presence failed: {e}"))?;
        }
        Ok(())
    }
}

impl ApiClient {
    /// Create a client from a resolved token.
    pub fn with_token(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            client: None,
        }
    }

    /// Create from the standard token resolution chain.
    pub fn from_env(flag: Option<&str>) -> Result<Self> {
        Ok(Self::with_token(resolve_token(flag)?))
    }

    /// Lazily build the underlying HTTP client (Chrome UA, locale, super-props).
    fn inner(&mut self) -> Result<&mut DiscordHttpClient> {
        if self.client.is_none() {
            let mut c = DiscordHttpClient::new(self.token.clone(), None, false);
            c.set_discord_locale(Some("en-US".to_string()));
            // Stealth (M8): attach X-Super-Properties so REST traffic looks
            // like the real Discord client.
            c.set_super_properties_b64(Some(crate::stealth::x_super_properties()));
            self.client = Some(c);
        }
        Ok(self.client.as_mut().unwrap())
    }

    /// `GET /users/@me` — current user.
    pub async fn get_me(&mut self) -> Result<Me> {
        let inner = self.inner()?;
        inner
            .get(Route::GetMe)
            .await
            .context("GET /users/@me failed")
    }

    /// Validate token: `GET /users/@me` returns 200.
    pub async fn validate(&mut self) -> Result<bool> {
        match self.get_me().await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// `GET /users/@me/guilds` — guilds the user belongs to.
    /// Response is a raw array; we deserialize to our `Guild` shape.
    pub async fn list_guilds(&mut self) -> Result<Vec<Guild>> {
        let inner = self.inner()?;
        let raw: Vec<RawGuild> = inner.get(Route::GetCurrentUserGuilds).await.map_err(|e| {
            // Surface the inner error chain (Debug) for troubleshooting.
            anyhow::anyhow!("GET /users/@me/guilds failed: {:?}", e)
        })?;
        Ok(raw
            .into_iter()
            .map(|g| Guild {
                id: g.id.to_string(),
                name: g.name,
                icon: g.icon,
                owner: Some(g.owner),
            })
            .collect())
    }

    /// Resolve a guild name or ID to a guild ID (jackwener `resolve_guild_id`).
    /// Returns Ok(None) if not found.
    pub async fn resolve_guild_id(&mut self, guild: &str) -> Result<Option<String>> {
        if guild.chars().all(|c| c.is_ascii_digit()) {
            return Ok(Some(guild.to_string()));
        }
        let guilds = self.list_guilds().await?;
        let needle = guild.to_lowercase();
        Ok(guilds
            .into_iter()
            .find(|g| g.name.to_lowercase().contains(&needle))
            .map(|g| g.id))
    }

    /// `GET /guilds/{id}/channels` — channels of a guild (text-like filtered).
    pub async fn list_channels(&mut self, guild_id: &str) -> Result<Vec<Channel>> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let inner = self.inner()?;
        let raw: Vec<RawChannel> = inner
            .get(Route::GetGuildChannels { guild_id: gid })
            .await
            .context("GET /guilds/{id}/channels failed")?;
        let mut channels: Vec<Channel> = raw
            .into_iter()
            .map(|c| Channel {
                id: c.id.to_string(),
                name: c.name.unwrap_or_default(),
                guild_id: Some(guild_id.to_string()),
                channel_type: c.channel_type,
                topic: c.topic,
                parent_id: c.parent_id.map(|p| p.to_string()),
                position: Some(c.position),
                rate_limit_per_user: None,
                permission_overwrites: Vec::new(),
            })
            .collect();
        // text/announcement/forum only, sorted by position (jackwener).
        channels.retain(|c| c.is_text_like());
        channels.sort_by_key(|c| c.position.unwrap_or(0));
        Ok(channels)
    }

    /// `GET /guilds/{id}/channels` — ALL channel types (no text-like filter).
    ///
    /// Returns every channel of the guild — text, voice, category, stage,
    /// forum, media — mapped to the workspace `Channel` (keeps
    /// `permission_overwrites` + `rate_limit_per_user` for F1/F5 admin ops).
    pub async fn get_guild_channels_all(&mut self, guild_id: &str) -> Result<Vec<Channel>> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let inner = self.inner()?;
        let raw: Vec<discord_user::types::Channel> = inner
            .get(Route::GetGuildChannels { guild_id: gid })
            .await
            .context("GET /guilds/{id}/channels (all) failed")?;
        let mut channels: Vec<Channel> = raw.into_iter().map(crate_channel_to_workspace).collect();
        channels.sort_by_key(|c| c.position.unwrap_or(0));
        Ok(channels)
    }

    /// `POST /guilds/{id}/channels` — create a guild channel.
    /// `CreateChannelRequest` is public (name 1–100, optional type/topic/
    /// slowmode/parent/bitrate/user_limit/nsfw/position).
    pub async fn create_channel(
        &mut self,
        guild_id: &str,
        req: discord_user::types::CreateChannelRequest,
    ) -> Result<Channel> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let inner = self.inner()?;
        let raw: discord_user::types::Channel = inner
            .post(Route::CreateGuildChannel { guild_id: gid }, req)
            .await
            .context("POST /guilds/{id}/channels failed")?;
        Ok(crate_channel_to_workspace(raw))
    }

    /// `PATCH /channels/{id}` — edit a channel (rename/topic/slowmode/move).
    pub async fn edit_channel(
        &mut self,
        channel_id: &str,
        req: discord_user::types::EditChannelRequest,
    ) -> Result<Channel> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let inner = self.inner()?;
        let raw: discord_user::types::Channel = inner
            .patch(Route::EditChannel { channel_id: cid }, req)
            .await
            .context("PATCH /channels/{id} failed")?;
        Ok(crate_channel_to_workspace(raw))
    }

    /// `DELETE /channels/{id}` — delete a channel (returns 204, no body).
    pub async fn delete_channel(&mut self, channel_id: &str) -> Result<()> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let inner = self.inner()?;
        inner
            .delete(Route::DeleteChannel { channel_id: cid })
            .await
            .context("DELETE /channels/{id} failed")?;
        Ok(())
    }

    /// `GET /channels/{id}` — fetch a single channel.
    ///
    /// Returns the workspace `Channel` (mapped from the crate `Channel` which
    /// carries `permission_overwrites` for F5 perm ops).
    pub async fn get_channel(&mut self, channel_id: &str) -> Result<Channel> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let inner = self.inner()?;
        let raw: discord_user::types::Channel = inner
            .get(Route::GetChannel { channel_id: cid })
            .await
            .context("GET /channels/{id} failed")?;
        Ok(crate_channel_to_workspace(raw))
    }

    /// Channel name valid (1–100 chars, no `#`).
    pub fn validate_channel_name(name: &str) -> bool {
        !name.contains('#') && (1..=100).contains(&name.chars().count())
    }

    /// Channel topic valid (≤1024 chars).
    pub fn validate_topic(topic: &str) -> bool {
        topic.chars().count() <= 1024
    }

    /// `GET /users/@me/channels` — DM + group-DM channels.
    /// `Route::CreateDm` maps to that path (POST creates; GET lists).
    pub async fn list_dms(&mut self) -> Result<Vec<DmChannel>> {
        let inner = self.inner()?;
        let raw: Vec<RawDm> = inner
            .get(Route::CreateDm)
            .await
            .context("GET /users/@me/channels failed")?;
        let mut dms: Vec<DmChannel> = raw
            .into_iter()
            .map(|d| {
                let recipient_count = d.recipients.as_ref().map(|r| r.len());
                let recipients: Vec<String> = d
                    .recipients
                    .unwrap_or_default()
                    .into_iter()
                    .map(|u| u.tag())
                    .collect();
                let label = match recipients.len() {
                    0 => d.name.clone().unwrap_or_else(|| d.id.to_string()),
                    1 => recipients[0].clone(),
                    _ => recipients.join(", "),
                };
                DmChannel {
                    id: d.id.to_string(),
                    label,
                    channel_type: d.channel_type,
                    recipient_count,
                }
            })
            .collect();
        dms.sort_by(|a, b| a.label.cmp(&b.label));
        Ok(dms)
    }

    /// `GET /guilds/{id}/members` — list members (jackwener list_members).
    pub async fn list_members(&mut self, guild_id: &str, limit: u32) -> Result<Vec<Member>> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let inner = self.inner()?;
        let raw: Vec<RawMember> = inner
            .get(Route::GetGuildMembers {
                guild_id: gid,
                limit: limit.min(1000),
            })
            .await
            .context("GET /guilds/{id}/members failed")?;
        Ok(raw
            .into_iter()
            .map(|m| Member {
                id: m.user.id.to_string(),
                username: m.user.username,
                global_name: m.user.global_name,
                nick: m.nick,
                joined_at: m.joined_at,
                bot: m.user.bot.unwrap_or(false),
            })
            .collect())
    }

    /// `GET /guilds/{id}?with_counts=true` — guild info (jackwener get_guild_info).
    pub async fn guild_info(&mut self, guild_id: &str) -> Result<GuildInfo> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let inner = self.inner()?;
        let raw: RawGuildInfo = inner
            .get(Route::GetGuild {
                guild_id: gid,
                with_counts: true,
            })
            .await
            .context("GET /guilds/{id} failed")?;
        Ok(GuildInfo {
            id: raw.id.to_string(),
            name: raw.name,
            description: raw.description,
            member_count: raw.approximate_member_count,
            online_count: raw.approximate_presence_count,
        })
    }

    /// `GET /guilds/{id}/messages/search?content=...` — Discord native search
    /// (jackwener search_guild_messages). Returns matching messages.
    pub async fn search_guild_messages(
        &mut self,
        guild_id: &str,
        query: &str,
        channel_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<crate::types::Message>> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let cid = channel_id.and_then(|c| c.parse().ok());
        let inner = self.inner()?;
        let raw: SearchResponse = inner
            .get(Route::SearchGuildMessages {
                guild_id: gid,
                content: query,
                channel_id: cid,
                limit: Some(limit),
            })
            .await
            .context("search failed")?;
        let mut out = Vec::new();
        for group in raw.messages {
            for msg in group {
                let urls = msg.url_list();
                let details = msg.details();
                let reactions = msg.reaction_total();
                out.push(crate::types::Message {
                    message_id: msg.id.to_string(),
                    channel_id: msg.channel_id.to_string(),
                    guild_id: Some(guild_id.to_string()),
                    author_id: Some(msg.author.id.to_string()),
                    author: msg.author.username,
                    timestamp: msg.timestamp,
                    content: msg.content,
                    attachments: urls,
                    attachment_details: details,
                    reactions,
                });
            }
            if out.len() >= limit as usize {
                break;
            }
        }
        Ok(out)
    }

    /// `GET /guilds/{id}/roles` — guild roles sorted by position.
    pub async fn list_roles(&mut self, guild_id: &str) -> Result<Vec<crate::types::Role>> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let inner = self.inner()?;
        let raw: Vec<RawRole> = inner
            .get(Route::GetGuildRoles { guild_id: gid })
            .await
            .context("GET /guilds/{id}/roles failed")?;
        let mut roles: Vec<crate::types::Role> = raw
            .into_iter()
            .map(|r| crate::types::Role {
                id: r.id.to_string(),
                name: r.name,
                color: r.color,
                position: r.position,
                permissions: r.permissions,
                hoist: r.hoist,
                mentionable: r.mentionable,
            })
            .collect();
        roles.sort_by_key(|r| std::cmp::Reverse(r.position));
        Ok(roles)
    }

    /// `POST /guilds/{id}/roles` — create a role.
    pub async fn create_role(
        &mut self,
        guild_id: &str,
        req: discord_user::types::CreateRoleRequest,
    ) -> Result<crate::types::Role> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let inner = self.inner()?;
        let raw: discord_user::types::Role = inner
            .post(Route::CreateGuildRole { guild_id: gid }, req)
            .await
            .context("POST /guilds/{id}/roles failed")?;
        Ok(crate_role_to_workspace(raw))
    }

    /// `PATCH /guilds/{id}/roles/{role_id}` — edit a role.
    /// `EditRoleRequest` is a type alias for `CreateRoleRequest` (same shape).
    pub async fn edit_role(
        &mut self,
        guild_id: &str,
        role_id: &str,
        req: discord_user::types::CreateRoleRequest,
    ) -> Result<crate::types::Role> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let rid: u64 = role_id.parse().context("invalid role id")?;
        let inner = self.inner()?;
        let raw: discord_user::types::Role = inner
            .patch(
                Route::EditGuildRole {
                    guild_id: gid,
                    role_id: rid,
                },
                req,
            )
            .await
            .context("PATCH /guilds/{id}/roles/{role_id} failed")?;
        Ok(crate_role_to_workspace(raw))
    }

    /// `DELETE /guilds/{id}/roles/{role_id}` — delete a role (204, no body).
    pub async fn delete_role(&mut self, guild_id: &str, role_id: &str) -> Result<()> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let rid: u64 = role_id.parse().context("invalid role id")?;
        let inner = self.inner()?;
        inner
            .delete(Route::DeleteGuildRole {
                guild_id: gid,
                role_id: rid,
            })
            .await
            .context("DELETE /guilds/{id}/roles/{role_id} failed")?;
        Ok(())
    }

    /// `PUT /guilds/{id}/members/{user_id}/roles/{role_id}` — assign a role
    /// (returns 204, no body → `put_empty`).
    pub async fn add_member_role(
        &mut self,
        guild_id: &str,
        user_id: &str,
        role_id: &str,
    ) -> Result<()> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let uid: u64 = user_id.parse().context("invalid user id")?;
        let rid: u64 = role_id.parse().context("invalid role id")?;
        let inner = self.inner()?;
        inner
            .put_empty(Route::AddGuildMemberRole {
                guild_id: gid,
                user_id: uid,
                role_id: rid,
            })
            .await
            .context("PUT member role failed")?;
        Ok(())
    }

    /// `DELETE /guilds/{id}/members/{user_id}/roles/{role_id}` — remove a role
    /// (204, no body).
    pub async fn remove_member_role(
        &mut self,
        guild_id: &str,
        user_id: &str,
        role_id: &str,
    ) -> Result<()> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let uid: u64 = user_id.parse().context("invalid user id")?;
        let rid: u64 = role_id.parse().context("invalid role id")?;
        let inner = self.inner()?;
        inner
            .delete(Route::RemoveGuildMemberRole {
                guild_id: gid,
                user_id: uid,
                role_id: rid,
            })
            .await
            .context("DELETE member role failed")?;
        Ok(())
    }

    /// Map a comma/space-separated list of permission names to a bitfield.
    /// Names are case-insensitive; an unknown name returns `Err` listing the
    /// valid names. Covers the crate `Permissions` bits (enums.rs:316).
    pub fn parse_permission_names(names: &[String]) -> anyhow::Result<u64> {
        use discord_user::types::Permissions;
        let mut bits: u64 = 0;
        let mut unknown: Vec<String> = Vec::new();
        for raw in names {
            let clean = raw.trim();
            if clean.is_empty() {
                continue;
            }
            let found = ALL_PERMISSION_NAMES
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(clean))
                .map(|(_, bit)| *bit);
            match found {
                Some(bit) => bits |= bit,
                None => unknown.push(clean.to_string()),
            }
        }
        if !unknown.is_empty() {
            let valid: Vec<&str> = ALL_PERMISSION_NAMES.iter().map(|(n, _)| *n).collect();
            return Err(anyhow::anyhow!(
                "unknown permission(s): {} (valid: {})",
                unknown.join(", "),
                valid.join(", ")
            ));
        }
        let _ = Permissions::empty(); // type-assert the bitflags exist
        Ok(bits)
    }

    /// Parse a hex color `#RRGGBB` or `RRGGBB` into a u32 RGB value.
    pub fn parse_color_hex(s: &str) -> anyhow::Result<u32> {
        let clean = s.trim().strip_prefix('#').unwrap_or(s.trim());
        if clean.len() != 6 || !clean.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(anyhow::anyhow!(
                "invalid color \"{s}\" (expected #RRGGBB or RRGGBB)"
            ));
        }
        u32::from_str_radix(clean, 16).map_err(|_| anyhow::anyhow!("invalid color \"{s}\""))
    }

    /// `GET /guilds/{id}/emojis` — list custom emojis.
    pub async fn list_emojis(
        &mut self,
        guild_id: &str,
    ) -> Result<Vec<discord_user::types::GuildEmoji>> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let inner = self.inner()?;
        inner
            .get(Route::GetGuildEmojis { guild_id: gid })
            .await
            .context("GET /guilds/{id}/emojis failed")
    }

    /// `POST /guilds/{id}/emojis` — upload a custom emoji.
    ///
    /// Reads `image_path`, validates size (≤256KiB), builds the
    /// `data:image/...;base64,...` data URI and POSTs it with an empty role
    /// allow-list (all roles may use it).
    pub async fn create_emoji(
        &mut self,
        guild_id: &str,
        name: &str,
        image_path: &str,
    ) -> Result<discord_user::types::GuildEmoji> {
        if !Self::validate_emoji_name(name) {
            return Err(anyhow::anyhow!(
                "invalid emoji name \"{name}\" (alphanumeric + underscore, 2-32 chars)"
            ));
        }
        let image = Self::build_image_data_uri(image_path).await?;
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let inner = self.inner()?;
        let req = discord_user::types::CreateEmojiRequest {
            name: name.to_string(),
            image,
            roles: Vec::new(),
        };
        inner
            .post(Route::CreateGuildEmoji { guild_id: gid }, req)
            .await
            .context("POST /guilds/{id}/emojis failed")
    }

    /// `DELETE /guilds/{id}/emojis/{emoji_id}` — delete a custom emoji (204).
    pub async fn delete_emoji(&mut self, guild_id: &str, emoji_id: &str) -> Result<()> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let eid: u64 = emoji_id.parse().context("invalid emoji id")?;
        let inner = self.inner()?;
        inner
            .delete(Route::DeleteGuildEmoji {
                guild_id: gid,
                emoji_id: eid,
            })
            .await
            .context("DELETE /guilds/{id}/emojis/{emoji_id} failed")?;
        Ok(())
    }

    /// Emoji image size cap (Discord: 256 KiB).
    pub const MAX_EMOJI_SIZE: u64 = 256 * 1024;

    /// Build a `data:{mime};base64,{b64}` URI from a local image file.
    ///
    /// Mime from extension (`.gif` → `image/gif`, `.png` → `image/png`,
    /// `.jpg`/`.jpeg` → `image/jpeg`). Rejects missing files, oversized
    /// (>256KiB), and unknown extensions.
    pub async fn build_image_data_uri(path: &str) -> anyhow::Result<String> {
        let meta = tokio::fs::metadata(path)
            .await
            .with_context(|| format!("cannot read file \"{path}\""))?;
        if meta.len() > Self::MAX_EMOJI_SIZE {
            return Err(anyhow::anyhow!(
                "file too large ({} bytes max for an emoji): {path}",
                Self::MAX_EMOJI_SIZE
            ));
        }
        let mime = match std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
        {
            Some(e) if e == "gif" => "image/gif",
            Some(e) if e == "png" => "image/png",
            Some(e) if e == "jpg" || e == "jpeg" => "image/jpeg",
            _ => {
                return Err(anyhow::anyhow!(
                    "unsupported emoji file type \"{path}\" (use .png, .jpg, .jpeg, or .gif)"
                ))
            }
        };
        let data = tokio::fs::read(path)
            .await
            .with_context(|| format!("cannot read file \"{path}\""))?;
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
        Ok(format!("data:{mime};base64,{b64}"))
    }

    /// Emoji name valid: alphanumeric + underscore only, length 2–32.
    pub fn validate_emoji_name(name: &str) -> bool {
        let len = name.chars().count();
        (2..=32).contains(&len) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    /// `DELETE /guilds/{id}/members/{user_id}` — kick a member (F4).
    ///
    /// Returns 204. The reason is sent via the `X-Audit-Log-Reason` header
    /// (crate `request_with_reason_no_response`), not the body — DELETE has
    /// no payload.
    pub async fn kick_member(
        &mut self,
        guild_id: &str,
        user_id: &str,
        reason: Option<&str>,
    ) -> Result<()> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let uid: u64 = user_id.parse().context("invalid user id")?;
        let inner = self.inner()?;
        match reason {
            Some(reason) => inner
                .request_with_reason_no_response::<serde_json::Value>(
                    reqwest::Method::DELETE,
                    Route::KickMember {
                        guild_id: gid,
                        user_id: uid,
                    },
                    None,
                    Some(reason),
                )
                .await
                .context("DELETE /guilds/{id}/members/{user_id} failed"),
            None => inner
                .delete(Route::KickMember {
                    guild_id: gid,
                    user_id: uid,
                })
                .await
                .context("DELETE /guilds/{id}/members/{user_id} failed"),
        }
    }

    /// `PUT /guilds/{id}/bans/{user_id}` — ban a member (F4).
    ///
    /// Returns 204. Body carries `delete_message_seconds` (SECONDS, capped at
    /// 604800 = 7 days; `delete_days` days × 86400) and an optional reason.
    pub async fn ban_member(
        &mut self,
        guild_id: &str,
        user_id: &str,
        reason: Option<&str>,
        delete_days: Option<u8>,
    ) -> Result<()> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let uid: u64 = user_id.parse().context("invalid user id")?;
        let inner = self.inner()?;
        let mut body = serde_json::json!({
            "delete_message_seconds": delete_days.map(|d| (d as u32) * 86400).unwrap_or(0).min(604800),
        });
        if let Some(reason) = reason {
            body["reason"] = serde_json::Value::String(reason.to_string());
        }
        inner
            .request_with_reason_no_response(
                reqwest::Method::PUT,
                Route::CreateGuildBan {
                    guild_id: gid,
                    user_id: uid,
                },
                Some(body),
                None,
            )
            .await
            .context("PUT /guilds/{id}/bans/{user_id} failed")
    }

    /// `DELETE /guilds/{id}/bans/{user_id}` — unban a user (F4).
    ///
    /// Banned users are not guild members, so the target is a plain user ID.
    /// Returns 204.
    pub async fn unban_member(&mut self, guild_id: &str, user_id: &str) -> Result<()> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let uid: u64 = user_id.parse().context("invalid user id")?;
        let inner = self.inner()?;
        inner
            .delete(Route::RemoveGuildBan {
                guild_id: gid,
                user_id: uid,
            })
            .await
            .context("DELETE /guilds/{id}/bans/{user_id} failed")?;
        Ok(())
    }

    /// `GET /guilds/{id}/audit-logs` — fetch the guild audit log (F7).
    ///
    /// `action_type` is the numeric code (resolve via `audit_action_code`).
    /// `limit` is capped at 100 (Discord's hard limit). Returns the crate
    /// `AuditLog` (entries + users) — callers map to `AuditEntryView` for
    /// output via `AuditEntryView::from_entry` when a username map is wanted.
    pub async fn audit_logs(
        &mut self,
        guild_id: &str,
        user_id: Option<u64>,
        action_type: Option<u32>,
        limit: Option<u8>,
    ) -> Result<discord_user::types::AuditLog> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let inner = self.inner()?;
        inner
            .get(Route::GetGuildAuditLogs {
                guild_id: gid,
                user_id,
                action_type,
                before: None,
                after: None,
                limit: limit.map(|l| l.min(100)),
            })
            .await
            .context("GET /guilds/{id}/audit-logs failed")
    }

    /// `PATCH /guilds/{id}/members/{user_id}` — set/clear a member's
    /// nickname (F4). `Some("")` clears the nickname.
    pub async fn set_nickname(
        &mut self,
        guild_id: &str,
        user_id: &str,
        nick: Option<&str>,
    ) -> Result<()> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let uid: u64 = user_id.parse().context("invalid user id")?;
        let inner = self.inner()?;
        let req = discord_user::types::EditGuildMemberRequest {
            nick: nick.map(str::to_string),
            ..Default::default()
        };
        inner
            .patch::<serde_json::Value, _>(
                Route::EditGuildMember {
                    guild_id: gid,
                    member_id: uid,
                },
                req,
            )
            .await
            .context("PATCH /guilds/{id}/members/{user_id} failed")?;
        Ok(())
    }

    /// `GET /channels/{id}` — the channel's permission overwrites (F5).
    ///
    /// Reuses `get_channel` (the workspace `Channel` carries
    /// `permission_overwrites` once F1a lands the crate `Channel` mapping).
    pub async fn get_channel_overwrites(
        &mut self,
        channel_id: &str,
    ) -> Result<Vec<crate::types::PermissionOverwrite>> {
        Ok(self.get_channel(channel_id).await?.permission_overwrites)
    }

    /// `PUT /channels/{id}/permissions/{overwrite_id}` — set a channel
    /// permission overwrite (F5). Returns 204.
    ///
    /// `allow`/`deny` are bitfields (as decimal strings). BOTH sides are
    /// always sent — the crate's `skip_serializing_if` would silently clear
    /// an absent side, so callers pass 0 explicitly for the unused side.
    /// `kind` is 0 = role, 1 = member.
    pub async fn edit_channel_permission(
        &mut self,
        channel_id: &str,
        overwrite_id: &str,
        allow: u64,
        deny: u64,
        kind: u8,
    ) -> Result<()> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let oid: u64 = overwrite_id.parse().context("invalid overwrite id")?;
        let inner = self.inner()?;
        let req = discord_user::types::EditChannelPermissionsRequest {
            allow: Some(allow.to_string()),
            deny: Some(deny.to_string()),
            overwrite_type: kind,
        };
        // PUT .../permissions/{overwrite_id} returns 204 No Content; the
        // crate `put` would fail parsing the empty body. Use the reason-aware
        // no-response variant (reason None = plain request, body sent).
        inner
            .request_with_reason_no_response(
                reqwest::Method::PUT,
                Route::EditChannelPermissions {
                    channel_id: cid,
                    overwrite_id: oid,
                },
                Some(req),
                None,
            )
            .await
            .context("PUT /channels/{id}/permissions/{overwrite_id} failed")
    }

    /// `DELETE /channels/{id}/permissions/{overwrite_id}` — remove a channel
    /// permission overwrite (F5). Returns 204.
    pub async fn delete_channel_permission(
        &mut self,
        channel_id: &str,
        overwrite_id: &str,
    ) -> Result<()> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let oid: u64 = overwrite_id.parse().context("invalid overwrite id")?;
        let inner = self.inner()?;
        inner
            .delete(Route::DeleteChannelPermission {
                channel_id: cid,
                overwrite_id: oid,
            })
            .await
            .context("DELETE /channels/{id}/permissions/{overwrite_id} failed")?;
        Ok(())
    }

    /// Bitfield denied by `lock_channel`: SEND_MESSAGES |
    /// SEND_MESSAGES_IN_THREADS | CREATE_PUBLIC_THREADS (F5).
    pub fn lock_deny_bitfield() -> u64 {
        use discord_user::types::Permissions;
        (Permissions::SEND_MESSAGES
            | Permissions::SEND_MESSAGES_IN_THREADS
            | Permissions::CREATE_PUBLIC_THREADS)
            .bits()
    }

    /// Lock a channel read-only for `@everyone` (F5).
    ///
    /// The `@everyone` role shares the guild's ID, so the overwrite target IS
    /// the guild ID (discli permission.ts:139 trick). Returns 204.
    pub async fn lock_channel(&mut self, channel_id: &str, guild_id: &str) -> Result<()> {
        self.edit_channel_permission(channel_id, guild_id, 0, Self::lock_deny_bitfield(), 0)
            .await
    }

    /// Unlock a channel locked via `lock_channel` (F5): delete the
    /// `@everyone` overwrite. Returns 204.
    pub async fn unlock_channel(&mut self, channel_id: &str, guild_id: &str) -> Result<()> {
        self.delete_channel_permission(channel_id, guild_id).await
    }

    /// `PATCH /guilds/{id}` — edit guild settings (F6).
    ///
    /// `EditGuildRequest` (requests.rs:325) covers name/description/
    /// verification_level/notifications/content_filter/afk_timeout/system/
    /// rules channels + icon data-URI. Returns the updated guild.
    pub async fn edit_guild(
        &mut self,
        guild_id: &str,
        req: discord_user::types::EditGuildRequest,
    ) -> Result<crate::types::GuildInfo> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let inner = self.inner()?;
        let raw: discord_user::types::Guild = inner
            .patch(Route::EditGuild { guild_id: gid }, req)
            .await
            .context("PATCH /guilds/{id} failed")?;
        Ok(crate::types::GuildInfo {
            id: raw.id,
            name: raw.name.unwrap_or_default(),
            description: raw.description,
            member_count: None,
            online_count: None,
        })
    }

    /// `PATCH /guilds/{id}` with `icon` only — set the guild icon (F6).
    ///
    /// Reuses the shared `build_image_data_uri` helper (F3a) for the
    /// `data:image/{png|gif|jpeg};base64,...` payload.
    pub async fn set_guild_icon(
        &mut self,
        guild_id: &str,
        image_path: &str,
    ) -> Result<crate::types::GuildInfo> {
        let icon = Self::build_image_data_uri(image_path).await?;
        let req = discord_user::types::EditGuildRequest {
            icon: Some(icon),
            ..Default::default()
        };
        self.edit_guild(guild_id, req).await
    }

    /// `GET /users/@me/relationships` — friends/blocked/pending.
    pub async fn relationships(&mut self) -> Result<Vec<crate::types::Relationship>> {
        let inner = self.inner()?;
        let raw: Vec<RawRelationship> = inner
            .get(Route::GetRelationships)
            .await
            .context("GET relationships failed")?;
        Ok(raw
            .into_iter()
            .map(|r| crate::types::Relationship {
                user_id: r.id.to_string(),
                username: r.username,
                relationship_type: r.relationship_type,
            })
            .collect())
    }

    /// `GET /users/{id}/profile` — user profile.
    pub async fn user_profile(&mut self, user_id: &str) -> Result<crate::types::UserProfile> {
        let uid: u64 = user_id.parse().context("invalid user id")?;
        let inner = self.inner()?;
        let raw: RawUserProfile = inner
            .get(Route::GetUserProfile {
                user_id: uid,
                guild_id: None,
            })
            .await
            .context("GET /users/{id}/profile failed")?;
        Ok(crate::types::UserProfile {
            user_id: raw.user.id.to_string(),
            username: raw.user.username,
            global_name: raw.user.global_name,
            bio: raw.user_bio,
        })
    }

    /// List active threads in a channel.
    ///
    /// **User-token pitfall (langkurt):** `GET /channels/{id}/threads` (active)
    /// is BOT-ONLY → 403 for user tokens. Fallback to what Discord's own app
    /// uses: `GET /channels/{id}/threads/search` (offset-paginated).
    pub async fn list_threads(&mut self, channel_id: &str) -> Result<Vec<Channel>> {
        // Try the bot-only active endpoint first; on 403 fall back.
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let inner = self.inner()?;
        let threads_url = format!("channels/{}/threads/active", channel_id);
        let bot_only: std::result::Result<ThreadActiveResponse, _> =
            inner.get(Route::Custom(threads_url.into())).await;
        match bot_only {
            Ok(resp) => Ok(resp
                .threads
                .into_iter()
                .map(raw_thread_to_channel)
                .collect()),
            Err(_) => {
                // 403 → user-token fallback: threads/search, offset-paginated.
                let mut out = Vec::new();
                let mut offset: u64 = 0;
                loop {
                    let url = format!(
                        "channels/{}/threads/search?limit=25&sort_by=last_message_time&sort_order=desc&archived=false&offset={}",
                        channel_id, offset
                    );
                    let resp: ThreadSearchResponse = inner
                        .get(Route::Custom(url.into()))
                        .await
                        .context("threads/search failed")?;
                    let n = resp.threads.len();
                    out.extend(resp.threads.into_iter().map(raw_thread_to_channel));
                    offset += n as u64;
                    if !resp.has_more || n == 0 {
                        break;
                    }
                    // rate-limit friendly pause (langkurt sleeps 300ms)
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                }
                let _ = cid;
                Ok(out)
            }
        }
    }

    /// `POST /channels/{id}/messages` — send a message (M3.1).
    /// Returns the new message id.
    pub async fn send_message(
        &mut self,
        channel_id: &str,
        content: &str,
        reply_to: Option<&str>,
    ) -> Result<String> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let inner = self.inner()?;
        let req = discord_user::types::SendMessageRequest {
            content,
            tts: false,
            flags: 0,
            message_reference: reply_to.map(|id| discord_user::types::MessageReference {
                reference_type: None,
                message_id: Some(id.to_string()),
                channel_id: None,
                guild_id: None,
            }),
            nonce: None,
            mobile_network_type: Some("unknown"), // mimic Discord mobile (selfbot)
        };
        let resp: RawMessage = inner
            .post(Route::CreateMessage { channel_id: cid }, req)
            .await
            .context("POST /channels/{id}/messages failed")?;
        Ok(resp.id.to_string())
    }

    /// Build the v10 multipart `payload_json` for a message with attachments.
    /// Exposed for unit testing (no network). The `attachments` descriptor
    /// array uses `id` = index-as-string (discord.js MessagePayload style).
    fn build_send_payload(
        content: &str,
        reply_to: Option<&str>,
        n_files: usize,
    ) -> anyhow::Result<serde_json::Value> {
        let req = discord_user::types::SendMessageRequest {
            content,
            tts: false,
            flags: 0,
            message_reference: reply_to.map(|id| discord_user::types::MessageReference {
                reference_type: None,
                message_id: Some(id.to_string()),
                channel_id: None,
                guild_id: None,
            }),
            nonce: None,
            mobile_network_type: Some("unknown"), // mimic Discord mobile (selfbot)
        };
        let mut payload = serde_json::to_value(&req).context("serialize send payload")?;
        let atts: Vec<serde_json::Value> = (0..n_files)
            .map(|i| serde_json::json!({ "id": i.to_string() }))
            .collect();
        payload["attachments"] = serde_json::Value::Array(atts);
        Ok(payload)
    }

    /// `POST /channels/{id}/messages` — send a message with file attachments
    /// (multipart). payload_json carries the message body; each file is a
    /// `files[N]` part with an `attachments:[{id:"0"}]` descriptor array
    /// (Discord v10 style, cf. discord.js MessagePayload).
    pub async fn send_message_with_files(
        &mut self,
        channel_id: &str,
        content: &str,
        reply_to: Option<&str>,
        attachments: Vec<discord_user::types::CreateAttachment>,
    ) -> Result<String> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let inner = self.inner()?;
        let payload = Self::build_send_payload(content, reply_to, attachments.len())?;
        let resp: RawMessage = inner
            .post_multipart(
                Route::CreateMessage { channel_id: cid },
                payload,
                attachments,
            )
            .await
            .context("POST /channels/{id}/messages (multipart) failed")?;
        Ok(resp.id.to_string())
    }

    /// `POST /channels/{id}/messages` — send a message with an embed (F9).
    ///
    /// Builds the message via the crate `MessageBuilder` + `EmbedBuilder`.
    /// `spec` is validated by `validate_embed` at the CLI/MCP boundary; this
    /// method maps it to the crate builders (content, reply, embed fields).
    pub async fn send_embed(
        &mut self,
        channel_id: &str,
        spec: crate::types::EmbedSpec,
    ) -> Result<discord_user::types::Message> {
        // Validate the channel id (the MessageBuilder validates payload limits
        // itself; we only confirm the ID parses before handing it over).
        let _cid: u64 = channel_id.parse().context("invalid channel id")?;
        crate::types::validate_embed(&spec).map_err(|e| anyhow::anyhow!(e))?;
        let inner = self.inner()?;
        let mut msg = discord_user::MessageBuilder::new(inner).channel(channel_id.to_string());
        // Discord REJECTS embed-only messages from user tokens with 50006
        // "Cannot send an empty message" — the message must carry non-empty
        // `content` alongside `embeds` (verified empirically + via curl).
        // When no --content was given but an embed exists, synthesize a
        // minimal content string so the send succeeds (title, else desc).
        let has_embed = spec.title.is_some()
            || spec.description.is_some()
            || spec.color.is_some()
            || spec.url.is_some()
            || spec.image_url.is_some()
            || spec.thumbnail_url.is_some()
            || spec.footer.is_some()
            || spec.author.is_some()
            || !spec.fields.is_empty();
        let content_override = if spec.content.is_none() && has_embed {
            spec.title
                .clone()
                .or_else(|| {
                    spec.description
                        .clone()
                        .map(|d| d.chars().take(200).collect())
                })
                .unwrap_or_default()
        } else {
            spec.content.clone().unwrap_or_default()
        };
        if !content_override.is_empty() {
            msg = msg.content(content_override);
        }
        if let Some(r) = &spec.reply_to {
            msg = msg.reply_to(r.clone());
        }
        if has_embed {
            let title = spec.title.clone();
            let description = spec.description.clone();
            let color = spec.color;
            let url = spec.url.clone();
            let image_url = spec.image_url.clone();
            let thumbnail_url = spec.thumbnail_url.clone();
            let footer = spec.footer.clone();
            let author = spec.author.clone();
            let fields = spec.fields.clone();
            msg = msg.embed(move |e| {
                let mut e = e;
                if let Some(t) = &title {
                    e = e.title(t.clone());
                }
                if let Some(d) = &description {
                    e = e.description(d.clone());
                }
                if let Some(c) = color {
                    e = e.color(c);
                }
                if let Some(u) = &url {
                    e = e.url(u.clone());
                }
                if let Some(u) = &image_url {
                    e = e.image(u.clone());
                }
                if let Some(u) = &thumbnail_url {
                    e = e.thumbnail(u.clone());
                }
                if let Some(f) = &footer {
                    e = e.footer(f.clone());
                }
                if let Some(a) = &author {
                    e = e.author(a.clone());
                }
                for f in &fields {
                    e = e.field(f.name.clone(), f.value.clone(), f.inline);
                }
                e
            });
        }
        msg.send()
            .await
            .context("POST /channels/{id}/messages (embed) failed")
    }

    /// `GET /invites/{code}` — preview an invite (guild name, member counts).
    /// Reference: RickvanLoo menu.go invite preview; crate Route::Invite.
    pub async fn get_invite(&mut self, code: &str) -> Result<discord_user::types::Invite> {
        let inner = self.inner()?;
        inner
            .get(Route::Invite {
                code: std::borrow::Cow::Borrowed(code),
                with_counts: Some(true),
                with_expiration: None,
                guild_scheduled_event_id: None,
            })
            .await
            .context("GET /invites/{code} failed")
    }

    /// `POST /invites/{code}` — accept an invite (join a server). Nil body.
    /// Reference: RickvanLoo InviteAccept; crate Route::JoinGuild.
    pub async fn accept_invite(&mut self, code: &str) -> Result<()> {
        let inner = self.inner()?;
        inner
            .post_no_response(Route::JoinGuild { code }, ())
            .await
            .context("POST /invites/{code} failed")?;
        Ok(())
    }

    /// `DELETE /users/@me/guilds/{guild_id}` — leave a server.
    /// Reference: RickvanLoo GuildLeave; crate Route::LeaveGuild.
    pub async fn leave_guild(&mut self, guild_id: &str) -> Result<()> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let inner = self.inner()?;
        inner
            .delete(Route::LeaveGuild { guild_id: gid })
            .await
            .context("DELETE /users/@me/guilds/{id} failed")?;
        Ok(())
    }

    /// `GET /guilds/{id}/invites` — list a guild's invites (F8).
    ///
    /// Requires MANAGE_CHANNELS (403 → classify → exit 4). Mapped to the
    /// workspace `Invite` display type (code + discord.gg URL + usage stats).
    pub async fn list_guild_invites(
        &mut self,
        guild_id: &str,
    ) -> Result<Vec<crate::types::Invite>> {
        let gid: u64 = guild_id.parse().context("invalid guild id")?;
        let inner = self.inner()?;
        let raw: Vec<discord_user::types::Invite> = inner
            .get(Route::GetGuildInvites { guild_id: gid })
            .await
            .context("GET /guilds/{id}/invites failed")?;
        Ok(raw.into_iter().map(invite_to_workspace).collect())
    }

    /// `GET /channels/{id}/invites` — list a channel's invites (F8, bonus).
    pub async fn list_channel_invites(
        &mut self,
        channel_id: &str,
    ) -> Result<Vec<crate::types::Invite>> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let inner = self.inner()?;
        let raw: Vec<discord_user::types::Invite> = inner
            .get(Route::GetChannelInvites { channel_id: cid })
            .await
            .context("GET /channels/{id}/invites failed")?;
        Ok(raw.into_iter().map(invite_to_workspace).collect())
    }

    /// `POST /channels/{id}/invites` — create a channel invite (F8).
    ///
    /// The CLI/MCP set `unique: Some(true)` by default (one-time links);
    /// `max_age` 0 = never expires, `max_uses` 0 = unlimited.
    pub async fn create_channel_invite(
        &mut self,
        channel_id: &str,
        req: discord_user::types::CreateInviteRequest,
    ) -> Result<crate::types::Invite> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let inner = self.inner()?;
        let raw: discord_user::types::Invite = inner
            .post(Route::CreateChannelInvite { channel_id: cid }, req)
            .await
            .context("POST /channels/{id}/invites failed")?;
        Ok(invite_to_workspace(raw))
    }

    /// `DELETE /invites/{code}` — delete an invite (F8).
    ///
    /// The code may be a bare code or a full URL — `extract_invite_code`
    /// strips `discord.gg/`, `discord.com/invite/`, etc. Returns 204.
    pub async fn delete_invite(&mut self, code: &str) -> Result<()> {
        let code = match Self::extract_invite_code(code) {
            Some(c) => c.to_string(),
            None => return Err(anyhow::anyhow!("invalid invite code \"{code}\"")),
        };
        let inner = self.inner()?;
        inner
            .delete(Route::DeleteInvite { code: &code })
            .await
            .context("DELETE /invites/{code} failed")?;
        Ok(())
    }

    /// Extract a bare invite code from a full URL or raw code.
    /// Strips known invite URL prefixes, trailing slashes, and `?`/`#`
    /// suffixes (review#21). Returns the remaining alnum token.
    pub fn extract_invite_code(s: &str) -> Option<&str> {
        let s = s.trim();
        for prefix in [
            "https://discord.com/invite/",
            "http://discord.com/invite/",
            "https://discordapp.com/invite/",
            "http://discordapp.com/invite/",
            "https://discord.gg/",
            "http://discord.gg/",
            "discord.gg/",
        ] {
            if let Some(rest) = s.strip_prefix(prefix) {
                let cut = rest.split(['?', '#']).next().unwrap_or(rest);
                let cut = cut.trim_end_matches('/');
                return if cut.is_empty() { None } else { Some(cut) };
            }
        }
        let cut = s.split(['?', '#']).next().unwrap_or(s);
        let cut = cut.trim_end_matches('/');
        if cut.is_empty() {
            None
        } else {
            Some(cut)
        }
    }

    /// `POST /channels/{id}/threads` — create a thread.
    ///
    /// - Forum (type 15) requires a starter `message` (defaults to the thread
    ///   name, Escape-Tech thread-create.js:11-15).
    /// - Standalone text threads use `channel_type: 11` (public).
    /// - Forum/media channels also accept `applied_tags` (crate field).
    pub async fn create_thread(
        &mut self,
        channel_id: &str,
        name: &str,
        archive_minutes: Option<u32>,
        starter: Option<&str>,
        applied_tags: Option<Vec<String>>,
    ) -> Result<ThreadResult> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let inner = self.inner()?;
        let mut req = discord_user::types::CreateThreadRequest::public(name);
        req.auto_archive_duration = archive_minutes;
        req.applied_tags = applied_tags;
        // Forum (15) / media (16) channels require a message payload.
        if let Some(starter) = starter {
            req.message = Some(serde_json::json!({
                "content": starter,
                "tts": false,
                "allowed_mentions": null,
                "attachments": [],
            }));
        }
        let resp: RawChannel = inner
            .post(Route::CreateThread { channel_id: cid }, req)
            .await
            .context("POST /channels/{id}/threads failed")?;
        Ok(ThreadResult {
            id: resp.id.to_string(),
            name: resp.name.clone().unwrap_or_default(),
            channel_id: channel_id.to_string(),
            channel_type: resp.channel_type,
            parent_message_id: None,
        })
    }

    /// `POST /channels/{id}/messages/{mid}/threads` — create a thread from a
    /// message (parent must be text/announcement; Escape-Tech path 2).
    pub async fn create_thread_from_message(
        &mut self,
        channel_id: &str,
        message_id: &str,
        name: &str,
        archive_minutes: Option<u32>,
    ) -> Result<ThreadResult> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let mid: u64 = message_id.parse().context("invalid message id")?;
        let inner = self.inner()?;
        let mut req = discord_user::types::CreateThreadRequest::public(name);
        req.auto_archive_duration = archive_minutes;
        let resp: RawChannel = inner
            .post(
                Route::CreateThreadFromMessage {
                    channel_id: cid,
                    message_id: mid,
                },
                req,
            )
            .await
            .context("POST /channels/{id}/messages/{mid}/threads failed")?;
        Ok(ThreadResult {
            id: resp.id.to_string(),
            name: resp.name.clone().unwrap_or_default(),
            channel_id: channel_id.to_string(),
            channel_type: resp.channel_type,
            parent_message_id: Some(message_id.to_string()),
        })
    }

    /// `POST /channels/{id}/typing` — send typing indicator (no body).
    /// Reference: discordo composer.sendTyping() → Client.Typing (10s throttle
    /// enforced by caller; API itself is fire-and-forget).
    pub async fn trigger_typing(&mut self, channel_id: &str) -> Result<()> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let inner = self.inner()?;
        inner
            .post_empty(Route::TriggerTyping { channel_id: cid })
            .await
            .context("POST /channels/{id}/typing failed")?;
        Ok(())
    }

    /// `PATCH /channels/{id}/messages/{mid}` — edit own message (M3.2).
    pub async fn edit_message(
        &mut self,
        channel_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<()> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let mid: u64 = message_id.parse().context("invalid message id")?;
        let inner = self.inner()?;
        let req = discord_user::types::EditMessageRequest {
            content: Some(content),
            flags: None,
        };
        inner
            .patch::<serde_json::Value, _>(
                Route::EditMessage {
                    channel_id: cid,
                    message_id: mid,
                },
                req,
            )
            .await
            .context("PATCH message failed")?;
        Ok(())
    }

    /// `DELETE /channels/{id}/messages/{mid}` — delete own message (M3.2).
    pub async fn delete_message(&mut self, channel_id: &str, message_id: &str) -> Result<()> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let mid: u64 = message_id.parse().context("invalid message id")?;
        let inner = self.inner()?;
        inner
            .delete(Route::DeleteMessage {
                channel_id: cid,
                message_id: mid,
            })
            .await
            .context("DELETE message failed")?;
        Ok(())
    }

    /// `PUT /channels/{id}/messages/{mid}/reactions/{emoji}/@me` — react.
    pub async fn add_reaction(
        &mut self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<()> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let mid: u64 = message_id.parse().context("invalid message id")?;
        let inner = self.inner()?;
        // PUT .../reactions/{emoji}/@me returns 204 No Content (no body);
        // `put` would try to parse the empty body as JSON and fail. Use
        // `put_empty` which discards the response body.
        inner
            .put_empty(Route::AddReaction {
                channel_id: cid,
                message_id: mid,
                emoji,
            })
            .await
            .context("react failed")?;
        Ok(())
    }

    /// `DELETE .../reactions/{emoji}/@me` — remove own reaction.
    pub async fn remove_reaction(
        &mut self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<()> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let mid: u64 = message_id.parse().context("invalid message id")?;
        let inner = self.inner()?;
        inner
            .delete(Route::RemoveOwnReaction {
                channel_id: cid,
                message_id: mid,
                emoji,
            })
            .await
            .context("unreact failed")?;
        Ok(())
    }

    /// `PUT /channels/{id}/pins/{mid}` — pin a message.
    pub async fn pin_message(&mut self, channel_id: &str, message_id: &str) -> Result<()> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let mid: u64 = message_id.parse().context("invalid message id")?;
        let inner = self.inner()?;
        // PUT .../pins/{mid} returns 204 No Content (no body); `put` would
        // fail parsing the empty body as JSON. Use `put_empty` instead.
        inner
            .put_empty(Route::PinMessage {
                channel_id: cid,
                message_id: mid,
            })
            .await
            .context("pin failed")?;
        Ok(())
    }

    /// `DELETE /channels/{id}/pins/{mid}` — unpin a message.
    pub async fn unpin_message(&mut self, channel_id: &str, message_id: &str) -> Result<()> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let mid: u64 = message_id.parse().context("invalid message id")?;
        let inner = self.inner()?;
        inner
            .delete(Route::UnpinMessage {
                channel_id: cid,
                message_id: mid,
            })
            .await
            .context("unpin failed")?;
        Ok(())
    }

    /// `GET /channels/{id}/pins` — list pinned messages.
    pub async fn pinned_messages(
        &mut self,
        channel_id: &str,
    ) -> Result<Vec<crate::types::Message>> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let inner = self.inner()?;
        let raw: Vec<RawMessage> = inner
            .get(Route::GetPins { channel_id: cid })
            .await
            .context("GET pins failed")?;
        Ok(raw
            .into_iter()
            .map(|m| {
                let urls = m.url_list();
                let details = m.details();
                let reactions = m.reaction_total();
                crate::types::Message {
                    message_id: m.id.to_string(),
                    channel_id: channel_id.to_string(),
                    guild_id: None,
                    author_id: Some(m.author.id.to_string()),
                    author: m.author.username,
                    timestamp: m.timestamp,
                    content: m.content,
                    attachments: urls,
                    attachment_details: details,
                    reactions,
                }
            })
            .collect())
    }

    /// `POST /users/@me/channels` — create a group DM (M3.4).
    pub async fn create_group_dm(&mut self, user_ids: &[String]) -> Result<String> {
        let inner = self.inner()?;
        let body = serde_json::json!({ "access_tokens": [], "recipients": user_ids });
        let resp: RawDm = inner
            .post(Route::CreateGroupDm, body)
            .await
            .context("create group DM failed")?;
        Ok(resp.id.to_string())
    }

    /// `PUT /channels/{id}/recipients/{user_id}` — add to group DM (M3.4).
    pub async fn group_dm_add(&mut self, channel_id: &str, user_id: &str) -> Result<()> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let uid: u64 = user_id.parse().context("invalid user id")?;
        let inner = self.inner()?;
        // PUT .../recipients/{uid} returns 204 No Content; `put` would fail
        // parsing the empty body. Use `put_empty` instead.
        inner
            .put_empty(Route::GroupDmAddRecipient {
                channel_id: cid,
                user_id: uid,
            })
            .await
            .context("add group DM recipient failed")?;
        Ok(())
    }

    /// `DELETE /channels/{id}/recipients/{user_id}` — remove from group DM.
    pub async fn group_dm_remove(&mut self, channel_id: &str, user_id: &str) -> Result<()> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let uid: u64 = user_id.parse().context("invalid user id")?;
        let inner = self.inner()?;
        inner
            .delete(Route::GroupDmRemoveRecipient {
                channel_id: cid,
                user_id: uid,
            })
            .await
            .context("remove group DM recipient failed")?;
        Ok(())
    }

    /// `GET /channels/{id}/messages/{mid}` — fetch a single message.
    pub async fn get_message(
        &mut self,
        channel_id: &str,
        message_id: &str,
    ) -> Result<crate::types::Message> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let mid: u64 = message_id.parse().context("invalid message id")?;
        let inner = self.inner()?;
        let raw: RawMessage = inner
            .get(Route::GetMessage {
                channel_id: cid,
                message_id: mid,
            })
            .await
            .context("GET message failed")?;
        let urls = raw.url_list();
        let details = raw.details();
        let reactions = raw.reaction_total();
        Ok(crate::types::Message {
            message_id: raw.id.to_string(),
            channel_id: channel_id.to_string(),
            guild_id: None,
            author_id: Some(raw.author.id.to_string()),
            author: raw.author.username,
            timestamp: raw.timestamp,
            content: raw.content,
            attachments: urls,
            attachment_details: details,
            reactions,
        })
    }

    /// `GET /channels/{id}/messages` — fetch messages, newest-first, paged.
    /// `before`/`after` are snowflake cursors. Returns sorted ascending.
    pub async fn fetch_messages(
        &mut self,
        channel_id: &str,
        limit: usize,
        before: Option<u64>,
        after: Option<u64>,
    ) -> Result<Vec<crate::types::Message>> {
        let cid: u64 = channel_id.parse().context("invalid channel id")?;
        let inner = self.inner()?;
        let mut all: Vec<RawMessage> = Vec::new();
        let mut remaining = limit.min(1000);
        let mut cur_before = before;
        let mut cur_after = after;

        while remaining > 0 {
            let batch = remaining.min(100) as u32;
            let route = Route::GetMessages {
                channel_id: cid,
                limit: Some(batch),
                before: cur_before,
                after: cur_after,
            };
            let msgs: Vec<RawMessage> = inner.get(route).await?;
            let n = msgs.len();
            if n == 0 {
                break;
            }
            remaining = remaining.saturating_sub(n);
            // Small delay between pages to be rate-limit friendly (jackwener).
            tokio::time::sleep(std::time::Duration::from_millis(400 + (randish() % 400))).await;

            if after.is_some() {
                cur_after = msgs[0].id.parse().ok();
            } else {
                cur_before = msgs[n - 1].id.parse().ok();
            }
            all.extend(msgs);
            if n < batch as usize {
                break;
            }
        }

        // Sort ascending by id (jackwener sorts by msg_id ascending).
        all.sort_by_key(|m| m.id.clone());
        Ok(all
            .into_iter()
            .map(|m| {
                let urls = m.url_list();
                let details = m.details();
                let reactions = m.reaction_total();
                crate::types::Message {
                    message_id: m.id.to_string(),
                    channel_id: channel_id.to_string(),
                    guild_id: None,
                    author_id: Some(m.author.id.to_string()),
                    author: m.author.username,
                    timestamp: m.timestamp,
                    content: m.content,
                    attachments: urls,
                    attachment_details: details,
                    reactions,
                }
            })
            .collect())
    }
}

/// Small deterministic-ish jitter (0..400). Not cryptographic.
fn randish() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos() as u64;
    n % 400
}

/// Raw Discord response shapes (subset we consume).
#[derive(Debug, Clone, serde::Deserialize)]
struct RawGuild {
    id: String,
    name: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    owner: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawChannel {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "type")]
    channel_type: u8,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    position: i32,
}

/// Result of thread creation (type discriminator for output).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ThreadResult {
    pub id: String,
    pub name: String,
    pub channel_id: String,
    pub channel_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_message_id: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawMessage {
    id: String,
    author: RawAuthor,
    content: String,
    timestamp: String,
    #[serde(default)]
    attachments: Option<Vec<RawAttachment>>,
    #[serde(default)]
    reactions: Option<Vec<RawReaction>>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawAttachment {
    url: String,
    filename: String,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    size: Option<i64>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawReaction {
    count: i32,
}

impl RawMessage {
    /// Legacy URL-only list (back-compat `attachments` field).
    fn url_list(&self) -> Option<Vec<String>> {
        self.attachments
            .as_ref()
            .map(|a| a.iter().map(|x| x.url.clone()).collect())
    }

    /// Detailed attachment info (F6 download pipeline).
    fn details(&self) -> Option<Vec<crate::types::AttachmentInfo>> {
        self.attachments.as_ref().map(|a| {
            a.iter()
                .map(|x| crate::types::AttachmentInfo {
                    url: x.url.clone(),
                    filename: x.filename.clone(),
                    content_type: x.content_type.clone(),
                    size: x.size,
                })
                .collect()
        })
    }

    /// Sum of reaction counts (F8).
    fn reaction_total(&self) -> Option<Vec<crate::types::ReactionInfo>> {
        self.reactions.as_ref().map(|r| {
            r.iter()
                .map(|x| crate::types::ReactionInfo { count: x.count })
                .collect()
        })
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawAuthor {
    id: String,
    username: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SearchResponse {
    messages: Vec<Vec<RawSearchMessage>>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawSearchMessage {
    id: String,
    channel_id: String,
    author: RawAuthor,
    content: String,
    timestamp: String,
    #[serde(default)]
    attachments: Option<Vec<RawAttachment>>,
    #[serde(default)]
    reactions: Option<Vec<RawReaction>>,
}

impl RawSearchMessage {
    fn url_list(&self) -> Option<Vec<String>> {
        self.attachments
            .as_ref()
            .map(|a| a.iter().map(|x| x.url.clone()).collect())
    }

    fn details(&self) -> Option<Vec<crate::types::AttachmentInfo>> {
        self.attachments.as_ref().map(|a| {
            a.iter()
                .map(|x| crate::types::AttachmentInfo {
                    url: x.url.clone(),
                    filename: x.filename.clone(),
                    content_type: x.content_type.clone(),
                    size: x.size,
                })
                .collect()
        })
    }

    fn reaction_total(&self) -> Option<Vec<crate::types::ReactionInfo>> {
        self.reactions.as_ref().map(|r| {
            r.iter()
                .map(|x| crate::types::ReactionInfo { count: x.count })
                .collect()
        })
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawRole {
    id: String,
    name: String,
    #[serde(default)]
    color: u32,
    #[serde(default)]
    position: i32,
    #[serde(default)]
    permissions: String,
    #[serde(default)]
    hoist: bool,
    #[serde(default)]
    mentionable: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawRelationship {
    id: String,
    #[serde(rename = "type")]
    relationship_type: u8,
    #[serde(default)]
    username: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawUserProfile {
    user: RawProfileUser,
    #[serde(default)]
    user_bio: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawProfileUser {
    id: String,
    username: String,
    #[serde(default)]
    global_name: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ThreadActiveResponse {
    threads: Vec<RawThread>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[allow(dead_code)] // raw response fields kept for completeness
struct ThreadSearchResponse {
    threads: Vec<RawThread>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    total_results: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawThread {
    id: String,
    name: String,
    #[serde(rename = "type")]
    channel_type: u8,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    position: i32,
}

fn raw_thread_to_channel(t: RawThread) -> Channel {
    Channel {
        id: t.id.to_string(),
        name: t.name,
        guild_id: None,
        channel_type: t.channel_type,
        topic: None,
        parent_id: t.parent_id.map(|p| p.to_string()),
        position: Some(t.position),
        rate_limit_per_user: None,
        permission_overwrites: Vec::new(),
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawMember {
    nick: Option<String>,
    joined_at: Option<String>,
    user: RawMemberUser,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawMemberUser {
    id: String,
    username: String,
    #[serde(default)]
    global_name: Option<String>,
    #[serde(default)]
    bot: Option<bool>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawGuildInfo {
    id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    approximate_member_count: Option<u32>,
    #[serde(default)]
    approximate_presence_count: Option<u32>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RawDm {
    id: String,
    #[serde(rename = "type")]
    channel_type: u8,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    recipients: Option<Vec<RawDmUser>>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[allow(dead_code)] // id kept for completeness
struct RawDmUser {
    id: String,
    username: String,
    #[serde(default)]
    discriminator: Option<String>,
    #[serde(default)]
    global_name: Option<String>,
}

impl RawDmUser {
    /// Human label `user#disc` or `global_name` fallback.
    fn tag(&self) -> String {
        if let Some(g) = &self.global_name {
            if !g.is_empty() {
                return g.clone();
            }
        }
        if let Some(d) = &self.discriminator {
            if d != "0" {
                return format!("{}#{}", self.username, d);
            }
        }
        self.username.clone()
    }
}

/// Map a crate `discord_user::types::Channel` to the workspace `Channel`.
///
/// The crate type carries `permission_overwrites` + `rate_limit_per_user`
/// which the workspace display type now exposes (F1/F5).
fn crate_channel_to_workspace(c: discord_user::types::Channel) -> Channel {
    Channel {
        id: c.id,
        name: c.name.unwrap_or_default(),
        guild_id: c.guild_id,
        channel_type: c.channel_type as u8,
        topic: c.topic,
        parent_id: c.parent_id,
        position: c.position,
        rate_limit_per_user: c.rate_limit_per_user,
        permission_overwrites: c
            .permission_overwrites
            .into_iter()
            .map(|p| crate::types::PermissionOverwrite {
                id: p.id,
                overwrite_type: p.overwrite_type,
                allow: p.allow,
                deny: p.deny,
            })
            .collect(),
    }
}

/// Map a crate `discord_user::types::Role` to the workspace `Role`.
fn crate_role_to_workspace(r: discord_user::types::Role) -> crate::types::Role {
    crate::types::Role {
        id: r.id,
        name: r.name,
        color: r.color,
        position: r.position,
        permissions: r.permissions,
        hoist: r.hoist,
        mentionable: r.mentionable,
    }
}

/// Map a crate `discord_user::types::Invite` to the workspace `Invite` (F8).
fn invite_to_workspace(i: discord_user::types::Invite) -> Invite {
    Invite {
        code: i.code.clone(),
        url: i.url(),
        channel_id: i.channel.as_ref().map(|c| c.id.clone()),
        channel_name: i.channel.as_ref().and_then(|c| c.name.clone()),
        guild_id: i.guild.as_ref().map(|g| g.id.clone()),
        guild_name: i.guild.as_ref().and_then(|g| g.name.clone()),
        uses: i.uses,
        max_uses: i.max_uses,
        max_age: i.max_age,
        temporary: i.temporary,
        inviter: i.inviter.as_ref().map(|u| u.username.clone()),
        created_at: i.created_at.clone(),
        expires_at: i.expires_at.clone(),
    }
}

/// Canonical permission-name → bit mapping for `parse_permission_names`.
/// Mirrors the crate `Permissions` bitflags (enums.rs:316) + discli `api.ts`.
/// Public so `perm list` (CLI/MCP) can render the name→bit table.
pub const ALL_PERMISSION_NAMES: &[(&str, u64)] = &[
    ("create_instant_invite", 1 << 0),
    ("kick_members", 1 << 1),
    ("ban_members", 1 << 2),
    ("administrator", 1 << 3),
    ("manage_channels", 1 << 4),
    ("manage_server", 1 << 5),
    ("manage_guild", 1 << 5),
    ("add_reactions", 1 << 6),
    ("view_audit_log", 1 << 7),
    ("priority_speaker", 1 << 8),
    ("stream", 1 << 9),
    ("video", 1 << 9),
    ("view_channel", 1 << 10),
    ("send_messages", 1 << 11),
    ("send_tts_messages", 1 << 12),
    ("manage_messages", 1 << 13),
    ("embed_links", 1 << 14),
    ("attach_files", 1 << 15),
    ("read_message_history", 1 << 16),
    ("mention_everyone", 1 << 17),
    ("use_external_emojis", 1 << 18),
    ("view_guild_insights", 1 << 19),
    ("connect", 1 << 20),
    ("speak", 1 << 21),
    ("mute_members", 1 << 22),
    ("deafen_members", 1 << 23),
    ("move_members", 1 << 24),
    ("use_voice_activity", 1 << 25),
    ("change_nickname", 1 << 26),
    ("manage_nicknames", 1 << 27),
    ("manage_roles", 1 << 28),
    ("manage_webhooks", 1 << 29),
    ("manage_expressions", 1 << 30),
    ("manage_guild_expressions", 1 << 30),
    ("use_application_commands", 1 << 31),
    ("use_slash_commands", 1 << 31),
    ("manage_events", 1 << 33),
    ("manage_threads", 1 << 34),
    ("create_public_threads", 1 << 35),
    ("create_private_threads", 1 << 36),
    ("use_external_stickers", 1 << 37),
    ("send_messages_in_threads", 1 << 38),
    ("moderate_members", 1 << 40),
    ("send_polls", 1 << 49),
];

/// The REST base, re-exported for callers that need the full URL.
pub const REST_BASE: &str = API_BASE;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_holds_token_without_network() {
        let c = ApiClient::with_token("testtoken");
        assert_eq!(c.token, "testtoken");
        assert!(c.client.is_none());
    }

    #[test]
    fn api_base_is_v10() {
        assert_eq!(REST_BASE, "https://discord.com/api/v10");
    }

    #[test]
    fn guild_id_resolution_detects_numeric() {
        // resolve_guild_id short-circuits numeric to Some(id) without network.
        // We can't call the async method here without a client, but we verify
        // the predicate used: all-digits.
        let numeric = "1234567890";
        assert!(numeric.chars().all(|c| c.is_ascii_digit()));
        let named = "my-server";
        assert!(!named.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn channel_text_like_filter() {
        // Mirrors list_channels retain() logic: keep 0/5/15 only.
        let types = [0u8, 2, 5, 13, 15, 16];
        let kept: Vec<u8> = types
            .into_iter()
            .filter(|&t| matches!(t, 0 | 5 | 15))
            .collect();
        assert_eq!(kept, vec![0, 5, 15]);
    }

    #[test]
    fn randish_is_bounded() {
        for _ in 0..100 {
            let r = randish();
            assert!(r < 400, "randish out of bounds: {r}");
        }
    }

    #[test]
    fn extract_invite_code_handles_urls_and_plain() {
        // Full URLs across known prefixes.
        assert_eq!(
            ApiClient::extract_invite_code("https://discord.gg/abc123"),
            Some("abc123")
        );
        assert_eq!(
            ApiClient::extract_invite_code("https://discord.com/invite/xyz789"),
            Some("xyz789")
        );
        assert_eq!(
            ApiClient::extract_invite_code("https://discordapp.com/invite/qqq"),
            Some("qqq")
        );
        // Plain code passes through.
        assert_eq!(ApiClient::extract_invite_code("abc123"), Some("abc123"));
        // Trailing slash stripped.
        assert_eq!(
            ApiClient::extract_invite_code("discord.gg/abc/"),
            Some("abc")
        );
        // ?query and #fragment suffixes stripped (review#21).
        assert_eq!(
            ApiClient::extract_invite_code("https://discord.gg/abc?with_counts=1"),
            Some("abc")
        );
        assert_eq!(
            ApiClient::extract_invite_code("https://discord.gg/abc#section"),
            Some("abc")
        );
        // Empty/edge -> None.
        assert_eq!(ApiClient::extract_invite_code(""), None);
        assert_eq!(ApiClient::extract_invite_code("https://discord.gg/"), None);
        assert_eq!(ApiClient::extract_invite_code("discord.gg/?x=1"), None);
    }

    #[test]
    fn build_send_payload_has_attachments_when_files() {
        let p = ApiClient::build_send_payload("hello", None, 1).unwrap();
        assert_eq!(p["content"], "hello");
        assert_eq!(p["attachments"], serde_json::json!([{ "id": "0" }]));
        // mobile_network_type preserved (user-token mimic).
        assert_eq!(p["mobile_network_type"], "unknown");
    }

    #[test]
    fn build_send_payload_multi_file_ids() {
        let p = ApiClient::build_send_payload("x", Some("123"), 3).unwrap();
        assert_eq!(
            p["attachments"],
            serde_json::json!([{ "id": "0" }, { "id": "1" }, { "id": "2" }])
        );
        assert_eq!(p["message_reference"]["message_id"], "123");
    }

    #[test]
    fn build_send_payload_no_files_no_attachments_key() {
        // Without files, no attachments descriptor (matches plain send path).
        let p = ApiClient::build_send_payload("plain", None, 0).unwrap();
        assert!(
            p.get("attachments").is_none()
                || p["attachments"].as_array().is_some_and(|a| a.is_empty())
        );
    }

    #[test]
    fn validate_channel_name_boundaries() {
        assert!(ApiClient::validate_channel_name("general"));
        assert!(ApiClient::validate_channel_name("a"));
        assert!(ApiClient::validate_channel_name(&"x".repeat(100)));
        assert!(!ApiClient::validate_channel_name(""));
        assert!(!ApiClient::validate_channel_name(&"x".repeat(101)));
        assert!(!ApiClient::validate_channel_name("#general"));
    }

    #[test]
    fn validate_topic_boundaries() {
        assert!(ApiClient::validate_topic("hi"));
        assert!(ApiClient::validate_topic(&"x".repeat(1024)));
        assert!(!ApiClient::validate_topic(&"x".repeat(1025)));
    }

    #[test]
    fn lock_deny_bitfield_covers_send_and_threads() {
        use discord_user::types::Permissions;
        let deny = ApiClient::lock_deny_bitfield();
        assert_eq!(
            deny,
            (Permissions::SEND_MESSAGES
                | Permissions::SEND_MESSAGES_IN_THREADS
                | Permissions::CREATE_PUBLIC_THREADS)
                .bits()
        );
        // Must NOT deny the base read perms.
        assert_eq!(deny & Permissions::VIEW_CHANNEL.bits(), 0);
        assert_eq!(deny & Permissions::READ_MESSAGE_HISTORY.bits(), 0);
        assert_eq!(deny & Permissions::SEND_MESSAGES.bits(), 1 << 11);
    }

    #[test]
    fn parse_color_hex_variants() {
        assert_eq!(ApiClient::parse_color_hex("#ff5733").unwrap(), 0xff5733);
        assert_eq!(ApiClient::parse_color_hex("ff5733").unwrap(), 0xff5733);
        assert_eq!(ApiClient::parse_color_hex("#000000").unwrap(), 0);
        assert_eq!(ApiClient::parse_color_hex("#FFFFFF").unwrap(), 0xffffff);
        assert!(ApiClient::parse_color_hex("#fff").is_err());
        assert!(ApiClient::parse_color_hex("zzz").is_err());
        assert!(ApiClient::parse_color_hex("#ff5733ff").is_err());
    }

    #[test]
    fn parse_permission_names_maps_bits() {
        let names = vec!["send_messages".to_string(), "manage_roles".to_string()];
        let bits = ApiClient::parse_permission_names(&names).unwrap();
        assert_eq!(bits, (1 << 11) | (1 << 28));
        // Case-insensitive + underscores.
        let names = vec!["ADMINISTRATOR".to_string()];
        assert_eq!(ApiClient::parse_permission_names(&names).unwrap(), 1 << 3);
        // Unknown -> Err mentioning valid names.
        let err = ApiClient::parse_permission_names(&["nope".to_string()]).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("unknown permission"), "msg: {msg}");
        assert!(msg.contains("send_messages"), "msg: {msg}");
    }

    #[test]
    fn validate_emoji_name_rules() {
        assert!(ApiClient::validate_emoji_name("my_emoji"));
        assert!(ApiClient::validate_emoji_name("ab"));
        assert!(ApiClient::validate_emoji_name(&"a".repeat(32)));
        assert!(!ApiClient::validate_emoji_name("a")); // too short
        assert!(!ApiClient::validate_emoji_name(&"a".repeat(33))); // too long
        assert!(!ApiClient::validate_emoji_name("emo-ji")); // hyphen
        assert!(!ApiClient::validate_emoji_name("emoji!")); // punctuation
    }

    #[tokio::test]
    async fn build_image_data_uri_mime_and_base64() {
        let dir = std::env::temp_dir();
        let png = dir.join("discord-test-emoji.png");
        tokio::fs::write(&png, b"\x89PNG\r\n\x1a\n").await.unwrap();
        let uri = ApiClient::build_image_data_uri(png.to_str().unwrap())
            .await
            .unwrap();
        assert!(uri.starts_with("data:image/png;base64,"));
        assert!(uri.ends_with("iVBORw0KGgo=")); // base64 of the PNG signature
        let _ = tokio::fs::remove_file(&png).await;

        let gif = dir.join("discord-test-emoji.gif");
        tokio::fs::write(&gif, b"GIF89a").await.unwrap();
        let uri = ApiClient::build_image_data_uri(gif.to_str().unwrap())
            .await
            .unwrap();
        assert!(uri.starts_with("data:image/gif;base64,"));
        let _ = tokio::fs::remove_file(&gif).await;

        // Unknown extension -> Err.
        let bad = dir.join("discord-test-emoji.bin");
        tokio::fs::write(&bad, b"data").await.unwrap();
        assert!(ApiClient::build_image_data_uri(bad.to_str().unwrap())
            .await
            .is_err());
        let _ = tokio::fs::remove_file(&bad).await;
    }

    #[tokio::test]
    async fn build_image_data_uri_rejects_oversize() {
        let dir = std::env::temp_dir();
        let big = dir.join("discord-test-emoji-big.png");
        let blob = vec![0u8; (ApiClient::MAX_EMOJI_SIZE + 1) as usize];
        tokio::fs::write(&big, blob).await.unwrap();
        let err = ApiClient::build_image_data_uri(big.to_str().unwrap())
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("too large"));
        let _ = tokio::fs::remove_file(&big).await;
    }

    #[tokio::test]
    async fn build_image_data_uri_missing_file() {
        let err = ApiClient::build_image_data_uri("/nonexistent/emoji.png")
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("cannot read file"));
    }
}
