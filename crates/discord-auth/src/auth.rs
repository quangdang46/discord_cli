//! Token auth: auto-detect (LevelDB scan), paste, keyring, device_id.
//!
//! Ported from jackwener `auth.py` (Apache-2.0, `.tmp/`) + discord-cli-rs
//! `auth/` (MIT).

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};

/// Token regexes (jackwener `_TOKEN_PATTERNS`).
const TOKEN_REGEX: &str = r#"[\w-]{24,}\.[\w-]{6}\.[\w-]{27,}|mfa\.[\w-]{84}"#;

/// LevelDB dirs to scan per-OS (jackwener `_get_search_paths`).
fn search_paths() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    // Used by macOS/Linux search paths.
    #[allow(unused_variables)]
    let home = dirs::home_dir().unwrap_or_default();

    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let appdata = PathBuf::from(appdata);
            for (name, sub) in [
                ("Discord App", "discord"),
                ("Discord PTB", "discordptb"),
                ("Discord Canary", "discordcanary"),
            ] {
                out.push((name.into(), appdata.join(sub).join("Local Storage/leveldb")));
            }
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            for (name, sub) in [
                ("Chrome", "Google/Chrome/User Data/Default"),
                ("Brave", "BraveSoftware/Brave-Browser/User Data/Default"),
                ("Edge", "Microsoft/Edge/User Data/Default"),
            ] {
                out.push((name.into(), local.join(sub).join("Local Storage/leveldb")));
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let app = home.join("Library/Application Support");
        for (name, sub) in [
            ("Discord App", "discord"),
            ("Discord PTB", "discordptb"),
            ("Discord Canary", "discordcanary"),
            ("Chrome", "Google/Chrome/Default"),
            ("Brave", "BraveSoftware/Brave-Browser/Default"),
            ("Edge", "Microsoft Edge/Default"),
        ] {
            out.push((name.into(), app.join(sub).join("Local Storage/leveldb")));
        }
    }

    #[cfg(target_os = "linux")]
    {
        let config = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".config"));
        for (name, sub) in [
            ("Discord App", "discord"),
            ("Discord PTB", "discordptb"),
            ("Discord Canary", "discordcanary"),
            ("Chrome", "google-chrome/Default"),
        ] {
            out.push((name.into(), config.join(sub).join("Local Storage/leveldb")));
        }
    }

    out.into_iter().filter(|(_, p)| p.exists()).collect()
}

/// Find Discord tokens by scanning LevelDB files in known locations.
/// Returns deduplicated (source, token) pairs.
pub fn find_tokens() -> Vec<(String, String)> {
    let re = match regex::Regex::new(TOKEN_REGEX) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (source, db) in search_paths() {
        let entries = match std::fs::read_dir(&db) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_ldb = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e == "ldb" || e == "log")
                .unwrap_or(false);
            if !is_ldb {
                continue;
            }
            let data = match std::fs::read(&path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            // LevelDB files are binary; scan raw bytes lossily.
            let text = String::from_utf8_lossy(&data);
            for cap in re.captures_iter(&text) {
                if let Some(m) = cap.get(0) {
                    let token = m.as_str().to_string();
                    if seen.insert(token.clone()) {
                        out.push((source.clone(), token));
                    }
                }
            }
        }
    }
    out
}

/// Write/upsert `DISCORD_TOKEN=<token>` into `./.env` (jackwener save_token_to_env).
pub fn save_token_to_env(token: &str, env_path: Option<&PathBuf>) -> Result<()> {
    let path = env_path.cloned().unwrap_or_else(|| PathBuf::from(".env"));
    let existing = if path.exists() {
        std::fs::read_to_string(&path).unwrap_or_default()
    } else {
        String::new()
    };
    let mut found = false;
    let mut lines: Vec<String> = existing
        .lines()
        .map(|l| {
            if l.starts_with("DISCORD_TOKEN=") {
                found = true;
                format!("DISCORD_TOKEN={token}")
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        lines.push(format!("DISCORD_TOKEN={token}"));
    }
    std::fs::write(&path, lines.join("\n") + "\n").context("write .env")?;
    // A file holding a live token must not be world-readable (Unix).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .context("chmod 0600 token file")?;
    }
    if env_path.is_none() {
        eprintln!(
            "warning: DISCORD_TOKEN written to ./.env in plaintext — prefer OS keyring storage"
        );
    }
    Ok(())
}

/// Save a token using the preferred storage: OS keyring first, `.env` fallback.
///
/// Keyring is preferred because `.env` is plaintext and easy to commit by
/// accident; when no keyring backend is available we still persist to `.env`
/// (chmod 0600) so `--save` never silently drops the token.
pub fn save_token(token: &str, env_path: Option<&PathBuf>) -> Result<()> {
    match discord_core::config::save_token_keyring(token) {
        Ok(()) => Ok(()),
        Err(_) => save_token_to_env(token, env_path),
    }
}

/// Validate a token against `GET /users/@me` via discord-core.
pub async fn validate_token(token: &str) -> Result<bool> {
    let mut client = discord_core::client::ApiClient::with_token(token);
    client.validate().await
}

/// Manual paste flow (M5.2): prompt for token, validate, save.
pub async fn auth_paste(save: bool) -> Result<String> {
    use std::io::Write;
    print!("Paste Discord token: ");
    std::io::stdout().flush().ok();
    let mut token = String::new();
    std::io::stdin()
        .read_line(&mut token)
        .context("read token")?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(anyhow!("empty token"));
    }
    if !validate_token(&token).await? {
        return Err(anyhow!("invalid token"));
    }
    if save {
        save_token(&token, None)?;
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regex_matches_token_shape() {
        let re = regex::Regex::new(TOKEN_REGEX).unwrap();
        // Real Discord token format: <24+ base64 id>.<6 timestamp>.<27+ hmac>
        // Build a token-shaped string programmatically (not a real token —
        // avoid tripping GitHub secret scanning).
        let part1 = "A".repeat(26);
        let part2 = "B".repeat(6); // timestamp is exactly 6 chars
        let part3 = "C".repeat(30);
        let fake = format!("{}.{}.{}", part1, part2, part3);
        assert!(re.is_match(&fake), "should match user-token shape");
        // mfa token: exactly 84 chars after "mfa."
        let mfa_body = "a".repeat(84);
        assert!(re.is_match(&format!("mfa.{}", mfa_body)));
        // Too short — no match
        assert!(!re.is_match("short.token.abc"));
    }

    #[test]
    fn env_upsert_works() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("discord-env-test-{}.env", std::process::id()));
        let _ = std::fs::remove_file(&path);
        save_token_to_env("tok1", Some(&path)).unwrap();
        save_token_to_env("tok2", Some(&path)).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            content
                .lines()
                .filter(|l| l.starts_with("DISCORD_TOKEN="))
                .count(),
            1
        );
        assert!(content.contains("DISCORD_TOKEN=tok2"));
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod debug_tests {
    #[test]
    fn debug_regex() {
        let re = regex::Regex::new(super::TOKEN_REGEX).unwrap();
        let part1 = "A".repeat(26);
        let part2 = "B".repeat(8);
        let part3 = "C".repeat(30);
        let fake = format!("{}.{}.{}", part1, part2, part3);
        eprintln!("fake: [{}]", &fake[..20]);
        eprintln!("p1 match: {}", re.is_match(&part1));
        eprintln!("full match: {}", re.is_match(&fake));
    }
}
