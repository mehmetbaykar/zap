use super::*;

#[test]
fn remote_proxy_command_uses_local_nonsecret_identity() {
    let transport = SshTransport::new(PathBuf::from("/tmp/control-master.sock"), true);

    let command = transport.remote_proxy_command();

    assert!(command.ends_with("remote-server-proxy --identity-key zap-local"));
    assert!(!command.contains("token"));
    assert!(!command.contains("auth"));
}
