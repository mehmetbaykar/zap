/// resolve_test_password unit tests
/// author: logic
/// date: 2026/06/01
use super::*;
use std::collections::HashMap;
use std::sync::Mutex;

/// In-process mock that bypasses the OS keychain. Supports error injection to simulate NoBackend / Keyring errors.
struct MockSecretStore {
    inner: Mutex<HashMap<String, String>>,
    get_err: Mutex<Option<SshSecretStoreError>>,
}

impl MockSecretStore {
    fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            get_err: Mutex::new(None),
        }
    }

    fn with_secret(node: &str, kind: SecretKind, value: &str) -> Self {
        let s = Self::new();
        s.set(node, kind, value).unwrap();
        s
    }

    fn inject_get_error(&self, err: SshSecretStoreError) {
        *self.get_err.lock().unwrap() = Some(err);
    }
}

fn account_key(node_id: &str, kind: SecretKind) -> String {
    let suffix = match kind {
        SecretKind::Password => "password",
        SecretKind::Passphrase => "passphrase",
        SecretKind::RootPassword => "root_password",
    };
    format!("{node_id}:{suffix}")
}

impl SshSecretStore for MockSecretStore {
    fn set(
        &self,
        node_id: &str,
        kind: SecretKind,
        secret: &str,
    ) -> Result<(), SshSecretStoreError> {
        self.inner
            .lock()
            .unwrap()
            .insert(account_key(node_id, kind), secret.to_string());
        Ok(())
    }

    fn get(
        &self,
        node_id: &str,
        kind: SecretKind,
    ) -> Result<Option<Zeroizing<String>>, SshSecretStoreError> {
        if let Some(err) = self.get_err.lock().unwrap().take() {
            return Err(err);
        }
        Ok(self
            .inner
            .lock()
            .unwrap()
            .get(&account_key(node_id, kind))
            .cloned()
            .map(Zeroizing::new))
    }

    fn delete(&self, _node_id: &str, _kind: SecretKind) -> Result<(), SshSecretStoreError> {
        unimplemented!()
    }
}

#[test]
fn empty_editor_empty_store_returns_none() {
    let store = MockSecretStore::new();
    assert!(resolve_test_password("n1", "", &store).is_none());
}

#[test]
fn empty_editor_stored_returns_secret() {
    let store = MockSecretStore::with_secret("n1", SecretKind::Password, "from-keychain");
    let pw = resolve_test_password("n1", "", &store).unwrap();
    assert_eq!(&*pw, "from-keychain");
}

#[test]
fn filled_editor_ignores_keychain() {
    // The keychain stored an old password, the form typed a new one → we must
    // use the form's new password, otherwise the test would be polluted by the
    // old password after the user changes the host.
    let store = MockSecretStore::with_secret("n1", SecretKind::Password, "old-pw");
    let pw = resolve_test_password("n1", "new-pw", &store).unwrap();
    assert_eq!(&*pw, "new-pw");
}

#[test]
fn empty_editor_no_backend_returns_none() {
    let store = MockSecretStore::new();
    store.inject_get_error(SshSecretStoreError::NoBackend);
    assert!(resolve_test_password("n1", "", &store).is_none());
}

#[test]
fn empty_editor_keyring_error_returns_none() {
    let store = MockSecretStore::new();
    store.inject_get_error(SshSecretStoreError::Keyring("locked".into()));
    assert!(resolve_test_password("n1", "", &store).is_none());
}
