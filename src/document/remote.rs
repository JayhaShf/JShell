use anyhow::Result;
use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum RemoteFileError {
    #[error("remote file not found")]
    NotFound,
    #[error("SFTP connection closed")]
    ChannelClosed,
}

pub fn is_not_found(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<RemoteFileError>(),
        Some(RemoteFileError::NotFound)
    )
}

pub fn is_connection_closed(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<RemoteFileError>(),
        Some(RemoteFileError::ChannelClosed)
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteMetadata {
    pub size: u64,
    pub mtime: u32,
    pub permissions: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub offset: u64,
    pub length: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RemoteDocumentKey {
    pub connection_id: String,
    pub remote_path: String,
}

impl RemoteDocumentKey {
    pub fn new(connection_id: impl Into<String>, path: &str) -> Self {
        let absolute = path.starts_with('/');
        let mut parts = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                value => parts.push(value),
            }
        }
        let normalized = format!("{}{}", if absolute { "/" } else { "" }, parts.join("/"));
        Self {
            connection_id: connection_id.into(),
            remote_path: normalized,
        }
    }
}

#[async_trait]
pub trait RemoteFileBackend: Clone + Send + Sync + 'static {
    async fn stat(&self, path: &str) -> Result<RemoteMetadata>;
    async fn read(&self, path: &str, range: Option<ByteRange>) -> Result<Vec<u8>>;
    async fn write_atomic(
        &self,
        path: &str,
        bytes: Vec<u8>,
        permissions: Option<u32>,
        operation_id: &str,
    ) -> Result<RemoteMetadata>;
}

#[derive(Clone)]
pub struct SftpRemoteFileBackend {
    handle: crate::sftp::SftpHandle,
}

impl SftpRemoteFileBackend {
    pub fn new(handle: crate::sftp::SftpHandle) -> Self {
        Self { handle }
    }

    pub fn download(&self, remote_path: String, local_directory: String) {
        self.handle.download(remote_path, local_directory);
    }
}

#[async_trait]
impl RemoteFileBackend for SftpRemoteFileBackend {
    async fn stat(&self, path: &str) -> Result<RemoteMetadata> {
        let (reply, receive) = tokio::sync::oneshot::channel();
        self.handle.document_stat(path.to_string(), reply);
        receive
            .await
            .map_err(|_| anyhow::Error::new(RemoteFileError::ChannelClosed))?
    }

    async fn read(&self, path: &str, range: Option<ByteRange>) -> Result<Vec<u8>> {
        let (reply, receive) = tokio::sync::oneshot::channel();
        self.handle.document_read(path.to_string(), range, reply);
        receive
            .await
            .map_err(|_| anyhow::Error::new(RemoteFileError::ChannelClosed))?
    }

    async fn write_atomic(
        &self,
        path: &str,
        bytes: Vec<u8>,
        permissions: Option<u32>,
        operation_id: &str,
    ) -> Result<RemoteMetadata> {
        let (reply, receive) = tokio::sync::oneshot::channel();
        self.handle.document_write_atomic(
            path.to_string(),
            bytes,
            permissions,
            operation_id.to_string(),
            reply,
        );
        receive
            .await
            .map_err(|_| anyhow::Error::new(RemoteFileError::ChannelClosed))?
    }
}

pub fn has_conflict(opened: &RemoteMetadata, current: &RemoteMetadata) -> bool {
    opened.size != current.size || opened.mtime != current.mtime
}

pub fn temporary_remote_path(path: &str, operation_id: &str) -> String {
    replacement_remote_path(path, operation_id, "tmp")
}

pub fn backup_remote_path(path: &str, operation_id: &str) -> String {
    replacement_remote_path(path, operation_id, "bak")
}

fn replacement_remote_path(path: &str, operation_id: &str, suffix: &str) -> String {
    let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
    if parent.is_empty() && path.starts_with('/') {
        format!("/.{name}.ashell-{operation_id}.{suffix}")
    } else if parent.is_empty() {
        format!(".{name}.ashell-{operation_id}.{suffix}")
    } else {
        format!("{parent}/.{name}.ashell-{operation_id}.{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_missing_files_from_closed_connections() {
        let missing = anyhow::Error::new(RemoteFileError::NotFound);
        let closed = anyhow::Error::new(RemoteFileError::ChannelClosed);

        assert!(is_not_found(&missing));
        assert!(!is_connection_closed(&missing));
        assert!(!is_not_found(&closed));
        assert!(is_connection_closed(&closed));
    }

    #[test]
    fn normalizes_document_keys_and_detects_conflicts() {
        let key = RemoteDocumentKey::new("session-1", "//etc/nginx/../nginx/nginx.conf");
        assert_eq!(key.remote_path, "/etc/nginx/nginx.conf");

        let opened = RemoteMetadata {
            size: 10,
            mtime: 20,
            permissions: Some(0o100644),
        };
        assert!(!has_conflict(&opened, &opened));
        assert!(has_conflict(
            &opened,
            &RemoteMetadata {
                size: 11,
                ..opened.clone()
            }
        ));
    }

    #[test]
    fn replacement_paths_stay_in_the_target_directory() {
        assert_eq!(
            temporary_remote_path("/etc/nginx/nginx.conf", "op-1"),
            "/etc/nginx/.nginx.conf.ashell-op-1.tmp"
        );
        assert_eq!(
            backup_remote_path("/etc/nginx/nginx.conf", "op-1"),
            "/etc/nginx/.nginx.conf.ashell-op-1.bak"
        );
        assert_eq!(
            temporary_remote_path("/nginx.conf", "op-1"),
            "/.nginx.conf.ashell-op-1.tmp"
        );
    }
}
