use super::bytes_look_like_password_prompt;

fn matches(input: &str) -> bool {
    bytes_look_like_password_prompt(input.as_bytes())
}

#[test]
fn matches_typical_password_prompt() {
    assert!(matches("user@host's password: "));
    assert!(matches("Password:"));
    assert!(matches("password: \r\n"));
}

#[test]
fn matches_sudo_password_prompt() {
    assert!(matches("[sudo] password for alice: "));
}

#[test]
fn matches_passphrase_prompt() {
    assert!(matches("Enter passphrase for key '/home/u/.ssh/id_rsa': "));
}

#[test]
fn does_not_match_motd_with_password_word() {
    assert!(!matches("Welcome! Please change your password soon.\n# "));
    assert!(!matches(
        "Last login: Mon Jan 1 password rotated yesterday\n"
    ));
}

#[test]
fn does_not_match_no_colon() {
    assert!(!matches("password\n"));
    assert!(!matches("Enter password please\n"));
}

/// This matcher is driven entirely by bytes the REMOTE HOST chooses, so it can only ever be a hint —
/// never an authorization to release a secret. These are real payloads from a working exploit against
/// a local sshd: each one made the removed `secret_injector` type a stored keychain credential into
/// the remote shell. The banner case lands before authentication, and the MOTD case lands after a
/// successful public-key login, where ssh never prompts at all.
///
/// They still match, and that is the point of pinning them here: anything that consumes this matcher
/// must assume a hostile host can trigger it at will. Do not use it to gate writing a secret to the
/// PTY. See `Workspace::open_ssh_terminal` for what replaced that, and note upstream Warp detects
/// prompts from local termios state (`!ECHO && ICANON`) instead, which a remote host cannot forge.
#[test]
fn matches_host_controlled_text_so_must_not_authorize_secret_release() {
    // sshd `Banner`, rendered pre-auth.
    assert!(matches(
        "*** CORPORATE GATEWAY ***\r\nEnter your domain Password: \r\n"
    ));
    // MOTD / shell rc line, rendered post-login.
    assert!(matches(
        "Welcome to Prod-01\r\nSecurity check. Password: \r\n"
    ));
    // The match need not be at the end of the buffer: trailing output does not clear it.
    assert!(matches(
        "Banner 1\nSecurity check. Password: \nBanner 3\nlast@host:~$ "
    ));
}

/// Conversely, the matcher silently misses styled prompts, so a miss proves nothing either. A real
/// prompt wrapped in SGR colour never matches, which is why absence of a match cannot be treated as
/// "no prompt is pending".
#[test]
fn does_not_match_ansi_styled_prompt() {
    assert!(!matches("\x1b[31mPassword:\x1b[0m "));
}
