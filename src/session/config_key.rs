use std::{fmt, sync::Mutex};

use anyhow::{Context, Result, anyhow};
use rand::{RngCore, rngs::OsRng};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct MasterKey([u8; 32]);

impl MasterKey {
    pub(crate) fn random() -> Self {
        let mut bytes = [0; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub(crate) fn from_secret(secret: Vec<u8>) -> Result<Self> {
        let length = secret.len();
        let bytes = secret.try_into().map_err(|_| {
            anyhow!("configuration master key must be exactly 32 bytes, got {length}")
        })?;
        Ok(Self(bytes))
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MasterKey([REDACTED])")
    }
}

pub(crate) trait ConfigKeyStore {
    fn load(&self) -> Result<Option<MasterKey>>;
    fn store(&self, key: &MasterKey) -> Result<()>;
}

pub(crate) trait ConfigKeyProvider {
    fn load_existing(&self) -> Result<MasterKey>;
    fn load_or_create(&self) -> Result<MasterKey>;
}

pub(crate) fn load_existing_key(store: &dyn ConfigKeyStore) -> Result<MasterKey> {
    store
        .load()?
        .ok_or_else(|| anyhow!("configuration master key is missing from system secure storage"))
}

pub(crate) fn load_or_create_key(store: &dyn ConfigKeyStore) -> Result<MasterKey> {
    if let Some(key) = store.load()? {
        return Ok(key);
    }

    let key = MasterKey::random();
    store.store(&key)?;
    Ok(key)
}

struct PlatformKeyStore;

pub(crate) struct PlatformKeyProvider;

impl ConfigKeyProvider for PlatformKeyProvider {
    fn load_existing(&self) -> Result<MasterKey> {
        load_existing_platform_key()
    }

    fn load_or_create(&self) -> Result<MasterKey> {
        load_or_create_platform_key()
    }
}

impl PlatformKeyStore {
    fn entry() -> Result<keyring::Entry> {
        keyring::Entry::new("dev.jshell.config", "local-config-master-key")
            .context("create system secure storage entry")
    }
}

impl ConfigKeyStore for PlatformKeyStore {
    fn load(&self) -> Result<Option<MasterKey>> {
        match Self::entry()?.get_secret() {
            Ok(secret) => MasterKey::from_secret(secret)
                .context("decode configuration master key from system secure storage")
                .map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => {
                Err(error).context("read configuration master key from system secure storage")
            }
        }
    }

    fn store(&self, key: &MasterKey) -> Result<()> {
        Self::entry()?
            .set_secret(key.as_bytes())
            .context("write configuration master key to system secure storage")
    }
}

static PLATFORM_KEY_CACHE: Mutex<Option<MasterKey>> = Mutex::new(None);

pub(crate) fn load_existing_platform_key() -> Result<MasterKey> {
    with_platform_key_cache(|store| load_existing_key(store))
}

pub(crate) fn load_or_create_platform_key() -> Result<MasterKey> {
    with_platform_key_cache(|store| load_or_create_key(store))
}

fn with_platform_key_cache(
    load: impl FnOnce(&dyn ConfigKeyStore) -> Result<MasterKey>,
) -> Result<MasterKey> {
    let mut cached = PLATFORM_KEY_CACHE
        .lock()
        .map_err(|_| anyhow!("configuration master key cache is poisoned"))?;
    if let Some(key) = cached.as_ref() {
        return Ok(key.clone());
    }

    let key = load(&PlatformKeyStore)?;
    *cached = Some(key.clone());
    Ok(key)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use anyhow::{Result, anyhow};

    use super::{ConfigKeyStore, MasterKey, load_existing_key, load_or_create_key};

    #[derive(Default)]
    struct MemoryKeyStore {
        key: RefCell<Option<MasterKey>>,
        store_calls: Cell<usize>,
    }

    impl ConfigKeyStore for MemoryKeyStore {
        fn load(&self) -> Result<Option<MasterKey>> {
            Ok(self.key.borrow().clone())
        }

        fn store(&self, key: &MasterKey) -> Result<()> {
            self.store_calls.set(self.store_calls.get() + 1);
            self.key.replace(Some(key.clone()));
            Ok(())
        }
    }

    struct LoadFailureStore {
        store_calls: Cell<usize>,
    }

    impl ConfigKeyStore for LoadFailureStore {
        fn load(&self) -> Result<Option<MasterKey>> {
            Err(anyhow!("secure storage is locked"))
        }

        fn store(&self, _key: &MasterKey) -> Result<()> {
            self.store_calls.set(self.store_calls.get() + 1);
            Ok(())
        }
    }

    struct StoreFailureStore;

    impl ConfigKeyStore for StoreFailureStore {
        fn load(&self) -> Result<Option<MasterKey>> {
            Ok(None)
        }

        fn store(&self, _key: &MasterKey) -> Result<()> {
            Err(anyhow!("secure storage rejected the write"))
        }
    }

    #[test]
    fn master_key_rejects_secret_with_wrong_length() {
        let error = MasterKey::from_secret(vec![7; 31])
            .expect_err("configuration master key must contain exactly 32 bytes");

        assert!(error.to_string().contains("32 bytes"));
    }

    #[test]
    fn creates_and_persists_master_key_when_store_is_empty() {
        let store = MemoryKeyStore::default();

        let key = load_or_create_key(&store).expect("create configuration master key");

        assert_eq!(store.key.borrow().as_ref(), Some(&key));
        assert_eq!(store.store_calls.get(), 1);
    }

    #[test]
    fn reuses_existing_master_key_without_writing() {
        let expected = MasterKey::from_secret(vec![11; 32]).unwrap();
        let store = MemoryKeyStore {
            key: RefCell::new(Some(expected.clone())),
            ..MemoryKeyStore::default()
        };

        let loaded = load_or_create_key(&store).expect("load existing master key");

        assert_eq!(loaded, expected);
        assert_eq!(loaded.as_bytes(), &[11; 32]);
        assert_eq!(store.store_calls.get(), 0);
    }

    #[test]
    fn existing_key_load_rejects_empty_store_without_creating_key() {
        let store = MemoryKeyStore::default();

        let error = load_existing_key(&store)
            .expect_err("version 2 configuration must not create a replacement key");

        assert!(error.to_string().contains("missing"));
        assert_eq!(store.store_calls.get(), 0);
    }

    #[test]
    fn load_failure_does_not_attempt_to_replace_key() {
        let store = LoadFailureStore {
            store_calls: Cell::new(0),
        };

        let error = load_or_create_key(&store).expect_err("locked storage must fail closed");

        assert!(error.to_string().contains("locked"));
        assert_eq!(store.store_calls.get(), 0);
    }

    #[test]
    fn store_failure_is_returned_to_caller() {
        let error = load_or_create_key(&StoreFailureStore)
            .expect_err("failed secure storage write must be reported");

        assert!(error.to_string().contains("rejected the write"));
    }
}
