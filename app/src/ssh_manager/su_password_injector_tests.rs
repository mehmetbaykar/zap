use super::{is_su_to_root, PASSWORD_PROMPT_REGEX, SU_ROOT_CMD_REGEX};

fn pw_matches(input: &str) -> bool {
    PASSWORD_PROMPT_REGEX.is_match(input.as_bytes())
}

fn su_matches(input: &str) -> bool {
    SU_ROOT_CMD_REGEX.is_match(input.as_bytes())
}

#[test]
fn password_prompt_matches_typical_forms() {
    // Halfwidth colon
    assert!(pw_matches("Password:"));
    assert!(pw_matches("Password: "));
    assert!(pw_matches("[sudo] password for alice: "));
    assert!(pw_matches("user@host's password: "));
    // Fullwidth colon (Chinese input method)
    assert!(pw_matches("密码:"));
    assert!(pw_matches("密码："));
    // Galaxy Kylin V10 colon-less special case
    assert!(pw_matches("输入密码"));
    assert!(pw_matches("输入密码 "));
    // passphrase
    assert!(pw_matches(
        "Enter passphrase for key '/home/u/.ssh/id_rsa': "
    ));
}

#[test]
fn password_prompt_rejects_false_positives() {
    // These all contain 'password' / '密码' but are not real prompts, so they must not false-positive
    assert!(!pw_matches("Your password has expired"));
    assert!(!pw_matches("Bad password, try again"));
    assert!(!pw_matches("password changed successfully"));
    assert!(!pw_matches("New password for root"));
    assert!(!pw_matches("Welcome! Please change your password soon.\n"));
    assert!(!pw_matches(
        "Last login: Mon Jan 1 password rotated yesterday\n"
    ));
    // Same for Chinese
    assert!(!pw_matches("您的密码已过期"));
}

#[test]
fn su_root_matches_common_variants() {
    // Most basic
    assert!(su_matches("su"));
    assert!(su_matches("su\n"));
    // Shortcut form without a username (defaults to root)
    assert!(su_matches("su -"));
    assert!(su_matches("su -l"));
    assert!(su_matches("su --login"));
    // Explicit root
    assert!(su_matches("su root"));
    assert!(su_matches("su - root"));
    assert!(su_matches("su -l root"));
    assert!(su_matches("su --login root"));
    // sudo su (\bsu still matches)
    assert!(su_matches("sudo su"));
}

#[test]
fn su_to_other_user_does_not_match() {
    // Switching to a non-root user should not trigger
    assert!(!su_matches("su lg"));
    assert!(!su_matches("su - lg"));
    assert!(!su_matches("su -l lg"));
    assert!(!su_matches("su --login lg"));
    assert!(!su_matches("su admin"));
}

#[test]
fn su_in_middle_of_other_command_does_not_match() {
    // su not at end-of-line should not trigger
    assert!(!su_matches("susan"));
    assert!(!su_matches("issue"));
    // A command like `grep su file`: the end-of-line is neither su nor the su root pattern
    assert!(!su_matches("grep su /etc/passwd"));
}

#[test]
fn is_su_to_root_detects_in_buffer() {
    let buf = b"user@host:~$ su root\r\nPassword: ";
    assert!(is_su_to_root(buf));

    let buf = b"user@host:~$ su lg\r\nPassword: ";
    assert!(!is_su_to_root(buf));
}

#[test]
fn full_pipeline_su_root_with_password_prompt() {
    // Simulate a full PTY sequence: the user enters `su -`, and a password prompt appears after the echo
    let buf = b"alice@kylin:~$ su -\r\n\xe5\xaf\x86\xe7\xa0\x81\xef\xbc\x9a";
    assert!(PASSWORD_PROMPT_REGEX.is_match(buf));
    assert!(is_su_to_root(buf));
}
