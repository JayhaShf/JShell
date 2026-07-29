use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use directories::BaseDirs;
use hmac::{Hmac, Mac};
use russh::keys::ssh_key::{
    PublicKey,
    known_hosts::{Entry, HostPatterns, Marker},
};
use sha1::Sha1;

#[derive(Clone)]
pub(crate) struct HostKeyVerifier {
    host: String,
    port: u16,
    known_hosts_path: PathBuf,
}

impl HostKeyVerifier {
    pub(crate) fn new(host: &str, port: u16) -> Result<Self> {
        let home = BaseDirs::new().context("could not determine user home directory")?;
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

    pub(crate) fn verify(&self, key: &PublicKey) -> Result<()> {
        verify_server_key_at_path(&self.host, self.port, key, &self.known_hosts_path)
    }
}

fn verify_server_key_at_path(host: &str, port: u16, key: &PublicKey, path: &Path) -> Result<()> {
    let contents = fs::read_to_string(path).with_context(|| {
        format!(
            "SSH host key verification failed for {host}:{port} using {}: could not read known_hosts file",
            path.display()
        )
    })?;
    let target = if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    };
    let normalized_target = target.to_ascii_lowercase();
    let mut trusted = false;
    let mut changed = false;

    for (index, raw_line) in contents.lines().enumerate() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }

        let normalized = line.split_whitespace().collect::<Vec<_>>().join(" ");
        let entry: Entry = normalized.parse().with_context(|| {
            format!(
                "SSH host key verification failed for {host}:{port}: invalid known_hosts entry at {}:{}",
                path.display(),
                index + 1
            )
        })?;

        if !host_patterns_match(entry.host_patterns(), &target, &normalized_target) {
            continue;
        }

        let same_key = entry.public_key().key_data() == key.key_data();
        match entry.marker() {
            Some(Marker::Revoked) if same_key => {
                return Err(anyhow!(
                    "SSH host key for {host}:{port} is revoked by {}:{}",
                    path.display(),
                    index + 1
                ));
            }
            Some(_) => {}
            None if same_key => trusted = true,
            None if entry.public_key().algorithm() == key.algorithm() => changed = true,
            None => {}
        }
    }

    if trusted {
        Ok(())
    } else if changed {
        Err(anyhow!(
            "SSH host key for {host}:{port} has changed; update the verified key in {}",
            path.display()
        ))
    } else {
        Err(anyhow!(
            "SSH host key for {host}:{port} is not trusted; add the verified key to {}",
            path.display()
        ))
    }
}

fn host_patterns_match(patterns: &HostPatterns, host: &str, normalized_host: &str) -> bool {
    match patterns {
        HostPatterns::HashedName { salt, hash } => {
            hashed_host_matches(salt, hash, host)
                || (host != normalized_host && hashed_host_matches(salt, hash, normalized_host))
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
                        return false;
                    }
                    positive_match = true;
                }
            }

            positive_match
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
    use std::{fs, path::PathBuf};

    use russh::keys::ssh_key::PublicKey;
    use uuid::Uuid;

    use super::verify_server_key_at_path;

    const KNOWN_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const CHANGED_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G1sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X";

    struct TempKnownHosts {
        path: PathBuf,
    }

    impl TempKnownHosts {
        fn new(contents: &str) -> Self {
            let path = std::env::temp_dir().join(format!("jshell-known-hosts-{}", Uuid::new_v4()));
            fs::write(&path, contents).expect("write temporary known_hosts");
            Self { path }
        }
    }

    impl Drop for TempKnownHosts {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    fn public_key(value: &str) -> PublicKey {
        PublicKey::from_openssh(value).expect("parse test public key")
    }

    #[test]
    fn accepts_matching_host_key() {
        let known_hosts = TempKnownHosts::new(&format!("example.test {KNOWN_KEY}\n"));

        assert!(
            verify_server_key_at_path(
                "example.test",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .is_ok()
        );
    }

    #[test]
    fn accepts_matching_hashed_host_key() {
        let known_hosts = TempKnownHosts::new(&format!(
            "|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak= {KNOWN_KEY}\n"
        ));

        assert!(
            verify_server_key_at_path(
                "example.com",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .is_ok()
        );
    }

    #[test]
    fn accepts_matching_key_from_wildcard_host_pattern() {
        let known_hosts = TempKnownHosts::new(&format!("*.example.test {KNOWN_KEY}\n"));

        assert!(
            verify_server_key_at_path(
                "api.example.test",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_host_excluded_by_negated_regular_pattern() {
        let known_hosts =
            TempKnownHosts::new(&format!("!bad.example.test,bad.example.test {KNOWN_KEY}\n"));

        let error = verify_server_key_at_path(
            "bad.example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("negated host pattern must override a positive pattern");

        assert!(error.to_string().contains("bad.example.test:22"));
    }

    #[test]
    fn rejects_unknown_host() {
        let known_hosts = TempKnownHosts::new(&format!("example.test {KNOWN_KEY}\n"));

        let error = verify_server_key_at_path(
            "unknown.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("unknown host must be rejected");

        assert!(error.to_string().contains("unknown.test:22"));
    }

    #[test]
    fn rejects_missing_known_hosts_file() {
        let missing_path =
            std::env::temp_dir().join(format!("jshell-missing-known-hosts-{}", Uuid::new_v4()));

        let error =
            verify_server_key_at_path("example.test", 22, &public_key(KNOWN_KEY), &missing_path)
                .expect_err("missing known_hosts must be rejected");

        assert!(error.to_string().contains("example.test:22"));
    }

    #[test]
    fn rejects_malformed_matching_entry() {
        let known_hosts = TempKnownHosts::new("example.test ssh-ed25519 not-base64\n");

        let error = verify_server_key_at_path(
            "example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("malformed matching entry must be rejected");

        assert!(error.to_string().contains("example.test:22"));
    }

    #[test]
    fn rejects_changed_host_key() {
        let known_hosts = TempKnownHosts::new(&format!("example.test {KNOWN_KEY}\n"));

        let error = verify_server_key_at_path(
            "example.test",
            22,
            &public_key(CHANGED_KEY),
            &known_hosts.path,
        )
        .expect_err("changed key must be rejected");

        assert!(error.to_string().contains("example.test:22"));
    }

    #[test]
    fn rejects_revoked_key_even_when_regular_entry_matches() {
        let known_hosts = TempKnownHosts::new(&format!(
            "example.test {KNOWN_KEY}\n@revoked example.test {KNOWN_KEY}\n"
        ));

        let error = verify_server_key_at_path(
            "example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("revoked host key must be rejected");

        assert!(error.to_string().contains("revoked"));
    }

    #[test]
    fn rejects_revoked_key_for_matching_wildcard_host() {
        let known_hosts = TempKnownHosts::new(&format!(
            "api.example.test {KNOWN_KEY}\n@revoked *.example.test {KNOWN_KEY}\n"
        ));

        let error = verify_server_key_at_path(
            "api.example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("key revoked by matching wildcard must be rejected");

        assert!(error.to_string().contains("revoked"));
    }

    #[test]
    fn rejects_revoked_key_for_matching_hashed_host() {
        let known_hosts = TempKnownHosts::new(&format!(
            "example.com {KNOWN_KEY}\n@revoked |1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak= {KNOWN_KEY}\n"
        ));

        let error =
            verify_server_key_at_path("example.com", 22, &public_key(KNOWN_KEY), &known_hosts.path)
                .expect_err("key revoked for hashed host must be rejected");

        assert!(error.to_string().contains("revoked"));
    }

    #[test]
    fn rejects_revoked_key_for_hashed_host_with_original_case() {
        let known_hosts = TempKnownHosts::new(&format!(
            "EXAMPLE.COM {KNOWN_KEY}\n@revoked |1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|mZKP+zxZ9/rd2Drc54FZJa7nqhw= {KNOWN_KEY}\n"
        ));

        let error =
            verify_server_key_at_path("EXAMPLE.COM", 22, &public_key(KNOWN_KEY), &known_hosts.path)
                .expect_err(
                    "hashed revocation must use the same host identity as regular matching",
                );

        assert!(error.to_string().contains("revoked"));
    }

    #[test]
    fn accepts_key_revoked_only_for_another_host() {
        let known_hosts = TempKnownHosts::new(&format!(
            "example.test {KNOWN_KEY}\n@revoked other.test {KNOWN_KEY}\n"
        ));

        assert!(
            verify_server_key_at_path(
                "example.test",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .is_ok()
        );
    }

    #[test]
    fn accepts_key_excluded_from_revocation_by_negated_pattern() {
        let known_hosts = TempKnownHosts::new(&format!(
            "safe.example.test {KNOWN_KEY}\n@revoked !safe.example.test,*.example.test {KNOWN_KEY}\n"
        ));

        assert!(
            verify_server_key_at_path(
                "safe.example.test",
                22,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_revoked_key_on_non_default_port() {
        let known_hosts = TempKnownHosts::new(&format!(
            "[example.test]:2222 {KNOWN_KEY}\n@revoked [example.test]:2222 {KNOWN_KEY}\n"
        ));

        let error = verify_server_key_at_path(
            "example.test",
            2222,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("revoked key on a non-default port must be rejected");

        assert!(error.to_string().contains("revoked"));
    }

    #[test]
    fn does_not_trust_cert_authority_as_a_direct_host_key() {
        let known_hosts =
            TempKnownHosts::new(&format!("@cert-authority example.test {KNOWN_KEY}\n"));

        let error = verify_server_key_at_path(
            "example.test",
            22,
            &public_key(KNOWN_KEY),
            &known_hosts.path,
        )
        .expect_err("certificate authority entry must not directly trust the same public key");

        assert!(error.to_string().contains("not trusted"));
    }

    #[test]
    fn accepts_matching_key_on_non_default_port() {
        let known_hosts = TempKnownHosts::new(&format!("[example.test]:2222 {KNOWN_KEY}\n"));

        assert!(
            verify_server_key_at_path(
                "example.test",
                2222,
                &public_key(KNOWN_KEY),
                &known_hosts.path,
            )
            .is_ok()
        );
    }
}
