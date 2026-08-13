use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::BaseDirs;

/// A parsed entry from ~/.ssh/config
#[derive(Debug, Clone)]
pub struct SshConfigEntry {
    /// The Host pattern (alias) from the config, e.g. "myserver"
    pub host_alias: String,
    /// The actual hostname (HostName), defaults to the host alias if not specified
    pub hostname: String,
    /// The user, defaults to empty (will use current OS user)
    pub user: String,
    /// The port, defaults to 22
    pub port: u16,
    /// Identity files specified for this host
    pub identity_files: Vec<String>,
    /// Whether this is a wildcard/pattern host (Host * or Host *)
    pub is_wildcard: bool,
}

/// Parse ~/.ssh/config and return a list of concrete host entries.
/// Wildcard patterns (Host *) are excluded.
pub fn parse_ssh_config() -> Result<Vec<SshConfigEntry>> {
    let config_path = ssh_config_path()?;
    if !config_path.is_file() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;

    parse_ssh_config_content(&content)
}

fn ssh_config_path() -> Result<PathBuf> {
    BaseDirs::new()
        .map(|dirs| dirs.home_dir().join(".ssh/config"))
        .context("failed to determine home directory")
}

/// Parse the content of an ssh config file into entries.
pub fn parse_ssh_config_content(content: &str) -> Result<Vec<SshConfigEntry>> {
    let mut entries: Vec<SshConfigEntry> = Vec::new();
    let mut current_host: Option<SshConfigEntry> = None;

    for line in content.lines() {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Split into keyword and value
        // OpenSSH supports both "keyword value" and "keyword=value" formats,
        // separated by either spaces or tabs.
        let (keyword, value) = if let Some(pos) = line.find('=') {
            (&line[..pos], line[pos + 1..].trim())
        } else if let Some(pos) = line.find([' ', '\t']) {
            (&line[..pos], line[pos..].trim())
        } else {
            continue;
        };

        let keyword_lower = keyword.trim().to_lowercase();
        let value = value.trim();

        match keyword_lower.as_str() {
            "host" => {
                // Save previous entry if it exists and is not a wildcard
                if let Some(entry) = current_host.take()
                    && !entry.is_wildcard
                {
                    entries.push(entry);
                }

                // Host line may contain multiple patterns (Host a b c)
                // Take the first non-wildcard pattern as the display alias
                let patterns: Vec<&str> = value.split_whitespace().collect();
                if patterns.is_empty() {
                    continue;
                }
                let host_alias = patterns
                    .iter()
                    .find(|p| !p.contains('*') && !p.contains('?'))
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| patterns[0].to_string());

                let is_wildcard = value.contains('*') || value.contains('?');

                current_host = Some(SshConfigEntry {
                    host_alias: host_alias.clone(),
                    hostname: host_alias,
                    user: String::new(),
                    port: 22,
                    identity_files: Vec::new(),
                    is_wildcard,
                });
            }
            "hostname" => {
                if let Some(entry) = current_host.as_mut() {
                    entry.hostname = value.to_string();
                }
            }
            "user" => {
                if let Some(entry) = current_host.as_mut() {
                    entry.user = value.to_string();
                }
            }
            "port" => {
                if let Some(entry) = current_host.as_mut() {
                    // Port 0 is not a valid TCP port; treat it like any other
                    // unparsable value and fall back to the default.
                    entry.port = value
                        .parse::<u16>()
                        .ok()
                        .filter(|port| *port > 0)
                        .unwrap_or(22);
                }
            }
            "identityfile" => {
                if let Some(entry) = current_host.as_mut() {
                    entry.identity_files.push(value.to_string());
                }
            }
            // Skip Match blocks and Include directives — not supported yet
            "match" | "include" => {
                // If we encounter a Match block, flush the current Host entry
                // since Match blocks apply to all subsequent hosts until the next Match/Host
                if let Some(entry) = current_host.take()
                    && !entry.is_wildcard
                {
                    entries.push(entry);
                }
                // Don't create a new entry for Match/Include
            }
            _ => {}
        }
    }

    // Save the last entry if it's not a wildcard
    if let Some(entry) = current_host.take()
        && !entry.is_wildcard
    {
        entries.push(entry);
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ssh_config_content() {
        let content = r#"
            Host myhost
                HostName 1.2.3.4
                User git
                Port 2222
                IdentityFile ~/.ssh/id_rsa

            Host
            Host = 

            Host anotherhost
                HostName 5.6.7.8
        "#;

        let entries = parse_ssh_config_content(content).unwrap();
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].host_alias, "myhost");
        assert_eq!(entries[0].hostname, "1.2.3.4");
        assert_eq!(entries[0].user, "git");
        assert_eq!(entries[0].port, 2222);
        assert_eq!(entries[0].identity_files, vec!["~/.ssh/id_rsa".to_string()]);

        assert_eq!(entries[1].host_alias, "anotherhost");
        assert_eq!(entries[1].hostname, "5.6.7.8");
    }

    #[test]
    fn defaults_apply_when_host_has_no_options() {
        let entries = parse_ssh_config_content("Host bare\n").unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].host_alias, "bare");
        assert_eq!(entries[0].hostname, "bare");
        assert_eq!(entries[0].user, "");
        assert_eq!(entries[0].port, 22);
        assert!(entries[0].identity_files.is_empty());
        assert!(!entries[0].is_wildcard);
    }

    #[test]
    fn wildcard_and_question_mark_hosts_are_excluded() {
        let content = "\
Host *
  HostName anything
Host prod?.example.com
  User admin
Host real
  User me
";

        let entries = parse_ssh_config_content(content).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].host_alias, "real");
        assert_eq!(entries[0].user, "me");
    }

    #[test]
    fn multi_pattern_host_is_treated_as_wildcard() {
        let content = "\
Host primary *.example.com
  HostName 10.0.0.1
Host solo
";

        let entries = parse_ssh_config_content(content).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].host_alias, "solo");
    }

    #[test]
    fn keyword_equals_value_format_is_supported() {
        let content = "Host eq\nHostName=192.0.2.7\nPort=2200\nUser=bob\n";

        let entries = parse_ssh_config_content(content).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].hostname, "192.0.2.7");
        assert_eq!(entries[0].port, 2200);
        assert_eq!(entries[0].user, "bob");
    }

    #[test]
    fn keywords_are_case_insensitive() {
        let content = "host casey\n  HOSTNAME 203.0.113.9\n  USER alice\n  PORT 2022\n";

        let entries = parse_ssh_config_content(content).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].hostname, "203.0.113.9");
        assert_eq!(entries[0].user, "alice");
        assert_eq!(entries[0].port, 2022);
    }

    #[test]
    fn tab_separated_keywords_are_supported() {
        let content = "Host\ttabbed\n\tHostName\t198.51.100.4\n\tPort\t2224\n";

        let entries = parse_ssh_config_content(content).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].hostname, "198.51.100.4");
        assert_eq!(entries[0].port, 2224);
    }

    #[test]
    fn invalid_ports_fall_back_to_the_default() {
        for bad in ["not-a-port", "70000", "-1", "0"] {
            let content = format!("Host p\n  Port {bad}\n");
            let entries = parse_ssh_config_content(&content).unwrap();
            assert_eq!(entries[0].port, 22, "port {bad:?}");
        }
    }

    #[test]
    fn multiple_identity_files_accumulate() {
        let content = "Host multi\n  IdentityFile ~/.ssh/a\n  IdentityFile ~/.ssh/b\n";

        let entries = parse_ssh_config_content(content).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].identity_files,
            vec!["~/.ssh/a".to_string(), "~/.ssh/b".to_string()],
        );
    }

    #[test]
    fn match_blocks_flush_and_detach_the_current_host() {
        let content = "\
Host before
  User u1
Match host x
  HostName 1.2.3.4
Host after
  User u2
";

        let entries = parse_ssh_config_content(content).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].host_alias, "before");
        assert_eq!(entries[1].host_alias, "after");
        // The HostName under Match must not attach to "after".
        assert_eq!(entries[1].hostname, "after");
    }

    #[test]
    fn include_directives_flush_the_current_host() {
        let content = "Host h1\n  User a\nInclude ~/.ssh/other\nHost h2\n";

        let entries = parse_ssh_config_content(content).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].host_alias, "h1");
        assert_eq!(entries[1].host_alias, "h2");
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let content = "# leading comment\n\nHost c\n  HostName 192.0.2.1\n";

        let entries = parse_ssh_config_content(content).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].host_alias, "c");
        assert_eq!(entries[0].hostname, "192.0.2.1");
    }

    #[test]
    fn unknown_keywords_are_ignored() {
        let content = "Host u\n  UnknownKeyword whatever\n  HostName 192.0.2.2\n";

        let entries = parse_ssh_config_content(content).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].hostname, "192.0.2.2");
    }
}
