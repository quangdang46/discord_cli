//! Serde types for Discord entities (Guild, Channel, Message, User, DM).
//!
//! Field shapes match jackwener `_parse_message` / `list_guilds` /
//! `list_channels` (Apache-2.0, `.tmp/`) and famasya `MessageItem` (MIT).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A Discord snowflake ID (u64). String comparisons are used for cursors.
pub type Snowflake = u64;

/// Convert a snowflake to UTC datetime.
/// `ms = (id >> 22) + DISCORD_EPOCH` (jackwener client.py).
pub fn snowflake_to_datetime(id: Snowflake) -> DateTime<Utc> {
    const DISCORD_EPOCH: u64 = 1420070400000;
    let ms = (id >> 22) + DISCORD_EPOCH;
    DateTime::from_timestamp_millis(ms as i64).unwrap_or_default()
}

/// Convert a datetime to a snowflake (for `after` cursor).
/// `ms = ts_ms - DISCORD_EPOCH; snowflake = ms << 22`.
pub fn datetime_to_snowflake(dt: DateTime<Utc>) -> Snowflake {
    const DISCORD_EPOCH: u64 = 1420070400000;
    let ms = dt.timestamp_millis() as u64;
    (ms.saturating_sub(DISCORD_EPOCH)) << 22
}

/// A guild the user has joined.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Guild {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<bool>,
}

/// A channel within a guild (or DM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<String>,
    /// 0=text, 2=voice, 5=announcement, 15=forum, 16=media
    #[serde(rename = "type")]
    pub channel_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
    /// Slowmode in seconds (0–21600) — admin channel CRUD (F1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_per_user: Option<u32>,
    /// Permission overwrites (role/member allow-deny) — F1/F5.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub permission_overwrites: Vec<PermissionOverwrite>,
}

/// A single channel permission overwrite (role or member).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PermissionOverwrite {
    pub id: String,
    /// 0 = role, 1 = member.
    #[serde(rename = "type")]
    pub overwrite_type: u8,
    #[serde(default)]
    pub allow: String,
    #[serde(default)]
    pub deny: String,
}

impl Channel {
    /// True for text-like channels (text/announcement/forum).
    pub fn is_text_like(&self) -> bool {
        matches!(self.channel_type, 0 | 5 | 15)
    }
}

/// Map a verification-level name to its numeric value (F6).
/// `none=0, low=1, medium=2, high=3, very_high=4`.
/// Case-insensitive; `None` for unknown names.
pub fn parse_verification_level(name: &str) -> Option<u8> {
    match name.trim().to_ascii_lowercase().as_str() {
        "none" => Some(0),
        "low" => Some(1),
        "medium" => Some(2),
        "high" => Some(3),
        "very_high" => Some(4),
        _ => None,
    }
}

/// Map a default-notification-level name to its numeric value (F6).
/// `all_messages=0, only_mentions=1`. Case-insensitive.
pub fn parse_notification_level(name: &str) -> Option<u8> {
    match name.trim().to_ascii_lowercase().as_str() {
        "all_messages" => Some(0),
        "only_mentions" => Some(1),
        _ => None,
    }
}

/// Map a content-filter name to its numeric value (F6).
/// `disabled=0, members_without_roles=1, all_members=2`. Case-insensitive.
pub fn parse_content_filter(name: &str) -> Option<u8> {
    match name.trim().to_ascii_lowercase().as_str() {
        "disabled" => Some(0),
        "members_without_roles" => Some(1),
        "all_members" => Some(2),
        _ => None,
    }
}

/// Map a channel-type name to its numeric value (CLI/MCP shared).
/// `text=0, voice=2, category=4, announcement=5, stage=13, forum=15`.
/// Case-insensitive; `None` for unknown names.
pub fn parse_channel_type_name(name: &str) -> Option<u8> {
    match name.trim().to_ascii_lowercase().as_str() {
        "text" => Some(0),
        "voice" => Some(2),
        "category" => Some(4),
        "announcement" => Some(5),
        "stage" => Some(13),
        "forum" => Some(15),
        _ => None,
    }
}

/// Reverse map: numeric channel type → display name (for errors/echo).
pub fn channel_type_name(t: u8) -> &'static str {
    match t {
        0 => "text",
        2 => "voice",
        4 => "category",
        5 => "announcement",
        13 => "stage",
        15 => "forum",
        _ => "unknown",
    }
}

/// A message attachment (F6: download pipeline needs url/filename/type/size).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttachmentInfo {
    pub url: String,
    pub filename: String,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub size: Option<i64>,
}

/// A message reaction (F8: reaction_count = sum of counts).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReactionInfo {
    pub count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub message_id: String,
    pub channel_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_id: Option<String>,
    pub author: String,
    pub timestamp: String,
    pub content: String,
    /// Attachment URLs (legacy field, kept for back-compat).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<String>>,
    /// Detailed attachments (url/filename/content_type/size) — F6.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_details: Option<Vec<AttachmentInfo>>,
    /// Reaction counts (F8) — sum of `count` is `reaction_count`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reactions: Option<Vec<ReactionInfo>>,
}

/// Current user profile (`GET /users/@me`). Field shapes match the
/// Discord REST response (mirrors discord-user-rs `MeResponse`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Me {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub mfa_enabled: bool,
    #[serde(default)]
    pub premium_type: u32,
}

/// A guild member (jackwener list_members shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    pub id: String,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nick: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub joined_at: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub bot: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Guild info with counts (jackwener get_guild_info shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuildInfo {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub online_count: Option<u32>,
}

/// A guild role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    pub id: String,
    pub name: String,
    pub color: u32,
    pub position: i32,
    pub permissions: String,
    /// Whether the role is displayed separately in the member list (F2).
    #[serde(default)]
    pub hoist: bool,
    /// Whether anyone can mention the role (F2).
    #[serde(default)]
    pub mentionable: bool,
}

impl Role {
    /// The `@everyone` role has the same ID as its guild.
    pub fn is_everyone(&self, guild_id: &str) -> bool {
        self.id == guild_id
    }
}

/// A user relationship (friend/blocked/pending).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    pub user_id: String,
    pub username: String,
    pub relationship_type: u8,
}

/// A user's public profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub user_id: String,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
}

/// A user's public info (`GET /users/{id}`).
///
/// Everything here comes from the public user object — no member-only fields
/// (nickname, roles, guild presence), which would require the bot-only
/// `GET /guilds/{id}/members/{uid}` and 403 for user tokens. `created_at` is
/// derived from the snowflake's embedded timestamp, `avatar_url`/`banner_url`
/// are built CDN links when a hash exists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub user_id: String,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discriminator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banner_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent_color: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mfa_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub premium_type: Option<u8>,
    /// Public flags bitfield (STAFF/PARTNER/HYPESQUAD/... badges).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_flags: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

/// A DM or group-DM channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmChannel {
    pub id: String,
    /// Human label: user#disc for DMs, joined tags for group DMs.
    pub label: String,
    #[serde(rename = "type")]
    pub channel_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipient_count: Option<usize>,
}

/// A guild invite (F8).
///
/// Display shape derived from the crate `discord_user::types::Invite`: the
/// code, the `https://discord.gg/<code>` link, channel/guild context, usage
/// stats, and the inviter's username when available.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invite {
    pub code: String,
    /// Full `https://discord.gg/<code>` link.
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_name: Option<String>,
    #[serde(default)]
    pub uses: u32,
    #[serde(default)]
    pub max_uses: u32,
    #[serde(default)]
    pub max_age: u32,
    #[serde(default)]
    pub temporary: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inviter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// One audit-log entry rendered for display (F7).
///
/// Mapped from the crate `AuditLogEntry`: carries the action type both as the
/// raw numeric code and its name (via `AUDIT_ACTION_MAP`), plus a compact
/// `change_summary` ("key: old → new" lines, capped).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntryView {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// Numeric action type (Discord audit-log event code).
    pub action_type: u32,
    /// Human action name (from `AUDIT_ACTION_MAP`), `unknown(<code>)` fallback.
    pub action_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Compact `key: old → new` lines (first 3 changes), joined with "; ".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_summary: Option<String>,
}

/// Name → numeric action-type map for the Discord audit log (F7).
///
/// Ported from discli `audit.ts` `AUDIT_ACTION` (MIT, `.tmp/`). The numeric
/// codes are the stable Discord API values; names are the lowercase
/// snake_case identifiers used by `audit-log --type` and the MCP
/// `get_audit_logs` tool.
pub const AUDIT_ACTION_MAP: &[(&str, u32)] = &[
    ("guild_update", 1),
    ("channel_create", 10),
    ("channel_update", 11),
    ("channel_delete", 12),
    ("member_kick", 20),
    ("member_prune", 21),
    ("member_ban_add", 22),
    ("member_ban_remove", 23),
    ("member_update", 24),
    ("member_role_update", 25),
    ("bot_add", 28),
    ("role_create", 30),
    ("role_update", 31),
    ("role_delete", 32),
    ("invite_create", 40),
    ("invite_delete", 42),
    ("webhook_create", 50),
    ("webhook_update", 51),
    ("webhook_delete", 52),
    ("emoji_create", 60),
    ("emoji_delete", 62),
    ("message_delete", 72),
    ("message_bulk_delete", 73),
    ("message_pin", 74),
    ("message_unpin", 75),
    ("integration_create", 80),
    ("integration_update", 81),
    ("integration_delete", 82),
    ("thread_create", 110),
    ("thread_update", 111),
    ("thread_delete", 112),
    ("automod_rule_create", 140),
    ("automod_rule_update", 141),
    ("automod_rule_delete", 142),
    ("automod_block_message", 143),
];

/// Reverse lookup: numeric audit-log action type → name. Case-insensitive.
pub fn audit_action_name(code: u32) -> Option<&'static str> {
    AUDIT_ACTION_MAP
        .iter()
        .find(|(_, v)| *v == code)
        .map(|(name, _)| *name)
}

/// Lookup an audit-log action name (case-insensitive) → numeric code.
///
/// Returns an error (with the available names) for unknown names — used by the
/// CLI/MCP to exit/abort with a helpful message.
pub fn audit_action_code(name: &str) -> Result<u32, String> {
    let needle = name.trim().to_ascii_lowercase();
    match AUDIT_ACTION_MAP
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(&needle))
    {
        Some((_, code)) => Ok(*code),
        None => {
            let valid: Vec<&str> = AUDIT_ACTION_MAP.iter().map(|(n, _)| *n).collect();
            Err(format!(
                "unknown audit action \"{}\" (valid: {})",
                name.trim(),
                valid.join(", ")
            ))
        }
    }
}

/// One embed field in an `EmbedSpec` (F9).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedFieldSpec {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub inline: bool,
}

/// Agent-facing spec for building an embed message (F9).
///
/// Either an embed (≥1 of `title`/`description`) or plain `content` must be
/// present — validated by `validate_embed` at the CLI/MCP boundary.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmbedSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// RGB color 0x000000..0xFFFFFF.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<EmbedFieldSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

/// Validate an `EmbedSpec` against Discord's embed limits (F9).
///
/// - title ≤ 256, description ≤ 4096
/// - ≤ 10 fields; field name ≤ 256, field value ≤ 1024
/// - color 0x000000..0xFFFFFF
/// - requires ≥1 of title/description/content (else usage error)
pub fn validate_embed(spec: &EmbedSpec) -> Result<(), String> {
    if spec.title.is_none() && spec.description.is_none() && spec.content.is_none() {
        return Err(
            "embed requires at least one of --title/--description (or --content)".to_string(),
        );
    }
    if let Some(t) = &spec.title {
        if t.chars().count() > 256 {
            return Err(format!(
                "embed title too long: {} chars (max 256)",
                t.chars().count()
            ));
        }
    }
    if let Some(d) = &spec.description {
        if d.chars().count() > 4096 {
            return Err(format!(
                "embed description too long: {} chars (max 4096)",
                d.chars().count()
            ));
        }
    }
    if spec.fields.len() > 10 {
        return Err(format!(
            "too many embed fields: {} (max 10)",
            spec.fields.len()
        ));
    }
    for f in &spec.fields {
        if f.name.chars().count() > 256 {
            return Err(format!(
                "embed field name too long: {} chars (max 256)",
                f.name.chars().count()
            ));
        }
        if f.value.chars().count() > 1024 {
            return Err(format!(
                "embed field value too long: {} chars (max 1024)",
                f.value.chars().count()
            ));
        }
    }
    if let Some(c) = spec.color {
        if c > 0xFF_FFFF {
            return Err(format!("invalid embed color 0x{c:06X} (max 0xFFFFFF)"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snowflake_roundtrip() {
        // Discord epoch is 2015-01-01; a known id.
        let dt = snowflake_to_datetime(123456789012345678);
        // Just verify it parses to a plausible date and round-trips approx.
        let back = datetime_to_snowflake(dt);
        // (id >> 22) << 22 loses low 22 bits — compare upper bits only.
        assert_eq!(back >> 22, 123456789012345678u64 >> 22);
    }

    #[test]
    fn verification_level_mapping() {
        assert_eq!(parse_verification_level("none"), Some(0));
        assert_eq!(parse_verification_level("low"), Some(1));
        assert_eq!(parse_verification_level("medium"), Some(2));
        assert_eq!(parse_verification_level("high"), Some(3));
        assert_eq!(parse_verification_level("very_high"), Some(4));
        assert_eq!(parse_verification_level("VERY_HIGH"), Some(4));
        assert_eq!(parse_verification_level("bogus"), None);
        // Every accepted string maps to a value in 0..=4.
        for (n, v) in [
            ("none", 0),
            ("low", 1),
            ("medium", 2),
            ("high", 3),
            ("very_high", 4),
        ] {
            assert_eq!(parse_verification_level(n), Some(v));
        }
    }

    #[test]
    fn notification_level_mapping() {
        assert_eq!(parse_notification_level("all_messages"), Some(0));
        assert_eq!(parse_notification_level("only_mentions"), Some(1));
        assert_eq!(parse_notification_level("ALL_MESSAGES"), Some(0));
        assert_eq!(parse_notification_level("bogus"), None);
    }

    #[test]
    fn content_filter_mapping() {
        assert_eq!(parse_content_filter("disabled"), Some(0));
        assert_eq!(parse_content_filter("members_without_roles"), Some(1));
        assert_eq!(parse_content_filter("all_members"), Some(2));
        assert_eq!(parse_content_filter("ALL_MEMBERS"), Some(2));
        assert_eq!(parse_content_filter("bogus"), None);
    }

    #[test]
    fn channel_text_like() {
        let text = Channel {
            id: "1".into(),
            name: "g".into(),
            guild_id: None,
            channel_type: 0,
            topic: None,
            parent_id: None,
            position: None,
            rate_limit_per_user: None,
            permission_overwrites: Vec::new(),
        };
        let forum = Channel {
            id: "2".into(),
            name: "f".into(),
            guild_id: None,
            channel_type: 15,
            topic: None,
            parent_id: None,
            position: None,
            rate_limit_per_user: None,
            permission_overwrites: Vec::new(),
        };
        let voice = Channel {
            id: "3".into(),
            name: "v".into(),
            guild_id: None,
            channel_type: 2,
            topic: None,
            parent_id: None,
            position: None,
            rate_limit_per_user: None,
            permission_overwrites: Vec::new(),
        };
        assert!(text.is_text_like());
        assert!(forum.is_text_like());
        assert!(!voice.is_text_like());
    }

    #[test]
    fn message_serializes_agent_friendly() {
        let m = Message {
            message_id: "1".into(),
            channel_id: "c".into(),
            guild_id: Some("g".into()),
            author_id: Some("u".into()),
            author: "alice".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            content: "hello".into(),
            attachments: None,
            attachment_details: None,
            reactions: None,
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["message_id"], "1");
        assert_eq!(v["content"], "hello");
    }

    #[test]
    fn audit_action_name_reverse_lookup() {
        assert_eq!(audit_action_name(1), Some("guild_update"));
        assert_eq!(audit_action_name(10), Some("channel_create"));
        assert_eq!(audit_action_name(20), Some("member_kick"));
        assert_eq!(audit_action_name(22), Some("member_ban_add"));
        assert_eq!(audit_action_name(30), Some("role_create"));
        assert_eq!(audit_action_name(999_999), None);
    }

    #[test]
    fn audit_action_code_forward_lookup() {
        assert_eq!(audit_action_code("guild_update").unwrap(), 1);
        assert_eq!(audit_action_code("member_kick").unwrap(), 20);
        // Case-insensitive.
        assert_eq!(audit_action_code("MEMBER_BAN_ADD").unwrap(), 22);
        assert_eq!(audit_action_code("  Role_Create  ").unwrap(), 30);
        // Round-trip.
        for (name, code) in AUDIT_ACTION_MAP {
            assert_eq!(audit_action_code(name).unwrap(), *code);
            assert_eq!(audit_action_name(*code), Some(*name));
        }
    }

    #[test]
    fn audit_action_code_unknown_lists_valid() {
        let err = audit_action_code("bogus").unwrap_err();
        assert!(err.contains("bogus"), "err: {err}");
        assert!(err.contains("member_kick"), "err: {err}");
        assert!(err.contains("channel_create"), "err: {err}");
    }

    #[test]
    fn validate_embed_requires_title_desc_or_content() {
        let spec = EmbedSpec::default();
        assert!(validate_embed(&spec).is_err());
        let spec = EmbedSpec {
            title: Some("t".into()),
            ..Default::default()
        };
        assert!(validate_embed(&spec).is_ok());
        let spec = EmbedSpec {
            description: Some("d".into()),
            ..Default::default()
        };
        assert!(validate_embed(&spec).is_ok());
        let spec = EmbedSpec {
            content: Some("c".into()),
            ..Default::default()
        };
        assert!(validate_embed(&spec).is_ok());
    }

    #[test]
    fn validate_embed_title_description_limits() {
        let ok = EmbedSpec {
            title: Some("x".repeat(256)),
            description: Some("y".repeat(4096)),
            ..Default::default()
        };
        assert!(validate_embed(&ok).is_ok());
        let long_title = EmbedSpec {
            title: Some("x".repeat(257)),
            description: Some("y".into()),
            ..Default::default()
        };
        assert!(validate_embed(&long_title).is_err());
        let long_desc = EmbedSpec {
            title: Some("x".into()),
            description: Some("y".repeat(4097)),
            ..Default::default()
        };
        assert!(validate_embed(&long_desc).is_err());
    }

    #[test]
    fn validate_embed_fields_limits() {
        // 10 fields ok.
        let fields: Vec<EmbedFieldSpec> = (0..10)
            .map(|i| EmbedFieldSpec {
                name: format!("f{i}"),
                value: "v".into(),
                inline: false,
            })
            .collect();
        let spec = EmbedSpec {
            title: Some("t".into()),
            fields,
            ..Default::default()
        };
        assert!(validate_embed(&spec).is_ok());
        // 11 fields fail.
        let mut fields = spec.fields.clone();
        fields.push(EmbedFieldSpec {
            name: "f11".into(),
            value: "v".into(),
            inline: false,
        });
        let spec = EmbedSpec {
            title: Some("t".into()),
            fields,
            ..Default::default()
        };
        assert!(validate_embed(&spec).is_err());
        // Field name/value limits.
        let spec = EmbedSpec {
            title: Some("t".into()),
            fields: vec![EmbedFieldSpec {
                name: "n".repeat(257),
                value: "v".into(),
                inline: false,
            }],
            ..Default::default()
        };
        assert!(validate_embed(&spec).is_err());
        let spec = EmbedSpec {
            title: Some("t".into()),
            fields: vec![EmbedFieldSpec {
                name: "n".into(),
                value: "v".repeat(1025),
                inline: false,
            }],
            ..Default::default()
        };
        assert!(validate_embed(&spec).is_err());
    }

    #[test]
    fn validate_embed_color_range() {
        let ok = EmbedSpec {
            title: Some("t".into()),
            color: Some(0x00_0000),
            ..Default::default()
        };
        assert!(validate_embed(&ok).is_ok());
        let ok = EmbedSpec {
            title: Some("t".into()),
            color: Some(0xFF_FFFF),
            ..Default::default()
        };
        assert!(validate_embed(&ok).is_ok());
        let bad = EmbedSpec {
            title: Some("t".into()),
            color: Some(0x01_00_00_00),
            ..Default::default()
        };
        assert!(validate_embed(&bad).is_err());
    }
}
