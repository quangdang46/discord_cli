//! discord-db: SQLite archive for Discord messages.

pub mod attachments;
pub mod db;

/// Re-exported so downstream crates can name the connection type without a
/// direct rusqlite dep.
pub use rusqlite::Connection;

/// A row in the `messages` table.
#[derive(Debug, Clone)]
pub struct MessageRow {
    pub id: String,
    pub channel_id: String,
    pub guild_id: Option<String>,
    pub author_id: String,
    pub author_name: String,
    pub content: String,
    /// UTC RFC3339 string.
    pub timestamp: String,
    pub edited: bool,
    pub reaction_count: u32,
}

/// A full-text search hit (FTS5 join result).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHit {
    pub id: String,
    pub channel_id: String,
    pub channel_name: String,
    pub guild_name: String,
    pub author_name: String,
    pub content: String,
    pub timestamp: String,
    pub rank: f64,
}

/// Per-channel message count (stats).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChannelStat {
    pub channel_name: String,
    pub guild_name: String,
    pub message_count: i64,
}

/// Top sender aggregate.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TopSender {
    pub author_name: String,
    pub message_count: i64,
}

/// Top-reacted message (F8: hottest messages by reaction_count).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TopReaction {
    pub message_id: String,
    pub channel_name: String,
    pub guild_name: String,
    pub author_name: String,
    pub content: String,
    pub reaction_count: i64,
    pub timestamp: String,
}

/// Export row (id, author, content, timestamp).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExportRow {
    pub id: String,
    pub author_name: String,
    pub content: String,
    pub timestamp: String,
}

/// Per-channel message count for today (today command).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TodayStat {
    pub channel_name: String,
    pub guild_name: String,
    pub message_count: i64,
}

/// One timeline bucket (day `YYYY-MM-DD` or hour `HH:00`).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TimelineBucket {
    pub bucket: String,
    pub count: i64,
}
