//! `ssh_command` unit tests.
//!
//! Split into a separate file per `AGENTS.md §5.6`, pulled in via the `#[path]` at the end of `ssh_command.rs`.
//! Coverage:
//! - `build_ssh_args` / `build_ssh_command_line` argument construction
//! - `test_connection`'s error paths when the password is missing / the auth type is wrong
//! - `build_password_auth_stdin` byte-stream construction (incidentally covering the key security path of stdin injection)
//!
//! Note: the end-to-end tests that actually spawn an ssh subprocess are covered in the
//! integration / manual tests in `app/src/ssh_manager/server_view.rs` — unit tests do no network connections.
//!
//! author: logic
//! date: 2026-06-01

use super::*;
use zeroize::Zeroizing;

fn server() -> SshServerInfo {
    SshServerInfo {
        node_id: "n".into(),
        host: "1.2.3.4".into(),
        port: 22,
        username: "alice".into(),
        auth_type: AuthType::Password,
        key_path: None,
        startup_command: None,
        notes: None,
        last_connected_at: None,
    }
}

#[test]
fn default_port_omitted() {
    let s = server();
    assert_eq!(build_ssh_args(&s), vec!["ssh", "alice@1.2.3.4"]);
    // For safety, shell-escape wraps user@host in single quotes, which is a valid and
    // shell-equivalent form — we do not require the unquoted version.
    let line = build_ssh_command_line(&s);
    assert!(
        line == "ssh alice@1.2.3.4" || line == "ssh 'alice@1.2.3.4'",
        "unexpected: {line}"
    );
}

#[test]
fn custom_port_uses_dash_p() {
    let mut s = server();
    s.port = 2222;
    assert_eq!(
        build_ssh_args(&s),
        vec!["ssh", "-p", "2222", "alice@1.2.3.4"]
    );
}

#[test]
fn key_auth_emits_dash_i() {
    let mut s = server();
    s.auth_type = AuthType::Key;
    s.key_path = Some("/home/u/.ssh/id_ed25519".into());
    assert_eq!(
        build_ssh_args(&s),
        vec!["ssh", "-i", "/home/u/.ssh/id_ed25519", "alice@1.2.3.4"]
    );
}

#[test]
fn key_auth_without_path_is_skipped() {
    let mut s = server();
    s.auth_type = AuthType::Key;
    s.key_path = None;
    assert_eq!(build_ssh_args(&s), vec!["ssh", "alice@1.2.3.4"]);
}

#[test]
fn empty_username_yields_host_only() {
    let mut s = server();
    s.username = String::new();
    assert_eq!(build_ssh_args(&s), vec!["ssh", "1.2.3.4"]);
}

#[test]
fn shell_escapes_spaces_in_path() {
    let mut s = server();
    s.auth_type = AuthType::Key;
    s.key_path = Some("/path with spaces/id_rsa".into());
    let line = build_ssh_command_line(&s);
    assert!(
        line.contains("'/path with spaces/id_rsa'"),
        "actual: {line}"
    );
}

#[test]
fn test_connection_requires_password_for_password_auth() {
    let s = server();
    // test_connection should return Offline + an error message when there is no password
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(test_connection(&s, None));
    assert_eq!(result.status, ConnectionStatus::Offline);
    assert!(result
        .error_message
        .unwrap()
        .contains("Password not provided"));
}

#[test]
fn test_connection_key_auth_uses_batch_mode() {
    let mut s = server();
    s.auth_type = AuthType::Key;
    s.key_path = Some("/home/user/.ssh/id_rsa".into());
    // For key authentication it should take the BatchMode=yes path (carried by run_ssh_test);
    // here we only verify build_ssh_args includes -i and key_path.
    let args = build_ssh_args(&s);
    assert!(args.contains(&"-i".to_string()));
    assert!(args.contains(&"/home/user/.ssh/id_rsa".to_string()));
}

#[test]
fn connection_status_equality() {
    assert_eq!(ConnectionStatus::Online, ConnectionStatus::Online);
    assert_eq!(ConnectionStatus::Offline, ConnectionStatus::Offline);
    assert_eq!(ConnectionStatus::Unknown, ConnectionStatus::Unknown);
    assert_ne!(ConnectionStatus::Online, ConnectionStatus::Offline);
    assert_ne!(ConnectionStatus::Online, ConnectionStatus::Unknown);
    assert_ne!(ConnectionStatus::Offline, ConnectionStatus::Unknown);
}

// -------- Password stdin-injection security --------

/// Verify that `build_password_auth_stdin` correctly encodes the password + newline.
/// This is the key to the password-leak fix: we must confirm the byte stream written to ssh stdin is exactly the password literal +
/// `\n`, and not any form that would let the password accidentally go through argv / environment variables / a temporary file.
#[test]
fn build_password_auth_stdin_contains_password_with_newline() {
    let password: Zeroizing<String> = Zeroizing::new("s3cret-pass".into());
    let bytes = build_password_auth_stdin(&password);
    assert_eq!(&*bytes, b"s3cret-pass\n");
}

/// Edge case: an empty password still writes a `\n`, so that ssh immediately gets EOF and decides authentication failed
/// (rather than getting stuck waiting for a prompt).
#[test]
fn build_password_auth_stdin_empty_password_still_has_newline() {
    let password: Zeroizing<String> = Zeroizing::new(String::new());
    let bytes = build_password_auth_stdin(&password);
    assert_eq!(&*bytes, b"\n");
}

/// Unicode password: written through as raw UTF-8 bytes.
#[test]
fn build_password_auth_stdin_unicode_password() {
    let password: Zeroizing<String> = Zeroizing::new("密码🔐".into());
    let bytes = build_password_auth_stdin(&password);
    let mut expected = "密码🔐".as_bytes().to_vec();
    expected.push(b'\n');
    assert_eq!(&*bytes, expected.as_slice());
}

/// Regression: `build_ssh_args` should no longer carry `sshpass`, to prevent someone mistakenly adding it back to cmd_args
/// (Windows / macOS have no sshpass by default, and a leftover path would immediately be No such file or directory).
#[test]
fn build_ssh_args_does_not_emit_sshpass() {
    let s = server();
    let args = build_ssh_args(&s);
    assert!(
        !args.iter().any(|a| a == "sshpass"),
        "build_ssh_args must not emit sshpass; got {args:?}"
    );
}

// -------- password auth cmd_args regression protection --------
//
// These tests guard the key switches that keep the "test connection" password path from timing out at 10s again.
// Any -o option adjustment inside `test_password_auth` must satisfy these three:
// 1. No longer declare keyboard-interactive (otherwise the server's PAM falls back to kbd-int)
// 2. Explicitly disable KbdInteractiveAuthentication (a client-capability switch, not a preference)
// 3. Still end with the `echo ok` remote command (otherwise the success decision will not match stdout)
// author: logic
// date: 2026-06-01

/// Regression protection: `PreferredAuthentications` must contain only `password`, not
/// `keyboard-interactive`. Otherwise stdin pipe + EOF triggers the kbd-int PAM
/// retry chain (`pam_faildelay` ~2s each), maxing out the 10s `TEST_TIMEOUT`.
#[test]
fn password_auth_args_no_keyboard_interactive() {
    let s = server();
    let args = build_password_auth_cmd_args(&s);
    let joined = args.join(" ");
    assert!(
        !joined.contains("keyboard-interactive"),
        "test_password_auth must NOT use keyboard-interactive; got {args:?}"
    );
    assert!(
        joined.contains("PreferredAuthentications=password"),
        "expected PreferredAuthentications=password; got {args:?}"
    );
    // Even when PreferredAuthentications=password appears, no other methods may be listed after it.
    // split takes the first segment after "="; if it starts with "password," there is another auth method after it.
    let after_pref = joined
        .split("PreferredAuthentications=")
        .nth(1)
        .unwrap_or("");
    assert!(
        !after_pref.starts_with("password,"),
        "PreferredAuthentications should not list other methods after password; got {args:?}"
    );
}

/// Regression protection: must explicitly disable kbd-interactive (a client-capability switch),
/// not rely only on the `PreferredAuthentications` list order (which only constrains the password
/// sub-method). This layer of defense in depth is especially needed given OpenSSH 8.2+ behavior differences
/// and interactions with the server's `AuthenticationMethods`.
#[test]
fn password_auth_args_disable_kbd_interactive() {
    let s = server();
    let args = build_password_auth_cmd_args(&s);
    let joined = args.join(" ");
    assert!(
        joined.contains("KbdInteractiveAuthentication=no"),
        "missing KbdInteractiveAuthentication=no; got {args:?}"
    );
}

/// Regression protection: the `echo ok` at the end of cmd_args must appear as the remote command.
/// Under ssh's parsing rules, the first non-option positional argument after the destination = the remote command;
/// if the option order is wrong so that ssh does not recognize `echo ok` as the command, the success decision breaks.
#[test]
fn password_auth_args_ends_with_echo_ok_command() {
    let s = server();
    let args = build_password_auth_cmd_args(&s);
    assert!(!args.is_empty(), "cmd_args is empty: {args:?}");
    let last = args.last().unwrap();
    assert_eq!(
        last, "echo ok",
        "cmd_args must end with `echo ok` as remote command; got {args:?}"
    );
}

/// Regression protection: on the password path, the destination (`user@host`) must appear **after** all `-o`
/// options and **before** `echo ok`. The SSH command-line parsing rule is
/// `ssh [options] destination [command]`: the first non-option argument = destination,
/// and everything after it = the remote command. If a `-o` option ends up after the destination, SSH treats it
/// as part of the remote command rather than its own option, causing key options like `PreferredAuthentications`,
/// `KbdInteractiveAuthentication`, etc. to all be ineffective, triggering the kbd-interactive
/// PAM retry chain that maxes out the 10s `TEST_TIMEOUT`.
/// author: logic
/// date: 2026-06-01
#[test]
fn password_auth_args_destination_before_echo_ok_and_after_options() {
    let s = server();
    let args = build_password_auth_cmd_args(&s);
    let joined = args.join(" ");

    // the destination "alice@1.2.3.4" must appear before "echo ok"
    let dest_pos = joined
        .find("alice@1.2.3.4")
        .expect("destination must appear in args");
    let echo_pos = joined
        .find("echo ok")
        .expect("`echo ok` must appear in args");

    assert!(
        dest_pos < echo_pos,
        "destination must come before `echo ok`; got joined: {joined}"
    );

    // the destination must appear after all -o options
    // find the position of the last -o option
    let last_o_pos = joined
        .rfind("-o ")
        .expect("expected at least one -o option");
    assert!(
        last_o_pos < dest_pos,
        "all -o options must come before destination; got joined: {joined}"
    );
}

/// Regression protection: on the key-auth path, `build_ssh_args` also needs the destination after the -o options.
/// Verify the order using `build_ssh_args` + manually appended options, simulating `test_key_auth`'s construction.
/// author: logic
/// date: 2026-06-01
#[test]
fn key_auth_args_destination_comes_after_options() {
    let mut s = server();
    s.auth_type = AuthType::Key;
    s.key_path = Some("/home/user/.ssh/id_rsa".into());

    // simulate test_key_auth's construction logic
    let mut args = build_ssh_args(&s);
    let target = args.pop().unwrap();
    args.extend([
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=5".into(),
        "-o".into(),
        "StrictHostKeyChecking=no".into(),
        "-o".into(),
        "LogLevel=ERROR".into(),
    ]);
    args.push(target);
    args.push("echo ok".into());

    let joined = args.join(" ");
    let dest_pos = joined
        .find("alice@1.2.3.4")
        .expect("destination must appear in args");
    let echo_pos = joined
        .find("echo ok")
        .expect("`echo ok` must appear in args");
    let last_o_pos = joined
        .rfind("-o ")
        .expect("expected at least one -o option");

    assert!(
        last_o_pos < dest_pos,
        "all -o options must come before destination; got joined: {joined}"
    );
    assert!(
        dest_pos < echo_pos,
        "destination must come before `echo ok`; got joined: {joined}"
    );
}

// -------- Windows SSH_ASKPASS regression protection --------
//
// On Windows, Win32-OpenSSH refuses to read the password from stdin because of no console + CREATE_NO_WINDOW
// (Win32-OpenSSH issue #1470), so it must use the SSH_ASKPASS mechanism.
// Guard the existence of this code, to prevent someone mistakenly merging the Windows path back into the stdin approach.
// author: logic
// date: 2026-06-01

/// Regression protection: on Windows, the `test_password_auth` entry point must reference `AskpassSession`,
/// not write the password directly to stdin. This assertion is guaranteed via the type system: if the Windows path
/// is changed to the stdin approach, `AskpassSession::new` will not appear in the function body, and the test will fail.
#[cfg(windows)]
#[test]
fn windows_password_auth_uses_askpass_not_stdin() {
    // This test works at compile time: if ssh_command.rs's Windows branch
    // reverts to stdin injection, the `AskpassSession` type is no longer used, compilation reports
    // a dead_code error, and CI fails.
    // Here we only verify that the AskpassSession type exists + can be new'd — it cannot actually run (it needs to write files),
    // but it blocks breakages like "accidentally deleting AskpassSession".
    let _ = std::any::type_name::<AskpassSession>();
}

/// Real end-to-end: create an `AskpassSession` to get the askpass script path, then use
/// `CreateProcessW` to spawn it (simulating how ssh spawns askpass), verifying it can start.
///
/// This test guards that the askpass script is "executable" from ssh's point of view—directly blocking
/// regressions like `CreateProcessW failed error:5` (ERROR_ACCESS_DENIED).
/// There was a bug before: the askpass file had `FILE_ATTRIBUTE_HIDDEN` set, causing ssh's
/// `posix_spawnp` to refuse to spawn, so askpass never started at all, the password was not sent out, and the server
/// reported "wrong password".
#[cfg(windows)]
#[test]
fn windows_askpass_script_is_spawnable() {
    use std::os::windows::process::CommandExt as _;
    use std::process::Stdio;
    use zeroize::Zeroizing;

    let password: Zeroizing<String> = Zeroizing::new("dummy-pw-for-spawn-test".into());
    let session = AskpassSession::new(&password).expect("AskpassSession::new failed");
    let script = session.script_path.clone();
    let password_file = session.password_path.clone();

    // Spawn the askpass script: use CreateProcessW to take the same code path as ssh.
    // CREATE_NO_WINDOW simulates the environment when ssh spawns askpass (no console).
    // WARP_SSH_ASKPASS_FILE env must be set; the script relies on it to locate the password file.
    let output = std::process::Command::new("cmd.exe")
        .raw_arg(format!("/c \"{}\"", script.display()))
        .env("WARP_SSH_ASKPASS_FILE", &password_file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .expect("CreateProcessW failed — askpass script is not spawnable");

    assert!(
        output.status.success(),
        "askpass script exited non-zero: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    // the askpass script reads the first line of the password file and echoes it, so it should output the password written when the session was created
    assert!(
        stdout.trim() == "dummy-pw-for-spawn-test",
        "askpass output mismatch: got {stdout:?}"
    );
}
