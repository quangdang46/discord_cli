# Implementation-Ready Plan — 3 Feature Batches for discord_cli

Upgrade of `COMPREHENSIVEPLAN-DISCORD-FEATURE-BATCHES.md` to implementation
detail: per-file changes with exact signatures, MCP tool schemas, docs diffs,
tests, verify checklists. All reference file:lines verified against the
cloned repos + `discord-user-rs` 0.6.1 source.

**Verified crate facts** (from registry source, `discord-user-rs-0.6.1`):
- `Gateway::send_presence(status: UserStatus)` and `send_raw(payload)` EXIST
  (`gateway.rs:953,1051`) → F4 has no fallback needed.
- `CreateThreadRequest` has `public()/private()`, `auto_archive_duration`,
  `applied_tags`, `message` (`types/requests.rs:80-152`) → F5 incl. forum tags.
- `Invite` struct has `approximate_member_count/presence_count`
  (`types/guild.rs:534`) → F3 preview.
- `Route::JoinGuild{code}` / `LeaveGuild{guild_id}` / `TriggerTyping` /
  `CreateThread` / `CreateThreadFromMessage` / `GetActiveThreads` all exist.
- `UserStatus` enum: Online/Idle/DoNotDisturb/Invisible (`types/enums.rs:9`).
- `post_multipart(route, payload_json, Vec<CreateAttachment>)` exists
  (`client.rs:429`); `CreateAttachment{filename,data,mime_type,description}`
  (`types/requests.rs:47`).
- Gateway typed callbacks: `on_typing_start`, `on_presence_update`,
  `on_message_create` (`discord_user.rs` macro).
- `tokio-tungstenite` (rustls) already in crate tree for gateway.
- Workspace deps available: `mime_guess`, `rand`, `uuid`, `base64`, `chrono`.
- NOT in workspace: `rsa`, `qrcode`, `md-5` → must add for F7.
- NO rust-toolchain file; edition 2021 → **no rustls nightly API** → F9 = no
  JA3 without vendoring BoringSSL. Decision: F9 → documented no-op (see F9).

---

## BATCH 1 — Write-path depth

### F1. `send --file` (attachments)

**CLI** (`crates/discord-cli/src/commands/dc.rs`):
```rust
Send {
    channel: String,
    #[arg(long)] text: Option<String>,          // "-" = read stdin
    #[arg(long)] file: Vec<String>,             // repeatable
    #[arg(long)] reply: Option<String>,
    #[arg(long)] confirm: bool,
}
```
Require `text.is_some() || !file.is_empty()` (else usage exit 2).
`--text -` → read all stdin (Escape-Tech), trim trailing newline.

**core** (`crates/discord-core/src/client.rs`):
```rust
pub async fn send_message_with_files(
    &mut self,
    channel_id: &str,
    content: &str,
    reply_to: Option<&str>,
    attachments: Vec<discord_user::types::CreateAttachment>,
) -> Result<String>
```
- Build `payload_json` = existing `SendMessageRequest` serde_json::to_value
  (content/tts/flags/message_reference/mobile_network_type) + inject
  `attachments: [{id:"0"}]` (crate's post_multipart style, see client.rs:429-447).
- `inner.post_multipart(Route::CreateMessage{channel_id}, payload_json, atts)`.
- Read files in dc.rs: `tokio::fs::read`, `mime_guess::from_path` → mime_type,
  `Path::file_name` → filename; error → stderr + exit 7 (famasya
  attachment-fail convention).

**MCP** (`crates/discord-mcp/src/server.rs`):
```rust
pub struct SendParams { channel_id, content, #[serde(default)] reply_to,
                        #[serde(default)] files: Option<Vec<String>> } // paths
```
Tool `send_message` unchanged name; if `files` present → call
`send_message_with_files`; document paths are server-local (agent on same host).

**Docs**: README `send` row → `--file PATH`; AGENTS.md quickstart add
`discord send "general" --file ./report.pdf --confirm`; SCHEMA.md envelope
unchanged (returns message_id).

**Tests**:
- unit: payload_json has attachments array when files present; exit 7 on
  unreadable file.
- integration (fake-API): multipart body contains files[0] part + filename.

**Verify**: `cargo test`; `discord send <ch> --file <tmpfile> --confirm`
  appears with attachment in Discord; `--text -` pipes stdin.

---

### F2. Typing indicator

**core**:
```rust
pub async fn trigger_typing(&mut self, channel_id: &str) -> Result<()>
// POST Route::TriggerTyping, no body (crate route exists)
```

**CLI** (`dc.rs`):
```rust
Typing { channel: String }                       // one-shot
// Send gains: #[arg(long)] typing: bool         // fire, 100ms, then send
// Watch/Tail gain: #[arg(long)] typing: bool    // emit typing JSONL
```
`dc_typing`: resolve channel (existing resolve_channel_id), trigger, exit 0.

**tail.rs / watch**: register `on_typing_start` when `--typing`:
```rust
if event.channel_id != target { return; }       // always filter channel
if event.user_id == me_id { return; }           // skip self (need me_id cache)
println!(json!({"type":"typing","channel_id","user_id","timestamp"}));
```
`me_id` from `client.get_me()` once at startup (F2 dependency: small).

**MCP**: none (typing is noise; agents don't need it).

**Docs**: README `typing` row; `watch --typing` mention.

**Tests**: unit — JSONL shape; smoke — watch --typing emits on another
account's typing.

**Verify**: `typing <ch>` returns 0; typing appears in Discord (person typing);
`watch --typing` prints events.

---

### F3. Join / leave server

**core**:
```rust
pub async fn get_invite(&mut self, code: &str) -> Result<Invite>        // GET /invites/{code}
pub async fn accept_invite(&mut self, code: &str) -> Result<()>         // POST /invites/{code}, nil body
pub async fn leave_guild(&mut self, guild_id: &str) -> Result<()>       // DELETE /users/@me/guilds/{gid}
```
- `Route::JoinGuild{code}` (crate path = `invites/{code}` — check: route.rs:556
  maps JoinGuild AND DeleteInvite to `invites/{code}`; GET preview needs
  `Route::Custom(format!("invites/{code}"))` since crate has no GetInvite route
  → verify; fallback Custom).
- code extraction: `fn extract_invite_code(s: &str) -> Option<&str>` — strip
  `discord.gg/`, `discord.com/invite/`, `discordapp.com/invite/` prefixes,
  trim trailing `/`, non-alnum chars. Unit-testable.

**CLI**:
```rust
Join  { invite: String, #[arg(long)] confirm: bool }
Leave { guild: String, #[arg(long)] confirm: bool }
```
- join: extract code → get_invite preview → print
  `Joining "<guild.name>" (approx {members})?` → require --confirm → accept.
- leave: resolve_guild → print name → require --confirm → leave_guild.

**MCP**: `join_guild` (params: invite_code, confirm: bool) → returns guild
  name/members; `leave_guild` (params: guild_id, confirm: bool) → "left".

**Docs**: README join/leave rows; AGENTS.md note: invite accept requires
  --confirm; ToS warning.

**Tests**: unit — extract_invite_code (URL/plain/edge); core — accept uses nil
  body (fake-API).

**Verify**: join a test server w/ invite; leave it.

---

### F4. Presence / status

**core**: `Gateway::send_presence` already public — expose on DiscordUser if
not already:
```rust
// discord_user.rs in crate: check; if absent add:
pub async fn set_presence(&self, status: UserStatus) -> Result<()>
// calls gateway.send_presence(status)
```
(Verify at coding time; if DiscordUser lacks it, use `gateway.send_raw` with
`{"op":3,"d":{"since":0,"activities":[],"status":"...","afk":false}}`.)

**CLI**:
```rust
Presence { status: Option<String> }   // none = show configured default
// config.rs: presence: String = "invisible"  (mrarfarf 3-layer coercion:
//   default invisible; "" → invisible; runtime guard)
```
- No arg → print current config presence.
- Arg → validate enum, persist to config, then if a live gateway session
  exists → send_presence (F4 is standalone; tail/watch apply config on
  connect via with_status already).

**MCP**: `set_presence` (params: status enum) → "ok".

**Docs**: README presence row; config default invisible (already true).

**Tests**: unit — status parse/validation; config roundtrip.

**Verify**: `presence dnd` persists; new tail shows dnd (check via friend's
  view or presence event).

---

### F5. Thread create

**core**:
```rust
pub async fn create_thread(
    &mut self, channel_id: &str, name: &str,
    archive_minutes: Option<u32>,          // 60/1440/4320/10080
    starter: Option<&str>,                 // forum: required; standalone: optional
    applied_tags: Option<Vec<String>>,     // forum tags (crate supports)
) -> Result<ThreadResult>
pub async fn create_thread_from_message(
    &mut self, channel_id: &str, message_id: &str, name: &str,
    archive_minutes: Option<u32>,
) -> Result<ThreadResult>
```
- Forum detection: fetch channel type; if 15 (forum) → include `message`
  payload (starter or name — Escape-Tech defaults starter to name).
- `CreateThreadRequest::public(name)` + set fields; post via
  `Route::CreateThread{channel_id}` / `CreateThreadFromMessage`.
- `ThreadResult { id, name, channel_id, channel_type, parent_message_id }`.

**CLI**:
```rust
ThreadCreate {
    channel: String,
    #[arg(long)] name: String,
    #[arg(long)] message_id: Option<String>,
    #[arg(long)] text: Option<String>,
    #[arg(long)] archive: Option<u32>,
    #[arg(long)] tags: Option<String>,      // comma-separated
}
```
- resolve channel; branch on message_id; output discriminator
  `"type": forum_post|message_thread|standalone_thread` (Escape-Tech).

**MCP**: `create_thread` (params: channel_id, name, message_id?, text?,
  archive?) → JSON ThreadResult.

**Docs**: README thread-create row; AGENTS.md.

**Tests**: unit — 3 payload shapes; integration — forum post has message.

**Verify**: create standalone + message thread + forum post in test server.

---

## BATCH 2 — Archive & media depth

### F6. Attachment download (offline)

**DB** (`crates/discord-db/src/db.rs` migrate + new `attachments.rs`):
```sql
CREATE TABLE IF NOT EXISTS attachments (
  id TEXT PRIMARY KEY,           -- md5(msg_id|url) hex
  message_id TEXT NOT NULL REFERENCES messages(id),
  channel_id TEXT NOT NULL,
  url TEXT NOT NULL, filename TEXT NOT NULL,
  content_type TEXT, size INTEGER, local_path TEXT
);
CREATE INDEX idx_attachments_channel ON attachments(channel_id);
CREATE INDEX idx_attachments_local_path ON attachments(local_path);
```
Fns: `upsert_attachment`, `list_pending_attachments(filters)`, `mark_downloaded`.
`md5` — add `md-5` crate (or use sha2 if already present — verify; md5 simplest
for langkurt parity).

**Sync capture** (`sync.rs` + `RawMessage`):
- Extend `RawMessage` with `attachments: Option<Vec<RawAttachment>>` where
  `RawAttachment { url, filename, content_type, size }`.
- `row_from_msg` → also `upsert_attachment` per attachment (INSERT OR IGNORE).
- FTS unchanged (content only).

**Snowflake cutoff** (`commands/download.rs` + helper):
```rust
fn parse_since(s: &str) -> Option<DateTime<Utc>>  // 30d/6m/1y/YYYY-MM-DD (langkurt timeutil)
fn time_to_snowflake(t: DateTime<Utc>) -> String  // ((ms - 1420070400000) << 22).max(0)
```

**CLI**:
```rust
Download {
    #[arg(long)] guild: Option<String>,
    #[arg(long)] channel: Option<String>,
    #[arg(long)] r#type: Option<String>,      // image|gif|video|all (validate)
    #[arg(long)] since: Option<String>,
    #[arg(long)] min_reactions: Option<i64>,
    #[arg(long)] limit: Option<i64>,
    #[arg(long)] out: Option<String>,          // default ~/.local/share/discord-cli/media
}
```
- Query: `list_pending_attachments` JOIN messages filters; type by
  content_type (gif exact, image !=gif, video LIKE 'video/%').
- Download loop: 200ms sleep between (langkurt download.go:77); naming
  `<msgID last 8>_<filename>`; skip if exists; partial-write cleanup.
- Progress → stderr `\r\x1b[K` (Escape-Tech), data summary → stdout JSON.
- CDN fetch: plain reqwest GET with browser UA (no auth needed).

**MCP**: `download_attachments` (params: channel, guild, media_type, limit,
  out_dir, since) — langkurt MCP tool mirror (no min-reactions in MCP).

**Docs**: README download row; AGENTS.md; SCHEMA.md attachments table.

**Tests**: unit — parse_since, time_to_snowflake, naming, filters; DB — upsert/
  list; integration — fake CDN.

**Verify**: sync a channel with attachments → download → files on disk with
  correct names; --since/--type filters work.

---

### F7. QR login (remote auth)

**Deps** (workspace Cargo.toml): `rsa = "0.9"`, `qrcode = "0.14"`,
  `md-5` (if F6 not yet), `tokio-tungstenite` already in tree (transitively —
  add explicit if needed).

**New module** `crates/discord-auth/src/qr.rs`:
```rust
pub async fn qr_login(save: bool) -> Result<String>   // returns token
```
Flow (discordo msg.go:32-307; mrarfarf qr_login.go):
1. Dial `wss://remote-auth-gateway.discord.gg/?v=2` with UA + browser headers.
2. Gen RSA-2048; send `{"op":"init","encoded_public_key":<b64 PKIX SPKI DER>}`.
3. On `nonce_proof` → OAEP-SHA256 decrypt (rand.Reader) →
   `{"op":"nonce_proof","nonce":<b64 RawURL>}`.
4. On `pending_remote_init` → print ASCII QR of
   `https://discord.com/ra/{fingerprint}` (qrcode crate to stderr);
   keep fingerprint.
5. On `pending_ticket` → decrypt user_payload → parse `discriminator:username`
   (4 `:` parts → [1],[3]) → print "Waiting for <username>...".
6. On `pending_login` → POST `https://discord.com/api/v9/users/@me/remote-auth/login`
   body `{"ticket":...}`, headers `X-Fingerprint` + `Referer: https://discord.com/login`
   (mrarfarf: `/ra/{fp}`) → decrypt `encrypted_token` → token.
7. On `cancel` → error exit. Heartbeat every hello interval.
- Save via existing auth save path if `--save`.

**CLI**: `auth --qr [--save]` → prints QR to stderr, token to stdout (agent
  mode: `--json` → `{"authenticated":true,"source":"qr"}`).

**Auto re-auth** (tail.rs/watch init; mrarfarf state.go:151-180):
- On `client.init()` error: if 401 REST or gateway close 4004 → stderr
  "token invalid or expired", call `qr_login(true)`, save, retry once.

**MCP**: none (interactive QR not agent-friendly).

**Docs**: README auth row → `--qr`; ToS risk section: QR uses login API
  (highest risk) — opt-in only; AGENTS.md note.

**Tests**: unit — RSA-OAEP roundtrip, base64 variants, user_payload parse,
  URL→code (shared F3 helper); integration — mock remote-auth WS (fake server
  implementing hello/init/nonce/pending seq) — hardest, mark as
  `#[ignore]` unless env set.

**Verify**: manual — `auth --qr` prints QR, phone scan, token saved; expired
  token → auto QR prompt.

---

### F8. Top reactions (archive analytics)

**DB**: add `CREATE INDEX idx_messages_reactions ON messages(reaction_count DESC)`
(migrate); `top_reacted(guild, channel, limit) -> Vec<{message_id, channel_id,
author, content, reaction_count, timestamp}>`.

**Sync capture** (`sync.rs` row_from_msg): extend `RawMessage` with
`reactions: Option<Vec<RawReaction>>` where `RawReaction { count: i32 }`;
`reaction_count = sum(count)` (langkurt upsertMsg sync.go:259-263).
- Note: gateway tail path (tail.rs) writes reaction_count 0 — leave (live).

**CLI**:
```rust
TopReactions { #[arg(long)] guild: Option<String>,
               #[arg(long)] channel: Option<String>,
               #[arg(long)] limit: Option<u32> }   // default 10
```
Query: WHERE reaction_count > 0 [+ filters] ORDER BY DESC LIMIT N. Output via
output::emit (JSON envelope) / human table.

**MCP**: `top_reactions` (params: guild?, channel?, limit?) → JSON list.

**Docs**: README `top-reactions` row (keep `top` = top senders — R2 resolved:
  new command, zero breakage); SCHEMA.md reactions.

**Tests**: DB unit — ordering/filters/empty; integration — sync populates count.

**Verify**: sync a hot channel → top-reactions ranks correctly.

---

## BATCH 3 — Stealth transport

### F9. TLS JA3 fingerprint spoof — DECISION: documented no-op (v1)

**Why**: no rust-toolchain file, edition 2021 → rustls nightly
`ClientHelloExt`/`CustomCertifiedKeyShares` unavailable on stable. The only
path is vendoring BoringSSL (boring-sys) as tls-client does — heavy build,
maintenance, supply-chain risk, marginal gain given our super-props +
launch_signature + is_fast_connect + device_id already shipped.

**Deliverable**: docs note in README (Stealth section) + AGENTS.md:
"TLS ClientHello fingerprint (JA3) not yet spoofed; optional future feature.
REST/gateway use rustls with Chrome 146 headers; X-Super-Properties and
launch_signature masking already active." Optionally:
- `--tls-chrome` flag reserved, returns "not implemented" (exit 2) — honest.
- No code change otherwise.

**Revisit gate**: if `boring` crate stabilizes a safe API or toolchain moves
to nightly, re-open.

**Tests**: none (doc-only).

---

## Cross-cutting (file-by-file)

| File | Change |
|---|---|
| `crates/discord-core/src/client.rs` | +F1/F2/F3/F5 methods; RawMessage extend (F6/F8); expose `set_presence` if needed (F4) |
| `crates/discord-core/src/types.rs` | +RawAttachment/RawReaction; Message maybe +reactions |
| `crates/discord-core/src/config.rs` | +presence default (F4) |
| `crates/discord-db/src/db.rs` | +attachments table, reactions index (F6/F8) |
| `crates/discord-db/src/lib.rs` | +attachments/top_reacted modules re-export |
| `crates/discord-db/src/attachments.rs` | NEW (F6) |
| `crates/discord-auth/src/qr.rs` | NEW (F7) |
| `crates/discord-auth/src/auth.rs` | +AuthCmd::Qr branch |
| `crates/discord-cli/src/commands/dc.rs` | +Typing/Join/Leave/Presence/ThreadCreate/TopReactions variants + handlers; Send --file/--typing |
| `crates/discord-cli/src/commands/download.rs` | NEW (F6) |
| `crates/discord-cli/src/commands/tail.rs` | +typing JSONL (F2); auto-reauth (F7) |
| `crates/discord-cli/src/main.rs` | wire new variants (match arms) |
| `crates/discord-mcp/src/server.rs` | +join_guild/leave_guild/set_presence/create_thread/top_reactions/download_attachments; send_message files |
| `Cargo.toml` (workspace) | +rsa, qrcode, md-5 (F6/F7) |
| `README.md` | commands table + stealth note (F9) |
| `AGENTS.md` | quickstart additions per batch |
| `SCHEMA.md` | attachments table, new commands' JSON shapes |

**Exit codes**: 0/2/3/4 existing; +7 attachment/IO (F1).
**stdout/stderr**: data→stdout, progress→stderr `\r\x1b[K` (F6).
**Rate limiting**: audit fetch_messages for 429 backoff 2s→10s + 400-700ms
jitter (langkurt) — add if missing (F6 prerequisite).

## Verify checklist (per batch, before commit)
```
□ cargo test (existing 44 + new) green
□ cargo fmt + clippy -D warnings (CI parity)
□ README/AGENTS/SCHEMA updated
□ Manual smoke per feature (see Verify lines)
□ Stealth posture unchanged: invisible default, launch_signature, is_fast_connect
```

## Sequencing
```
Batch 1: F1 → F2 → F3 → F4 → F5   (each: code → test → docs → commit)
Batch 2: F6 → F7 → F8
Batch 3: F9 (docs only)
```
