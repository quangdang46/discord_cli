//! Local offline queries over the SQLite archive (plan §9).
//! `search`, `recent`, `stats`, `top`, `today`, `timeline` — top-level
//! (not under `dc`).

use std::process::ExitCode;

use discord_core::config;
use discord_core::output::{self, exit, Format};
use discord_db::db as ddb;

use super::download::since_cutoff;

/// `search <KEYWORD> [-c CH] [--author A] [--since S]` — FTS5 search of
/// local archive. `--author`/`--since` are post-hoc filters on the hits
/// (the FTS5 query is native).
pub fn cmd_search(
    query: &str,
    channel: Option<&str>,
    author: Option<&str>,
    since: Option<&str>,
    limit: usize,
    format: Format,
) -> ExitCode {
    let db_path = match config::db_path() {
        Ok(p) => p,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    let conn = match ddb::open(db_path.to_str().unwrap_or("discord.db")) {
        Ok(c) => c,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    // Basic per-channel filter is applied post-hoc (query is FTS5-native).
    match ddb::search_messages(&conn, query, limit as i64) {
        Ok(mut hits) => {
            if let Some(ch) = channel {
                hits.retain(|h| h.channel_name.to_lowercase().contains(&ch.to_lowercase()));
            }
            if let Some(a) = author {
                let a = a.to_lowercase();
                hits.retain(|h| h.author_name.to_lowercase().contains(&a));
            }
            if let Some(s) = since {
                // RFC3339 string compare: stored timestamps are UTC `Z` strings,
                // the naive-UTC cutoff is lexically comparable (download.rs).
                match since_cutoff(s) {
                    Some(cutoff) => hits.retain(|h| h.timestamp >= cutoff),
                    None => {
                        return ExitCode::from(output::emit_error(
                            "UsageError",
                            &format!("invalid --since: \"{s}\" (use 12h|30d|YYYY-MM-DD)"),
                            exit::USAGE,
                        ))
                    }
                }
            }
            let _ = output::emit(&hits, format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR)),
    }
}

/// `recent [-c CH] [--hours N] [--since S]` — newest stored messages.
/// `--since` is a superset of `--hours` (both are `timestamp >= cutoff`).
pub fn cmd_recent(
    channel: Option<&str>,
    hours: Option<i64>,
    since: Option<&str>,
    limit: usize,
    format: Format,
) -> ExitCode {
    let db_path = match config::db_path() {
        Ok(p) => p,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    let conn = match ddb::open(db_path.to_str().unwrap_or("discord.db")) {
        Ok(c) => c,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    // SQL-level `--hours` (recent_messages) + post-hoc `--since` filter.
    let mut hits = match ddb::recent_messages(&conn, channel, hours, limit as i64) {
        Ok(h) => h,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    if let Some(s) = since {
        // RFC3339 string compare: stored timestamps are UTC `Z` strings,
        // the naive-UTC cutoff is lexically comparable (download.rs).
        match since_cutoff(s) {
            Some(cutoff) => hits.retain(|h| h.timestamp >= cutoff),
            None => {
                return ExitCode::from(output::emit_error(
                    "UsageError",
                    &format!("invalid --since: \"{s}\" (use 12h|30d|YYYY-MM-DD)"),
                    exit::USAGE,
                ))
            }
        }
    }
    let _ = output::emit(&hits, format);
    ExitCode::from(exit::OK)
}

/// `today` — per-channel message counts since 00:00 local time.
pub fn cmd_today(format: Format) -> ExitCode {
    let db_path = match config::db_path() {
        Ok(p) => p,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    let conn = match ddb::open(db_path.to_str().unwrap_or("discord.db")) {
        Ok(c) => c,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    // Local midnight expressed in UTC — stored timestamps are RFC3339 with
    // `+00:00`, so the cutoff must carry the real UTC instant of local
    // midnight (e.g. `2026-08-09T17:00:00Z` for +07), not the naive local
    // clock time, or the lexicographic compare mis-cuts the local day.
    // Local midnight as a UTC instant: take the current local offset
    // (e.g. +07) and subtract it from the naive local midnight, so the
    // cutoff is `2026-08-09T17:00:00Z` — not `2026-08-10T00:00:00` — and
    // matches stored RFC3339 (`+00:00`) timestamps lexicographically.
    let cutoff = chrono::Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|local_midnight| {
            let offset_min = chrono::Local::now().offset().local_minus_utc();
            (local_midnight - chrono::Duration::minutes(offset_min as i64))
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string()
        });
    match cutoff {
        Some(c) => match ddb::today_messages(&conn, &c, 50) {
            Ok(stats) => {
                let _ = output::emit(&stats, format);
                ExitCode::from(exit::OK)
            }
            Err(e) => ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR)),
        },
        None => ExitCode::from(output::emit_error(
            "DbError",
            "failed to compute local date",
            exit::ERROR,
        )),
    }
}

/// `timeline [--by day|hour]` — message volume per day or per hour.
/// Text/TTY renders proportional ASCII bars; `--json` gives plain buckets.
pub fn cmd_timeline(by: &str, format: Format) -> ExitCode {
    let db_path = match config::db_path() {
        Ok(p) => p,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    let conn = match ddb::open(db_path.to_str().unwrap_or("discord.db")) {
        Ok(c) => c,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    match ddb::timeline(&conn, by) {
        Ok(buckets) => {
            if format == Format::Rich {
                // ASCII bar chart: bar width proportional to count/max.
                let max = buckets.iter().map(|b| b.count).max().unwrap_or(0);
                const WIDTH: f64 = 24.0;
                for b in &buckets {
                    let bar = if max == 0 {
                        String::new()
                    } else {
                        "█".repeat(((b.count as f64 / max as f64) * WIDTH).round() as usize)
                    };
                    println!("{} {bar} {}", b.bucket, b.count);
                }
            } else {
                let _ = output::emit(&buckets, format);
            }
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR)),
    }
}

/// `stats` — per-channel message counts.
pub fn cmd_stats(format: Format) -> ExitCode {
    let db_path = match config::db_path() {
        Ok(p) => p,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    let conn = match ddb::open(db_path.to_str().unwrap_or("discord.db")) {
        Ok(c) => c,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    match ddb::channel_stats(&conn) {
        Ok(stats) => {
            let _ = output::emit(&stats, format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR)),
    }
}

/// `export <CHANNEL> [-f text|json] [-o FILE]` — export stored messages.
pub fn cmd_export(channel: &str, as_json: bool, output: Option<&str>, format: Format) -> ExitCode {
    let db_path = match config::db_path() {
        Ok(p) => p,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    let conn = match ddb::open(db_path.to_str().unwrap_or("discord.db")) {
        Ok(c) => c,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    // Resolve channel name → ID (channel may be passed as name, e.g.
    // "chit-chat" for "chit-chat┊💬"). Falls back to treating input as ID.
    let channel_id = match ddb::find_channel_id(&conn, channel) {
        Ok(Some(id)) => id,
        _ => channel.to_string(),
    };
    match ddb::channel_messages(&conn, &channel_id, 1_000_000) {
        Ok(rows) => {
            let text = if as_json {
                serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into())
            } else {
                rows.iter()
                    .map(|r| format!("[{}] {}: {}", r.timestamp, r.author_name, r.content))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            if let Some(path) = output {
                match std::fs::write(path, &text) {
                    Ok(_) => {
                        let data = serde_json::json!({ "exported": true, "file": path, "messages": rows.len() });
                        let _ = output::emit(&data, format);
                        ExitCode::from(exit::OK)
                    }
                    Err(e) => {
                        ExitCode::from(output::emit_error("IOError", &e.to_string(), exit::ERROR))
                    }
                }
            } else {
                println!("{}", text);
                ExitCode::from(exit::OK)
            }
        }
        Err(e) => ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR)),
    }
}

/// `purge <CHANNEL> [-y]` — delete stored messages for a channel.
pub fn cmd_purge(channel: &str, yes: bool, format: Format) -> ExitCode {
    if !yes {
        eprintln!("This will delete stored messages for channel \"{channel}\". Add -y to proceed.");
        return ExitCode::from(exit::USAGE);
    }
    let db_path = match config::db_path() {
        Ok(p) => p,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    let conn = match ddb::open(db_path.to_str().unwrap_or("discord.db")) {
        Ok(c) => c,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    // Resolve channel name → ID like export.
    let channel_id = match ddb::find_channel_id(&conn, channel) {
        Ok(Some(id)) => id,
        _ => channel.to_string(),
    };
    match ddb::purge_channel(&conn, &channel_id) {
        Ok(n) => {
            let data =
                serde_json::json!({ "purged": true, "channel": channel, "messages_deleted": n });
            let _ = output::emit(&data, format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR)),
    }
}

/// `top [-c CH]` — top senders.
pub fn cmd_top(channel: Option<&str>, limit: usize, format: Format) -> ExitCode {
    let db_path = match config::db_path() {
        Ok(p) => p,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    let conn = match ddb::open(db_path.to_str().unwrap_or("discord.db")) {
        Ok(c) => c,
        Err(e) => {
            return ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR))
        }
    };
    match ddb::top_senders(&conn, channel, limit as i64) {
        Ok(senders) => {
            let _ = output::emit(&senders, format);
            ExitCode::from(exit::OK)
        }
        Err(e) => ExitCode::from(output::emit_error("DbError", &e.to_string(), exit::ERROR)),
    }
}
