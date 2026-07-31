use std::sync::Arc;

use anyhow::Result;
use directories::BaseDirs;
use russh::{
    client::{self, Handler},
    keys::{Algorithm, HashAlg, PrivateKey, key::PrivateKeyWithHashAlg, load_secret_key},
};

use crate::session::config::Session;

pub const DEFAULT_KEY_NAMES: &[&str] = &["id_ed25519", "id_rsa", "id_ecdsa", "id_dsa"];

pub fn session_has_explicit_key(session: &Session) -> bool {
    !session.private_key_path.trim().is_empty()
        || !normalize_inline_private_key(&session.private_key_inline).is_empty()
}

pub fn normalize_inline_private_key(value: &str) -> String {
    let mut normalized = value
        .trim()
        .replace("\\r\\n", "\n")
        .replace("\\n", "\n")
        .replace("\r\n", "\n");
    if normalized.is_empty() {
        return String::new();
    }
    if !normalized.ends_with('\n') {
        normalized.push('\n');
    }
    normalized
}

const RSA_HASH_ALGS: &[Option<HashAlg>] = &[Some(HashAlg::Sha512), Some(HashAlg::Sha256)];
const NATIVE_KEY_ALG: &[Option<HashAlg>] = &[None];

fn hash_algs_for_key(algorithm: &Algorithm) -> &'static [Option<HashAlg>] {
    if algorithm.clone().is_rsa() {
        RSA_HASH_ALGS
    } else {
        NATIVE_KEY_ALG
    }
}

pub fn private_keys_with_algs(keypair: PrivateKey) -> Vec<PrivateKeyWithHashAlg> {
    let key_arc = Arc::new(keypair);
    hash_algs_for_key(&key_arc.algorithm())
        .iter()
        .map(|hash_alg| PrivateKeyWithHashAlg::new(key_arc.clone(), *hash_alg))
        .collect()
}

pub async fn authenticate_with_default_keys<H>(
    handle: &mut client::Handle<H>,
    user: &str,
    passphrase: Option<&str>,
) -> Result<bool>
where
    H: Handler + Send + Sync,
    H::Error: Into<anyhow::Error>,
{
    let Some(ssh_dir) = BaseDirs::new().map(|d| d.home_dir().join(".ssh")) else {
        return Ok(false);
    };

    for key_name in DEFAULT_KEY_NAMES {
        let key_path = ssh_dir.join(key_name);
        if !key_path.exists() {
            continue;
        }
        tracing::debug!("[ssh] trying default key {}", key_path.display());
        match load_secret_key(&key_path, passphrase) {
            Ok(keypair) => {
                for key in private_keys_with_algs(keypair) {
                    match handle.authenticate_publickey(user, key).await {
                        Ok(result) if result.success() => return Ok(true),
                        Ok(_) | Err(_) => continue,
                    }
                }
            }
            Err(_) => continue,
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::hash_algs_for_key;
    use russh::keys::{Algorithm, HashAlg};

    #[test]
    fn rsa_authentication_uses_sha2_only() {
        assert_eq!(
            hash_algs_for_key(&Algorithm::Rsa { hash: None }),
            [Some(HashAlg::Sha512), Some(HashAlg::Sha256)]
        );
    }

    #[test]
    fn non_rsa_authentication_uses_its_native_algorithm() {
        assert_eq!(hash_algs_for_key(&Algorithm::Ed25519), [None]);
    }
}
