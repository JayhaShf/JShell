use std::{
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};

use anyhow::Result;
use russh_sftp::client::SftpSession;
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::Notify,
};

const MAX_SFTP_PREFIX_BYTES: usize = 4096;
const MAX_VERSION_FRAME_BYTES: usize = 64 * 1024;
const VERSION_FRAME_HEADER_BYTES: usize = 9;
const SSH_FXP_VERSION: u8 = 2;
const SUPPORTED_SFTP_VERSION: u32 = 3;

#[derive(Debug, Error)]
#[error(
    "remote server produced non-SFTP output before the SFTP protocol handshake; disable shell startup output for non-interactive sessions"
)]
pub(crate) struct SftpHandshakeOutputError;

pub(crate) async fn open_sftp_session<S>(stream: S) -> Result<SftpSession>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let diagnostics = Arc::new(HandshakeDiagnostics::default());
    let stream = VersionAlignedStream::new(stream, diagnostics.clone());
    let session = SftpSession::new(stream);
    tokio::pin!(session);

    let result = tokio::select! {
        result = &mut session => result,
        _ = diagnostics.wait_for_rejection() => {
            return Err(SftpHandshakeOutputError.into());
        }
    };

    match result {
        Ok(session) => Ok(session),
        Err(_) if diagnostics.has_unresolved_prefix() => Err(SftpHandshakeOutputError.into()),
        Err(error) => Err(error.into()),
    }
}

#[derive(Default)]
struct HandshakeDiagnostics {
    saw_prefix: AtomicBool,
    aligned: AtomicBool,
    rejected: AtomicBool,
    rejection: Notify,
}

impl HandshakeDiagnostics {
    fn mark_prefix(&self) {
        self.saw_prefix.store(true, Ordering::Release);
    }

    fn mark_aligned(&self) {
        self.aligned.store(true, Ordering::Release);
    }

    fn reject(&self) {
        self.rejected.store(true, Ordering::Release);
        self.rejection.notify_one();
    }

    fn has_unresolved_prefix(&self) -> bool {
        self.saw_prefix.load(Ordering::Acquire) && !self.aligned.load(Ordering::Acquire)
    }

    async fn wait_for_rejection(&self) {
        if self.rejected.load(Ordering::Acquire) {
            return;
        }
        self.rejection.notified().await;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadState {
    Aligning,
    Passthrough,
    Rejected,
}

struct VersionAlignedStream<S> {
    inner: S,
    buffered: Vec<u8>,
    state: ReadState,
    diagnostics: Arc<HandshakeDiagnostics>,
}

impl<S> VersionAlignedStream<S> {
    fn new(inner: S, diagnostics: Arc<HandshakeDiagnostics>) -> Self {
        Self {
            inner,
            buffered: Vec::new(),
            state: ReadState::Aligning,
            diagnostics,
        }
    }

    fn try_align(&mut self) {
        if let Some(prefix_len) = find_version_frame(&self.buffered) {
            if prefix_len > 0 {
                self.diagnostics.mark_prefix();
                tracing::warn!(
                    skipped_bytes = prefix_len,
                    "ignored remote output before SFTP version packet"
                );
                self.buffered.drain(..prefix_len);
            }
            self.diagnostics.mark_aligned();
            self.state = ReadState::Passthrough;
            return;
        }

        if !could_start_with_version_frame(&self.buffered) {
            self.diagnostics.mark_prefix();
        }

        if !could_still_align(&self.buffered) {
            self.diagnostics.mark_prefix();
            self.diagnostics.reject();
            self.state = ReadState::Rejected;
        }
    }

    fn reject_unresolved_prefix(&mut self) {
        if self.diagnostics.has_unresolved_prefix() {
            self.diagnostics.reject();
            self.state = ReadState::Rejected;
        }
    }

    fn copy_buffered(&mut self, output: &mut ReadBuf<'_>) -> bool {
        if self.buffered.is_empty() || output.remaining() == 0 {
            return false;
        }

        let length = output.remaining().min(self.buffered.len());
        output.put_slice(&self.buffered[..length]);
        self.buffered.drain(..length);
        true
    }
}

impl<S> AsyncRead for VersionAlignedStream<S>
where
    S: AsyncRead + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }

        loop {
            match this.state {
                ReadState::Passthrough => {
                    if this.copy_buffered(output) {
                        return Poll::Ready(Ok(()));
                    }
                    return Pin::new(&mut this.inner).poll_read(cx, output);
                }
                ReadState::Rejected => return Poll::Ready(Ok(())),
                ReadState::Aligning => {
                    this.try_align();
                    if this.state != ReadState::Aligning {
                        continue;
                    }

                    let mut incoming = [0_u8; 8192];
                    let mut incoming_buf = ReadBuf::new(&mut incoming);
                    match Pin::new(&mut this.inner).poll_read(cx, &mut incoming_buf) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Err(error)) => {
                            this.reject_unresolved_prefix();
                            return Poll::Ready(Err(error));
                        }
                        Poll::Ready(Ok(())) if incoming_buf.filled().is_empty() => {
                            this.try_align();
                            this.reject_unresolved_prefix();
                            return Poll::Ready(Ok(()));
                        }
                        Poll::Ready(Ok(())) => {
                            this.buffered.extend_from_slice(incoming_buf.filled());
                        }
                    }
                }
            }
        }
    }
}

impl<S> AsyncWrite for VersionAlignedStream<S>
where
    S: AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buffer)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

fn find_version_frame(buffer: &[u8]) -> Option<usize> {
    let last_start =
        MAX_SFTP_PREFIX_BYTES.min(buffer.len().saturating_sub(VERSION_FRAME_HEADER_BYTES));
    (0..=last_start).find(|&start| {
        let Some(frame_len) = version_frame_len(&buffer[start..]) else {
            return false;
        };
        buffer.len() >= start + frame_len
            && valid_extension_payload(
                &buffer[start + VERSION_FRAME_HEADER_BYTES..start + frame_len],
            )
    })
}

fn version_frame_len(buffer: &[u8]) -> Option<usize> {
    if buffer.len() < VERSION_FRAME_HEADER_BYTES {
        return None;
    }

    let packet_len = read_u32(&buffer[..4]) as usize;
    let frame_len = packet_len.checked_add(4)?;
    if packet_len < 5 || frame_len > MAX_VERSION_FRAME_BYTES {
        return None;
    }
    if buffer[4] != SSH_FXP_VERSION || read_u32(&buffer[5..9]) != SUPPORTED_SFTP_VERSION {
        return None;
    }

    Some(frame_len)
}

fn valid_extension_payload(payload: &[u8]) -> bool {
    let mut offset = 0;
    while offset < payload.len() {
        if !consume_ssh_string(payload, &mut offset) || !consume_ssh_string(payload, &mut offset) {
            return false;
        }
    }
    true
}

fn consume_ssh_string(payload: &[u8], offset: &mut usize) -> bool {
    let Some(length_end) = offset.checked_add(4) else {
        return false;
    };
    if length_end > payload.len() {
        return false;
    }

    let string_len = read_u32(&payload[*offset..length_end]) as usize;
    let Some(string_end) = length_end.checked_add(string_len) else {
        return false;
    };
    if string_end > payload.len() {
        return false;
    }

    *offset = string_end;
    true
}

fn could_start_with_version_frame(buffer: &[u8]) -> bool {
    if buffer.first().is_some_and(|byte| *byte != 0) || buffer.get(1).is_some_and(|byte| *byte != 0)
    {
        return false;
    }
    if buffer.len() < 4 {
        return true;
    }

    let packet_len = read_u32(&buffer[..4]) as usize;
    let Some(frame_len) = packet_len.checked_add(4) else {
        return false;
    };
    if packet_len < 5 || frame_len > MAX_VERSION_FRAME_BYTES {
        return false;
    }
    if buffer.get(4).is_some_and(|byte| *byte != SSH_FXP_VERSION) {
        return false;
    }
    if buffer.len() >= VERSION_FRAME_HEADER_BYTES
        && read_u32(&buffer[5..9]) != SUPPORTED_SFTP_VERSION
    {
        return false;
    }
    if buffer.len() >= frame_len
        && !valid_extension_payload(&buffer[VERSION_FRAME_HEADER_BYTES..frame_len])
    {
        return false;
    }

    true
}

fn could_still_align(buffer: &[u8]) -> bool {
    if buffer.len() < MAX_SFTP_PREFIX_BYTES + VERSION_FRAME_HEADER_BYTES {
        return true;
    }

    (0..=MAX_SFTP_PREFIX_BYTES).any(|start| {
        version_frame_len(&buffer[start..])
            .is_some_and(|frame_len| buffer.len() < start + frame_len)
    })
}

fn read_u32(buffer: &[u8]) -> u32 {
    u32::from_be_bytes(
        buffer
            .try_into()
            .expect("SFTP frame parser reads exactly four bytes"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt as _;

    fn version_frame(extensions: &[(&[u8], &[u8])]) -> Vec<u8> {
        let mut payload = Vec::new();
        for (name, data) in extensions {
            payload.extend_from_slice(&(name.len() as u32).to_be_bytes());
            payload.extend_from_slice(name);
            payload.extend_from_slice(&(data.len() as u32).to_be_bytes());
            payload.extend_from_slice(data);
        }

        let mut frame = Vec::with_capacity(VERSION_FRAME_HEADER_BYTES + payload.len());
        frame.extend_from_slice(&(5_u32 + payload.len() as u32).to_be_bytes());
        frame.push(SSH_FXP_VERSION);
        frame.extend_from_slice(&SUPPORTED_SFTP_VERSION.to_be_bytes());
        frame.extend_from_slice(&payload);
        frame
    }

    #[test]
    fn accepts_a_valid_sftp_v3_version_frame() {
        let frame = version_frame(&[(b"vendor-id", b"jshell-test")]);

        assert_eq!(version_frame_len(&frame), Some(frame.len()));
        assert_eq!(find_version_frame(&frame), Some(0));
    }

    #[test]
    fn aligns_after_remote_output_prefix() {
        let prefix = b"Last login: test\r\n";
        let frame = version_frame(&[]);
        let mut input = prefix.to_vec();
        input.extend_from_slice(&frame);

        assert_eq!(find_version_frame(&input), Some(prefix.len()));
    }

    #[test]
    fn accepts_the_maximum_prefix_but_rejects_a_frame_beyond_it() {
        let frame = version_frame(&[]);
        let mut at_limit = vec![b'x'; MAX_SFTP_PREFIX_BYTES];
        at_limit.extend_from_slice(&frame);
        assert_eq!(find_version_frame(&at_limit), Some(MAX_SFTP_PREFIX_BYTES));

        let mut beyond_limit = vec![b'x'; MAX_SFTP_PREFIX_BYTES + 1];
        beyond_limit.extend_from_slice(&frame);
        assert_eq!(find_version_frame(&beyond_limit), None);
        assert!(!could_still_align(&beyond_limit));
    }

    #[test]
    fn rejects_an_unresolved_prefix_when_the_scan_window_is_exhausted() {
        let before_boundary = vec![b'x'; MAX_SFTP_PREFIX_BYTES + VERSION_FRAME_HEADER_BYTES - 1];
        assert!(could_still_align(&before_boundary));

        let at_boundary = vec![b'x'; MAX_SFTP_PREFIX_BYTES + VERSION_FRAME_HEADER_BYTES];
        assert!(!could_still_align(&at_boundary));
    }

    #[test]
    fn rejects_pseudo_version_frames() {
        let mut wrong_type = version_frame(&[]);
        wrong_type[4] = 1;
        assert_eq!(version_frame_len(&wrong_type), None);
        assert_eq!(find_version_frame(&wrong_type), None);

        let mut wrong_version = version_frame(&[]);
        wrong_version[5..9].copy_from_slice(&4_u32.to_be_bytes());
        assert_eq!(version_frame_len(&wrong_version), None);
        assert_eq!(find_version_frame(&wrong_version), None);
    }

    #[test]
    fn rejects_a_version_frame_with_a_truncated_extension() {
        let mut frame = version_frame(&[]);
        frame[..4].copy_from_slice(&9_u32.to_be_bytes());
        frame.extend_from_slice(&1_u32.to_be_bytes());

        assert_eq!(version_frame_len(&frame), Some(frame.len()));
        assert!(!valid_extension_payload(
            &frame[VERSION_FRAME_HEADER_BYTES..]
        ));
        assert_eq!(find_version_frame(&frame), None);
        assert!(!could_start_with_version_frame(&frame));
    }

    #[tokio::test]
    async fn unresolved_remote_output_requires_manual_retry() {
        let (client, mut server) = tokio::io::duplex(8192);
        let server_task = tokio::spawn(async move {
            let invalid_output = [b'x'; MAX_SFTP_PREFIX_BYTES + VERSION_FRAME_HEADER_BYTES];
            server
                .write_all(&invalid_output)
                .await
                .expect("write invalid handshake output");
            server.shutdown().await.expect("close test server stream");
        });

        let error = match open_sftp_session(client).await {
            Ok(_) => panic!("unresolved remote output must fail the handshake"),
            Err(error) => error,
        };
        server_task.await.expect("join test server task");

        assert!(error.downcast_ref::<SftpHandshakeOutputError>().is_some());
        assert_eq!(
            crate::sftp::sftp_retry_policy(&error),
            crate::sftp::SftpRetryPolicy::Manual
        );
    }
}
