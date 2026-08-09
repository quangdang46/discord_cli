# COMPREHENSIVE PLAN — Admin & Moderation for the Discord CLI in Rust for AI Agents

> **Goal:** Close the admin/moderation parity gap vs the reference repos (notably `ibbybuilds-discli`, which ships complete `channel`/`role`/`member`/`permission`/`emoji`/`server`/`audit`/`invite` command groups) by adding a typed admin surface to the existing 5-crate workspace: channel CRUD, role CRUD + assign/remove, member moderation (kick/ban/unban/nick), embed builder, permission overwrites (view/set/lock/unlock), emoji CRUD, server settings + icon, audit log view, and invite management. Every op ships as (1) an `ApiClient` core method, (2) a top-level clap command in `discord-cli`, (3) an MCP `#[tool]` where it is a write or a first-class read, and (4) docs + smoke tests.
>
> **Auth:** user account token (self-bot style) — identical to the existing tool. Bot-only endpoints are not used; every route below is verified user-token-safe or is a documented, permissive superset.
>
> **⚠️ ToS notice:** automating a user account violates Discord's Terms of Service and can result in account termination. **Admin/moderation actions — especially kick, ban, bulk-delete, channel/role/emoji deletion, and permission lock/unlock — are the highest-risk class of operation in this tool.** Every destructive admin command REQUIRES the existing non-interactive `--confirm` flag; there is no bulk automation, no auto-moderation, no mass-kick. This warning MUST appear in the README and `--help`, and each destructive handler carries an `eprintln!` describing exactly what will happen.

---

## Table of Contents

1. [Context & Motivation](#1-context--motivation)
2. [Research Findings](#2-research-findings)
   - 2.1 The reference repos (verified features)
   - 2.2 Rust ecosystem (what the crate already provides)
   - 2.3 What to copy / what to avoid (licensing)
3. [Confirmed Decisions](#3-confirmed-decisions)
4. [Architecture Overview](#4-architecture-overview)
5. [Dependency Manifest](#5-dependency-manifest)
6. [SQLite Schema](#6-sqlite-schema)
7. [Stealth / Anti-Detection Layer](#7-stealth--anti-detection-layer)
8. [Authentication](#8-authentication)
9. [Command Surface](#9-command-surface)
10. [Output Contract](#10-output-contract)
11. [MCP Server Tools](#11-mcp-server-tools)
12. [Rate Limiting & Safety](#12-rate-limiting--safety)
13. [Error Handling & Exit Codes](#13-error-handling--exit-codes)
14. [Implementation Order (Milestones)](#14-implementation-order-milestones)
15. [Files to Create](#15-files-to-create)
16. [References & Sources](#16-references--sources)
17. [Verification Plan](#17-verification-plan)
18. [Distinctive techniques to port (research re-run)](#18-distinctive-techniques-to-port-research-re-run)
19. [Open Items / Future Work](#19-open-items--future-work)

---

## 1. Context & Motivation

The CLI already reads, sends, searches, syncs, threads, reacts, pins, downloads, and manages group DMs — but it cannot *administer* a server. The reference `ibbybuilds-discli` ships complete `channel`/`role`/`member`/`permission`/`emoji`/`server`/`audit`/`invite` groups; the plan's stated admin goal is discli parity. Today the workspace is read-plus-light-write: a grep for mutation across `crates/` finds only message ops (`send`/`edit`/`delete`), reactions, pins, join/leave, threads, and group-DM — **zero** channel/role/member/emoji/permission/server mutation.

**The critical realization (this plan):** all nine admin capabilities are *already* supported end-to-end by the pinned `discord-user-rs` 0.6.1 crate — dedicated `Route` variants, high-level operation traits, and typed request/response structs — and by the MIT `discli` reference. There is **no new crate surface, no new dependency, no schema change** required for the MUST-HAVE batch. The work is: (a) thin typed `ApiClient` wrappers (the existing `inner().post/patch/put/delete` + `Route` pattern), (b) name→ID resolvers (member, role, category, emoji), (c) clap command + handler plumbing, and (d) MCP tools. The only genuinely new *code* is the disclosure's missing `@everyone`-as-guild-ID trick for `lock`/`unlock`, the permission-name→bit const table, the audit action-name map, and base64 data-URI image upload (all small).

The plan is deliberately **phased** so the parity-critical, crate-backed features (channel CRUD, role CRUD, emoji CRUD) land first as MUST-HAVE, followed by the permission-gated extras (member mod, permissions, server settings, audit, invites, embed) as NICE-TO-HAVE in a second batch. A final batch covers explicit SKIP rationale for voice-state.

## 2. Research Findings

### 2.1 The reference repos (verified features)

Re-audit of `.tmp/` (this time scoped to admin/moderation). The single complete reference is `ibbybuilds-discli` (MIT, TypeScript, bot-token — but every endpoint below is identical for user tokens):

| Repo | Lang | Admin surface (verified from source) | Verdict |
|---|---|---|---|
| `ibbybuilds-discli` | TS | **Full**: `channel` (list/create/delete/rename/topic/move/clone/slowmode), `role` (list/create/edit/delete/assign/remove), `member` (list/info/kick/ban/nick), `permission` (view/set/lock/unlock/list), `emoji` (list/upload/delete), `server` (list/select/info/set/icon), `audit` (log/types), `invite` (list/create/delete), `message` (embed + bulk-delete) | **Primary template** — command grammar, `--confirm`/`--dry-run`, name→ID resolution, exit codes 0/1/2/3 |
| `jackwener/discord-cli` | Python | Member list, roles list (read-only), guild info | Read half already ported; no admin writes |
| `langkurt/discord-cli` | Go | None (read/sync/search only) | Thread fallback pattern only |
| `famasya/discord-cli-agent` | Go | None (scan/search/send) | JSONL + exit-code model only |
| `discord-user-rs` v0.6.1 crate | Rust | **Crate-level full support** (see §2.2): all routes, ops, request/response types, and a bundled `dcw_*` write CLI to crib from | The actual implementation engine |

**Key confirmed facts (verified from discli source, all in `.tmp/ibbybuilds-discli/src/`):**
- Every admin command resolves a *default server* via `config.ts` (`requireServer`) — this CLI instead passes an explicit `<GUILD>` argument (matching the existing `dc channels <GUILD>` convention). No config-file default server.
- Destructive commands (`channel delete`, `role delete`, `emoji delete`, `member kick/ban`, `invite delete`) require a **non-interactive** `--confirm` and exit 2 with `"This will <verb> <target> (<id>). Add --confirm to proceed."` when absent — this is exactly the existing repo convention (`dc_join`/`dc_leave`/`dc_delete`).
- `lock` uses **`@everyone` overwrite id == guild id** (`permission.ts:139`), denying `send_messages | send_messages_in_threads | create_public_threads`; `unlock` DELETEs that overwrite. Both are pure client-side conveniences over `EditChannelPermissions`/`DeleteChannelPermission`.
- `permission set` always transmits **both** `allow` and `deny` bitfields as strings with `type: 0` (role) — Discord replaces the whole overwrite, so omitting a side silently clears it.
- Emoji upload = `readFileSync` → base64 → `data:image/{ext};base64,...` (`emoji.ts`), ext `gif` for `.gif` else `png`.
- `member kick/ban` accept `--reason` but the reference **prints** it only (does not send it). This plan improves on the reference by sending `reason` (crate supports it: `ban_with_reason`).
- `audit log` maps a hand-maintained `AUDIT_ACTION` name table (~50 entries) to `action_type` ints; unknown `--type` exits 2 listing available values.

### 2.2 Rust ecosystem (what the crate already provides)

Verified directly from the pinned source (`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/discord-user-rs-0.6.1/`):

| Capability | Route variant (route.rs) | High-level op | Request / response types |
|---|---|---|---|
| Channel CRUD | `GetChannel{id}`, `EditChannel{id}`, `DeleteChannel{id}`, `CreateGuildChannel{guild_id}`, `BulkChannelPositions{guild_id}` (485) | `ChannelOps::get_channel/create_channel/edit_channel/delete_channel/get_guild_channels` (channel.rs:60-111) | `CreateChannelRequest` (706, `name`+type/topic/rate_limit/parent/position), `EditChannelRequest` (900, all-optional incl. slowmode/position/parent) |
| Role CRUD | `CreateGuildRole{gid}`, `EditGuildRole{gid,rid}`, `DeleteGuildRole{gid,rid}` (90-94) | `GuildOps::create_role/edit_role/delete_role` (guild.rs:91-115) | `CreateRoleRequest` (requests.rs:19), `EditRoleRequest = CreateRoleRequest` alias |
| Role assign/remove | `AddGuildMemberRole{gid,uid,rid}`, `RemoveGuildMemberRole{gid,uid,rid}` (376-379) | — (raw route via `http()`) | — (empty PUT/DELETE; 204) |
| Member mod | `KickMember{gid,uid}`, `CreateGuildBan{gid,uid}`, `RemoveGuildBan{gid,uid}`, `GetGuildBans{gid}`, `GetGuildBan{gid,uid}`, `EditGuildMember{gid,mid}` (96, 123-133, 370) | `GuildOps::kick_member/ban_user/ban_with_reason/unban/get_bans/get_ban/edit_guild_member` (guild.rs:254-370) | `EditGuildMemberRequest` (requests.rs:856: `nick`/`roles`/`mute`/`deaf`/`channel_id`/`communication_disabled_until`) |
| Permission overwrites | `EditChannelPermissions{cid,owid}` (PUT), `DeleteChannelPermission{cid,owid}` (DELETE) (328-331); `GetChannel{cid}` returns `permission_overwrites` | `ChannelOps::edit_channel_permissions` (allow/deny/type) + `delete_channel_permission` (channel.rs:161-176) | `EditChannelPermissionsRequest` (959: `allow`/`deny` Option<String> + `type`), `PermissionOverwrite{id,type,allow,deny}` (guild.rs:389) |
| Emoji CRUD | `GetGuildEmojis{gid}`, `GetGuildEmoji{gid,eid}`, `CreateGuildEmoji{gid}`, `EditGuildEmoji{gid,eid}`, `DeleteGuildEmoji{gid,eid}` | `GuildOps::get_guild_emojis/get_guild_emoji/create_emoji/edit_emoji/delete_emoji` (guild.rs:213-223) | `CreateEmojiRequest{name,image,roles}` (245), `EditEmojiRequest` (258), `GuildEmoji` (guild.rs:460, with `managed`/`animated`/`available`/`url()`/`reaction_string()`) |
| Server settings + icon | `EditGuild{gid}` (PATCH) | `GuildOps::edit_guild` (guild.rs:246) | `EditGuildRequest` (325: name/description/icon/banner/splash/afk/verification/notifications/content-filter/system+rules channels/locale) |
| Audit log | `GetGuildAuditLogs{gid,user_id,action_type,before,after,limit}` (116) | `GuildOps::get_audit_logs` (guild.rs:127) | `AuditLog`/`AuditLogEntry`/`AuditLogChange` (guild.rs:620-657) |
| Invites | `CreateChannelInvite{cid}`, `GetGuildInvites{gid}`, `GetChannelInvites{cid}` (325), `DeleteInvite{code}` (104) | `GuildOps::get_guild_invites/create_guild_invite_link/delete_guild_invite` (guild.rs:144-165) | `CreateInviteRequest` (975: max_age/max_uses/temporary/unique), `Invite` (guild.rs:534) |
| Embed builder | reuses `Route::CreateMessage` | `MessageBuilder` (message_builder.rs:26: `.channel/.content/.embed/.reply_to/.send`) + `EmbedBuilder` (:327: `.title/.description/.color/.url/.image/.thumbnail/.footer/.author/.field/.build`) with client-side Discord-limit validation | `Embed`/`EmbedField`/`EmbedFooter`/`EmbedMedia`/`EmbedAuthor` (types) |
| Crosspost / bulk-delete | ~~`BulkDeleteMessages{cid}` (74)~~ — **removed (2026-08): Discord rejects user-token bulk-delete (403 code 20002, bot-only)** | `delete <CH> <MSG_ID>` single-message fallback stays | — |
| Webhook mgmt | `GetChannelWebhooks/GetGuildWebhooks/CreateWebhook/GetWebhook/GetWebhookWithToken/EditWebhook/EditWebhookWithToken/DeleteWebhook/DeleteWebhookWithToken/ExecuteWebhook` | `WebhookOps` trait (operations/webhook.rs, **not** feature-gated) | `Webhook` (guild.rs:665, token only on create), `CreateWebhookRequest`/`EditWebhookRequest` (206/216) |
| Sticker CRUD | `GetGuildStickers/GetGuildSticker/CreateGuildSticker/EditGuildSticker/DeleteGuildSticker` (106-114) | `GuildOps::create_guild_sticker` (multipart via `post_raw_multipart`) / edit / delete | `EditStickerRequest` (274) |
| Scheduled events | `GetGuildScheduledEvents/GetGuildScheduledEvent/CreateGuildScheduledEvent/EditGuildScheduledEvent/DeleteGuildScheduledEvent/GetGuildScheduledEventUsers` | `ScheduledEventOps` trait + bundled `cli/commands/dc_events.rs` | `CreateScheduledEventRequest`/`EditScheduledEventRequest` (431/459) |
| Guild create/delete | `CreateGuild` (POST /guilds), `DeleteGuild{gid}` | `GuildOps::create_guild` (crate notes: user accounts capped at <10 guilds) / `delete_guild` (owner only) | `CreateGuildRequest` (290, name-only minimum) |
| Member roles replacement | — | `GuildOps::set_member_roles` (guild.rs:138 — PATCH `EditGuildMember` with full `roles` array — **overwrites the whole role set**) | `EditGuildMemberRequest.roles` |

**Infrastructure the crate already gives us:**
- `DiscordHttpClient` verbs: `get`/`post`/`patch`/`put`/`delete`/`put_empty`/`post_empty`/`post_no_response`/`put_optional`/`post_raw_multipart` (client.rs:368-590). The codebase's documented 204 rule ("no-body 204 endpoints MUST use `put_empty`/`post_empty`") applies to role assign/remove, permission delete, ban (returns 204), etc.
- `DiscordError` taxonomy with 403 → `PermissionDenied { permission }`, 404 → `NotFound { resource_type, id }`, 400 → `InvalidRequest`, 429 → `RateLimited { retry_after, .. }` (error.rs:31-110, mapped at client.rs:835-880). **This gives clean 403 handling for free** — the admin plan's documented 403→exit 4 path.
- `Permissions` bitflags (`types/enums.rs:316`) — the full name→bit const table for `role --permissions` / `perm set --allow/--deny` / `perm list` is `1 << n` and already expressed in the crate. `discord_user::permissions` module exists (`lib.rs:34`).
- `Colour(pub u32)` with `from_rgb` + `From<u32>` (`types/colour.rs:15`).

**User-token constraints verified:**
- `GetActiveThreads` (`threads/active`) is bot-only → 403 on user tokens (already handled by the `list_threads` fallback). The admin batch adds **no** new bot-only endpoints.
- The crate has **no single-member GET route** (no `GetGuildMember` variant) — member name→ID resolution for `assign`/`remove`/`kick`/`ban`/`nick` falls back to `list_members` (capped at 1000 by `client.rs:191`) or accepts a bare numeric ID (the existing ID-or-name convention). Minor limitation in very large guilds only.
- **User accounts can only create guilds when in fewer than 10 guilds** (crate doc on `create_guild`), and `DeleteGuild` requires guild ownership — the two reasons guild create/delete is NICE-TO-HAVE, not MUST-HAVE.

### 2.3 What to copy / what to avoid (licensing)

| Source | License | Copy status |
|---|---|---|
| `ibbybuilds-discli` | MIT | ✅ Port command grammar, resolution semantics, `--confirm`/`--dry-run` text, lock/unlock bit composition, audit action-name map. Verb-for-verb port of the *behavior* into the existing Rust handler pattern. |
| `discord-user-rs` | MIT | ✅ Already the core dependency. Use its routes/ops/types; do **not** enable its `cli` feature (pulls clap/rusqlite/anyhow/indicatif/etc.) — write against `DiscordHttpClient` + `Route` + ops traits instead. |
| `jackwener`, `langkurt`, `famasya`, `mrarfarf` | permissive | ✅ Concepts only (rate-limit pacing, name→ID resolution, JSONL). |
| `ayn2op-discordo`, `Stone-Red-Code`, `Rivalo` | GPL-3.0 | ⚠️ Concepts only — re-implement in Rust. Nothing new needed here (no admin specifics are GPL-only). |

## 3. Confirmed Decisions

1. **Crate types over raw `serde_json`.** All request/response structs (`CreateChannelRequest`, `EditChannelRequest`, `CreateRoleRequest`, `EditGuildMemberRequest`, `EditChannelPermissionsRequest`, `CreateEmojiRequest`, `CreateInviteRequest`, `EditGuildRequest`, `AuditLog`, `Ban`, `Webhook`, `GuildEmoji`, `PermissionOverwrite`, `Permissions` bitflags) are public in `discord-user-rs`. Use them; they are already serde and carry the correct `skip_serializing_if` semantics (the permission-set pitfall of omitting a side is a *crate* field behavior we respect by always filling both). Raw `serde_json::json!` is reserved for two cases only: (a) the `lock` deny bitfield (composed from `Permissions` constants) and (b) `premium_progress_bar_enabled` (absent from `EditGuildRequest` — the one deliberate `--boost-bar` parity gap, §19).
2. **`ApiClient` core methods are the only mutation surface.** Handlers and MCP tools call `ApiClient::*`; they never touch `Route`/`inner()` directly. New methods live in `impl ApiClient` in `crates/discord-core/src/client.rs` following the existing `parse id → inner() → Route → context("... failed") → map Raw*/crate-type → Ok` pattern exactly (e.g. `list_roles` at client.rs:276).
3. **`resolve_role`/`resolve_member`/`resolve_category` join `resolve.rs`** mirroring discli `resolve.ts` semantics: ID match first → strip `@`/`#` → case-insensitive **exact** (roles/members: match username OR global_name OR nick) → ambiguity → stderr list + `exit::USAGE` (2) → not-found → `exit::NOT_FOUND` (3). Member resolution falls back to `list_members(guild, 1000)`.
4. **Top-level clap commands, not a nested `admin` group.** The existing CLI is flat (`guilds`, `channels`, `members`, `roles`, `send`, …) with the live dispatch in `main.rs`; a nested `admin` group would break the established noun-verb surface. New top-level variants: `channel-create|rename|topic|move|clone|slowmode|delete`, `role-create|edit|delete|assign|remove`, `member-kick|ban|unban|nick`, `embed`, `perm view|set|lock|unlock|list`, `emoji list|upload|delete`, `server set|icon`, `audit log|types`, `invite list|create|delete`. (The dead `DcCmd` enum in dc.rs is NOT extended — follow main.rs, per the codebase's documented note.)
5. **`--confirm` gating, never interactive, checked BEFORE building the client.** Destructive ops: `channel delete`, `role delete`, `emoji delete`, `member kick|ban`, `invite delete`, `perm lock` (changes @everyone deny) — all exit `USAGE` (2) with `"This will <verb> <target> (<id>). Add --confirm to proceed."` when absent. `--dry-run` (structured `{action,...}` record) ships for the create/edit verbs (`channel create|rename`, `role create|edit`, `perm set|lock`) mirroring `dc_send`. Advisory `confirm: bool` param on MCP write tools.
6. **Idempotent CLI names.** `channel-create` etc. (kebab-case, top-level) match the existing `thread-create`/`dm-group` precedent rather than discli's nested `channel create`.
7. **No new dependencies.** `base64` (upload), `mime_guess` (already in tree via dc.rs `load_attachments`), `Permissions`/`Colour` from the crate. The `--boost-bar` server-settings flag is deferred (§19) rather than pulling raw JSON.
8. **403 is surfaced cleanly.** The crate's `DiscordError::PermissionDenied` maps to exit `FORBIDDEN` (4) with message text — the codebase's documented-but-unused exit code becomes live for admin ops (§13).
9. **Paginated/admin reads reuse the existing fixed-sleep+jitter pattern** (`fetch_messages` client.rs:876): no new 429/backoff code in this batch. `list_members` already caps at 1000; audit log is a single `limit≤100` call.
10. **Plan language:** English, technical terms in English (consistent with the existing plan).

## 4. Architecture Overview

```
discord_cli/
├── Cargo.toml                       workspace (5 crates) — NO new members
├── crates/
│   ├── discord-core/                ← all admin core methods land here
│   │   └── src/
│   │       ├── client.rs            ApiClient: +~30 admin methods (this plan)
│   │       ├── types.rs             +PermissionOverwrite, +AuditEntryView, +Emoji,
│   │       │                        +BanView, +InviteView (display structs)
│   │       ├── output.rs            exit::FORBIDDEN(4) becomes LIVE (no change)
│   │       └── stealth.rs           unchanged
│   ├── discord-cli/
│   │   ├── src/main.rs              +~22 Command variants + dispatch arms
│   │   ├── src/commands/dc.rs       +dc_* admin handlers (~22 fns)
│   │   ├── src/resolve.rs           +resolve_role/member/category/emoji
│   │   └── tests/cli_smoke.rs       +exit-code/confirm/dry-run smoke tests
│   ├── discord-mcp/
│   │   └── src/server.rs            +~14 #[tool] + params structs
│   ├── discord-db/                  UNCHANGED (admin ops do not touch SQLite)
│   └── discord-auth/                UNCHANGED
├── docs/                            +docs/ADMIN.md (permission matrix, risk table)
└── README.md                        ToS warning extended for admin actions
```

**Design principle (unchanged from the base plan):** the CLI and the MCP server share the same `ApiClient` core; `dc.rs` and `server.rs` are thin presentation layers. Admin handlers are structurally identical to the ~25 existing handlers (the "boilerplate skeleton"): `ctx.client()` → resolve IDs → one core method → `output::emit`/`emit_error` with the documented exit codes.

**New cross-cutting helper (in dc.rs):** `fn check_confirm(target_desc: &str, action: &str, id: &str, confirm: bool) -> Option<ExitCode>` — emits `eprintln!("This will {action} {target_desc} ({id}). Add --confirm to proceed.")` and returns `Some(USAGE)` when `!confirm`, else `None`. This is the single code path for the destructive gate (removes the copy-pasted 4-line block from every handler).

## 5. Dependency Manifest

```toml
# workspace Cargo.toml — NO new dependencies required.
# The admin batch consumes existing workspace deps only.

[dependencies]  # in crates/discord-core
tokio.workspace = true
serde.workspace = true
serde_json.workspace = true
discord-user-rs.workspace = true    # already provides Route, ops, Permissions,
                                    # Colour, GuildEmoji, AuditLog, Ban, Webhook,
                                    # EditGuildRequest, CreateEmojiRequest, ...
reqwest.workspace = true            # reqwest::multipart only if sticker upload (M5)
base64.workspace = true             # emoji/sticker/icon data-URI upload
anyhow.workspace = true

[dependencies]  # in crates/discord-cli (existing)
clap.workspace = true
discord-core.workspace = true
# mime_guess already available (dc.rs load_attachments) for file mime sniffing
```

No changes to `discord-db`, `discord-auth`, or `discord-mcp` manifests. The crate's `cli` feature stays OFF (it pulls clap/rusqlite/anyhow/indicatif/etc. and would collide with our own CLI).

## 6. SQLite Schema

**No schema changes.** Admin/moderation is a live-API concern; nothing is written to the SQLite archive (which stores messages for FTS5 search/download). `discord-db` is untouched. If a later milestone wants an audit trail of *our own* admin actions, that is an additive `admin_actions` table (§19) — deliberately out of scope to keep the MUST-HAVE batch green.

## 7. Stealth / Anti-Detection Layer

**Unchanged.** All admin requests flow through the same `inner()`-built `DiscordHttpClient` that already attaches the Chrome UA, `en-US` locale, and X-Super-Properties (`set_super_properties_b64`) — so admin traffic is indistinguishable from normal read/send traffic at the header level. Notes specific to admin:

- **No new header surface.** PATCH/PUT/DELETE admin calls carry the identical header set; nothing to add in `stealth.rs`.
- **Base64 uploads are normal Discord traffic.** Emoji/icon uploads send `data:image/png;base64,...` exactly like the real client (`data:image/gif` for `.gif`; `png` for `.png`; `jpeg` otherwise for icons — crate `cli/api_emojis.rs:26-27` is the reference pattern).
- **Rate-limit pacing** for the one paginated admin op (`list_members` is a single capped call; member-resolution falls back to a single 1000-cap list) reuses the existing 400ms+jitter sleep where looping is ever needed. No new stealth work.

## 8. Authentication

**Unchanged.** Token resolution (`--token` flag → `DISCORD_TOKEN` env → `.env` → keyring) and `auth` semantics are untouched. Every admin command shares `DcCtx::client()` → `ApiClient::from_env`. Admin ops additionally depend on the *account's permissions in the target guild* — Discord enforces these server-side (403 `PermissionDenied` on lack), and the CLI surfaces that cleanly (§13). No new auth paths.

## 9. Command Surface

### Top-level (existing, unchanged)
`auth`, `status`, `whoami`, `serve` … (see base plan §9).

### Admin / moderation commands (NEW — all top-level)
Every command accepts `<GUILD>` as name or ID (resolved via `resolve::resolve_guild`), or `<CHANNEL>`/`<USER>`/`<ROLE>` as name or ID (resolved via the new `resolve.rs` helpers). Destructive commands require `--confirm`. `--dry-run` prints a structured record and exits 0.

| Command | Description | API |
|---|---|---|
| `channel-create <GUILD> <name> [--type text\|voice\|category\|announcement\|stage\|forum] [--category <name>] [--topic <text>] [--slowmode <sec>] [--dry-run]` | Create a channel (type 0/2/4/5/13/15; parent resolved to id) | `POST /guilds/{id}/channels` |
| `channel-rename <GUILD> <channel> <new-name> [--dry-run]` | Rename | `PATCH /channels/{id}` `{name}` |
| `channel-topic <GUILD> <channel> <topic>` | Set topic | `PATCH /channels/{id}` `{topic}` |
| `channel-move <GUILD> <channel> [--category <name>] [--position <n>]` (≥1 of --category/--position else exit 2) | Move / reorder | `PATCH /channels/{id}` `{parent_id, position}` |
| `channel-clone <GUILD> <channel> [--name <name>]` | Clone (GET + POST; copies type/parent/topic) | `POST /guilds/{id}/channels` |
| `channel-slowmode <GUILD> <channel> <seconds>` | Set slowmode (0 disables) | `PATCH /channels/{id}` `{rate_limit_per_user}` |
| `channel-delete <GUILD> <channel> [--confirm]` | Delete a channel (irreversible) | `DELETE /channels/{id}` |
| `role-create <GUILD> <name> [--color <hex>] [--permissions <p1,p2>] [--mentionable] [--hoist] [--dry-run]` | Create a role | `POST /guilds/{id}/roles` |
| `role-edit <GUILD> <role> [--name <n>] [--color <hex>] [--permissions <p1,p2>] [--mentionable\|--no-mentionable] [--hoist\|--no-hoist] [--dry-run]` (≥1 option else exit 2) | Edit a role | `PATCH /guilds/{id}/roles/{rid}` |
| `role-delete <GUILD> <role> [--confirm]` | Delete a role | `DELETE /guilds/{id}/roles/{rid}` |
| `role-assign <GUILD> <role> <user>` | Assign role to member (single-role PUT) | `PUT /guilds/{id}/members/{uid}/roles/{rid}` |
| `role-remove <GUILD> <role> <user>` | Remove role from member (single-role DELETE) | `DELETE /guilds/{id}/members/{uid}/roles/{rid}` |
| `member-kick <GUILD> <user> [--reason <text>] [--confirm]` | Kick (reason sent — improvement over discli) | `DELETE /guilds/{id}/members/{uid}` |
| `member-ban <GUILD> <user> [--reason <text>] [--delete-days <0-7>] [--confirm]` | Ban (reason + message purge window) | `PUT /guilds/{id}/bans/{uid}` |
| `member-unban <GUILD> <user> [--confirm]` | Unban | `DELETE /guilds/{id}/bans/{uid}` |
| `member-nick <GUILD> <user> <nickname>` (empty string clears) | Set nickname | `PATCH /guilds/{id}/members/{uid}` `{nick}` |
| `embed <CHANNEL> [--title <t>] [--description <d>] [--color <hex>] [--url <u>] [--image <url>] [--thumbnail <url>] [--footer <text>] [--author <name>] [--field <"Name\|Value\|inline">] [--content <text>] [--reply <id>] [--confirm] [--dry-run]` | Send a rich-card message (≥1 of title/description required) | `POST /channels/{id}/messages` via `MessageBuilder` |
| `perm view <GUILD> <channel>` | List permission overwrites (role/member names resolved) | `GET /channels/{id}` + `GET /guilds/{id}/roles` |
| `perm set <GUILD> <channel> <role> [--allow <p1,p2>] [--deny <p1,p2>]` (≥1 else exit 2; both sides always transmitted) | Set an overwrite (type 0 role) | `PUT /channels/{id}/permissions/{owid}` |
| `perm lock <GUILD> <channel> [--dry-run]` | Read-only for @everyone (deny send/thread-send/create-threads) | `PUT /channels/{id}/permissions/{guild_id}` |
| `perm unlock <GUILD> <channel>` | Delete the @everyone overwrite | `DELETE /channels/{id}/permissions/{guild_id}` |
| `perm list` | Print the permission name→bit table (local, no API) | none |
| `emoji list <GUILD> [-n <count>]` | List custom emojis | `GET /guilds/{id}/emojis` |
| `emoji upload <GUILD> <name> <file>` | Upload (base64 data URI; png/jpg/gif ≤256KiB) | `POST /guilds/{id}/emojis` |
| `emoji delete <GUILD> <name-or-id> [--confirm]` | Delete (not-found → exit 3) | `DELETE /guilds/{id}/emojis/{eid}` |
| `server set <GUILD> [--name <n>] [--description <t>] [--verification none\|low\|medium\|high\|very_high] [--notifications all_messages\|only_mentions] [--content-filter disabled\|members_without_roles\|all_members] [--afk-timeout <60\|300\|900\|1800\|3600>] [--system-channel <id>] [--rules-channel <id>]` (≥1 else exit 2) | Edit server settings | `PATCH /guilds/{id}` |
| `server icon <GUILD> <file>` | Set server icon (base64; png/jpg/gif) | `PATCH /guilds/{id}` `{icon}` |
| `audit log <GUILD> [-n <1-100>] [--type <action-name>] [--user <id>]` | View audit log (action-name → int map; unknown → exit 2) | `GET /guilds/{id}/audit-logs` |
| `audit types` | Print the action-name→value table (local) | none |
| `invite list <GUILD>` | List guild invites | `GET /guilds/{id}/invites` |
| `invite create <GUILD> <channel> [--max-age <sec>] [--max-uses <n>] [--temporary]` | Create invite (unique:true) | `POST /channels/{id}/invites` |
| `invite delete <GUILD> <code> [--confirm]` | Delete invite | `DELETE /invites/{code}` |

**Bulk-delete** (`bulk-delete <CHANNEL> -n <count> [--confirm]`, via `BulkDeleteMessages` with a single-message fallback) and **crosspost** (`crosspost <CHANNEL> <msg-id>`) are NICE-TO-HAVE (§19) — not in the MUST-HAVE surface.

### Local query (offline SQLite) — unchanged

## 10. Output Contract

**Unchanged** from the base plan (`output.rs`): JSONL when piped / Rich when TTY, `--json`/`--yaml`/`--format` overrides, envelope `{ok, schema_version, data|error}` for `--json`/`--yaml`, data→stdout, progress/errors→stderr, `emit_error(code, message, exit_code)`.

Admin-specific data shapes:
- List commands emit the crate-typed arrays directly (`Vec<GuildEmoji>`, `Vec<Invite>`, `Vec<Role>`, `Vec<Ban>`, `AuditLog`), or thin `crate::types::*` display structs that map IDs to strings (`PermissionOverwrite`, `AuditEntryView`, `BanView`, `InviteView`) — following the existing `Raw*`→`crate::types::*` mapping pattern.
- Single-op success emits a small JSON object: `{"action":"channel_created","id":...,"name":...}`, `{"action":"role_created","id":...,"name":...}`, `{"action":"kicked","user_id":...,"username":...}`, `{"deleted":true,...}`, `{"emoji":":name:","id":...}`, `{"invite":"https://discord.gg/<code>",...}`, `{"left":true,...}`-style.
- `--dry-run` always emits `{"action":"<verb>_<noun>", ...}` (e.g. `{"action":"create_channel","name":...,"type":...,"category":...}`) and exits 0 — never touches the network.
- **Exit codes** (see §13): 0 OK, 1 ERROR, 2 USAGE (missing `--confirm`, no-op option sets, invalid enums/unknown permission/audit names), 3 NOT_FOUND (unresolvable guild/channel/role/member/emoji), 4 FORBIDDEN (403 `PermissionDenied` — becomes live), 7 `EXIT_ATTACHMENT` (emoji/icon file read or size failures).

## 11. MCP Server Tools

New `#[tool]`s on `DiscordMcpServer` (all return JSON strings via `serde_json::to_string`, errors via `.map_err(|e| e.to_string())?`, write tools carry advisory `confirm: bool` checked as `if !req.confirm { return Err("... requires confirm: true".into()) }`):

| Tool | Params | Purpose | Gate |
|---|---|---|---|
| `create_channel` | `guild_id, name, channel_type?, category_id?, topic?, slowmode?` | Create channel | none |
| `delete_channel` | `channel_id, confirm` | Delete channel | advisory confirm |
| `edit_channel` | `channel_id, name?, topic?, slowmode?, parent_id?, position?` | Rename/topic/slowmode/move | none |
| `list_roles` | `guild_id, limit?` | Roles | none |
| `create_role` | `guild_id, name, color?, permissions?, mentionable?, hoist?` | Create role | none |
| `edit_role` | `guild_id, role_id, name?, color?, permissions?, mentionable?, hoist?` | Edit role | none |
| `delete_role` | `guild_id, role_id, confirm` | Delete role | advisory confirm |
| `assign_role` / `remove_role` | `guild_id, user_id, role_id` | Single-role assign/remove | none |
| `kick_member` / `ban_member` / `unban_member` | `guild_id, user_id, reason?, confirm` (+`delete_message_days?` for ban) | Member mod | advisory confirm |
| `set_nickname` | `guild_id, user_id, nickname` | Nick | none |
| `list_emojis` / `create_emoji` / `delete_emoji` | `guild_id, name?, file_path?, confirm` | Emoji CRUD | delete: advisory confirm |
| `get_audit_logs` | `guild_id, user_id?, action_type?, limit?` | Audit log (read-only) | none |
| `create_invite` / `delete_invite` | `guild_id, channel_id?, code?, max_age?, max_uses?, temporary?, confirm` | Invites | delete: advisory confirm |
| `send_embed` | `channel_id, title?, description?, color?, url?, image?, footer?, fields?, content?, reply_to?, confirm` | Rich-card send | advisory confirm |

The confirm-gated MCP tools (channel/role/emoji/invite delete, kick/ban/unban, embed) mirror the existing `join_guild`/`leave_guild` advisory pattern exactly.

## 12. Rate Limiting & Safety

**Reuse, don't re-add.** Admin ops are single-shot except: `resolve_role`/`resolve_member` (one `list_roles`/`list_members(1000)` call) and `perm view` (getChannel + listRoles). No pagination loops → no new backoff code. Where a future feature loops (bulk-delete's recent-message fetch), it reuses `fetch_messages`' 400ms+0..400ms jitter (client.rs:876).

**Safety defaults (extending §12 of the base plan):**
- Every destructive admin command requires `--confirm` (non-interactive), checked **before** `ctx.client()` (no network for usage errors — the documented `dc_dm_group` pattern).
- `--dry-run` for create/edit verbs previews the exact request payload.
- **No bulk automation**: single target per invocation. `bulk-delete` (if built, §19) is `--confirm` + capped count only.
- **Permission hygiene**: Discord enforces MANAGE_* server-side; the CLI documents required permissions (docs/ADMIN.md) and surfaces 403 as exit 4. `role-create/edit/delete` additionally guard `@everyone` (id == guild_id — cannot be created/deleted) with a usage error.
- **Token leakage**: webhook management (if built, §19) must NOT print the webhook `token` in list output (the crate returns it only on create; the CLI warns instead).
- The Cloudflare-1015 `InvalidRequestTracker` already inside `DiscordHttpClient` covers admin 403s/401s in the same 10-min window as everything else.

## 13. Error Handling & Exit Codes

`output.rs::exit` is unchanged; **`FORBIDDEN = 4` becomes live** in this plan.

| Error condition | Detection | Code string | Exit |
|---|---|---|---|
| Success | — | — | 0 |
| Generic API failure | any core method `Err(e)` | `"ApiError"` | 1 |
| Token/auth failure | `ctx.client()` / `ApiClient::from_env` | `"AuthError"` | 1 |
| **403 Forbidden (admin)** | crate `DiscordError::PermissionDenied { permission }` (maps HTTP 403) | `"Forbidden"` (message = `permission`) | **4** |
| 404 / resolution failure | crate `DiscordError::NotFound`; `resolve.rs` not-found | `"NotFound"` | 3 |
| Usage errors | missing `--confirm`, `role edit` no-options, `channel move` no-options, invalid enum, unknown permission/audit name, `@everyone` create/delete, `perm set` no allow/deny | `"UsageError"` | 2 |
| File read/size (emoji/icon upload) | `tokio::fs` / size > 256KiB | `eprintln!` + `EXIT_ATTACHMENT` | 7 |
| Ambiguity (role/member/category name) | `resolve.rs` n>1 match | `eprintln!` list + `exit::USAGE` | 2 |
| Rate limited | crate `DiscordError::RateLimited` | `"ApiError"` (message includes `retry after Ns`) | 1 (unchanged) |

**Implementation detail:** a small `fn classify(e: &anyhow::Error) -> u8` helper in dc.rs walks the anyhow chain (`e.chain()`) for `DiscordError::PermissionDenied`/`NotFound`/`RateLimited` and picks the exit code; handlers become `ExitCode::from(classify(&e))` + `emit_error(code, msg, classify(&e))`. This is the one place admin code touches error handling.

## 14. Implementation Order (Milestones)

Phases B1–B3 (core+CLI+MCP+docs+tests per feature, following the codebase's documented 4-layer "bead" pattern — each feature is a bead `F#a core / F#b cli / F#c mcp / F#d docs+tests`).

**B1 — MUST-HAVE: the parity-critical, crate-backed batch (1–3).**
1. **Channel CRUD** (flagship admin op) —
   - F1a `client.rs`: `create_channel(guild_id, CreateChannelRequest) -> Result<Channel>`, `edit_channel(channel_id, EditChannelRequest) -> Result<Channel>`, `delete_channel(channel_id) -> Result<()>`, `get_channel(channel_id) -> Result<crate::types::Channel>` (raw `permission_overwrites` kept in a `RawChannel` field for F4 `perm view`).
   - F1b `main.rs` + `dc.rs`: 7 top-level variants + handlers (`dc_channel_create/rename/topic/move/clone/slowmode/delete`); `resolve_category` (type 4 filter); new **unfiltered** channel resolver `resolve_channel_admin(client, guild_id, name)` returning ALL channel types (the blocker — `list_channels` filters to text-like; voice/category cannot be named today).
   - F1c MCP: `create_channel`, `edit_channel`, `delete_channel`.
   - F1d docs + smoke tests (channel-create/rename/delete exit codes; delete needs `--confirm`; `channel-move` no-option exit 2).
2. **Role CRUD + assign/remove** —
   - F2a `client.rs`: `create_role(guild_id, CreateRoleRequest)`, `edit_role(guild_id, role_id, EditRoleRequest)`, `delete_role(guild_id, role_id)`, `add_member_role(guild_id, user_id, role_id)` (via `Route::AddGuildMemberRole` + `put_empty`), `remove_member_role(...)` (`delete`).
   - F2b `main.rs` + `dc.rs`: 5 variants + handlers; `resolve_role`; `resolve_member` (username/global_name/nick over `list_members(1000)`); `--permissions` name→bit via `Permissions`; `@everyone` guard.
   - F2c MCP: `list_roles`, `create_role`, `edit_role`, `delete_role`, `assign_role`, `remove_role`.
   - F2d docs + tests (role-create dry-run payload, edit ≥1 option, `--confirm` delete, assign/remove need numeric-or-resolvable member).
3. **Emoji CRUD** —
   - F3a `client.rs`: `list_emojis(guild_id) -> Vec<GuildEmoji>`, `create_emoji(guild_id, name, image_path)` (fs::read + base64 STANDARD + `data:image/{png|gif};base64,` mime), `delete_emoji(guild_id, emoji_id)`.
   - F3b `main.rs` + `dc.rs`: 3 variants + handlers; resolve by name-or-id from `list_emojis`; `--confirm` delete; 256KiB + alnum/underscore name validation; `managed: true` delete → clean error.
   - F3c MCP: `list_emojis`, `create_emoji`, `delete_emoji`.
   - F3d docs + tests (upload mime selection, not-found exit 3, missing file exit 7).

**B2 — NICE-TO-HAVE: permission-gated moderation extras (4–6).**
4. **Member moderation (kick/ban/unban/nick)** —
   - F4a `client.rs`: `kick_member(guild_id, user_id, reason)`, `ban_member(guild_id, user_id, reason, delete_days)` (`Route::CreateGuildBan` + `put` with body incl. `reason`; `ban_with_reason` pattern), `unban_member`, `set_nickname` (`EditGuildMemberRequest{nick: Some(..)}`).
   - F4b `main.rs` + `dc.rs`: 4 variants + handlers; kick/ban `--confirm`; `--delete-days 0..7`.
   - F4c MCP: `kick_member`, `ban_member`, `unban_member`, `set_nickname`.
   - F4d docs (permission matrix) + tests.
5. **Permission overwrites (view/set/lock/unlock)** —
   - F5a `client.rs`: `get_channel_overwrites(channel_id) -> Vec<PermissionOverwrite>`, `edit_channel_permission(channel_id, overwrite_id, allow, deny, type)`, `delete_channel_permission(channel_id, overwrite_id)`. `lock`/`unlock` compose `Permissions::{SEND_MESSAGES | SEND_MESSAGES_IN_THREADS | CREATE_PUBLIC_THREADS}` and use `overwrite_id == guild_id`.
   - F5b `main.rs` + `dc.rs`: `perm view/set/lock/unlock/list`; name→bit const table via `Permissions`; `perm set` transmits both allow+deny; `perm lock` `--dry-run`; `perm unlock` warns `This will restore @everyone send access` + `--confirm` per the reference's delete-to-unlock (discli unlocks without confirm; this plan gates it).
   - F5c MCP: none required (blocked by `RawChannel` dropping overwrites unless F1a lands; if F1a landed, one `view_overwrites` tool).
   - F5d docs + tests.
6. **Server settings + icon** —
   - F6a `client.rs`: `edit_guild(guild_id, EditGuildRequest)`; `set_icon` variant builds `data:image/{...};base64`.
   - F6b `main.rs` + `dc.rs`: `server set` (9 options, enums → ints 0-4/0-1/0-2, `--afk-timeout` whitelist) + `server icon`; `--dry-run`.
   - F6c MCP: `edit_guild`.
   - F6d docs + tests.

**B3 — NICE-TO-HAVE: read/moderation extras (7–9).**
7. **Audit log** —
   - F7a `client.rs`: `audit_logs(guild_id, user_id, action_type, limit) -> AuditLog`.
   - F7b `main.rs` + `dc.rs`: `audit log` (action-name→int map ~50 entries, unknown → exit 2 listing available) + `audit types`.
   - F7c MCP: `get_audit_logs`.
   - F7d docs + tests (types table local-only; log requires VIEW_AUDIT_LOG).
8. **Invite management** —
   - F8a `client.rs`: `list_guild_invites`, `create_channel_invite(channel_id, CreateInviteRequest)`, `delete_invite(code)`.
   - F8b `main.rs` + `dc.rs`: `invite list/create/delete`; `--confirm` on delete; `discord.gg/<code>` output.
   - F8c MCP: `create_invite`, `delete_invite`.
   - F8d docs + tests.
9. **Embed builder** —
   - F9a `client.rs`: `send_embed(channel_id, EmbedSpec)` via `MessageBuilder::new(inner).channel(id).content(..).embed(|e| ...).reply_to(..).send()`; client-side limit validation (title ≤256, desc ≤4096, ≤10 fields, field name ≤256/value ≤1024).
   - F9b `main.rs` + `dc.rs`: `embed` command reusing `dc_send`'s `--confirm`/`--dry-run` and `--field "Name|Value|inline"` parsing.
   - F9c MCP: `send_embed`.
   - F9d docs + tests (builder validation pure unit tests; no network).

**B4 — explicitly optional/deferred (documented in §19):** crosspost / bulk-delete, webhook management, sticker CRUD, scheduled events, guild create/delete, voice-state (SKIP).

## 15. Files to Create

- `crates/discord-core/src/client.rs` — edit: ~30 new `ApiClient` methods (B1: create/edit/delete/get_channel, role CRUD + assign/remove; B2: member mod + permissions + edit_guild; B3: audit/invites/send_embed).
- `crates/discord-core/src/types.rs` — edit: `PermissionOverwrite` (id/type/allow/deny), `AuditEntryView`, `EmojiView`, `BanView`, `InviteView` display structs (mirror the `Raw*`→types mapping pattern).
- `crates/discord-cli/src/main.rs` — edit: ~22 `Command` variants + dispatch arms.
- `crates/discord-cli/src/commands/dc.rs` — edit: ~22 handlers + `check_confirm` helper + `classify` exit-code helper + `EmbedSpec`/`ChannelCreateOpts`/`RoleEditOpts` structs (clippy `too_many_arguments`).
- `crates/discord-cli/src/resolve.rs` — edit: `resolve_role`, `resolve_member`, `resolve_category`, `resolve_emoji` (+ unit tests for the strip/match/ambiguity logic).
- `crates/discord-cli/src/commands/dc.rs` — `resolve_channel_admin` (unfiltered, guild-scoped) helper.
- `crates/discord-mcp/src/server.rs` — edit: ~14 `#[tool]`s + params structs.
- `crates/discord-cli/tests/cli_smoke.rs` — edit: admin exit-code/confirm/dry-run smoke tests.
- `docs/ADMIN.md` — new: permission matrix (required perms per command), ToS risk table, command reference.
- `README.md` — edit: extended ToS warning for admin actions + admin command section.

No new files in `discord-db`/`discord-auth`; no schema migration.

## 16. References & Sources

**In-repo (`.tmp/`, MIT — OK to port):**
- `ibbybuilds-discli/src/commands/{channel,role,member,permission,emoji,server,audit,invite}.ts` — command grammar, option sets, `--confirm`/`--dry-run`, output text, lock/unlock bit composition (`permission.ts:139-151`), emoji data-URI (`emoji.ts`), audit action map (`audit.ts`).
- `ibbybuilds-discli/src/utils/resolve.ts` — `resolveChannel`/`resolveCategory`/`resolveRole`/`resolveMember` semantics (ID-first, `#`/`@` strip, case-insensitive, ambiguity→exit 1, not-found→exit 3).
- `ibbybuilds-discli/src/utils/api.ts` — endpoint mapping (listChannels/createChannel/modifyChannel/deleteChannel, createRole/modifyRole/deleteRole/addRoleToMember/removeRoleFromMember, createEmoji/deleteEmoji, editChannelPermission/deleteChannelPermission, createInvite/deleteInvite, getAuditLogs).

**Crate (pinned source, MIT):** `discord-user-rs-0.6.1` — `src/route.rs` (all routes cited above), `src/operations/{channel,guild}.rs` (ChannelOps/GuildOps), `src/types/requests.rs` (CreateChannelRequest 706, EditChannelRequest 900, CreateRoleRequest 19, EditGuildMemberRequest 856, EditChannelPermissionsRequest 959, CreateEmojiRequest 245, CreateInviteRequest 975, EditGuildRequest 325), `src/types/guild.rs` (PermissionOverwrite 389, GuildEmoji 460, AuditLog 620-657, Ban 692, Webhook 665), `src/types/enums.rs:316` (Permissions bitflags), `src/types/colour.rs` (Colour), `src/error.rs` (DiscordError + 403→PermissionDenied), `src/client.rs` (verbs 368-590, retry + InvalidRequestTracker), `src/message_builder.rs` (MessageBuilder 26 / EmbedBuilder 327), `src/cli/api_emojis.rs` (base64 data-URI pattern), `src/cli/commands/dcw_webhooks.rs` + `dc_events.rs` (deferred-feature references).

## 17. Verification Plan

1. `cargo build --release` — compiles; `cargo clippy -- -D warnings` clean (structs keep `too_many_arguments` at bay).
2. **Unit tests (no network):** `resolve.rs` strip/match/ambiguity helpers; `types.rs` PermissionOverwrite/display-struct serde; `client.rs` pure helpers (embed-limit validation, emoji mime/data-URI builder, permission name→bit parse, `@everyone` guard). Existing pattern = pure helper extraction, so no fake API server is needed — the codebase has **no** mock-HTTP harness today and this plan does not introduce one (§19 considers a `#[cfg(test)]` fake `DiscordHttpClient` trait, which the crate does not expose as injectable).
3. **Smoke tests (`tests/cli_smoke.rs`, no token):** each destructive command exits 2 without `--confirm`; `--dry-run` exits 0 with `"action"` in stdout; `role edit` with zero options exits 2; `channel move` with neither option exits 2; `perm set` with neither `--allow`/`--deny` exits 2; `perm list`/`audit types` print their tables; `embed` without title/description exits 2; missing emoji file exits 7.
4. **Manual smoke (real token, a server the user administers, low volume):** `channel-create` → `channel-rename` → `channel-topic` → `channel-slowmode` → `channel-clone` → `channel-move` → `channel-delete --confirm`; `role-create` → `role-assign` → `role-remove` → `role-delete --confirm`; `emoji upload` (tiny png) → `emoji list` → `emoji delete --confirm`; `perm view` → `perm lock` → `perm unlock`; `member nick`; `audit log`; `invite create` → `invite list` → `invite delete --confirm`. Verify 403 surfaces as exit 4 on a guild the account does not administer.
5. **MCP wiring test:** `claude mcp add discord ...` then ask Claude Code to "create a channel called #ops in <guild>" and "kick the user <id>" — verify each tool round-trips and confirm-gated tools require approval.
6. **Safety check:** confirm every destructive op is blocked without `--confirm`; no admin command paginates or loops; `--dry-run` never hits the network.

## 18. Distinctive techniques to port (research re-run)

**Permission name→bit table without new constants:** `Permissions` bitflags in the crate *is* the map. `role create --permissions send_messages,manage_messages` → `Permissions::from_bits_truncate(parsed)` via a name→flag lookup table derived from the bitflags constants; `perm list` prints `{name, bit}`.

**`@everyone` = guild ID (discli `permission.ts:139`):** `perm lock`/`unlock` target `overwrite_id == guild_id` — the single most surprising trick in the admin batch, and the reason lock/unlock need no new route. `role create/edit/delete` must reject `role == guild_id` (cannot create/delete @everyone).

**Bitfields as strings, always both sides:** `EditChannelPermissionsRequest.allow/deny` are `Option<String>` with `skip_serializing_if`; `perm set` therefore always sets both, and role `permissions` is sent as a decimal string (crate `CreateRoleRequest.permissions: Option<String>`).

**204 no-body discipline (existing rule, now used widely):** role assign/remove (`AddGuildMemberRole`/`RemoveGuildMemberRole` return 204), `perm unlock` (204), emoji delete (204), invite delete (204) → `put_empty`/`delete`. Ban `PUT` returns 204 → `put` with `serde_json::json!` body works only because the crate's `put` discards the empty body; prefer the crate's ops traits where they exist (`ban_user`) to sidestep this.

**Member resolution without a single-member GET (crate gap):** no `GetGuildMember` route → `resolve_member` uses `list_members(guild, 1000)` matching username/global_name/nick, with bare-ID passthrough. Documented limitation in guilds >1000 members (search fallback `GetGuildMembersByQuery` is a §19 candidate).

**Base64 upload mime (`api_emojis.rs:26-27`):** emoji: `.gif`→`data:image/gif`, else `data:image/png`; icon: `.gif`/`.png` else `data:image/jpeg`. `base64::engine::general_purpose::STANDARD.encode(&bytes)` + `format!("data:{};base64,{}", mime, b64)`.

**Reason transmission (improvement over discli):** discli prints `--reason` without sending it; this plan sends `reason` on kick (`json!({"reason":..})` via `Route::KickMember`) and ban (`ban_with_reason`). Audit-log reasons are then populated.

**Exit-code convergence for admin:** the base plan's `0/1/2/3/4/5` split finally uses `4 = FORBIDDEN` (403 `PermissionDenied`) — admin ops are the first commands that routinely see 403s, so this is where the defined-but-unused code becomes live.

**Doc-driven permission matrix (discli SCHEMA.md):** `docs/ADMIN.md` lists required permission per command (MANAGE_CHANNELS for channel CRUD/perm set, MANAGE_ROLES for role ops, KICK/BAN_MEMBERS, MANAGE_GUILD for server set, VIEW_AUDIT_LOG, MANAGE_WEBHOOKS) so a user-token holder knows which commands 403 before running them.

## 19. Open Items / Future Work

- **NICE-TO-HAVE not in the MUST-HAVE surface (each is a small, crate-backed bead — see §2.2):**
  - **Crosspost / bulk-delete** — `CrosspostMessage` + `BulkDeleteMessages`; single-message fallback when <2 (crate `ModelError::BulkDeleteAmount`); 14-day age-limit surfacing; `--confirm`; MANAGE_MESSAGES gated. Reference implements bulk-delete only (`message.ts:233-264`), so crosspost is bonus.
  - **Webhook management** — `WebhookOps` (un-gated); create/delete/edit/list-channel; token leak warning; discli has it only as an unchecked roadmap item (`README.md:368`) so no parity requirement; crate ships `dcw_webhooks.rs` to crib.
  - **Sticker CRUD** — multipart via `post_raw_multipart` (keep crate `cli` feature OFF); PNG/APNG/GIF/Lottie ≤512KiB validation; MANAGE_GUILD_EXPRESSIONS. Reference has none — additive.
  - **Scheduled events CRUD** — `ScheduledEventOps` + crate `dc_events.rs`; entity_type branching (stage/voice need channel_id; external needs location + end time); future-dated ISO8601; MANAGE_EVENTS.
  - **Guild create/delete** — `Route::CreateGuild`/`DeleteGuild`; user accounts capped at <10 guilds; owner-only delete; `--confirm`; higher ToS flag risk (programmatic guild creation is a classic abuse signal).
  - **Member search** — `GetGuildMembersByQuery` (`GET /guilds/{id}/members/search?query=&limit=`) as a `resolve_member` fallback for guilds >1000 members.
- **`--boost-bar` server-settings flag** — `premium_progress_bar_enabled` is absent from `EditGuildRequest`; would need `Route::Custom` raw JSON. Deferred to keep the crate-types decision clean.
- **`admin_actions` SQLite table** — optional additive audit trail of the CLI's own admin writes (who did what when). Out of scope for the MUST-HAVE batch.
- **Fake API server for admin testing** — the codebase has no mock-HTTP harness and `DiscordHttpClient` does not expose an injectable transport; a `#[cfg(test)]` HTTP-layer fake (famasya's `API`-trait pattern) is a possible follow-up but not required for the pure-helper + smoke strategy above.
- **Voice state** — **SKIP** (per research): no discli parity requirement, rare for user tokens, and the crate's voice support is gateway-heavy (`MUTE/DEAFEN/MOVE` are voice-server operations without meaningful REST surface).
- **`--confirm` audit** for the two unlock-style actions (`perm unlock`, `member unban`) — the base plan's open item extends to admin; both are currently planned with `--confirm` but should be reviewed once more at implementation.
- **Permission denial messaging**: decide whether `PermissionDenied` should print the raw Discord `message` (e.g. "Missing Permissions") or a computed friendly string — favor the former (crate already carries it).
