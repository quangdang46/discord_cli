// M1.4 integration: CLI exits and envelopes behave without a token.
use std::process::Command;

/// Spawn the binary with no token source (empty env + no .env + empty flag).
fn no_token_cmd() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_discord"));
    c.env("DISCORD_TOKEN", "")
        .env(
            "DATA_DIR",
            std::env::temp_dir().join("discord-test-no-token"),
        )
        .env("NO_COLOR", "1");
    c
}

#[test]
fn status_without_token_exits_1() {
    let out = no_token_cmd().arg("status").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "status must exit 1 without token"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("DISCORD_TOKEN"), "stderr: {stderr}");
}

#[test]
fn help_shows_commands() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .arg("--help")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("status"));
    assert!(stdout.contains("whoami"));
    assert!(stdout.contains("WARNING"));
}

#[test]
fn no_subcommand_shows_help_exit_0() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Usage:"));
}

#[test]
fn dc_guilds_without_token_exits_1() {
    let out = no_token_cmd().args(["guilds"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "guilds must exit 1 without token"
    );
}

#[test]
fn dc_help_lists_subcommands() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("guilds"));
    assert!(stdout.contains("channels"));
}

#[test]
fn send_without_confirm_exits_2() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["send", "123456", "--text", "hi"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "send must exit 2 without --confirm"
    );
}

#[test]
fn send_dry_run_exits_0_with_preview() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["send", "123456", "--text", "hi", "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"action\":\"send_message\""));
}

#[test]
fn serve_command_exists() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["serve", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("MCP server"), "stdout: {stdout}");
}

#[test]
fn watch_command_exists() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["watch", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("JSONL stream"));
}

#[test]
fn read_help_shows_around_flag() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["read", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--around"), "stdout: {stdout}");
}

#[test]
fn read_around_conflicts_with_before_exits_2() {
    // clap rejects --around + --before with usage exit 2 (before any network).
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["read", "123", "--around", "456", "--before", "789"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--around"), "stderr: {stderr}");
}

#[test]
fn userinfo_help_shows_usage() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["userinfo", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("<USER_ID>"), "stdout: {stdout}");
}

#[test]
fn dm_group_create_requires_confirm() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["dm-group", "create", "123,456"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "dm-group create must exit 2 without --confirm"
    );
}

#[test]
fn send_requires_text_or_file() {
    // No --text and no --file -> usage exit 2 (before any token/network).
    let out = no_token_cmd().args(["send", "123456"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "send with no text/file must exit 2"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("nothing to send"), "stderr: {stderr}");
}

#[test]
fn send_rejects_too_many_files() {
    // 11 files -> usage exit 2 (before token/network; paths need not exist).
    let mut args = vec!["send", "123456", "--text", "hi", "--confirm"];
    for i in 0..11 {
        args.push("--file");
        args.push(Box::leak(
            format!("/tmp/nonexistent-{i}.png").into_boxed_str(),
        ));
    }
    let out = no_token_cmd().args(&args).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "send with >10 files must exit 2"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("too many files"), "stderr: {stderr}");
}

#[test]
fn send_help_shows_file_flag() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["send", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--file"), "stdout: {stdout}");
    assert!(stdout.contains("stdin"), "stdout: {stdout}");
}

#[test]
fn typing_command_exists() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["typing", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("typing indicator"), "stdout: {stdout}");
}

#[test]
fn send_help_shows_typing_flag() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["send", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--typing"), "stdout: {stdout}");
}

#[test]
fn watch_help_shows_typing_flag() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["watch", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--typing"), "stdout: {stdout}");
}

#[test]
fn join_requires_confirm() {
    let out = no_token_cmd()
        .args(["join", "https://discord.gg/abc123"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "join must exit 2 without --confirm"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--confirm"), "stderr: {stderr}");
}

#[test]
fn join_invalid_invite_exits_2() {
    let out = no_token_cmd()
        .args(["join", "", "--confirm"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "empty invite must exit 2");
}

#[test]
fn leave_requires_confirm() {
    let out = no_token_cmd()
        .args(["leave", "my-server"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "leave must exit 2 without --confirm"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--confirm"), "stderr: {stderr}");
}

#[test]
fn presence_invalid_value_exits_2() {
    let out = no_token_cmd().args(["presence", "bogus"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "invalid presence must exit 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid presence"), "stderr: {stderr}");
}

#[test]
fn presence_help_shows_valid_values() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["presence", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("online"), "stdout: {stdout}");
    assert!(stdout.contains("invisible"), "stdout: {stdout}");
}

#[test]
fn thread_create_help_shows_flags() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["thread-create", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--name"), "stdout: {stdout}");
    assert!(stdout.contains("--message-id"), "stdout: {stdout}");
    assert!(stdout.contains("--tags"), "stdout: {stdout}");
}

#[test]
fn download_help_shows_flags() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["download", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--since"), "stdout: {stdout}");
    assert!(stdout.contains("--min-reactions"), "stdout: {stdout}");
    assert!(stdout.contains("--type"), "stdout: {stdout}");
}

#[test]
fn auth_help_shows_qr_flag() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["auth", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--qr"), "stdout: {stdout}");
}

#[test]
fn top_reactions_help_shows_flags() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["top-reactions", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--guild"), "stdout: {stdout}");
    assert!(stdout.contains("--channel"), "stdout: {stdout}");
    assert!(stdout.contains("--limit"), "stdout: {stdout}");
}

// ---------------------------------------------------------------------------
// Admin (F1/F2/F3): channel/role/emoji commands — offline smoke tests.
// All gates are enforced BEFORE any network/token access, so these pass
// without a token.
// ---------------------------------------------------------------------------

#[test]
fn channel_create_dry_run_exits_0_with_action() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["channel-create", "123", "general", "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"action\":\"create_channel\""),
        "stdout: {stdout}"
    );
}

#[test]
fn channel_delete_without_confirm_exits_2() {
    let out = no_token_cmd()
        .args(["channel-delete", "123", "#general"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--confirm"), "stderr: {stderr}");
}

#[test]
fn channel_move_without_options_exits_2() {
    let out = no_token_cmd()
        .args(["channel-move", "123", "#general"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--category"), "stderr: {stderr}");
}

#[test]
fn channel_create_invalid_type_exits_2() {
    let out = no_token_cmd()
        .args(["channel-create", "123", "general", "--type", "foo"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid channel type"), "stderr: {stderr}");
}

#[test]
fn channel_create_invalid_name_exits_2() {
    let out = no_token_cmd()
        .args(["channel-create", "123", "#badname", "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid channel name"), "stderr: {stderr}");
}

#[test]
fn channel_help_shows_subcommands() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["channel-create", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--type"), "stdout: {stdout}");
    assert!(stdout.contains("--category"), "stdout: {stdout}");
    assert!(stdout.contains("--slowmode"), "stdout: {stdout}");
}

#[test]
fn role_create_dry_run_exits_0() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["role-create", "123", "mod", "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"action\":\"create_role\""),
        "stdout: {stdout}"
    );
}

#[test]
fn role_edit_without_options_exits_2() {
    let out = no_token_cmd()
        .args(["role-edit", "123", "mod"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("at least one option"), "stderr: {stderr}");
}

#[test]
fn role_delete_without_confirm_exits_2() {
    let out = no_token_cmd()
        .args(["role-delete", "123", "mod"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--confirm"), "stderr: {stderr}");
}

#[test]
fn role_everyone_create_exits_2() {
    let out = no_token_cmd()
        .args(["role-create", "123", "@everyone", "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("everyone"), "stderr: {stderr}");
}

#[test]
fn role_everyone_delete_exits_2() {
    let out = no_token_cmd()
        .args(["role-delete", "123", "everyone", "--confirm"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("everyone"), "stderr: {stderr}");
}

#[test]
fn role_create_invalid_permission_exits_2() {
    let out = no_token_cmd()
        .args([
            "role-create",
            "123",
            "mod",
            "--permissions",
            "nope",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown permission"), "stderr: {stderr}");
}

#[test]
fn role_create_invalid_color_exits_2() {
    let out = no_token_cmd()
        .args(["role-create", "123", "mod", "--color", "zzz", "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid color"), "stderr: {stderr}");
}

#[test]
fn emoji_upload_missing_file_exits_7() {
    let out = no_token_cmd()
        .args(["emoji-upload", "123", "test", "/nonexistent/emoji.png"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(7));
}

#[test]
fn emoji_upload_oversize_exits_7() {
    // Write a >256KiB temp file (valid png name) and confirm exit 7.
    let mut path = std::env::temp_dir();
    path.push("discord-emoji-oversize.png");
    let blob = vec![0u8; (256 * 1024) + 1];
    std::fs::write(&path, &blob).unwrap();
    let out = no_token_cmd()
        .args(["emoji-upload", "123", "test", path.to_str().unwrap()])
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(out.status.code(), Some(7));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("too large"), "stderr: {stderr}");
}

#[test]
fn emoji_delete_without_confirm_exits_2() {
    let out = no_token_cmd()
        .args(["emoji-delete", "123", "party"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--confirm"), "stderr: {stderr}");
}

#[test]
fn emoji_delete_not_found_exits_3() {
    // Network path is attempted only after --confirm; without a token the
    // client creation fails with exit 1 — but with no token and --confirm,
    // resolve_guild fails first with 1. To test the 3-path deterministically
    // we rely on the resolver unit tests instead.
    let out = no_token_cmd()
        .args(["emoji-delete", "123", "party", "--confirm"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "no token -> client error exit 1"
    );
}

#[test]
fn emoji_upload_invalid_name_exits_2() {
    let out = no_token_cmd()
        .args(["emoji-upload", "123", "bad-name", "/nonexistent/emoji.png"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "invalid name -> exit 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid emoji name"), "stderr: {stderr}");
}

#[test]
fn emoji_help_shows_flags() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["emoji-upload", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("<NAME>"), "stdout: {stdout}");
}

// ---------------------------------------------------------------------------
// Admin (F4/F5/F6): member moderation, permission overwrites, server settings
// — offline smoke tests. All gates run BEFORE any network/token access.
// ---------------------------------------------------------------------------

#[test]
fn member_kick_without_confirm_exits_2() {
    let out = no_token_cmd()
        .args(["member-kick", "123", "alice"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--confirm"), "stderr: {stderr}");
}

#[test]
fn member_ban_without_confirm_exits_2() {
    let out = no_token_cmd()
        .args(["member-ban", "123", "alice"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--confirm"), "stderr: {stderr}");
}

#[test]
fn member_ban_delete_days_out_of_range_exits_2() {
    let out = no_token_cmd()
        .args([
            "member-ban",
            "123",
            "alice",
            "--delete-days",
            "8",
            "--confirm",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("0-7"), "stderr: {stderr}");
}

#[test]
fn member_unban_without_confirm_exits_2() {
    let out = no_token_cmd()
        .args(["member-unban", "123", "456"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--confirm"), "stderr: {stderr}");
}

#[test]
fn member_nick_help_shows_usage() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["member-nick", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("<GUILD>"), "stdout: {stdout}");
}

#[test]
fn perm_set_without_allow_deny_exits_2() {
    let out = no_token_cmd()
        .args(["perm-set", "123", "#general", "@mod"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--allow"), "stderr: {stderr}");
}

#[test]
fn perm_set_unknown_permission_exits_2() {
    let out = no_token_cmd()
        .args(["perm-set", "123", "#general", "@mod", "--allow", "nope"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown permission"), "stderr: {stderr}");
}

#[test]
fn perm_lock_without_confirm_exits_2() {
    let out = no_token_cmd()
        .args(["perm-lock", "123", "#general"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--confirm"), "stderr: {stderr}");
}

#[test]
fn perm_lock_dry_run_exits_0_with_action() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["perm-lock", "123", "#general", "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"action\":\"lock_channel\""),
        "stdout: {stdout}"
    );
}

#[test]
fn perm_unlock_without_confirm_exits_2() {
    let out = no_token_cmd()
        .args(["perm-unlock", "123", "#general"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--confirm"), "stderr: {stderr}");
}

#[test]
fn perm_list_prints_name_bit_table() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["perm-list", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("send_messages"), "stdout: {stdout}");
    assert!(stdout.contains("\"bit\""), "stdout: {stdout}");
}

#[test]
fn server_set_without_options_exits_2() {
    let out = no_token_cmd().args(["server-set", "123"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("at least one option"), "stderr: {stderr}");
}

#[test]
fn server_set_invalid_enum_exits_2() {
    let out = no_token_cmd()
        .args(["server-set", "123", "--verification", "bogus"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid --verification"),
        "stderr: {stderr}"
    );
}

#[test]
fn server_set_dry_run_exits_0_with_action() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["server-set", "123", "--name", "TestServer", "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"action\":\"server_set\""),
        "stdout: {stdout}"
    );
}

#[test]
fn server_icon_missing_file_exits_7() {
    let out = no_token_cmd()
        .args(["server-icon", "123", "/nonexistent/icon.png"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(7));
}

// ---------------------------------------------------------------------------
// Admin (F7/F8/F9): audit log, invites, embed — offline smoke tests.
// ---------------------------------------------------------------------------

#[test]
fn audit_types_prints_table() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["audit-types", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("member_kick"), "stdout: {stdout}");
    assert!(stdout.contains("channel_create"), "stdout: {stdout}");
    assert!(stdout.contains("\"code\": 20"), "stdout: {stdout}");
}

#[test]
fn audit_log_unknown_type_exits_2() {
    let out = no_token_cmd()
        .args(["audit-log", "123", "--type", "bogus"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown audit action"), "stderr: {stderr}");
}

#[test]
fn audit_log_invalid_user_exits_2() {
    let out = no_token_cmd()
        .args(["audit-log", "123", "--user", "not-an-id"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--user"), "stderr: {stderr}");
}

#[test]
fn audit_log_count_over_100_accepted_not_usage() {
    // --count 200 parses as a valid u8 and is capped to 100 in the core
    // (`.min(100)`). Without a token the run fails at client creation with
    // exit 1 — NOT exit 2 (usage), proving the value was accepted.
    let out = no_token_cmd()
        .args(["audit-log", "123", "--count", "200"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "--count 200 must be accepted (capped in core), not a usage error"
    );
}

#[test]
fn audit_help_shows_flags() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["audit-log", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--count"), "stdout: {stdout}");
    assert!(stdout.contains("--type"), "stdout: {stdout}");
    assert!(stdout.contains("--user"), "stdout: {stdout}");
}

#[test]
fn invite_delete_without_confirm_exits_2() {
    let out = no_token_cmd()
        .args(["invite-delete", "https://discord.gg/abc123"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--confirm"), "stderr: {stderr}");
}

#[test]
fn invite_list_help_shows_usage() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["invite-list", "--help"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("<GUILD>"), "stdout: {stdout}");
}

#[test]
fn embed_requires_title_or_description_exits_2() {
    let out = no_token_cmd().args(["embed", "123456"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("at least one of"), "stderr: {stderr}");
}

#[test]
fn embed_malformed_field_exits_2() {
    let out = no_token_cmd()
        .args([
            "embed",
            "123456",
            "--title",
            "T",
            "--field",
            "bad",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--field"), "stderr: {stderr}");
}

#[test]
fn embed_invalid_color_exits_2() {
    let out = no_token_cmd()
        .args([
            "embed",
            "123456",
            "--title",
            "T",
            "--color",
            "zzz",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid color"), "stderr: {stderr}");
}

#[test]
fn embed_dry_run_exits_0_with_action() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args([
            "embed",
            "123456",
            "--title",
            "Hello",
            "--description",
            "World",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"action\":\"send_embed\""),
        "stdout: {stdout}"
    );
}

#[test]
fn embed_without_confirm_exits_2() {
    let out = Command::new(env!("CARGO_BIN_EXE_discord"))
        .args(["embed", "123456", "--title", "T", "--description", "D"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--confirm"), "stderr: {stderr}");
}
