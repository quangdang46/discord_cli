//! Name→ID resolution helpers (discli `resolve.ts` pattern, ported to Rust).
//!
//! Canonical behavior (plan §18):
//! - ID match first.
//! - Strip `#`/`@` prefix.
//! - Case-insensitive exact match.
//! - Ambiguity → print matches to stderr + exit 1 (agent must disambiguate).
//! - Not found → exit 3.

use std::process::ExitCode;

use discord_core::client::{ApiClient, GuildResolution};
use discord_core::output::{self, exit};
use discord_core::types::{Channel, Role};

/// Emit a structured error envelope for a resolver API failure and return the
/// matching exit code (admin commands must never leak plain `error:` text —
/// agents parse `{ok:false,error:{code}}`).
fn api_err(e: &anyhow::Error) -> ExitCode {
    ExitCode::from(output::emit_error(
        "ApiError",
        &e.to_string(),
        crate::commands::dc::classify(e),
    ))
}

/// Resolve a guild name or ID to a guild ID.
/// Errors (exit 3) if not found; exit 2 with candidates on ambiguity.
pub async fn resolve_guild(client: &mut ApiClient, name: &str) -> Result<String, ExitCode> {
    match client.resolve_guild(name).await {
        Ok(GuildResolution::Found(id)) => Ok(id),
        Ok(GuildResolution::Ambiguous(guilds)) => {
            let names: Vec<String> = guilds
                .iter()
                .map(|g| format!("\"{}\" ({})", g.name, g.id))
                .collect();
            Err(ExitCode::from(output::emit_error(
                "Usage",
                &format!(
                    "ambiguous guild \"{name}\". Matches: {}. Use the guild ID instead.",
                    names.join(", ")
                ),
                exit::USAGE,
            )))
        }
        Ok(GuildResolution::NotFound) => Err(ExitCode::from(output::emit_error(
            "NotFound",
            &format!("guild \"{name}\" not found. Use `discord dc guilds` to list."),
            exit::NOT_FOUND,
        ))),
        Err(e) => Err(api_err(&e)),
    }
}

/// Resolve a channel name or ID within a guild (admin variant).
///
/// Uses `get_guild_channels_all` (ALL channel types — unlike the read-path
/// `resolve_channel` which filters text-like). Semantics (F0):
/// ID-match first → strip `#` → case-insensitive exact name →
/// ambiguity → stderr list + exit 2 → not-found → None (caller exits 3).
pub async fn resolve_channel_admin(
    client: &mut ApiClient,
    guild_id: &str,
    name: &str,
) -> Result<Option<String>, ExitCode> {
    let channels = client
        .get_guild_channels_all(guild_id)
        .await
        .map_err(|e| api_err(&e))?;

    // ID match first.
    if let Some(c) = channels.iter().find(|c| c.id == name) {
        return Ok(Some(c.id.clone()));
    }

    // Strip # prefix + case-insensitive exact match.
    let clean = name.strip_prefix('#').unwrap_or(name);
    let needle = clean.to_lowercase();
    let matches: Vec<&Channel> = channels
        .iter()
        .filter(|c| c.name.to_lowercase() == needle)
        .collect();

    match matches.len() {
        1 => Ok(Some(matches[0].id.clone())),
        n if n > 1 => {
            let names: Vec<String> = matches
                .iter()
                .map(|m| format!("#{} (type {})", m.name, m.channel_type))
                .collect();
            Err(ExitCode::from(output::emit_error(
                "Usage",
                &format!(
                    "ambiguous channel \"{clean}\". Matches: {}. Use the channel ID instead.",
                    names.join(", ")
                ),
                exit::USAGE,
            )))
        }
        _ => Ok(None),
    }
}

/// Resolve a category name or ID within a guild (channel_type == 4).
/// Same match semantics as `resolve_channel_admin`.
pub async fn resolve_category(
    client: &mut ApiClient,
    guild_id: &str,
    name: &str,
) -> Result<Option<String>, ExitCode> {
    let channels = client
        .get_guild_channels_all(guild_id)
        .await
        .map_err(|e| api_err(&e))?;
    let categories: Vec<&Channel> = channels.iter().filter(|c| c.channel_type == 4).collect();

    if let Some(c) = categories.iter().find(|c| c.id == name) {
        return Ok(Some(c.id.clone()));
    }
    let clean = name.strip_prefix('#').unwrap_or(name);
    let needle = clean.to_lowercase();
    let matches: Vec<&&Channel> = categories
        .iter()
        .filter(|c| c.name.to_lowercase() == needle)
        .collect();
    match matches.len() {
        1 => Ok(Some(matches[0].id.clone())),
        n if n > 1 => Err(ExitCode::from(output::emit_error(
            "Usage",
            &format!("ambiguous category \"{clean}\". Use the category ID instead."),
            exit::USAGE,
        ))),
        _ => Ok(None),
    }
}

/// Resolve a role name or ID within a guild.
/// ID-match first, strip `@`/`#`, case-insensitive exact over `list_roles`
/// (sorted position-desc); ambiguity → stderr + exit 2; not-found → None.
pub async fn resolve_role(
    client: &mut ApiClient,
    guild_id: &str,
    name: &str,
) -> Result<Option<String>, ExitCode> {
    let roles = client.list_roles(guild_id).await.map_err(|e| api_err(&e))?;
    if let Some(r) = roles.iter().find(|r| r.id == name) {
        return Ok(Some(r.id.clone()));
    }
    let clean = name.trim_start_matches(['@', '#']);
    let needle = clean.to_lowercase();
    let matches: Vec<&Role> = roles
        .iter()
        .filter(|r| r.name.to_lowercase() == needle)
        .collect();
    match matches.len() {
        1 => Ok(Some(matches[0].id.clone())),
        n if n > 1 => {
            let names: Vec<String> = matches.iter().map(|m| format!("@{}", m.name)).collect();
            Err(ExitCode::from(output::emit_error(
                "Usage",
                &format!(
                    "ambiguous role \"{clean}\". Matches: {}. Use the role ID instead.",
                    names.join(", ")
                ),
                exit::USAGE,
            )))
        }
        _ => Ok(None),
    }
}

/// Resolve a member (bare-ID passthrough; else list_members up to 1000 and
/// match username/global_name/nick case-insensitively). Ambiguity → exit 2;
/// not-found → None. Guilds >1000 members are a documented limitation.
pub async fn resolve_member(
    client: &mut ApiClient,
    guild_id: &str,
    name: &str,
) -> Result<Option<String>, ExitCode> {
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()) {
        return Ok(Some(name.to_string()));
    }
    let members = client
        .list_members(guild_id, 1000)
        .await
        .map_err(|e| api_err(&e))?;
    let clean = name.trim_start_matches('@').to_lowercase();
    let matches: Vec<&discord_core::types::Member> = members
        .iter()
        .filter(|m| {
            m.username.to_lowercase() == clean
                || m.global_name
                    .as_ref()
                    .is_some_and(|g| g.to_lowercase() == clean)
                || m.nick.as_ref().is_some_and(|n| n.to_lowercase() == clean)
        })
        .collect();
    match matches.len() {
        1 => Ok(Some(matches[0].id.clone())),
        n if n > 1 => {
            let names: Vec<String> = matches.iter().map(|m| m.username.clone()).collect();
            Err(ExitCode::from(output::emit_error(
                "Usage",
                &format!(
                    "ambiguous user \"{clean}\". Matches: {}. Use the user ID instead.",
                    names.join(", ")
                ),
                exit::USAGE,
            )))
        }
        _ => Ok(None),
    }
}

/// Resolve a member for moderation commands (kick/ban/nick): ONE
/// `list_members` call returning `(user_id, username)`.
///
/// Unlike [`resolve_member`] (bare-ID passthrough, id only), this always
/// consults the member list so the caller gets the display username without
/// a second fetch. Numeric input not present in the list still passes
/// through with `username: None` (members beyond the 1000 cap, or already
/// gone — the API call itself will then surface the real error).
pub async fn resolve_member_info(
    client: &mut ApiClient,
    guild_id: &str,
    name: &str,
) -> Result<Option<(String, Option<String>)>, ExitCode> {
    let numeric = !name.is_empty() && name.chars().all(|c| c.is_ascii_digit());
    let members = client
        .list_members(guild_id, 1000)
        .await
        .map_err(|e| api_err(&e))?;
    let clean = name.trim_start_matches('@').to_lowercase();
    let matches: Vec<&discord_core::types::Member> = members
        .iter()
        .filter(|m| {
            m.id == name
                || m.username.to_lowercase() == clean
                || m.global_name
                    .as_ref()
                    .is_some_and(|g| g.to_lowercase() == clean)
                || m.nick.as_ref().is_some_and(|n| n.to_lowercase() == clean)
        })
        .collect();
    match matches.len() {
        1 => Ok(Some((
            matches[0].id.clone(),
            Some(matches[0].username.clone()),
        ))),
        n if n > 1 => {
            let names: Vec<String> = matches.iter().map(|m| m.username.clone()).collect();
            Err(ExitCode::from(output::emit_error(
                "Usage",
                &format!(
                    "ambiguous user \"{clean}\". Matches: {}. Use the user ID instead.",
                    names.join(", ")
                ),
                exit::USAGE,
            )))
        }
        _ if numeric => Ok(Some((name.to_string(), None))),
        _ => Ok(None),
    }
}

/// Resolve a custom emoji by name (`:name:` or bare) or ID.
/// Name resolution requires a `list_emojis` call; ambiguity → exit 2;
/// not-found → None.
pub async fn resolve_emoji(
    client: &mut ApiClient,
    guild_id: &str,
    name_or_id: &str,
) -> Result<Option<discord_user::types::GuildEmoji>, ExitCode> {
    let emojis = client
        .list_emojis(guild_id)
        .await
        .map_err(|e| api_err(&e))?;
    // Bare-ID passthrough.
    if let Some(e) = emojis.iter().find(|e| e.id == name_or_id) {
        return Ok(Some(e.clone()));
    }
    let clean = name_or_id
        .strip_prefix(':')
        .and_then(|s| s.strip_suffix(':'))
        .unwrap_or(name_or_id);
    let needle = clean.to_lowercase();
    let matches: Vec<&discord_user::types::GuildEmoji> = emojis
        .iter()
        .filter(|e| {
            e.name
                .as_deref()
                .is_some_and(|n| n.to_lowercase() == needle)
        })
        .collect();
    match matches.len() {
        1 => Ok(Some(matches[0].clone())),
        n if n > 1 => Err(ExitCode::from(output::emit_error(
            "Usage",
            &format!("ambiguous emoji \"{clean}\". Use the emoji ID instead."),
            exit::USAGE,
        ))),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use discord_core::types::{Channel, Role};

    fn ch(id: &str, name: &str, channel_type: u8) -> Channel {
        Channel {
            id: id.into(),
            name: name.into(),
            guild_id: Some("g".into()),
            channel_type,
            topic: None,
            parent_id: None,
            position: Some(0),
            rate_limit_per_user: None,
            permission_overwrites: Vec::new(),
        }
    }

    // Pure helpers extracted for testing: strip + match logic.
    fn strip_prefix(name: &str) -> &str {
        name.strip_prefix('#').unwrap_or(name)
    }

    #[test]
    fn strip_hash_prefix() {
        assert_eq!(strip_prefix("#general"), "general");
        assert_eq!(strip_prefix("general"), "general");
        assert_eq!(strip_prefix("#"), "");
    }

    #[test]
    fn exact_case_insensitive_match() {
        let channels = [ch("1", "General", 0), ch("2", "announcements", 5)];
        let needle = "general";
        let matches: Vec<&Channel> = channels
            .iter()
            .filter(|c| c.name.to_lowercase() == needle.to_lowercase())
            .collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, "1");
    }

    #[test]
    fn ambiguity_detects_multiple() {
        let channels = [ch("1", "general", 0), ch("2", "General", 2)];
        let needle = "general";
        let matches: Vec<&Channel> = channels
            .iter()
            .filter(|c| c.name.to_lowercase() == needle.to_lowercase())
            .collect();
        assert_eq!(matches.len(), 2); // both text + voice "general"
    }

    // Pure strip/match helpers mirroring the async resolvers (no network).
    fn strip_channel(name: &str) -> &str {
        name.strip_prefix('#').unwrap_or(name)
    }

    fn strip_role(name: &str) -> &str {
        name.trim_start_matches(['@', '#'])
    }

    fn strip_emoji(name: &str) -> &str {
        name.strip_prefix(':')
            .and_then(|s| s.strip_suffix(':'))
            .unwrap_or(name)
    }

    #[test]
    fn strip_variants() {
        assert_eq!(strip_channel("#general"), "general");
        assert_eq!(strip_channel("general"), "general");
        assert_eq!(strip_role("@admin"), "admin");
        assert_eq!(strip_role("#admin"), "admin");
        assert_eq!(strip_role("@#mod"), "mod");
        assert_eq!(strip_emoji(":party:"), "party");
        assert_eq!(strip_emoji("party"), "party");
        assert_eq!(strip_emoji(":a"), ":a");
    }

    #[test]
    fn resolve_match_case_insensitive_exact() {
        let roles = [Role {
            id: "1".into(),
            name: "Moderator".into(),
            color: 0,
            position: 2,
            permissions: "0".into(),
            hoist: false,
            mentionable: false,
        }];
        let m: Vec<&Role> = roles
            .iter()
            .filter(|r| r.name.to_lowercase() == "moderator")
            .collect();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id, "1");
    }

    #[test]
    fn resolve_role_ambiguity() {
        let roles = [
            Role {
                id: "1".into(),
                name: "Mod".into(),
                color: 0,
                position: 3,
                permissions: "0".into(),
                hoist: false,
                mentionable: false,
            },
            Role {
                id: "2".into(),
                name: "mod".into(),
                color: 0,
                position: 2,
                permissions: "0".into(),
                hoist: false,
                mentionable: false,
            },
        ];
        let needle = "mod";
        let m: Vec<&Role> = roles
            .iter()
            .filter(|r| r.name.to_lowercase() == needle)
            .collect();
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn member_match_fields() {
        // Emulates resolve_member's predicate on the three name fields.
        let members = [
            discord_core::types::Member {
                id: "1".into(),
                username: "alice".into(),
                global_name: Some("Alice".into()),
                nick: None,
                joined_at: None,
                bot: false,
            },
            discord_core::types::Member {
                id: "2".into(),
                username: "bob".into(),
                global_name: None,
                nick: Some("Bob".into()),
                joined_at: None,
                bot: false,
            },
        ];
        let clean = "alice";
        let m: Vec<_> = members
            .iter()
            .filter(|m| {
                m.username.to_lowercase() == clean
                    || m.global_name
                        .as_ref()
                        .is_some_and(|g| g.to_lowercase() == clean)
                    || m.nick.as_ref().is_some_and(|n| n.to_lowercase() == clean)
            })
            .collect();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id, "1");
    }

    #[test]
    fn resolve_member_bare_id_passthrough() {
        // Emulates resolve_member's numeric passthrough predicate.
        let name = "123456789012345678";
        assert!(name.chars().all(|c| c.is_ascii_digit()));
        // Non-numeric falls through to the member-list lookup.
        let non_numeric = "alice";
        assert!(!non_numeric.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn member_match_global_name_and_nick() {
        let members = [
            discord_core::types::Member {
                id: "1".into(),
                username: "alice".into(),
                global_name: Some("Alice".into()),
                nick: None,
                joined_at: None,
                bot: false,
            },
            discord_core::types::Member {
                id: "2".into(),
                username: "bob".into(),
                global_name: None,
                nick: Some("Bob".into()),
                joined_at: None,
                bot: false,
            },
        ];
        // global_name match.
        let clean = "alice";
        let m: Vec<_> = members
            .iter()
            .filter(|m| {
                m.username.to_lowercase() == clean
                    || m.global_name
                        .as_ref()
                        .is_some_and(|g| g.to_lowercase() == clean)
                    || m.nick.as_ref().is_some_and(|n| n.to_lowercase() == clean)
            })
            .collect();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id, "1");
        // nick match.
        let clean = "bob";
        let m: Vec<_> = members
            .iter()
            .filter(|m| {
                m.username.to_lowercase() == clean
                    || m.global_name
                        .as_ref()
                        .is_some_and(|g| g.to_lowercase() == clean)
                    || m.nick.as_ref().is_some_and(|n| n.to_lowercase() == clean)
            })
            .collect();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id, "2");
    }

    #[test]
    fn member_not_found_yields_empty() {
        let members: Vec<discord_core::types::Member> = vec![];
        let clean = "carol";
        let m: Vec<_> = members
            .iter()
            .filter(|m| {
                m.username.to_lowercase() == clean
                    || m.global_name
                        .as_ref()
                        .is_some_and(|g| g.to_lowercase() == clean)
                    || m.nick.as_ref().is_some_and(|n| n.to_lowercase() == clean)
            })
            .collect();
        assert!(m.is_empty());
    }

    #[test]
    fn emoji_match_name_or_id() {
        let emojis = [
            discord_user::types::GuildEmoji {
                id: "10".into(),
                name: Some("party".into()),
                roles: vec![],
                user: None,
                require_colons: true,
                managed: false,
                animated: false,
                available: true,
            },
            discord_user::types::GuildEmoji {
                id: "20".into(),
                name: Some("sad".into()),
                roles: vec![],
                user: None,
                require_colons: true,
                managed: false,
                animated: false,
                available: true,
            },
        ];
        // ID match.
        assert!(emojis.iter().any(|e| e.id == "20"));
        // Name match (strip :).
        let needle = strip_emoji(":party:");
        let m: Vec<_> = emojis
            .iter()
            .filter(|e| e.name.as_deref().is_some_and(|n| n == needle))
            .collect();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id, "10");
    }
}
