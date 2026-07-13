use super::*;

#[test]
fn remote_proxy_command_has_no_account_arguments() {
    let transport = SshTransport::new(PathBuf::from("/tmp/control-master.sock"), true);

    let command = transport.remote_proxy_command();

    assert!(command.ends_with("remote-server-proxy"));
    assert!(!command.contains("identity"));
}
