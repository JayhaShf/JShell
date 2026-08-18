use std::{
    error::Error as StdError,
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use directories::BaseDirs;
use hmac::{Hmac, Mac};
use russh::keys::ssh_key::{
    HashAlg, PublicKey,
    known_hosts::{Entry, HostPatterns, Marker},
};
use sha1::Sha1;

static KNOWN_HOSTS_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostKeyVerification {
    Trusted,
    AcceptedNew,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostPatternMatch {
    None,
    Positive,
    Negated,
}

#[derive(Debug)]
enum HostKeyCheck {
    Trusted,
    Unknown,
    Changed { stored: Vec<String> },
    Revoked { line: usize, fingerprint: String },
    Excluded { line: usize },
    UnsupportedMarker { line: usize, marker: String },
}

#[derive(Debug)]
pub(crate) enum HostKeyError {
    HomeDirectoryUnavailable,
    ReadKnownHosts {
        target: String,
        path: PathBuf,
        source: io::Error,
    },
    InvalidEntry {
        target: String,
        path: PathBuf,
        line: usize,
        source: russh::keys::ssh_key::Error,
    },
    Changed {
        target: String,
        path: PathBuf,
        stored: String,
        received: String,
    },
    Revoked {
        target: String,
        path: PathBuf,
        line: usize,
        fingerprint: String,
    },
    Excluded {
        target: String,
        path: PathBuf,
        line: usize,
    },
    UnsupportedMarker {
        target: String,
        path: PathBuf,
        line: usize,
        marker: String,
    },
    EncodeKey {
        target: String,
        path: PathBuf,
        source: russh::keys::ssh_key::Error,
    },
    CreateDirectory {
        path: PathBuf,
        source: io::Error,
    },
    WriteKnownHosts {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for HostKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::HomeDirectoryUnavailable => {
                rust_i18n::t!("ssh_home_directory_unavailable").to_string()
            }
            Self::ReadKnownHosts {
                target,
                path,
                source,
            } => rust_i18n::t!(
                "ssh_known_hosts_read_failed",
                target = target.as_str(),
                path = path.display().to_string(),
                error = source.to_string()
            )
            .to_string(),
            Self::InvalidEntry {
                target,
                path,
                line,
                source,
            } => rust_i18n::t!(
                "ssh_known_hosts_invalid_entry",
                target = target.as_str(),
                path = path.display().to_string(),
                line = *line,
                error = source.to_string()
            )
            .to_string(),
            Self::Changed {
                target,
                path,
                stored,
                received,
            } => rust_i18n::t!(
                "ssh_host_key_changed",
                target = target.as_str(),
                stored = stored.as_str(),
                received = received.as_str(),
                path = path.display().to_string()
            )
            .to_string(),
            Self::Revoked {
                target,
                path,
                line,
                fingerprint,
            } => rust_i18n::t!(
                "ssh_host_key_revoked",
                target = target.as_str(),
                fingerprint = fingerprint.as_str(),
                path = path.display().to_string(),
                line = *line
            )
            .to_string(),
            Self::Excluded { target, path, line } => rust_i18n::t!(
                "ssh_host_key_excluded",
                target = target.as_str(),
                path = path.display().to_string(),
                line = *line
            )
            .to_string(),
            Self::UnsupportedMarker {
                target,
                path,
                line,
                marker,
            } => rust_i18n::t!(
                "ssh_host_key_marker_unsupported",
                target = target.as_str(),
                marker = marker.as_str(),
                path = path.display().to_string(),
                line = *line
            )
            .to_string(),
            Self::EncodeKey {
                target,
                path,
                source,
            } => rust_i18n::t!(
                "ssh_known_hosts_write_failed",
                target = target.as_str(),
                path = path.display().to_string(),
                error = source.to_string()
            )
            .to_string(),
            Self::CreateDirectory { path, source } | Self::WriteKnownHosts { path, source } => {
                rust_i18n::t!(
                    "ssh_known_hosts_write_failed",
                    path = path.display().to_string(),
                    error = source.to_string()
                )
                .to_string()
            }
        };
        f.write_str(&message)
    }
}

impl StdError for HostKeyError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::ReadKnownHosts { source, .. }
            | Self::CreateDirectory { source, .. }
            | Self::WriteKnownHosts { source, .. } => Some(source),
            Self::InvalidEntry { source, .. } | Self::EncodeKey { source, .. } => Some(source),
            Self::HomeDirectoryUnavailable
            | Self::Changed { .. }
            | Self::Revoked { .. }
            | Self::Excluded { .. }
            | Self::UnsupportedMarker { .. } => None,
        }
    }
}

pub(crate) fn is_permanent_host_key_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<HostKeyError>().is_some())
}

#[derive(Clone)]
pub(crate) struct HostKeyVerifier {
    host: String,
    port: u16,
    known_hosts_path: PathBuf,
}

impl HostKeyVerifier {
    pub(crate) fn new(host: &str, port: u16) -> Result<Self, HostKeyError> {
        let home = BaseDirs::new().ok_or(HostKeyError::HomeDirectoryUnavailable)?;
        Ok(Self::with_path(
            host,
            port,
            home.home_dir().join(".ssh").join("known_hosts"),
        ))
    }

    #[cfg(test)]
    pub(crate) fn with_known_hosts_path(host: &str, port: u16, path: PathBuf) -> Self {
        Self::with_path(host, port, path)
    }

    fn with_path(host: &str, port: u16, known_hosts_path: PathBuf) -> Self {
        Self {
            host: host.to_string(),
            port,
            known_hosts_path,
        }
    }

    pub(crate) fn verify(&self, key: &PublicKey) -> Result<HostKeyVerification, HostKeyError> {
        verify_server_key_at_path(&self.host, self.port, key, &self.known_hosts_path)
    }
}

fn known_hosts_target(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

fn key_fingerprint(key: &PublicKey) -> String {
    format!(
        "{} {}",
        key.algorithm().as_str(),
        key.fingerprint(HashAlg::Sha256)
    )
}

fn read_known_hosts(host: &str, port: u16, path: &Path) -> Result<String, HostKeyError> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(HostKeyError::ReadKnownHosts {
            target: known_hosts_target(host, port),
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn check_server_key_contents(
    host: &str,
    port: u16,
    key: &PublicKey,
    path: &Path,
    contents: &str,
) -> Result<HostKeyCheck, HostKeyError> {
    let target = known_hosts_target(host, port);
    let normalized_target = target.to_ascii_lowercase();
    let mut trusted = false;
    let mut changed = Vec::new();
    let mut revoked = None;
    let mut excluded = None;
    let mut unsupported_marker = None;

    for (index, raw_line) in contents.lines().enumerate() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }

        let normalized = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !raw_entry_could_match_target(&normalized, &target, &normalized_target) {
            continue;
        }
        let entry: Entry = normalized
            .parse()
            .map_err(|source| HostKeyError::InvalidEntry {
                target: target.clone(),
                path: path.to_path_buf(),
                line: index + 1,
                source,
            })?;

        match host_patterns_match(entry.host_patterns(), &target, &normalized_target) {
            HostPatternMatch::None => continue,
            HostPatternMatch::Negated => {
                if entry.marker().is_none() {
                    excluded.get_or_insert(index + 1);
                }
                continue;
            }
            HostPatternMatch::Positive => {}
        }

        let same_key = entry.public_key().key_data() == key.key_data();
        match entry.marker() {
            Some(Marker::Revoked) if same_key => {
                revoked.get_or_insert((index + 1, key_fingerprint(key)));
            }
            Some(Marker::Revoked) => changed.push(key_fingerprint(entry.public_key())),
            Some(marker) => {
                unsupported_marker.get_or_insert((index + 1, format!("{marker:?}")));
            }
            None if same_key => trusted = true,
            None => changed.push(key_fingerprint(entry.public_key())),
        }
    }

    if let Some((line, fingerprint)) = revoked {
        return Ok(HostKeyCheck::Revoked { line, fingerprint });
    }
    if let Some(line) = excluded {
        return Ok(HostKeyCheck::Excluded { line });
    }
    if let Some((line, marker)) = unsupported_marker {
        return Ok(HostKeyCheck::UnsupportedMarker { line, marker });
    }
    if trusted {
        return Ok(HostKeyCheck::Trusted);
    }
    if !changed.is_empty() {
        changed.sort();
        changed.dedup();
        return Ok(HostKeyCheck::Changed { stored: changed });
    }
    Ok(HostKeyCheck::Unknown)
}

fn raw_entry_could_match_target(line: &str, target: &str, normalized_target: &str) -> bool {
    let mut fields = line.split_whitespace();
    let Some(first) = fields.next() else {
        return false;
    };
    let patterns = if first.starts_with('@') {
        let Some(patterns) = fields.next() else {
            // The malformed marker might have belonged to this target, so fail closed.
            return true;
        };
        patterns
    } else {
        first
    };

    if patterns.starts_with('|') {
        // Valid hashed records can be filtered without parsing the public key. If the
        // hash itself is malformed, retain the line so the strict parser fails closed.
        let mut parts = patterns.split('|');
        let valid_prefix = parts.next() == Some("") && parts.next() == Some("1");
        let Some(salt) = parts.next() else {
            return true;
        };
        let Some(hash) = parts.next() else {
            return true;
        };
        if !valid_prefix || parts.next().is_some() {
            return true;
        }
        let Ok(salt) =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, salt.as_bytes())
        else {
            return true;
        };
        let Ok(hash) =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, hash.as_bytes())
        else {
            return true;
        };
        let Ok(hash): Result<[u8; 20], _> = hash.try_into() else {
            return true;
        };
        return hashed_host_matches(&salt, &hash, target)
            || (target != normalized_target
                && hashed_host_matches(&salt, &hash, normalized_target));
    }

    patterns.split(',').any(|pattern| {
        let pattern = pattern.strip_prefix('!').unwrap_or(pattern);
        wildcard_match(&pattern.to_ascii_lowercase(), normalized_target)
    })
}

fn append_server_key_at_path(
    host: &str,
    port: u16,
    key: &PublicKey,
    path: &Path,
    existing_contents: &str,
) -> Result<(), HostKeyError> {
    let target = known_hosts_target(host, port);
    let encoded = key.to_openssh().map_err(|source| HostKeyError::EncodeKey {
        target: target.clone(),
        path: path.to_path_buf(),
        source,
    })?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| HostKeyError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut options = OpenOptions::new();
    options.create(true).read(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|source| HostKeyError::WriteKnownHosts {
            path: path.to_path_buf(),
            source,
        })?;

    let mut record = String::new();
    if !existing_contents.is_empty() && !existing_contents.ends_with('\n') {
        record.push('\n');
    }
    record.push_str(&target);
    record.push(' ');
    record.push_str(&encoded);
    record.push('\n');

    file.write_all(record.as_bytes())
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_data())
        .map_err(|source| HostKeyError::WriteKnownHosts {
            path: path.to_path_buf(),
            source,
        })
}

fn verify_server_key_at_path(
    host: &str,
    port: u16,
    key: &PublicKey,
    path: &Path,
) -> Result<HostKeyVerification, HostKeyError> {
    let _guard = KNOWN_HOSTS_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let contents = read_known_hosts(host, port, path)?;

    match check_server_key_contents(host, port, key, path, &contents)? {
        HostKeyCheck::Trusted => Ok(HostKeyVerification::Trusted),
        HostKeyCheck::Unknown => {
            append_server_key_at_path(host, port, key, path, &contents)?;
            Ok(HostKeyVerification::AcceptedNew)
        }
        HostKeyCheck::Changed { stored } => Err(HostKeyError::Changed {
            target: known_hosts_target(host, port),
            path: path.to_path_buf(),
            stored: stored.join(", "),
            received: key_fingerprint(key),
        }),
        HostKeyCheck::Revoked { line, fingerprint } => Err(HostKeyError::Revoked {
            target: known_hosts_target(host, port),
            path: path.to_path_buf(),
            line,
            fingerprint,
        }),
        HostKeyCheck::Excluded { line } => Err(HostKeyError::Excluded {
            target: known_hosts_target(host, port),
            path: path.to_path_buf(),
            line,
        }),
        HostKeyCheck::UnsupportedMarker { line, marker } => Err(HostKeyError::UnsupportedMarker {
            target: known_hosts_target(host, port),
            path: path.to_path_buf(),
            line,
            marker,
        }),
    }
}

fn host_patterns_match(
    patterns: &HostPatterns,
    host: &str,
    normalized_host: &str,
) -> HostPatternMatch {
    match patterns {
        HostPatterns::HashedName { salt, hash } => {
            if hashed_host_matches(salt, hash, host)
                || (host != normalized_host && hashed_host_matches(salt, hash, normalized_host))
            {
                HostPatternMatch::Positive
            } else {
                HostPatternMatch::None
            }
        }
        HostPatterns::Patterns(patterns) => {
            let mut positive_match = false;

            for pattern in patterns {
                let (negated, pattern) = match pattern.strip_prefix('!') {
                    Some(pattern) => (true, pattern),
                    None => (false, pattern.as_str()),
                };

                if wildcard_match(&pattern.to_ascii_lowercase(), normalized_host) {
                    if negated {
                        return HostPatternMatch::Negated;
                    }
                    positive_match = true;
                }
            }

            if positive_match {
                HostPatternMatch::Positive
            } else {
                HostPatternMatch::None
            }
        }
    }
}

fn hashed_host_matches(salt: &[u8], hash: &[u8; 20], host: &str) -> bool {
    let mut hmac = Hmac::<Sha1>::new_from_slice(salt).expect("HMAC-SHA1 accepts salts of any size");
    hmac.update(host.as_bytes());
    hmac.verify_slice(hash).is_ok()
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut pattern_index, mut value_index) = (0, 0);
    let (mut star_index, mut star_value_index) = (None, 0);

    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            star_value_index = value_index;
            pattern_index += 1;
        } else if let Some(star) = star_index {
            star_value_index += 1;
            value_index = star_value_index;
            pattern_index = star + 1;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }

    pattern_index == pattern.len()
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, sync::Arc};

    use russh::keys::ssh_key::PublicKey;

    use super::{HostKeyError, HostKeyVerification, verify_server_key_at_path};

    const KNOWN_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const CHANGED_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G1sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X";
    const ECDSA_KEY: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBHwf2HMM5TRXvo2SQJjsNkiDD5KqiiNjrGVv3UUh+mMT5RHxiRtOnlqvjhQtBq0VpmpCV/PwUdhOig4vkbqAcEc=";

    struct TempKnownHosts {
        _root: tempfile::TempDir,
        path: PathBuf,
    }

    impl TempKnownHosts {
        fn existing(contents: &str) -> Self {
            let root = tempfile::tempdir().expect("create temporary known_hosts root");
            let path = root.path().join("known_hosts");
            fs::write(&path, contents).expect("write temporary known_hosts");
            Self { _root: root, path }
        }

        fn missing() -> Self {
            let root = tempfile::tempdir().expect("create temporary known_hosts root");
            let path = root.path().join(".ssh").join("known_hosts");
            Self { _root: root, path }
        }

        fn contents(&self) -> String {
            fs::read_to_string(&self.path).expect("read temporary known_hosts")
        }
    }

    fn public_key(value: &str) -> PublicKey {
        PublicKey::from_openssh(value).expect("parse test public key")
    }

    #[test]
    fn accepts_matching_host_key() {
        let known_hosts = TempKnownHosts::existing(&format!("example.test {KNOWN_KEY}\n"));

        assert_eq!(
            verify_server_key_at_path(
                "example.test",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .expect("matching host key should be trusted"),
            HostKeyVerification::Trusted,
        );
    }

    #[test]
    fn accepts_matching_hashed_host_key() {
        let known_hosts = TempKnownHosts::existing(&format!(
            "|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak= {KNOWN_KEY}\n"
        ));

        assert_eq!(
            verify_server_key_at_path(
                "example.com",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .expect("matching hashed host key should be trusted"),
            HostKeyVerification::Trusted,
        );
    }

    #[test]
    fn accepts_matching_key_from_wildcard_host_pattern() {
        let known_hosts = TempKnownHosts::existing(&format!("*.example.test {KNOWN_KEY}\n"));

        assert_eq!(
            verify_server_key_at_path(
                "api.example.test",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .expect("matching wildcard host key should be trusted"),
            HostKeyVerification::Trusted,
        );
    }

    #[test]
    fn rejects_host_excluded_by_negated_regular_pattern() {
        let original = format!("!bad.example.test,bad.example.test {KNOWN_KEY}\n");
        let known_hosts = TempKnownHosts::existing(&original);

        let error = verify_server_key_at_path(
            "bad.example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("negated host pattern must override a positive pattern");

        assert!(matches!(error, HostKeyError::Excluded { line: 1, .. }));
        assert_eq!(known_hosts.contents(), original);
    }

    #[test]
    fn accepts_and_persists_new_host_in_existing_file() {
        let original = format!("example.test {KNOWN_KEY}\n");
        let known_hosts = TempKnownHosts::existing(&original);

        let result =
            verify_server_key_at_path("new.test", 22, &public_key(CHANGED_KEY), &known_hosts.path)
                .expect("new host should be accepted");

        assert_eq!(result, HostKeyVerification::AcceptedNew);
        let contents = known_hosts.contents();
        assert!(contents.starts_with(&original));
        assert_eq!(contents.matches("new.test ssh-ed25519 ").count(), 1);
    }

    #[test]
    fn accepts_and_creates_missing_known_hosts_file() {
        let known_hosts = TempKnownHosts::missing();

        assert_eq!(
            verify_server_key_at_path(
                "example.test",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .expect("missing known_hosts should be created"),
            HostKeyVerification::AcceptedNew,
        );

        let contents = known_hosts.contents();
        assert!(contents.starts_with("example.test ssh-ed25519 "));
        assert!(!contents.starts_with('\n'));
        assert_eq!(
            verify_server_key_at_path(
                "example.test",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .expect("persisted host key should be trusted"),
            HostKeyVerification::Trusted,
        );
    }

    #[test]
    fn appends_after_file_without_trailing_newline() {
        let known_hosts = TempKnownHosts::existing(&format!("other.test {KNOWN_KEY}"));

        verify_server_key_at_path(
            "example.test",
            22,
            &public_key(CHANGED_KEY),
            &known_hosts.path,
        )
        .expect("new host should be appended");

        let contents = known_hosts.contents();
        let lines = contents.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("other.test ssh-ed25519 "));
        assert!(lines[1].starts_with("example.test ssh-ed25519 "));
    }

    #[test]
    fn does_not_duplicate_an_already_accepted_key() {
        let known_hosts = TempKnownHosts::missing();

        assert_eq!(
            verify_server_key_at_path(
                "example.test",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .expect("new host should be accepted"),
            HostKeyVerification::AcceptedNew,
        );
        assert_eq!(
            verify_server_key_at_path(
                "example.test",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .expect("accepted host should be trusted"),
            HostKeyVerification::Trusted,
        );
        assert_eq!(
            known_hosts
                .contents()
                .matches("example.test ssh-ed25519 ")
                .count(),
            1,
        );
    }

    #[test]
    fn trusts_exact_key_even_when_a_different_old_key_is_also_stored() {
        let original = format!("example.test {KNOWN_KEY}\nexample.test {CHANGED_KEY}\n");
        let known_hosts = TempKnownHosts::existing(&original);

        assert_eq!(
            verify_server_key_at_path(
                "example.test",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .expect("an exact stored key must take precedence over stale keys"),
            HostKeyVerification::Trusted,
        );
        assert_eq!(known_hosts.contents(), original);
    }

    #[test]
    fn trusts_exact_key_when_multiple_host_key_algorithms_are_stored() {
        let original = format!("example.test {ECDSA_KEY}\nexample.test {KNOWN_KEY}\n");
        let known_hosts = TempKnownHosts::existing(&original);

        assert_eq!(
            verify_server_key_at_path(
                "example.test",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .expect("the exact Ed25519 key must survive an ECDSA record for the same host"),
            HostKeyVerification::Trusted,
        );
        assert_eq!(known_hosts.contents(), original);
    }

    #[test]
    fn rejects_negated_target_even_when_current_key_is_also_stored() {
        let original =
            format!("example.test {KNOWN_KEY}\n!example.test,example.test {KNOWN_KEY}\n");
        let known_hosts = TempKnownHosts::existing(&original);

        let error = verify_server_key_at_path(
            "example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("a matching negated pattern must block the connection");

        assert!(matches!(error, HostKeyError::Excluded { line: 2, .. }));
        assert_eq!(known_hosts.contents(), original);
    }

    #[test]
    fn rejects_ca_marker_even_when_current_key_is_also_stored() {
        let original =
            format!("example.test {KNOWN_KEY}\n@cert-authority example.test {KNOWN_KEY}\n");
        let known_hosts = TempKnownHosts::existing(&original);

        let error = verify_server_key_at_path(
            "example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("a matching CA marker must block direct key verification");

        assert!(matches!(
            error,
            HostKeyError::UnsupportedMarker { line: 2, .. }
        ));
        assert_eq!(known_hosts.contents(), original);
    }

    #[test]
    fn concurrent_accept_new_writes_one_entry() {
        let known_hosts = TempKnownHosts::missing();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let threads = (0..2)
            .map(|_| {
                let path = known_hosts.path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    verify_server_key_at_path("example.test", 22, &public_key(KNOWN_KEY), &path)
                        .expect("concurrent host key verification")
                })
            })
            .collect::<Vec<_>>();
        let results = threads
            .into_iter()
            .map(|thread| thread.join().expect("verification thread"))
            .collect::<Vec<_>>();

        assert_eq!(
            results
                .iter()
                .filter(|result| **result == HostKeyVerification::AcceptedNew)
                .count(),
            1,
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == HostKeyVerification::Trusted)
                .count(),
            1,
        );
        assert_eq!(
            known_hosts
                .contents()
                .matches("example.test ssh-ed25519 ")
                .count(),
            1,
        );
    }

    #[test]
    fn rejects_malformed_matching_entry() {
        let original = "example.test ssh-ed25519 not-base64\n";
        let known_hosts = TempKnownHosts::existing(original);

        let error = verify_server_key_at_path(
            "example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("malformed matching entry must be rejected");

        assert!(matches!(error, HostKeyError::InvalidEntry { line: 1, .. }));
        assert_eq!(known_hosts.contents(), original);
    }

    #[test]
    fn ignores_malformed_entry_for_an_unrelated_host() {
        let original =
            format!("unrelated.example ssh-ed25519 not-base64\nexample.test {KNOWN_KEY}\n");
        let known_hosts = TempKnownHosts::existing(&original);

        assert_eq!(
            verify_server_key_at_path(
                "example.test",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .expect("an unrelated malformed record must not block this host"),
            HostKeyVerification::Trusted,
        );
        assert_eq!(known_hosts.contents(), original);
    }

    #[test]
    fn malformed_hashed_entry_still_fails_closed() {
        let original = "|1|not-base64|also-not-base64 ssh-ed25519 not-base64\n";
        let known_hosts = TempKnownHosts::existing(original);

        let error = verify_server_key_at_path(
            "example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("a malformed hashed identity cannot be proven unrelated");

        assert!(matches!(error, HostKeyError::InvalidEntry { line: 1, .. }));
        assert_eq!(known_hosts.contents(), original);
    }

    #[test]
    fn rejects_changed_host_key() {
        let original = format!("example.test {KNOWN_KEY}\n");
        let known_hosts = TempKnownHosts::existing(&original);

        let error = verify_server_key_at_path(
            "example.test",
            22,
            &public_key(CHANGED_KEY),
            &known_hosts.path,
        )
        .expect_err("changed key must be rejected");

        assert!(matches!(error, HostKeyError::Changed { .. }));
        assert_eq!(known_hosts.contents(), original);
    }

    #[test]
    fn rejects_different_algorithm_for_known_target() {
        let original = format!("example.test {KNOWN_KEY}\n");
        let known_hosts = TempKnownHosts::existing(&original);

        let error = verify_server_key_at_path(
            "example.test",
            22,
            &public_key(ECDSA_KEY),
            &known_hosts.path,
        )
        .expect_err("a different key algorithm for an existing target must be rejected");

        assert!(matches!(error, HostKeyError::Changed { .. }));
        assert_eq!(known_hosts.contents(), original);
    }

    #[test]
    fn rejects_revoked_key_even_when_regular_entry_matches() {
        let original = format!("example.test {KNOWN_KEY}\n@revoked example.test {KNOWN_KEY}\n");
        let known_hosts = TempKnownHosts::existing(&original);

        let error = verify_server_key_at_path(
            "example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("revoked host key must be rejected");

        assert!(matches!(error, HostKeyError::Revoked { line: 2, .. }));
        assert_eq!(known_hosts.contents(), original);
    }

    #[test]
    fn rejects_revoked_key_for_matching_wildcard_host() {
        let known_hosts = TempKnownHosts::existing(&format!(
            "api.example.test {KNOWN_KEY}\n@revoked *.example.test {KNOWN_KEY}\n"
        ));

        let error = verify_server_key_at_path(
            "api.example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("key revoked by matching wildcard must be rejected");

        assert!(matches!(error, HostKeyError::Revoked { line: 2, .. }));
    }

    #[test]
    fn rejects_revoked_key_for_matching_hashed_host() {
        let known_hosts = TempKnownHosts::existing(&format!(
            "example.com {KNOWN_KEY}\n@revoked |1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak= {KNOWN_KEY}\n"
        ));

        let error =
            verify_server_key_at_path("example.com", 22, &public_key(KNOWN_KEY), &known_hosts.path)
                .expect_err("key revoked for hashed host must be rejected");

        assert!(matches!(error, HostKeyError::Revoked { line: 2, .. }));
    }

    #[test]
    fn rejects_revoked_key_for_hashed_host_with_original_case() {
        let known_hosts = TempKnownHosts::existing(&format!(
            "EXAMPLE.COM {KNOWN_KEY}\n@revoked |1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|mZKP+zxZ9/rd2Drc54FZJa7nqhw= {KNOWN_KEY}\n"
        ));

        let error =
            verify_server_key_at_path("EXAMPLE.COM", 22, &public_key(KNOWN_KEY), &known_hosts.path)
                .expect_err(
                    "hashed revocation must use the same host identity as regular matching",
                );

        assert!(matches!(error, HostKeyError::Revoked { line: 2, .. }));
    }

    #[test]
    fn accepts_key_revoked_only_for_another_host() {
        let known_hosts = TempKnownHosts::existing(&format!(
            "example.test {KNOWN_KEY}\n@revoked other.test {KNOWN_KEY}\n"
        ));

        assert_eq!(
            verify_server_key_at_path(
                "example.test",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .expect("revocation for another host must not apply"),
            HostKeyVerification::Trusted,
        );
    }

    #[test]
    fn accepts_key_excluded_from_revocation_by_negated_pattern() {
        let known_hosts = TempKnownHosts::existing(&format!(
            "safe.example.test {KNOWN_KEY}\n@revoked !safe.example.test,*.example.test {KNOWN_KEY}\n"
        ));

        assert_eq!(
            verify_server_key_at_path(
                "safe.example.test",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .expect("negated revocation must not apply"),
            HostKeyVerification::Trusted,
        );
    }

    #[test]
    fn rejects_revoked_key_on_non_default_port() {
        let known_hosts = TempKnownHosts::existing(&format!(
            "[example.test]:2222 {KNOWN_KEY}\n@revoked [example.test]:2222 {KNOWN_KEY}\n"
        ));

        let error = verify_server_key_at_path(
            "example.test",
            2222,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("revoked key on a non-default port must be rejected");

        assert!(matches!(error, HostKeyError::Revoked { line: 2, .. }));
    }

    #[test]
    fn does_not_trust_cert_authority_as_a_direct_host_key() {
        let original = format!("@cert-authority example.test {KNOWN_KEY}\n");
        let known_hosts = TempKnownHosts::existing(&original);

        let error = verify_server_key_at_path(
            "example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("certificate authority entry must not directly trust the same public key");

        assert!(matches!(
            error,
            HostKeyError::UnsupportedMarker { line: 1, .. }
        ));
        assert_eq!(known_hosts.contents(), original);
    }

    #[test]
    fn accepts_matching_key_on_non_default_port() {
        let known_hosts = TempKnownHosts::existing(&format!("[example.test]:2222 {KNOWN_KEY}\n"));

        assert_eq!(
            verify_server_key_at_path(
                "example.test",
                2222,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .expect("non-default port key should be trusted"),
            HostKeyVerification::Trusted,
        );
    }

    #[test]
    fn writes_bracketed_target_for_non_default_port() {
        let known_hosts = TempKnownHosts::missing();

        assert_eq!(
            verify_server_key_at_path(
                "example.test",
                2222,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .expect("new non-default port should be accepted"),
            HostKeyVerification::AcceptedNew,
        );
        assert!(
            known_hosts
                .contents()
                .starts_with("[example.test]:2222 ssh-ed25519 ")
        );
    }

    #[test]
    fn does_not_accept_new_when_known_hosts_read_fails() {
        let root = tempfile::tempdir().expect("create temporary root");
        let path = root.path().join("known_hosts");
        fs::create_dir(&path).expect("create directory at known_hosts path");

        let error = verify_server_key_at_path("example.test", 22, &public_key(KNOWN_KEY), &path)
            .expect_err("unreadable known_hosts must fail closed");

        assert!(matches!(error, HostKeyError::ReadKnownHosts { .. }));
        assert!(path.is_dir());
    }
}
