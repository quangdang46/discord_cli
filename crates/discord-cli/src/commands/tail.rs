//! `dc tail <CHANNEL>` — gateway live message follow (invisible presence).
//!
//! Uses `discord-user-rs`'s `DiscordUser` (auto-reconnect, RESUME/fresh-ID by
//! close code, auto-fetch build number). `--once` fetches history and exits.

use std::process::ExitCode;

use discord_core::output::{self, exit};

use super::dc::{classify, DcCtx};

/// REST auth preflight before opening the gateway.
///
/// The gateway auto-reconnects indefinitely (4004 auth failures are fatal
/// per-close but the loop retries), so an invalid token would look "live"
/// while silently dropping every event. `GET /users/@me` surfaces bad
/// credentials immediately with a nonzero exit.
async fn auth_preflight(token: &str) -> Result<(), ExitCode> {
    let mut api = discord_core::client::ApiClient::with_token(token);
    match api.get_me().await {
        Ok(_) => Ok(()),
        Err(e) => {
            let is_auth = e.chain().any(|c| {
                matches!(
                    c.downcast_ref::<discord_user::DiscordError>(),
                    Some(
                        discord_user::DiscordError::InvalidToken
                            | discord_user::DiscordError::AuthenticationFailed
                    )
                )
            });
            let (code, ec) = if is_auth {
                ("AuthError", exit::ERROR)
            } else {
                ("ApiError", classify(&e))
            };
            Err(ExitCode::from(output::emit_error(
                code,
                &format!("token preflight failed: {e}"),
                ec,
            )))
        }
    }
}

/// Map the configured presence string to a gateway UserStatus.
/// Defaults to Invisible (stealth posture; mrarfarf 3-layer coercion).
fn configured_status() -> discord_user::UserStatus {
    match discord_core::config::configured_presence().as_str() {
        "online" => discord_user::UserStatus::Online,
        "idle" => discord_user::UserStatus::Idle,
        "dnd" => discord_user::UserStatus::DoNotDisturb,
        _ => discord_user::UserStatus::Invisible,
    }
}

/// `dc tail <CHANNEL> [--once]` — stream new messages.
pub async fn dc_tail(ctx: &DcCtx, channel_id: &str, once: bool) -> ExitCode {
    let token = match discord_core::config::resolve_token(ctx.token.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            return ExitCode::from(output::emit_error("AuthError", &e.to_string(), exit::ERROR))
        }
    };

    // Resolve a channel NAME → ID first (e.g. "general" → snowflake) so the
    // gateway filter matches. A raw numeric ID passes through unchanged.
    let resolved_id = if channel_id.chars().all(|c| c.is_ascii_digit()) {
        channel_id.to_string()
    } else {
        let mut api = discord_core::client::ApiClient::with_token(token.clone());
        match super::dc::resolve_channel_id(&mut api, channel_id).await {
            Ok(id) => id,
            Err(code) => return code,
        }
    };

    if let Err(code) = auth_preflight(&token).await {
        return code;
    }

    let mut client = discord_user::DiscordUser::new(token.clone()).with_status(configured_status()); // stealth default invisible

    if let Err(e) = client.init().await {
        return ExitCode::from(output::emit_error(
            "GatewayError",
            &e.to_string(),
            exit::ERROR,
        ));
    }

    let target = resolved_id;
    let _sub = client
        .on_message_create(move |event| {
            // Only stream from the requested channel (or all if no filter).
            if !target.is_empty() && event.message.channel_id.as_str() != target {
                return;
            }
            // JSONL line: timestamp, author, content — agent-friendly.
            let author = event.message.author.username.clone();
            let content = event.message.content.clone();
            let ts = chrono::Utc::now().to_rfc3339();
            let line = serde_json::json!({
                "type": "message",
                "timestamp": ts,
                "channel_id": event.message.channel_id.to_string(),
                "author": author,
                "content": content,
            });
            output::out_line(&serde_json::to_string(&line).unwrap_or_default());
        })
        .await;

    if once {
        // --once: brief listen then exit (history fetch is elsewhere).
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let _ = client.disconnect().await;
        return ExitCode::from(exit::OK);
    }

    // Stay alive until ctrl-c.
    tokio::signal::ctrl_c().await.ok();
    let _ = client.disconnect().await;
    ExitCode::from(exit::OK)
}

/// `dc watch [--channel C] [--keyword K] [--typing]` — long-running JSONL
/// stream for agents. Streams MESSAGE_CREATE as JSONL with optional
/// channel/keyword filters; with `--typing` also emits TYPING_START events.
pub async fn dc_watch(
    ctx: &DcCtx,
    channel: Option<&str>,
    keyword: Option<&str>,
    typing: bool,
) -> ExitCode {
    let token = match discord_core::config::resolve_token(ctx.token.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            return ExitCode::from(output::emit_error("AuthError", &e.to_string(), exit::ERROR))
        }
    };

    if let Err(code) = auth_preflight(&token).await {
        return code;
    }

    let mut client = discord_user::DiscordUser::new(token.clone()).with_status(configured_status());

    if let Err(e) = client.init().await {
        return ExitCode::from(output::emit_error(
            "GatewayError",
            &e.to_string(),
            exit::ERROR,
        ));
    }

    // Resolve a channel NAME → ID first (gateway compares against snowflake).
    let target_ch = match channel {
        Some(ch) if !ch.chars().all(|c| c.is_ascii_digit()) => {
            let mut api = discord_core::client::ApiClient::with_token(token.clone());
            match super::dc::resolve_channel_id(&mut api, ch).await {
                Ok(id) => Some(id),
                Err(code) => return code,
            }
        }
        other => other.map(|s| s.to_string()),
    };
    let target_kw = keyword.map(|s| s.to_lowercase());
    // Cache own user id once (skip self typing events).
    let me_id = discord_core::client::ApiClient::with_token(token.clone())
        .get_me()
        .await
        .ok()
        .map(|me| me.id);
    let msg_ch = target_ch.clone();
    let _sub = client
        .on_message_create(move |event| {
            if let Some(c) = &msg_ch {
                if event.message.channel_id.as_str() != c {
                    return;
                }
            }
            let content = event.message.content.clone();
            if let Some(kw) = &target_kw {
                if !content.to_lowercase().contains(kw) {
                    return;
                }
            }
            let line = serde_json::json!({
                "type": "message",
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "channel_id": event.message.channel_id.to_string(),
                "author": event.message.author.username,
                "content": content,
            });
            output::out_line(&serde_json::to_string(&line).unwrap_or_default());
        })
        .await;

    // Optional typing-event stream (F2b): emit TYPING_START as JSONL,
    // filtered to the target channel (empty = all) and skipping self.
    if typing {
        let tch = target_ch.clone();
        let mid = me_id.clone();
        let _tsub = client
            .on_typing_start(move |event| {
                if let Some(c) = &tch {
                    if event.channel_id != *c {
                        return;
                    }
                }
                if let Some(me) = &mid {
                    if &event.user_id == me {
                        return;
                    }
                }
                let line = serde_json::json!({
                    "type": "typing",
                    "channel_id": event.channel_id,
                    "user_id": event.user_id,
                    "timestamp": event.timestamp,
                });
                output::out_line(&serde_json::to_string(&line).unwrap_or_default());
            })
            .await;
    }

    tokio::signal::ctrl_c().await.ok();
    let _ = client.disconnect().await;
    ExitCode::from(exit::OK)
}
