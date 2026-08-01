use std::fmt;

use super::SyncTargetId;

const SERVICE: &str = "dev.jshell.sync";
const R2_SECRET_PREFIX: &str = "r2-secret-access-key";
const ENCRYPTION_PASSWORD_PREFIX: &str = "sync-encryption-password";

#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(zeroize::Zeroizing<String>);

impl SecretString {
    pub fn new(secret: String) -> Self {
        Self(zeroize::Zeroizing::new(secret))
    }

    pub fn expose_secret(&self) -> &str {
        self.0.as_str()
    }
}

impl zeroize::ZeroizeOnDrop for SecretString {}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum SyncError {
    InvalidInput(String),
    Network(String),
    Timeout,
    Unauthorized,
    NotFound,
    Conflict,
    PayloadTooLarge { limit: usize },
    DecryptFailed,
    InvalidPayload(String),
    CredentialStore(&'static str),
    LocalSave(String),
}

impl fmt::Display for SyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(_) => formatter.write_str("invalid sync input"),
            Self::Network(_) => formatter.write_str("sync network request failed"),
            Self::Timeout => formatter.write_str("sync request timed out"),
            Self::Unauthorized => formatter.write_str("sync credentials were rejected"),
            Self::NotFound => formatter.write_str("remote sync object was not found"),
            Self::Conflict => formatter.write_str("remote sync object changed"),
            Self::PayloadTooLarge { limit } => {
                write!(formatter, "remote sync payload exceeds {limit} bytes")
            }
            Self::DecryptFailed => {
                formatter.write_str("remote sync payload could not be decrypted")
            }
            Self::InvalidPayload(_) => formatter.write_str("remote sync payload is invalid"),
            Self::CredentialStore(operation) => {
                write!(formatter, "credential store operation failed: {operation}")
            }
            Self::LocalSave(_) => formatter.write_str("local sync save failed"),
        }
    }
}

impl fmt::Debug for SyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(_) => formatter.write_str("InvalidInput([REDACTED])"),
            Self::Network(_) => formatter.write_str("Network([REDACTED])"),
            Self::Timeout => formatter.write_str("Timeout"),
            Self::Unauthorized => formatter.write_str("Unauthorized"),
            Self::NotFound => formatter.write_str("NotFound"),
            Self::Conflict => formatter.write_str("Conflict"),
            Self::PayloadTooLarge { limit } => formatter
                .debug_struct("PayloadTooLarge")
                .field("limit", limit)
                .finish(),
            Self::DecryptFailed => formatter.write_str("DecryptFailed"),
            Self::InvalidPayload(_) => formatter.write_str("InvalidPayload([REDACTED])"),
            Self::CredentialStore(operation) => formatter
                .debug_tuple("CredentialStore")
                .field(operation)
                .finish(),
            Self::LocalSave(_) => formatter.write_str("LocalSave([REDACTED])"),
        }
    }
}

impl std::error::Error for SyncError {}

pub trait SyncCredentialStore {
    fn load_r2_secret(&self, target: &SyncTargetId) -> Result<Option<SecretString>, SyncError>;

    fn store_r2_secret(
        &self,
        target: &SyncTargetId,
        secret: &SecretString,
    ) -> Result<(), SyncError>;

    fn delete_r2_secret(&self, target: &SyncTargetId) -> Result<(), SyncError>;

    fn load_encryption_password(
        &self,
        target: &SyncTargetId,
    ) -> Result<Option<SecretString>, SyncError>;

    fn store_encryption_password(
        &self,
        target: &SyncTargetId,
        password: &SecretString,
    ) -> Result<(), SyncError>;

    fn delete_encryption_password(&self, target: &SyncTargetId) -> Result<(), SyncError>;
}

#[derive(Clone, Copy)]
enum BackendError {
    NoEntry,
    Locked,
    Failed,
}

trait CredentialBackend {
    fn get_password(&self, service: &str, username: &str) -> Result<String, BackendError>;

    fn set_password(
        &self,
        service: &str,
        username: &str,
        password: &str,
    ) -> Result<(), BackendError>;

    fn delete_credential(&self, service: &str, username: &str) -> Result<(), BackendError>;
}

struct CredentialStore<B> {
    backend: B,
}

impl<B> CredentialStore<B> {
    fn new(backend: B) -> Self {
        Self { backend }
    }
}

impl<B: CredentialBackend> CredentialStore<B> {
    fn load(
        &self,
        target: &SyncTargetId,
        prefix: &str,
        operation: &'static str,
    ) -> Result<Option<SecretString>, SyncError> {
        let username = credential_username(prefix, target);
        match self.backend.get_password(SERVICE, &username) {
            Ok(secret) => Ok(Some(SecretString::new(secret))),
            Err(BackendError::NoEntry) => Ok(None),
            Err(BackendError::Locked | BackendError::Failed) => {
                Err(SyncError::CredentialStore(operation))
            }
        }
    }

    fn store(
        &self,
        target: &SyncTargetId,
        prefix: &str,
        secret: &SecretString,
        operation: &'static str,
    ) -> Result<(), SyncError> {
        let username = credential_username(prefix, target);
        self.backend
            .set_password(SERVICE, &username, secret.expose_secret())
            .map_err(|_| SyncError::CredentialStore(operation))
    }

    fn delete(
        &self,
        target: &SyncTargetId,
        prefix: &str,
        operation: &'static str,
    ) -> Result<(), SyncError> {
        let username = credential_username(prefix, target);
        match self.backend.delete_credential(SERVICE, &username) {
            Ok(()) | Err(BackendError::NoEntry) => Ok(()),
            Err(BackendError::Locked | BackendError::Failed) => {
                Err(SyncError::CredentialStore(operation))
            }
        }
    }
}

impl<B: CredentialBackend> SyncCredentialStore for CredentialStore<B> {
    fn load_r2_secret(&self, target: &SyncTargetId) -> Result<Option<SecretString>, SyncError> {
        self.load(target, R2_SECRET_PREFIX, "read R2 secret")
    }

    fn store_r2_secret(
        &self,
        target: &SyncTargetId,
        secret: &SecretString,
    ) -> Result<(), SyncError> {
        self.store(target, R2_SECRET_PREFIX, secret, "write R2 secret")
    }

    fn delete_r2_secret(&self, target: &SyncTargetId) -> Result<(), SyncError> {
        self.delete(target, R2_SECRET_PREFIX, "delete R2 secret")
    }

    fn load_encryption_password(
        &self,
        target: &SyncTargetId,
    ) -> Result<Option<SecretString>, SyncError> {
        self.load(
            target,
            ENCRYPTION_PASSWORD_PREFIX,
            "read encryption password",
        )
    }

    fn store_encryption_password(
        &self,
        target: &SyncTargetId,
        password: &SecretString,
    ) -> Result<(), SyncError> {
        self.store(
            target,
            ENCRYPTION_PASSWORD_PREFIX,
            password,
            "write encryption password",
        )
    }

    fn delete_encryption_password(&self, target: &SyncTargetId) -> Result<(), SyncError> {
        self.delete(
            target,
            ENCRYPTION_PASSWORD_PREFIX,
            "delete encryption password",
        )
    }
}

fn credential_username(prefix: &str, target: &SyncTargetId) -> String {
    format!("{prefix}:{}", target.as_str())
}

struct KeyringBackend;

impl KeyringBackend {
    fn entry(service: &str, username: &str) -> Result<keyring::Entry, BackendError> {
        keyring::Entry::new(service, username).map_err(map_keyring_error)
    }
}

impl CredentialBackend for KeyringBackend {
    fn get_password(&self, service: &str, username: &str) -> Result<String, BackendError> {
        Self::entry(service, username)?
            .get_password()
            .map_err(map_keyring_error)
    }

    fn set_password(
        &self,
        service: &str,
        username: &str,
        password: &str,
    ) -> Result<(), BackendError> {
        Self::entry(service, username)?
            .set_password(password)
            .map_err(map_keyring_error)
    }

    fn delete_credential(&self, service: &str, username: &str) -> Result<(), BackendError> {
        Self::entry(service, username)?
            .delete_credential()
            .map_err(map_keyring_error)
    }
}

fn map_keyring_error(error: keyring::Error) -> BackendError {
    match error {
        keyring::Error::NoEntry => BackendError::NoEntry,
        keyring::Error::NoStorageAccess(_) => BackendError::Locked,
        _ => BackendError::Failed,
    }
}

pub struct PlatformSyncCredentialStore {
    store: CredentialStore<KeyringBackend>,
}

impl PlatformSyncCredentialStore {
    pub fn new() -> Self {
        Self {
            store: CredentialStore::new(KeyringBackend),
        }
    }
}

impl Default for PlatformSyncCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncCredentialStore for PlatformSyncCredentialStore {
    fn load_r2_secret(&self, target: &SyncTargetId) -> Result<Option<SecretString>, SyncError> {
        self.store.load_r2_secret(target)
    }

    fn store_r2_secret(
        &self,
        target: &SyncTargetId,
        secret: &SecretString,
    ) -> Result<(), SyncError> {
        self.store.store_r2_secret(target, secret)
    }

    fn delete_r2_secret(&self, target: &SyncTargetId) -> Result<(), SyncError> {
        self.store.delete_r2_secret(target)
    }

    fn load_encryption_password(
        &self,
        target: &SyncTargetId,
    ) -> Result<Option<SecretString>, SyncError> {
        self.store.load_encryption_password(target)
    }

    fn store_encryption_password(
        &self,
        target: &SyncTargetId,
        password: &SecretString,
    ) -> Result<(), SyncError> {
        self.store.store_encryption_password(target, password)
    }

    fn delete_encryption_password(&self, target: &SyncTargetId) -> Result<(), SyncError> {
        self.store.delete_encryption_password(target)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
        rc::Rc,
    };

    use super::{
        BackendError, CredentialBackend, CredentialStore, SecretString, SyncCredentialStore,
        SyncError, map_keyring_error,
    };
    use crate::sync::SyncTargetId;

    const SERVICE: &str = "dev.jshell.sync";
    const SECRET: &str = "r2-secret-sentinel";
    const PASSWORD: &str = "encryption-password-sentinel";

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct BackendCall {
        operation: &'static str,
        service: String,
        username: String,
    }

    #[derive(Clone, Default)]
    struct MemoryBackend {
        values: Rc<RefCell<HashMap<(String, String), String>>>,
        calls: Rc<RefCell<Vec<BackendCall>>>,
        get_error: Rc<Cell<Option<BackendError>>>,
        set_error: Rc<Cell<Option<BackendError>>>,
        delete_error: Rc<Cell<Option<BackendError>>>,
    }

    impl CredentialBackend for MemoryBackend {
        fn get_password(&self, service: &str, username: &str) -> Result<String, BackendError> {
            self.calls.borrow_mut().push(BackendCall {
                operation: "get",
                service: service.to_string(),
                username: username.to_string(),
            });
            if let Some(error) = self.get_error.get() {
                return Err(error);
            }
            self.values
                .borrow()
                .get(&(service.to_string(), username.to_string()))
                .cloned()
                .ok_or(BackendError::NoEntry)
        }

        fn set_password(
            &self,
            service: &str,
            username: &str,
            password: &str,
        ) -> Result<(), BackendError> {
            self.calls.borrow_mut().push(BackendCall {
                operation: "set",
                service: service.to_string(),
                username: username.to_string(),
            });
            if let Some(error) = self.set_error.get() {
                return Err(error);
            }
            self.values.borrow_mut().insert(
                (service.to_string(), username.to_string()),
                password.to_string(),
            );
            Ok(())
        }

        fn delete_credential(&self, service: &str, username: &str) -> Result<(), BackendError> {
            self.calls.borrow_mut().push(BackendCall {
                operation: "delete",
                service: service.to_string(),
                username: username.to_string(),
            });
            if let Some(error) = self.delete_error.get() {
                return Err(error);
            }
            let removed = self
                .values
                .borrow_mut()
                .remove(&(service.to_string(), username.to_string()));
            if removed.is_some() {
                Ok(())
            } else {
                Err(BackendError::NoEntry)
            }
        }
    }

    fn target(bucket: &str) -> SyncTargetId {
        SyncTargetId::for_r2("account-a", bucket, "sync.json")
    }

    fn store() -> (CredentialStore<MemoryBackend>, MemoryBackend) {
        let backend = MemoryBackend::default();
        (CredentialStore::new(backend.clone()), backend)
    }

    fn assert_credential_store_error(error: SyncError) {
        assert!(matches!(error, SyncError::CredentialStore(_)));
        let debug = format!("{error:?}");
        let display = error.to_string();
        assert!(
            !debug.contains(SECRET),
            "debug leaked the R2 secret: {debug}"
        );
        assert!(
            !debug.contains(PASSWORD),
            "debug leaked the password: {debug}"
        );
        assert!(
            !display.contains(SECRET),
            "display leaked the R2 secret: {display}"
        );
        assert!(
            !display.contains(PASSWORD),
            "display leaked the password: {display}"
        );
    }

    fn assert_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}

    fn assert_zeroizing_string(_: &zeroize::Zeroizing<String>) {}

    #[test]
    fn secret_string_and_each_clone_use_zeroizing_drop_storage() {
        assert_zeroize_on_drop::<SecretString>();
        let secret = SecretString::new(SECRET.to_string());
        let cloned = secret.clone();

        assert_zeroizing_string(&secret.0);
        assert_zeroizing_string(&cloned.0);
        assert_eq!(secret, cloned);
    }

    #[test]
    fn keyring_errors_are_classified_without_accessing_platform_storage() {
        let locked = keyring::Error::NoStorageAccess(Box::new(std::io::Error::other(
            "test credential store is locked",
        )));
        let platform_failure = keyring::Error::PlatformFailure(Box::new(std::io::Error::other(
            "test platform failure",
        )));
        let invalid = keyring::Error::Invalid("service".to_string(), "test failure".to_string());

        assert!(matches!(
            map_keyring_error(keyring::Error::NoEntry),
            BackendError::NoEntry
        ));
        assert!(matches!(map_keyring_error(locked), BackendError::Locked));
        assert!(matches!(
            map_keyring_error(platform_failure),
            BackendError::Failed
        ));
        assert!(matches!(map_keyring_error(invalid), BackendError::Failed));
    }

    #[test]
    fn secret_string_debug_is_fixed_and_redacted() {
        let secret = SecretString::new(SECRET.to_string());

        assert_eq!(secret.expose_secret(), SECRET);
        assert_eq!(format!("{secret:?}"), "SecretString([REDACTED])");
        assert!(!format!("{secret:?}").contains(SECRET));
    }

    #[test]
    fn sync_error_rendering_and_source_never_expose_embedded_secrets() {
        let errors = [
            SyncError::InvalidInput(SECRET.to_string()),
            SyncError::Network(PASSWORD.to_string()),
            SyncError::InvalidPayload(SECRET.to_string()),
            SyncError::LocalSave(PASSWORD.to_string()),
        ];

        for error in errors {
            let rendered = format!("{error} {error:?}");
            assert!(
                !rendered.contains(SECRET),
                "error leaked secret: {rendered}"
            );
            assert!(
                !rendered.contains(PASSWORD),
                "error leaked password: {rendered}"
            );
            assert!(std::error::Error::source(&error).is_none());
        }
    }

    #[test]
    fn r2_secret_round_trips_and_target_fingerprints_isolate_entries() {
        let (store, backend) = store();
        let first = target("bucket-a");
        let second = target("bucket-b");

        store
            .store_r2_secret(&first, &SecretString::new(SECRET.to_string()))
            .unwrap();

        assert_eq!(
            store.load_r2_secret(&first).unwrap(),
            Some(SecretString::new(SECRET.to_string()))
        );
        assert_eq!(store.load_r2_secret(&second).unwrap(), None);

        let calls = backend.calls.borrow();
        assert_eq!(calls[0].service, SERVICE);
        assert_eq!(calls[0].username, format!("r2-secret-access-key:{}", first));
        assert_eq!(calls[1].username, format!("r2-secret-access-key:{}", first));
        assert_eq!(
            calls[2].username,
            format!("r2-secret-access-key:{}", second)
        );
        assert_ne!(calls[1].username, calls[2].username);
    }

    #[test]
    fn encryption_password_round_trips_and_delete_removes_only_its_entry() {
        let (store, backend) = store();
        let first = target("bucket-a");
        let second = target("bucket-b");

        store
            .store_encryption_password(&first, &SecretString::new(PASSWORD.to_string()))
            .unwrap();
        store
            .store_r2_secret(&second, &SecretString::new(SECRET.to_string()))
            .unwrap();

        assert_eq!(
            store.load_encryption_password(&first).unwrap(),
            Some(SecretString::new(PASSWORD.to_string()))
        );
        store.delete_encryption_password(&first).unwrap();
        assert_eq!(store.load_encryption_password(&first).unwrap(), None);
        assert_eq!(
            store.load_r2_secret(&second).unwrap(),
            Some(SecretString::new(SECRET.to_string()))
        );

        let calls = backend.calls.borrow();
        assert!(calls.iter().any(|call| {
            call.operation == "set"
                && call.service == SERVICE
                && call.username == format!("sync-encryption-password:{first}")
        }));
        assert!(calls.iter().any(|call| {
            call.operation == "delete"
                && call.service == SERVICE
                && call.username == format!("sync-encryption-password:{first}")
        }));
    }

    #[test]
    fn deleting_missing_encryption_password_is_idempotent() {
        let (store, _) = store();
        let never_stored = target("never-stored");
        let deleted_twice = target("deleted-twice");

        store.delete_encryption_password(&never_stored).unwrap();
        store
            .store_encryption_password(&deleted_twice, &SecretString::new(PASSWORD.to_string()))
            .unwrap();
        store.delete_encryption_password(&deleted_twice).unwrap();
        store.delete_encryption_password(&deleted_twice).unwrap();
    }

    #[test]
    fn missing_entries_load_as_none_and_empty_entries_load_as_empty_secrets() {
        let (store, backend) = store();
        let target = target("bucket-a");

        assert_eq!(store.load_r2_secret(&target).unwrap(), None);
        store
            .store_r2_secret(&target, &SecretString::new(String::new()))
            .unwrap();

        assert_eq!(
            store.load_r2_secret(&target).unwrap(),
            Some(SecretString::new(String::new()))
        );
        assert_eq!(
            backend
                .values
                .borrow()
                .get(&(
                    SERVICE.to_string(),
                    format!("r2-secret-access-key:{target}")
                ))
                .map(String::as_str),
            Some("")
        );
    }

    #[test]
    fn locked_or_failed_reads_are_typed_credential_store_errors() {
        let (store, backend) = store();
        let target = target("bucket-a");

        backend.get_error.set(Some(BackendError::Locked));
        assert_credential_store_error(store.load_r2_secret(&target).unwrap_err());

        backend.get_error.set(Some(BackendError::Failed));
        assert_credential_store_error(store.load_encryption_password(&target).unwrap_err());
    }

    #[test]
    fn failed_writes_are_typed_credential_store_errors() {
        let (store, backend) = store();
        backend.set_error.set(Some(BackendError::Failed));

        assert_credential_store_error(
            store
                .store_r2_secret(&target("bucket-a"), &SecretString::new(SECRET.to_string()))
                .unwrap_err(),
        );
    }

    #[test]
    fn failed_deletes_are_typed_credential_store_errors() {
        let (store, backend) = store();
        backend.delete_error.set(Some(BackendError::Failed));

        assert_credential_store_error(
            store
                .delete_encryption_password(&target("bucket-a"))
                .unwrap_err(),
        );
    }
}
