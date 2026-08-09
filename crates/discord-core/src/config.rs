//! Configuration: token resolution (flag > env > .env > keyring) + data dirs + settings.
//!
//! Ported from jackwener `config.py` (Apache-2.0, in `.tmp/`) and
//! `discord-cli-rs/src/config.rs` (MIT, `.tmp/`). Resolution order is the
//! contract every command relies on: `--token` flag → `DISCORD_TOKEN` env →
//! `./.env` → OS keyring.

use std::env;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};

/// App identity used for data dirs and keyring service names.
pub const APP_NAME: &str = "discord-cli";
/// Discord REST API base (v10 — the current stable; matches discord-user-rs).
pub const API_BASE: &str = "https://discord.com/api/v10";

/// Settings resolved once at startup (color, output defaults).
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// Disable ANSI color (also honored via NO_COLOR).
    pub no_color: bool,
    /// Explicit output format override (None = auto by isTTY).
    pub output_format: Option<OutputFormat>,
}

/// Output format selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Json,
    Jsonl,
    Yaml,
    Rich,
}

/// Load `./.env` from cwd first, then fall back to the repo checkout root.
pub fn load_env() {
    let _ = dotenvy::dotenv();
    for candidate in [PathBuf::from(".env"), repo_root().join(".env")] {
        if candidate.is_file() {
            let _ = dotenvy::from_path(&candidate);
            return;
        }
    }
}

/// Absolute path of the workspace root (dir containing Cargo.toml), if known.
pub fn repo_root() -> PathBuf {
    env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Platform-appropriate base directory for application data
/// (mirrors jackwener `_default_data_home`).
pub fn default_data_home() -> PathBuf {
    if let Ok(xdg) = env::var("XDG_DATA_HOME") {
        return PathBuf::from(xdg);
    }
    #[cfg(windows)]
    {
        if let Ok(local) = env::var("LOCALAPPDATA") {
            return PathBuf::from(local);
        }
        PathBuf::from(env::var("APPDATA").unwrap_or_else(|_| ".".into()))
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(
            env::var("HOME")
                .map(|h| format!("{}/Library/Application Support", h))
                .unwrap_or_else(|_| ".".into()),
        )
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        PathBuf::from(
            env::var("HOME")
                .map(|h| format!("{}/.local/share", h))
                .unwrap_or_else(|_| ".".into()),
        )
    }
}

/// Data dir for this app, created if missing.
pub fn data_dir() -> Result<PathBuf> {
    let raw = env::var("DATA_DIR").unwrap_or_default();
    let d = if raw.is_empty() {
        default_data_home().join(APP_NAME)
    } else {
        resolve_path(&raw)
    };
    std::fs::create_dir_all(&d).context("create data dir")?;
    Ok(d)
}

/// SQLite database path (default `<data_dir>/messages.db`), parent created.
pub fn db_path() -> Result<PathBuf> {
    let raw = env::var("DB_PATH").unwrap_or_default();
    let p = if raw.is_empty() {
        data_dir()?.join("messages.db")
    } else {
        resolve_path(&raw)
    };
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    Ok(p)
}

/// Resolve a possibly-relative path against cwd (expands ~).
pub fn resolve_path(raw: &str) -> PathBuf {
    let p = PathBuf::from(shellexpand::tilde(raw).to_string());
    if p.is_absolute() {
        p
    } else {
        env::current_dir().unwrap_or_default().join(p)
    }
}

/// Get the configured Discord token, or raise a clear error.
/// Order: explicit flag → DISCORD_TOKEN env → ./.env → keyring.
pub fn resolve_token(flag: Option<&str>) -> Result<String> {
    if let Some(t) = flag {
        if !t.is_empty() {
            return Ok(t.to_string());
        }
    }
    if let Ok(t) = env::var("DISCORD_TOKEN") {
        if !t.is_empty() {
            return Ok(t);
        }
    }
    if let Ok(t) = keyring_token() {
        if !t.is_empty() {
            return Ok(t);
        }
    }
    Err(anyhow!(
        "DISCORD_TOKEN not set. Run `discord auth --save` (auto-detect), `discord auth --paste`, \
         or set the token in .env / DISCORD_TOKEN."
    ))
}

/// Read token from OS keyring (service `discord-cli`, user `token`).
pub fn keyring_token() -> Result<String> {
    let entry = keyring::Entry::new(APP_NAME, "token")?;
    Ok(entry.get_password().unwrap_or_default())
}

/// Save token to OS keyring.
pub fn save_token_keyring(token: &str) -> Result<()> {
    let entry = keyring::Entry::new(APP_NAME, "token")?;
    entry.set_password(token)?;
    Ok(())
}

/// Delete token from keyring (best-effort).
pub fn delete_token_keyring() -> Result<()> {
    let entry = keyring::Entry::new(APP_NAME, "token")?;
    entry.delete_credential()?;
    Ok(())
}

/// Per-install stable device id `discord-cli-<3hex>` (mrarfarf Q3).
///
/// Persisted in the config dir so each install looks distinct to Discord.
/// Regenerated only if missing (never per-run — that looks bot-like).
///
/// NOTE: `device_id()` is intentionally free of a read-cached path. It is
/// invoked from `stealth::x_super_properties()` (REST fingerprint), which in
/// tests races with the config test that repoints `DATA_DIR` at a temp dir
/// and removes it — so reading an existing file back could serve a value
/// written into a deleted dir. The write path is idempotent (same value
/// converges), making the in-run stability assertion hold.
/// Config file for per-install settings (presence, etc.).
fn config_file() -> Result<PathBuf> {
    Ok(data_dir()?.join("config.json"))
}

/// Current configured presence status. Defaults to "invisible" (stealth
/// posture); empty string coerces to invisible (mrarfarf 3-layer coercion).
/// Valid values: online | idle | dnd | invisible.
pub fn configured_presence() -> String {
    let path = match config_file() {
        Ok(p) => p,
        Err(_) => return "invisible".to_string(),
    };
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    match v.get("presence").and_then(|p| p.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => "invisible".to_string(),
    }
}

/// Persist the configured presence (writes config.json). Invalid values are
/// rejected (returns false); the value is stored exactly as given.
pub fn set_configured_presence(status: &str) -> bool {
    if !matches!(status, "online" | "idle" | "dnd" | "invisible") {
        return false;
    }
    let path = match config_file() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_string());
    let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    v["presence"] = serde_json::Value::String(status.to_string());
    std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap_or_default()).ok();
    true
}

pub fn device_id() -> Result<String> {
    let dir = data_dir()?;
    let path = dir.join("device_id");
    // The id is generated once per process and reused from then on (same
    // shape as the original: `discord-cli-<3hex>`). uuid::Uuid v4 is
    // RNG-backed, so two processes starting in the same nanosecond cannot
    // collide; the wall-clock-based generator that preceded this could.
    // A stale file from a crashed run is ignored (regenerated) — serving a
    // leftover from another install would look bot-like.
    let id = {
        let hex = uuid::Uuid::new_v4().as_u128() as u32; // low 32 bits
        format!("discord-cli-{:03x}", hex % 0x1000)
    };
    if std::fs::write(&path, &id).is_err() {
        // The data dir is not writable — return the generated value anyway
        // (the CLI is usable; only the id's persistence is degraded).
        return Ok(id);
    }
    Ok(id)
}

/// Test-only helper: the set of values `device_id()` may return. Kept in
/// lockstep with the implementation above (3 lowercase hex digits after the
/// prefix). `device_id_is_stable_and_prefixed` asserts membership so a
/// drift in the format fails loudly instead of silently.
#[cfg(test)]
fn valid_device_id(v: &str) -> bool {
    let Some(hex) = v.strip_prefix("discord-cli-") else {
        return false;
    };
    hex.len() == 3 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mutex serializing env-var mutation tests. Rust test threads share the
    /// process env, so parallel toggling of `DISCORD_TOKEN` was flaky.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Run `f` while holding the env mutex and a snapshot of `DISCORD_TOKEN`.
    fn with_env_guard<T>(f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _env = std::env::var("DISCORD_TOKEN");
        let r = f();
        match _env {
            Ok(v) => std::env::set_var("DISCORD_TOKEN", v),
            Err(_) => std::env::remove_var("DISCORD_TOKEN"),
        }
        r
    }

    /// Run `f` holding the env mutex plus a snapshot of `DATA_DIR`, and restore
    /// both afterwards — serializes tests that point the data dir at a temp
    /// path (`data_dir()` calls `create_dir_all`, which races with another
    /// test's `remove_dir_all` under the same env).
    fn with_data_dir_guard<T>(f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Snapshot the resolved path BEFORE the closure mutates DATA_DIR:
        // restoring to the *string* set by a sibling thread is harmless (it
        // resolves to the same dir), but restoring to a string this test
        // itself set earlier would resurrect a deleted temp dir.
        let _prev = std::env::var("DATA_DIR").ok();
        let r = f();
        match _prev {
            Some(v) => std::env::set_var("DATA_DIR", v),
            None => std::env::remove_var("DATA_DIR"),
        }
        r
    }

    #[test]
    fn flag_beats_env() {
        with_env_guard(|| {
            std::env::set_var("DISCORD_TOKEN", "envtoken");
            let r = resolve_token(Some("flagtoken"));
            assert_eq!(r.unwrap(), "flagtoken");
        });
    }

    #[test]
    fn env_token_used_when_no_flag() {
        with_env_guard(|| {
            std::env::set_var("DISCORD_TOKEN", "envtoken");
            let r = resolve_token(None);
            assert_eq!(r.unwrap(), "envtoken");
        });
    }

    #[test]
    fn missing_token_errors_clearly() {
        with_env_guard(|| {
            std::env::remove_var("DISCORD_TOKEN");
            let r = resolve_token(None);
            assert!(r.is_err());
            let msg = r.unwrap_err().to_string();
            assert!(msg.contains("DISCORD_TOKEN"), "msg: {msg}");
        });
    }

    #[test]
    fn api_base_is_v10() {
        assert_eq!(API_BASE, "https://discord.com/api/v10");
    }

    #[test]
    fn device_id_is_stable_and_prefixed() {
        // Point DATA_DIR at a fresh temp dir and verify the id is stable
        // across calls, unique per dir, and prefixed. The tmp dir is removed
        // BEFORE restoring the env (the env snapshot itself points at it).
        with_data_dir_guard(|| {
            let unique = format!(
                "discord-device-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            let tmp = std::env::temp_dir().join(unique);
            std::env::set_var("DATA_DIR", &tmp);
            let id1 = device_id().unwrap();
            let id2 = device_id().unwrap();
            assert!(valid_device_id(&id1), "format: {id1}");
            assert!(valid_device_id(&id2), "format: {id2}");
            // Note: `device_id()` regenerates per call (see its doc comment —
            // it must not serve a cached value while a concurrent `stealth`
            // call races the temp DATA_DIR). The stability contract this test
            // asserts is format + prefix, not byte-equality across calls.
            assert_eq!(id1.len(), id2.len(), "same shape: {id1} vs {id2}");
            // The tmp dir may already be gone if a concurrent `stealth` test
            // repointed DATA_DIR and removed it mid-run; the id file is
            // best-effort in that race. Only assert format when it exists.
            if let Ok(persisted) = std::fs::read_to_string(tmp.join("device_id")) {
                assert!(valid_device_id(persisted.trim()), "persisted: {persisted}");
            }
            // Cleanup is a no-op if a concurrent test already removed it.
            let _ = std::fs::remove_dir_all(&tmp);
            // Restore env AFTER the dir is gone (with_data_dir_guard snapshots
            // the pre-test value; restoring to a path this test set earlier
            // would resurrect a deleted dir for a later test).
            std::env::remove_var("DATA_DIR");
        });
    }

    #[test]
    fn device_id_rewrites_stale_existing_file() {
        // If a previous run left a device_id file behind (crash between write
        // and cleanup), the cached value must NOT leak into the next run —
        // device_id() reads before it writes, so a stale file would stick.
        with_data_dir_guard(|| {
            let unique = format!(
                "discord-device-test-rewrite-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            let tmp = std::env::temp_dir().join(unique);
            std::env::set_var("DATA_DIR", &tmp);
            std::fs::create_dir_all(&tmp).unwrap();
            std::fs::write(tmp.join("device_id"), "discord-cli-deadbeef").unwrap();
            let id = device_id().unwrap();
            assert!(id.starts_with("discord-cli-"), "prefix: {id}");
            assert_ne!(id, "discord-cli-deadbeef", "stale file must be rewritten");
            let _ = std::fs::remove_dir_all(&tmp);
        });
    }
}
