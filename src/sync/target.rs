use std::fmt::{self, Write as _};

use sha2::{Digest, Sha256};

const DEFAULT_OBJECT_KEY: &str = "jshell-sync.json";

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SyncTargetId(String);

impl SyncTargetId {
    pub fn for_webdav(endpoint: &str, username: &str) -> Self {
        let resource = super::sync_url(endpoint);
        Self::fingerprint(&["webdav", &resource, username.trim()])
    }

    pub fn for_s3(endpoint: &str, region: &str, bucket: &str, object_key: &str) -> Self {
        let region = region.trim();
        let endpoint = normalize_endpoint(endpoint);
        let endpoint = if endpoint.is_empty() {
            format!("https://s3.{region}.amazonaws.com")
        } else {
            endpoint
        };
        let object_key = normalize_object_key(object_key);
        Self::fingerprint(&["s3", &endpoint, region, bucket.trim(), object_key])
    }

    pub fn for_r2(account_id: &str, bucket: &str, object_key: &str) -> Self {
        Self::fingerprint(&[
            "r2",
            account_id.trim(),
            bucket.trim(),
            normalize_object_key(object_key),
        ])
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn fingerprint(parts: &[&str]) -> Self {
        let mut hasher = Sha256::new();
        for part in parts {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        let mut fingerprint = String::with_capacity(64);
        for byte in hasher.finalize() {
            write!(&mut fingerprint, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Self(fingerprint)
    }
}

impl fmt::Display for SyncTargetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for SyncTargetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

fn normalize_endpoint(endpoint: &str) -> String {
    endpoint.trim().trim_end_matches('/').to_string()
}

fn normalize_object_key(object_key: &str) -> &str {
    let object_key = object_key.trim().trim_start_matches('/');
    if object_key.is_empty() {
        DEFAULT_OBJECT_KEY
    } else {
        object_key
    }
}

#[cfg(test)]
mod tests {
    use super::SyncTargetId;

    #[test]
    fn webdav_collection_and_file_targets_normalize_to_the_same_fingerprint() {
        let collection = SyncTargetId::for_webdav(" https://dav.example.test/config/ ", "alice");
        let collection_without_slash =
            SyncTargetId::for_webdav("https://dav.example.test/config", "alice");
        let explicit_file =
            SyncTargetId::for_webdav("https://dav.example.test/config/jshell-sync.json", "alice");

        assert_eq!(collection, collection_without_slash);
        assert_eq!(collection, explicit_file);
    }

    #[test]
    fn webdav_file_and_collection_urls_have_distinct_fingerprints() {
        assert_ne!(
            SyncTargetId::for_webdav("https://dav.example.test/config.json", "alice"),
            SyncTargetId::for_webdav("https://dav.example.test/config.json/", "alice")
        );
    }

    #[test]
    fn webdav_distinct_trailing_slashes_have_distinct_fingerprints() {
        assert_ne!(
            SyncTargetId::for_webdav("https://dav.example.test/config/", "alice"),
            SyncTargetId::for_webdav("https://dav.example.test/config//", "alice")
        );
    }

    #[test]
    fn webdav_username_changes_the_fingerprint() {
        assert_ne!(
            SyncTargetId::for_webdav("https://dav.example.test/config", "alice"),
            SyncTargetId::for_webdav("https://dav.example.test/config", "bob")
        );
    }

    #[test]
    fn object_targets_normalize_endpoint_and_object_key_boundaries() {
        let s3 = SyncTargetId::for_s3(
            " https://s3.example.test/// ",
            "us-east-1",
            "bucket-a",
            "/configs/sync.json",
        );
        let normalized_s3 = SyncTargetId::for_s3(
            "https://s3.example.test",
            "us-east-1",
            "bucket-a",
            "configs/sync.json",
        );
        let default_s3 = SyncTargetId::for_s3("", "us-east-1", "bucket-a", "");
        let explicit_default_s3 = SyncTargetId::for_s3(
            "https://s3.us-east-1.amazonaws.com",
            "us-east-1",
            "bucket-a",
            "jshell-sync.json",
        );
        let r2 = SyncTargetId::for_r2("account-a", "bucket-a", "");
        let explicit_r2 = SyncTargetId::for_r2("account-a", "bucket-a", "/jshell-sync.json");

        assert_eq!(s3, normalized_s3);
        assert_eq!(default_s3, explicit_default_s3);
        assert_eq!(r2, explicit_r2);
    }

    #[test]
    fn provider_bucket_and_object_key_each_change_the_fingerprint() {
        let s3 = SyncTargetId::for_s3("https://objects.example", "auto", "bucket-a", "a.json");

        assert_ne!(
            s3,
            SyncTargetId::for_s3("https://objects.example", "auto", "bucket-b", "a.json")
        );
        assert_ne!(
            s3,
            SyncTargetId::for_s3("https://objects.example", "auto", "bucket-a", "b.json")
        );
        assert_ne!(
            s3,
            SyncTargetId::for_r2("https://objects.example", "bucket-a", "a.json")
        );
    }

    #[test]
    fn formatting_exposes_only_the_sha256_fingerprint() {
        let target = SyncTargetId::for_webdav(
            "https://access-key-sentinel:secret-key-sentinel@dav.example.test/config",
            "username-sentinel",
        );
        let display = target.to_string();
        let debug = format!("{target:?}");

        assert_eq!(display.len(), 64);
        assert!(display.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(debug, display);
        assert!(!display.contains("access-key-sentinel"));
        assert!(!display.contains("secret-key-sentinel"));
        assert!(!display.contains("username-sentinel"));
    }
}
