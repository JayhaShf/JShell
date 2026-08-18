pub mod connection;
pub(crate) mod cwd_follow;
mod handshake;
pub mod ops;
pub mod permissions;

use connection::{ConnectionSupervisor, SftpGeneration};
use handshake::{SftpHandshakeOutputError, open_sftp_session};
use permissions::{RemoteFileType, file_type_from_mode};

use std::{
    collections::{HashMap, VecDeque},
    fs,
    future::Future,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, TimeZone, Utc};
use directories::BaseDirs;
use flate2::read::GzDecoder;
use russh::{
    Disconnect,
    client::{self, Handler},
    keys::{PrivateKey, decode_secret_key, load_secret_key},
};
use russh_sftp::client::SftpSession;
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
};
use uuid::Uuid;
use walkdir::WalkDir;
use zip::read::ZipArchive;

use rust_i18n::t;

use crate::{
    session::{
        config::{AuthMethod, ConnectionProxyConfig, Session},
        host_keys::{HostKeyVerifier, is_permanent_host_key_error},
        ssh_keys::{
            authenticate_with_default_keys, normalize_inline_private_key, private_keys_with_algs,
            session_has_explicit_key,
        },
    },
    terminal::{BackendEvent, SftpConnectionState},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SftpRetryPolicy {
    Backoff,
    Manual,
}

fn sftp_retry_policy(error: &anyhow::Error) -> SftpRetryPolicy {
    if is_permanent_host_key_error(error)
        || error.downcast_ref::<SftpHandshakeOutputError>().is_some()
    {
        SftpRetryPolicy::Manual
    } else {
        SftpRetryPolicy::Backoff
    }
}

#[derive(Debug, Clone)]
pub struct RemoteEntry {
    pub name: String,
    pub full_path: String,
    pub is_dir: bool,
    pub file_type: RemoteFileType,
    pub permissions: Option<u32>,
    pub size: u64,
    pub modified: u32,
}

#[derive(Debug, Clone)]
pub struct PreviewData {
    pub path: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug)]
pub enum SftpCommand {
    ListDir {
        path: String,
        request_id: Option<u64>,
        expected_generation: Option<u64>,
    },
    Preview(String),
    Download {
        remote: String,
        local_dir: String,
    },
    CreateDir(String),
    DeletePaths(Vec<String>),
    DeleteFinished {
        id: String,
        deleted_paths: Vec<String>,
        errors: Vec<String>,
    },
    UploadPaths {
        locals: Vec<String>,
        remote_dir: String,
    },
    PauseTransfer(String),
    ResumeTransfer(String),
    CancelTransfer(String),
    TransferFinished(String),
    ReconnectNow,
    DocumentStat {
        path: String,
        reply:
            tokio::sync::oneshot::Sender<anyhow::Result<crate::document::remote::RemoteMetadata>>,
    },
    DocumentRead {
        path: String,
        range: Option<crate::document::remote::ByteRange>,
        reply: tokio::sync::oneshot::Sender<anyhow::Result<Vec<u8>>>,
    },
    DocumentWriteAtomic {
        path: String,
        bytes: Vec<u8>,
        permissions: Option<u32>,
        operation_id: String,
        reply:
            tokio::sync::oneshot::Sender<anyhow::Result<crate::document::remote::RemoteMetadata>>,
    },
    Close,
}

impl SftpCommand {
    fn is_replayable(&self) -> bool {
        matches!(
            self,
            Self::ListDir { .. }
                | Self::Preview(_)
                | Self::DocumentStat { .. }
                | Self::DocumentRead { .. }
        )
    }
}

const MAX_PENDING_SFTP_COMMANDS: usize = 128;
const SFTP_SETUP_TIMEOUT: Duration = Duration::from_secs(30);
const REMOTE_COMMAND_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_REMOTE_COMMAND_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug)]
enum SftpSetupOutcome<T> {
    Ready {
        value: T,
        pending: VecDeque<SftpCommand>,
    },
    Cancelled,
}

async fn await_sftp_setup_or_close<T>(
    setup: impl Future<Output = Result<T>>,
    commands: &mut UnboundedReceiver<SftpCommand>,
    timeout: Duration,
) -> Result<SftpSetupOutcome<T>> {
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(setup);
    tokio::pin!(deadline);
    let mut pending = VecDeque::new();

    loop {
        tokio::select! {
            result = &mut setup => {
                return result.map(|value| SftpSetupOutcome::Ready { value, pending });
            }
            _ = &mut deadline => {
                return Err(anyhow!(
                    "SFTP setup timed out after {} seconds",
                    timeout.as_secs()
                ));
            }
            command = commands.recv() => {
                match command {
                    None | Some(SftpCommand::Close) => {
                        return Ok(SftpSetupOutcome::Cancelled);
                    }
                    Some(command) => pending.push_back(command),
                }
            }
        }
    }
}

async fn await_sftp_auxiliary_setup<T>(
    setup: impl Future<Output = Result<T>>,
    timeout: Duration,
) -> Result<T> {
    tokio::time::timeout(timeout, setup).await.map_err(|_| {
        anyhow!(
            "auxiliary SFTP setup timed out after {} seconds",
            timeout.as_secs()
        )
    })?
}

use std::sync::atomic::{AtomicU8, AtomicU64};

pub struct TransferStateFlag(pub Arc<AtomicU8>);

impl TransferStateFlag {
    pub fn new() -> Self {
        Self(Arc::new(AtomicU8::new(0)))
    }

    pub fn pause(&self) {
        self.0.store(1, Ordering::SeqCst);
    }
    pub fn resume(&self) {
        self.0.store(0, Ordering::SeqCst);
    }
    pub fn cancel(&self) {
        self.0.store(2, Ordering::SeqCst);
    }

    pub async fn yield_if_paused(
        &self,
        events: &std::sync::mpsc::Sender<crate::terminal::BackendEvent>,
        tab_id: &str,
        id: &str,
        generation: u64,
        transferred: u64,
        total: Option<u64>,
    ) -> anyhow::Result<()> {
        let mut was_paused = false;
        loop {
            let state = self.0.load(Ordering::SeqCst);
            if state == 2 {
                return Err(anyhow::anyhow!("transfer cancelled"));
            }
            if state == 1 {
                if !was_paused {
                    let _ = events.send(crate::terminal::BackendEvent::TransferProgress {
                        tab_id: tab_id.to_string(),
                        generation,
                        id: id.to_string(),
                        transferred,
                        total,
                        state: crate::terminal::TransferState::Paused,
                    });
                    was_paused = true;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            } else {
                if was_paused {
                    let _ = events.send(crate::terminal::BackendEvent::TransferProgress {
                        tab_id: tab_id.to_string(),
                        generation,
                        id: id.to_string(),
                        transferred,
                        total,
                        state: crate::terminal::TransferState::Running,
                    });
                }
                return Ok(());
            }
        }
    }
}

#[derive(Clone, Copy)]
struct TransferContext<'a> {
    flag: &'a TransferStateFlag,
    events: &'a std::sync::mpsc::Sender<BackendEvent>,
    tab_id: &'a str,
    id: &'a str,
    generation: u64,
}

impl<'a> TransferContext<'a> {
    fn new(
        flag: &'a TransferStateFlag,
        events: &'a std::sync::mpsc::Sender<BackendEvent>,
        tab_id: &'a str,
        id: &'a str,
        generation: u64,
    ) -> Self {
        Self {
            flag,
            events,
            tab_id,
            id,
            generation,
        }
    }
}

struct SftpHandleInner {
    commands: UnboundedSender<SftpCommand>,
    _join: Option<JoinHandle<Result<()>>>,
}

impl Drop for SftpHandleInner {
    fn drop(&mut self) {
        let _ = self.commands.send(SftpCommand::Close);
    }
}

#[derive(Clone)]
pub struct SftpHandle {
    inner: Arc<SftpHandleInner>,
}

impl SftpHandle {
    pub(crate) fn send(&self, command: SftpCommand) -> bool {
        self.inner.commands.send(command).is_ok()
    }

    #[cfg(test)]
    fn from_sender_for_test(commands: UnboundedSender<SftpCommand>) -> Self {
        Self {
            inner: Arc::new(SftpHandleInner {
                commands,
                _join: None,
            }),
        }
    }

    pub fn list_dir(&self, path: String, generation: u64) {
        self.send(SftpCommand::ListDir {
            path,
            request_id: None,
            expected_generation: Some(generation),
        });
    }

    pub(crate) fn follow_dir(&self, path: String, request_id: u64, generation: u64) -> bool {
        self.send(SftpCommand::ListDir {
            path,
            request_id: Some(request_id),
            expected_generation: Some(generation),
        })
    }

    pub fn preview(&self, path: String) {
        self.send(SftpCommand::Preview(path));
    }

    pub fn download(&self, remote: String, local_dir: String) {
        self.send(SftpCommand::Download { remote, local_dir });
    }

    pub fn upload_paths(&self, locals: Vec<String>, remote_dir: String) {
        self.send(SftpCommand::UploadPaths { locals, remote_dir });
    }

    pub fn pause_transfer(&self, id: String) {
        self.send(SftpCommand::PauseTransfer(id));
    }

    pub fn resume_transfer(&self, id: String) {
        self.send(SftpCommand::ResumeTransfer(id));
    }

    pub fn cancel_transfer(&self, id: String) {
        self.send(SftpCommand::CancelTransfer(id));
    }

    pub fn reconnect_now(&self) {
        self.send(SftpCommand::ReconnectNow);
    }

    pub fn document_stat(
        &self,
        path: String,
        reply: tokio::sync::oneshot::Sender<
            anyhow::Result<crate::document::remote::RemoteMetadata>,
        >,
    ) {
        self.send(SftpCommand::DocumentStat { path, reply });
    }

    pub fn document_read(
        &self,
        path: String,
        range: Option<crate::document::remote::ByteRange>,
        reply: tokio::sync::oneshot::Sender<anyhow::Result<Vec<u8>>>,
    ) {
        self.send(SftpCommand::DocumentRead { path, range, reply });
    }

    pub fn document_write_atomic(
        &self,
        path: String,
        bytes: Vec<u8>,
        permissions: Option<u32>,
        operation_id: String,
        reply: tokio::sync::oneshot::Sender<
            anyhow::Result<crate::document::remote::RemoteMetadata>,
        >,
    ) {
        self.send(SftpCommand::DocumentWriteAtomic {
            path,
            bytes,
            permissions,
            operation_id,
            reply,
        });
    }
}

pub fn spawn_sftp(
    runtime: &tokio::runtime::Handle,
    tab_id: String,
    session: Session,
    proxy_config: ConnectionProxyConfig,
    events: std::sync::mpsc::Sender<BackendEvent>,
) -> SftpHandle {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let join = runtime.spawn(run_sftp_supervisor(
        runtime.clone(),
        tab_id,
        session,
        proxy_config,
        cmd_rx,
        events,
    ));
    SftpHandle {
        inner: Arc::new(SftpHandleInner {
            commands: cmd_tx,
            _join: Some(join),
        }),
    }
}

struct SftpWorker {
    commands: UnboundedSender<SftpCommand>,
    join: JoinHandle<Result<()>>,
    connected: Arc<AtomicBool>,
    ready: Arc<tokio::sync::Notify>,
    generation: SftpGeneration,
}

fn spawn_sftp_worker(
    runtime: &tokio::runtime::Handle,
    tab_id: &str,
    session: &Session,
    proxy_config: &ConnectionProxyConfig,
    events: &std::sync::mpsc::Sender<BackendEvent>,
    generation: SftpGeneration,
) -> SftpWorker {
    let (commands, receiver) = mpsc::unbounded_channel();
    let connected = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(tokio::sync::Notify::new());
    let connected_worker = connected.clone();
    let ready_worker = ready.clone();
    let join = runtime.spawn(run_sftp(
        tab_id.to_string(),
        session.clone(),
        proxy_config.clone(),
        receiver,
        commands.clone(),
        events.clone(),
        connected_worker,
        ready_worker,
        generation,
    ));
    SftpWorker {
        commands,
        join,
        connected,
        ready,
        generation,
    }
}

fn reject_unavailable_command(
    tab_id: &str,
    generation: SftpGeneration,
    command: SftpCommand,
    events: &std::sync::mpsc::Sender<BackendEvent>,
) {
    let error = || anyhow::Error::new(crate::document::remote::RemoteFileError::ChannelClosed);
    match command {
        SftpCommand::DocumentStat { reply, .. } => {
            let _ = reply.send(Err(error()));
        }
        SftpCommand::DocumentRead { reply, .. } => {
            let _ = reply.send(Err(error()));
        }
        SftpCommand::DocumentWriteAtomic { reply, .. } => {
            let _ = reply.send(Err(error()));
        }
        SftpCommand::DeletePaths(paths) => {
            let _ = events.send(BackendEvent::SftpStatus {
                tab_id: tab_id.to_string(),
                generation: generation.0,
                text: t!("sftp_command_channel_closed").to_string(),
            });
            let _ = events.send(BackendEvent::SftpDeleteFinished {
                tab_id: tab_id.to_string(),
                generation: generation.0,
                paths,
                deleted_paths: Vec::new(),
            });
        }
        SftpCommand::ListDir {
            path, request_id, ..
        } => {
            let _ = events.send(BackendEvent::SftpListDirFailed {
                tab_id: tab_id.to_string(),
                generation: generation.0,
                request_id,
                path,
                reason: t!("sftp_command_channel_closed").to_string(),
            });
        }
        SftpCommand::Preview(_)
        | SftpCommand::CreateDir(_)
        | SftpCommand::Download { .. }
        | SftpCommand::UploadPaths { .. } => {
            let _ = events.send(BackendEvent::SftpStatus {
                tab_id: tab_id.to_string(),
                generation: generation.0,
                text: t!("sftp_command_channel_closed").to_string(),
            });
        }
        SftpCommand::PauseTransfer(_)
        | SftpCommand::ResumeTransfer(_)
        | SftpCommand::CancelTransfer(_)
        | SftpCommand::TransferFinished(_)
        | SftpCommand::DeleteFinished { .. }
        | SftpCommand::ReconnectNow
        | SftpCommand::Close => {}
    }
}

fn dispatch_pending_commands(worker: &SftpWorker, pending: &mut VecDeque<SftpCommand>) {
    if !worker.connected.load(Ordering::SeqCst) {
        return;
    }
    while let Some(command) = pending.pop_front() {
        if let Err(error) = worker.commands.send(command) {
            pending.push_front(error.0);
            break;
        }
    }
}

fn discard_pending_automatic_list_dirs(pending: &mut VecDeque<SftpCommand>) {
    pending.retain(|command| {
        !matches!(
            command,
            SftpCommand::ListDir {
                request_id: Some(_),
                ..
            }
        )
    });
}

fn list_dir_generation_matches(
    expected_generation: Option<u64>,
    generation: SftpGeneration,
) -> bool {
    expected_generation.is_none_or(|expected| expected == generation.0)
}

fn queue_pending_command(
    pending: &mut VecDeque<SftpCommand>,
    command: SftpCommand,
    tab_id: &str,
    generation: SftpGeneration,
    events: &std::sync::mpsc::Sender<BackendEvent>,
) {
    if matches!(
        &command,
        SftpCommand::ListDir {
            request_id: Some(_),
            ..
        }
    ) {
        pending.retain(|pending_command| {
            !matches!(
                pending_command,
                SftpCommand::ListDir {
                    request_id: Some(_),
                    ..
                }
            )
        });
    }

    if pending.len() >= MAX_PENDING_SFTP_COMMANDS {
        reject_unavailable_command(tab_id, generation, command, events);
    } else {
        pending.push_back(command);
    }
}

async fn stop_sftp_worker(worker: &mut SftpWorker) {
    let _ = worker.commands.send(SftpCommand::Close);
    if tokio::time::timeout(std::time::Duration::from_secs(2), &mut worker.join)
        .await
        .is_err()
    {
        worker.join.abort();
        let _ = (&mut worker.join).await;
    }
}

async fn run_sftp_supervisor(
    runtime: tokio::runtime::Handle,
    tab_id: String,
    session: Session,
    proxy_config: ConnectionProxyConfig,
    mut commands: UnboundedReceiver<SftpCommand>,
    events: std::sync::mpsc::Sender<BackendEvent>,
) -> Result<()> {
    let mut worker: Option<SftpWorker> = None;
    let mut pending = VecDeque::new();
    let mut connection = ConnectionSupervisor::new();
    let mut retry_timer: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;

    loop {
        if worker.is_none() && retry_timer.is_none() && !connection.is_blocked() {
            // The UI coordinator will resend its newest path for this generation.
            discard_pending_automatic_list_dirs(&mut pending);
            let generation = connection
                .begin_connecting()
                .expect("SFTP supervisor is not closed");
            let _ = events.send(BackendEvent::SftpGeneration {
                tab_id: tab_id.clone(),
                generation: generation.0,
            });
            worker = Some(spawn_sftp_worker(
                &runtime,
                &tab_id,
                &session,
                &proxy_config,
                &events,
                generation,
            ));
        }

        if let Some(mut active_worker) = worker.take() {
            tokio::select! {
                result = &mut active_worker.join => {
                    let generation = active_worker.generation;
                    let (reason, retry_policy) = match result {
                        Ok(Ok(())) => (
                            "SFTP worker stopped".to_string(),
                            SftpRetryPolicy::Backoff,
                        ),
                        Ok(Err(error)) => {
                            let retry_policy = sftp_retry_policy(&error);
                            (format!("sftp error: {error:#}"), retry_policy)
                        }
                        Err(error) => (
                            format!("sftp worker stopped: {error}"),
                            SftpRetryPolicy::Backoff,
                        ),
                    };
                    match retry_policy {
                        SftpRetryPolicy::Backoff => {
                            let delay = connection
                                .disconnect(generation)
                                .and_then(|outcome| outcome.retry_after)
                                .unwrap_or_else(|| std::time::Duration::from_secs(30));
                            let _ = events.send(BackendEvent::SftpConnectionStatus {
                                tab_id: tab_id.clone(),
                                generation: generation.0,
                                state: SftpConnectionState::Reconnecting,
                                text: format!("{} ({reason})", t!("sftp_reconnecting")),
                            });
                            retry_timer = Some(Box::pin(tokio::time::sleep(delay)));
                        }
                        SftpRetryPolicy::Manual => {
                            let _ = connection.block(generation);
                            let _ = events.send(BackendEvent::SftpConnectionBlocked {
                                tab_id: tab_id.clone(),
                                generation: generation.0,
                                reason,
                            });
                        }
                    }
                }
                _ = active_worker.ready.notified() => {
                    if active_worker.connected.load(Ordering::SeqCst) {
                        connection.mark_connected(active_worker.generation);
                        dispatch_pending_commands(&active_worker, &mut pending);
                    }
                    worker = Some(active_worker);
                }
                command = commands.recv() => {
                    match command {
                        None | Some(SftpCommand::Close) => {
                            stop_sftp_worker(&mut active_worker).await;
                            connection.close();
                            break;
                        }
                        Some(SftpCommand::ReconnectNow) => {
                            stop_sftp_worker(&mut active_worker).await;
                            let _ = connection.disconnect(active_worker.generation);
                            retry_timer = None;
                        }
                        Some(command) if !active_worker.connected.load(Ordering::SeqCst) => {
                            if command.is_replayable() {
                                queue_pending_command(
                                    &mut pending,
                                    command,
                                    &tab_id,
                                    active_worker.generation,
                                    &events,
                                );
                            } else {
                                reject_unavailable_command(
                                    &tab_id,
                                    active_worker.generation,
                                    command,
                                    &events,
                                );
                            }
                            worker = Some(active_worker);
                        }
                        Some(command) => {
                            if let Err(error) = active_worker.commands.send(command) {
                                let command = error.0;
                                if command.is_replayable() {
                                    queue_pending_command(
                                        &mut pending,
                                        command,
                                        &tab_id,
                                        active_worker.generation,
                                        &events,
                                    );
                                } else {
                                    reject_unavailable_command(
                                        &tab_id,
                                        active_worker.generation,
                                        command,
                                        &events,
                                    );
                                }
                            }
                            worker = Some(active_worker);
                        }
                    }
                }
            }
        } else if let Some(mut timer) = retry_timer.take() {
            let mut keep_waiting = false;
            tokio::select! {
                _ = &mut timer => {}
                command = commands.recv() => {
                    match command {
                        None | Some(SftpCommand::Close) => break,
                        Some(SftpCommand::ReconnectNow) => {}
                        Some(command) if command.is_replayable() => {
                            queue_pending_command(
                                &mut pending,
                                command,
                                &tab_id,
                                connection.generation(),
                                &events,
                            );
                            keep_waiting = true;
                        }
                        Some(command) => {
                            reject_unavailable_command(
                                &tab_id,
                                connection.generation(),
                                command,
                                &events,
                            );
                            keep_waiting = true;
                        }
                    }
                }
            }
            if keep_waiting {
                retry_timer = Some(timer);
            }
        } else if connection.is_blocked() {
            match commands.recv().await {
                None | Some(SftpCommand::Close) => break,
                Some(SftpCommand::ReconnectNow) => {
                    let _ = connection.manual_reconnect();
                }
                Some(command) if command.is_replayable() => {
                    queue_pending_command(
                        &mut pending,
                        command,
                        &tab_id,
                        connection.generation(),
                        &events,
                    );
                }
                Some(command) => {
                    reject_unavailable_command(&tab_id, connection.generation(), command, &events);
                }
            }
        } else {
            tokio::task::yield_now().await;
        }
    }
    connection.close();
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "the SFTP worker owns the connection identity, command channels, cancellation token, and generation"
)]
async fn run_sftp(
    tab_id: String,
    session: Session,
    proxy_config: ConnectionProxyConfig,
    mut commands: UnboundedReceiver<SftpCommand>,
    commands_tx: UnboundedSender<SftpCommand>,
    events: std::sync::mpsc::Sender<BackendEvent>,
    connected: Arc<AtomicBool>,
    ready: Arc<tokio::sync::Notify>,
    generation: SftpGeneration,
) -> Result<()> {
    let _ = events.send(BackendEvent::SftpConnectionStatus {
        tab_id: tab_id.clone(),
        generation: generation.0,
        state: SftpConnectionState::Connecting,
        text: t!("sftp_connecting").to_string(),
    });

    let setup = async {
        let handle = connect_and_authenticate(&session, &proxy_config).await?;
        let channel = handle
            .channel_open_session()
            .await
            .context("open sftp channel")?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .context("request sftp subsystem")?;
        let sftp = open_sftp_session(channel.into_stream())
            .await
            .context("sftp handshake")?;
        Ok::<_, anyhow::Error>((handle, sftp))
    };
    let SftpSetupOutcome::Ready {
        value: (handle, sftp),
        mut pending,
    } = await_sftp_setup_or_close(setup, &mut commands, SFTP_SETUP_TIMEOUT).await?
    else {
        return Ok(());
    };
    connected.store(true, Ordering::SeqCst);
    ready.notify_one();

    let home = sftp
        .canonicalize(".")
        .await
        .unwrap_or_else(|_| "/".to_string());

    let _ = events.send(BackendEvent::SftpHome {
        tab_id: tab_id.clone(),
        generation: generation.0,
        home: home.clone(),
    });

    let _ = events.send(BackendEvent::SftpConnectionStatus {
        tab_id: tab_id.clone(),
        generation: generation.0,
        state: SftpConnectionState::Connected,
        text: t!("sftp_connected").to_string(),
    });

    if let Err(error) = emit_entries(&events, &tab_id, generation, None, &sftp, &home).await {
        let _ = events.send(BackendEvent::SftpListDirFailed {
            tab_id: tab_id.clone(),
            generation: generation.0,
            request_id: None,
            path: home.clone(),
            reason: format!("list failed: {error:#}"),
        });
    }

    let mut active_transfers: HashMap<String, TransferStateFlag> = HashMap::new();
    let mut active_deletes: HashMap<String, Vec<String>> = HashMap::new();
    let mut child_tasks = tokio::task::JoinSet::new();
    let mut health_check = tokio::time::interval(std::time::Duration::from_secs(2));
    health_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    health_check.tick().await;

    let worker_result = loop {
        let command = tokio::select! {
            command = async {
                if let Some(command) = pending.pop_front() {
                    Some(command)
                } else {
                    commands.recv().await
                }
            } => command,
            health = async {
                health_check.tick().await;
                if handle.is_closed() {
                    return Err(anyhow!("SFTP SSH transport closed"));
                }
                sftp.canonicalize(".")
                    .await
                    .context("SFTP subsystem health check")?;
                Ok::<(), anyhow::Error>(())
            } => {
                if let Err(error) = health {
                    break Err(error);
                }
                continue;
            }
            joined = child_tasks.join_next(), if !child_tasks.is_empty() => {
                if let Some(Err(error)) = joined {
                    tracing::warn!("SFTP child task failed: {error}");
                }
                continue;
            }
        };
        let Some(command) = command else {
            break Ok(());
        };
        if handle.is_closed() {
            reject_unavailable_command(&tab_id, generation, command, &events);
            break Err(anyhow!("SFTP SSH transport closed"));
        }
        match command {
            SftpCommand::Close => break Ok(()),
            SftpCommand::ReconnectNow => {}
            SftpCommand::PauseTransfer(id) => {
                if let Some(flag) = active_transfers.get(&id) {
                    flag.pause();
                }
            }
            SftpCommand::ResumeTransfer(id) => {
                if let Some(flag) = active_transfers.get(&id) {
                    flag.resume();
                }
            }
            SftpCommand::CancelTransfer(id) => {
                if let Some(flag) = active_transfers.remove(&id) {
                    flag.cancel();
                }
            }
            SftpCommand::TransferFinished(id) => {
                active_transfers.remove(&id);
            }
            SftpCommand::ListDir {
                path,
                request_id,
                expected_generation,
            } => {
                if !list_dir_generation_matches(expected_generation, generation) {
                    continue;
                }
                let actual_path = if path == "~" {
                    home.clone()
                } else if let Some(rest) = path.strip_prefix("~/") {
                    crate::sftp::join_remote(&home, rest)
                } else {
                    path
                };

                if let Err(err) = emit_entries(
                    &events,
                    &tab_id,
                    generation,
                    request_id,
                    &sftp,
                    &actual_path,
                )
                .await
                {
                    let _ = events.send(BackendEvent::SftpListDirFailed {
                        tab_id: tab_id.clone(),
                        generation: generation.0,
                        request_id,
                        path: actual_path,
                        reason: format!("list failed: {err:#}"),
                    });
                }
            }
            SftpCommand::Preview(path) => match preview_impl(&sftp, &path).await {
                Ok(preview) => {
                    let _ = events.send(BackendEvent::SftpPreview {
                        tab_id: tab_id.clone(),
                        generation: generation.0,
                        preview,
                    });
                }
                Err(err) => {
                    let _ = events.send(BackendEvent::SftpStatus {
                        tab_id: tab_id.clone(),
                        generation: generation.0,
                        text: t!("preview_failed", err = format!("{err:#}")).into(),
                    });
                }
            },
            SftpCommand::DocumentStat { path, reply } => {
                let handle = handle.clone();
                child_tasks.spawn(async move {
                    let result = async {
                        let sftp = open_sftp_subsystem(&handle).await?;
                        document_stat_impl(&sftp, &path).await
                    }
                    .await;
                    let _ = reply.send(result);
                });
            }
            SftpCommand::DocumentRead { path, range, reply } => {
                let handle = handle.clone();
                child_tasks.spawn(async move {
                    let result = async {
                        let sftp = open_sftp_subsystem(&handle).await?;
                        document_read_impl(&sftp, &path, range).await
                    }
                    .await;
                    let _ = reply.send(result);
                });
            }
            SftpCommand::DocumentWriteAtomic {
                path,
                bytes,
                permissions,
                operation_id,
                reply,
            } => {
                let handle = handle.clone();
                child_tasks.spawn(async move {
                    let result = async {
                        let sftp = open_sftp_subsystem(&handle).await?;
                        document_write_atomic_impl(&sftp, &path, &bytes, permissions, &operation_id)
                            .await
                    }
                    .await;
                    let _ = reply.send(result);
                });
            }
            SftpCommand::Download { remote, local_dir } => {
                let id = uuid::Uuid::new_v4().to_string();
                let flag = TransferStateFlag::new();
                active_transfers.insert(id.clone(), TransferStateFlag(flag.0.clone()));

                let info = crate::terminal::TransferInfo {
                    id: id.clone(),
                    name: base_name(&remote).to_string(),
                    source: remote.clone(),
                    target: local_dir.clone(),
                    kind: crate::terminal::TransferType::Download,
                    total_bytes: None,
                };
                let _ = events.send(BackendEvent::TransferStarted {
                    tab_id: tab_id.clone(),
                    generation: generation.0,
                    info,
                });

                let handle_clone = handle.clone();
                let events_clone = events.clone();
                let tab_id_clone = tab_id.clone();
                let commands_tx_clone = commands_tx.clone();

                child_tasks.spawn(async move {
                    let result = async {
                        let sftp_session = open_sftp_subsystem(&handle_clone).await?;
                        let _ = events_clone.send(BackendEvent::SftpStatus {
                            tab_id: tab_id_clone.clone(),
                            generation: generation.0,
                            text: t!("downloading_file", base = base_name(&remote)).to_string(),
                        });
                        download_path_impl(
                            &handle_clone,
                            &sftp_session,
                            &remote,
                            Path::new(&local_dir),
                            TransferContext::new(
                                &flag,
                                &events_clone,
                                &tab_id_clone,
                                &id,
                                generation.0,
                            ),
                        )
                        .await
                    }
                    .await;

                    match result {
                        Ok(summary) => {
                            let _ = events_clone.send(BackendEvent::SftpStatus {
                                tab_id: tab_id_clone.clone(),
                                generation: generation.0,
                                text: summary,
                            });
                        }
                        Err(err) => {
                            let err_msg = format!("{err:#}");
                            let is_cancelled = err_msg.contains("transfer cancelled");
                            let state = if is_cancelled {
                                crate::terminal::TransferState::Interrupted(
                                    "User cancelled".to_string(),
                                )
                            } else {
                                crate::terminal::TransferState::Failed(err_msg.clone())
                            };
                            let _ = events_clone.send(BackendEvent::SftpStatus {
                                tab_id: tab_id_clone.clone(),
                                generation: generation.0,
                                text: if is_cancelled {
                                    "Transmission cancelled".to_string()
                                } else {
                                    t!("download_failed", err = err_msg.clone()).to_string()
                                },
                            });
                            let _ = events_clone.send(BackendEvent::TransferProgress {
                                tab_id: tab_id_clone.clone(),
                                generation: generation.0,
                                id: id.clone(),
                                transferred: 0,
                                total: None,
                                state,
                            });
                        }
                    }
                    let _ = commands_tx_clone.send(SftpCommand::TransferFinished(id));
                });
            }
            SftpCommand::UploadPaths { locals, remote_dir } => {
                let id = uuid::Uuid::new_v4().to_string();
                let flag = TransferStateFlag::new();
                active_transfers.insert(id.clone(), TransferStateFlag(flag.0.clone()));

                let name = if locals.len() == 1 {
                    Path::new(&locals[0])
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| locals[0].clone())
                } else {
                    let mut file_count = 0;
                    let mut folder_count = 0;
                    for local in &locals {
                        if std::path::Path::new(local).is_dir() {
                            folder_count += 1;
                        } else {
                            file_count += 1;
                        }
                    }
                    if file_count > 0 && folder_count == 0 {
                        t!("n_files", files = file_count).to_string()
                    } else if file_count == 0 && folder_count > 0 {
                        t!("n_folders", folders = folder_count).to_string()
                    } else {
                        t!(
                            "n_files_and_folders",
                            files = file_count,
                            folders = folder_count
                        )
                        .to_string()
                    }
                };

                let info = crate::terminal::TransferInfo {
                    id: id.clone(),
                    name,
                    source: "local".to_string(),
                    target: remote_dir.clone(),
                    kind: crate::terminal::TransferType::Upload,
                    total_bytes: None,
                };
                let _ = events.send(BackendEvent::TransferStarted {
                    tab_id: tab_id.clone(),
                    generation: generation.0,
                    info,
                });

                let handle_clone = handle.clone();
                let events_clone = events.clone();
                let tab_id_clone = tab_id.clone();
                let commands_tx_clone = commands_tx.clone();

                child_tasks.spawn(async move {
                    let result = async {
                        let sftp_session = open_sftp_subsystem(&handle_clone).await?;
                        let _ = events_clone.send(BackendEvent::SftpStatus {
                            tab_id: tab_id_clone.clone(),
                            generation: generation.0,
                            text: t!("uploading").to_string(),
                        });
                        let transfer = TransferContext::new(
                            &flag,
                            &events_clone,
                            &tab_id_clone,
                            &id,
                            generation.0,
                        );
                        upload_paths_impl(&sftp_session, &locals, &remote_dir, transfer).await
                    }
                    .await;

                    match result {
                        Ok(summary) => {
                            let _ = events_clone.send(BackendEvent::SftpStatus {
                                tab_id: tab_id_clone.clone(),
                                generation: generation.0,
                                text: summary,
                            });
                            let _ = commands_tx_clone.send(SftpCommand::ListDir {
                                path: remote_dir,
                                request_id: None,
                                expected_generation: None,
                            });
                        }
                        Err(err) => {
                            let err_msg = format!("{err:#}");
                            let is_cancelled = err_msg.contains("transfer cancelled");
                            let state = if is_cancelled {
                                crate::terminal::TransferState::Interrupted(
                                    "User cancelled".to_string(),
                                )
                            } else {
                                crate::terminal::TransferState::Failed(err_msg.clone())
                            };
                            let _ = events_clone.send(BackendEvent::SftpStatus {
                                tab_id: tab_id_clone.clone(),
                                generation: generation.0,
                                text: if is_cancelled {
                                    "Transmission cancelled".to_string()
                                } else {
                                    t!("upload_failed", err = err_msg.clone()).to_string()
                                },
                            });
                            let _ = events_clone.send(BackendEvent::TransferProgress {
                                tab_id: tab_id_clone,
                                generation: generation.0,
                                id: id.clone(),
                                transferred: 0,
                                total: None,
                                state,
                            });
                        }
                    }
                    let _ = commands_tx_clone.send(SftpCommand::TransferFinished(id));
                });
            }
            SftpCommand::CreateDir(path) => {
                let actual_path = if path == "~" {
                    home.clone()
                } else if let Some(rest) = path.strip_prefix("~/") {
                    crate::sftp::join_remote(&home, rest)
                } else {
                    path.clone()
                };

                tracing::info!("[sftp] creating directory: '{}'", actual_path);

                match sftp.create_dir(&actual_path).await {
                    Ok(_) => {
                        let _ = events.send(BackendEvent::SftpStatus {
                            tab_id: tab_id.clone(),
                            generation: generation.0,
                            text: t!("create_folder_success", name = base_name(&actual_path))
                                .to_string(),
                        });

                        // Re-fetch the parent directory to show the newly created folder
                        if let Some(parent) = parent_dir(&actual_path) {
                            let _ = commands_tx.send(SftpCommand::ListDir {
                                path: parent,
                                request_id: None,
                                expected_generation: None,
                            });
                        } else {
                            let _ = commands_tx.send(SftpCommand::ListDir {
                                path: "/".to_string(),
                                request_id: None,
                                expected_generation: None,
                            });
                        }
                    }
                    Err(err) => {
                        let _ = events.send(BackendEvent::SftpStatus {
                            tab_id: tab_id.clone(),
                            generation: generation.0,
                            text: t!("create_folder_failed", err = format!("{err:#}")).to_string(),
                        });
                    }
                }
            }
            SftpCommand::DeletePaths(paths) => {
                tracing::info!("[sftp] batch deleting {} paths", paths.len());
                let _ = events.send(BackendEvent::SftpStatus {
                    tab_id: tab_id.clone(),
                    generation: generation.0,
                    text: t!("deleting_paths", count = paths.len()).to_string(),
                });

                let id = Uuid::new_v4().to_string();
                active_deletes.insert(id.clone(), paths.clone());
                let handle_clone = handle.clone();
                let commands_tx_clone = commands_tx.clone();
                let home_clone = home.clone();
                child_tasks.spawn(async move {
                    let mut errors = Vec::new();
                    let mut deleted_paths = Vec::new();
                    match open_sftp_subsystem(&handle_clone).await {
                        Ok(delete_sftp) => {
                            for path in paths {
                                let actual_path = if path == "~" {
                                    home_clone.clone()
                                } else if let Some(rest) = path.strip_prefix("~/") {
                                    crate::sftp::join_remote(&home_clone, rest)
                                } else {
                                    path.clone()
                                };
                                match recursive_delete(&delete_sftp, actual_path).await {
                                    Ok(()) => deleted_paths.push(path),
                                    Err(error) => errors.push(format!("{path}: {error:#}")),
                                }
                            }
                        }
                        Err(error) => errors.push(format!("open delete session: {error:#}")),
                    }
                    let _ = commands_tx_clone.send(SftpCommand::DeleteFinished {
                        id,
                        deleted_paths,
                        errors,
                    });
                });
            }
            SftpCommand::DeleteFinished {
                id,
                deleted_paths,
                errors,
            } => {
                let Some(paths) = active_deletes.remove(&id) else {
                    continue;
                };
                if errors.is_empty() {
                    let _ = events.send(BackendEvent::SftpStatus {
                        tab_id: tab_id.clone(),
                        generation: generation.0,
                        text: t!("delete_success", count = paths.len()).to_string(),
                    });
                } else {
                    let _ = events.send(BackendEvent::SftpStatus {
                        tab_id: tab_id.clone(),
                        generation: generation.0,
                        text: t!("delete_failed", err = errors.join(", ")).to_string(),
                    });
                }

                let _ = events.send(BackendEvent::SftpDeleteFinished {
                    tab_id: tab_id.clone(),
                    generation: generation.0,
                    paths: paths.clone(),
                    deleted_paths,
                });

                if let Some(first) = paths.first() {
                    let actual_path = if first == "~" {
                        home.clone()
                    } else if let Some(rest) = first.strip_prefix("~/") {
                        crate::sftp::join_remote(&home, rest)
                    } else {
                        first.clone()
                    };
                    if let Some(parent) = parent_dir(&actual_path) {
                        let _ = commands_tx.send(SftpCommand::ListDir {
                            path: parent,
                            request_id: None,
                            expected_generation: None,
                        });
                    } else {
                        let _ = commands_tx.send(SftpCommand::ListDir {
                            path: "/".to_string(),
                            request_id: None,
                            expected_generation: None,
                        });
                    }
                }
            }
        }
    };

    for paths in take_interrupted_deletes(&mut active_deletes) {
        let _ = events.send(BackendEvent::SftpDeleteFinished {
            tab_id: tab_id.clone(),
            generation: generation.0,
            paths,
            deleted_paths: Vec::new(),
        });
    }
    child_tasks.abort_all();
    while child_tasks.join_next().await.is_some() {}
    let _ = handle
        .disconnect(Disconnect::ByApplication, "bye", "")
        .await;
    worker_result
}

fn take_interrupted_deletes(active: &mut HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    active.drain().map(|(_, paths)| paths).collect()
}

use std::pin::Pin;

fn recursive_delete<'a>(
    sftp: &'a SftpSession,
    path: String,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let metadata = sftp
            .symlink_metadata(&path)
            .await
            .with_context(|| format!("Failed to inspect {path}"))?;
        if metadata.is_symlink() || !metadata.is_dir() {
            sftp.remove_file(&path)
                .await
                .with_context(|| format!("Failed to delete file {path}"))?;
            return Ok(());
        }

        let entries = sftp
            .read_dir(&path)
            .await
            .with_context(|| format!("Failed to read directory {path}"))?;
        for entry in entries {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            recursive_delete(sftp, crate::sftp::join_remote(&path, &name)).await?;
        }
        sftp.remove_dir(&path)
            .await
            .with_context(|| format!("Failed to delete directory {path}"))?;
        Ok(())
    })
}

async fn emit_entries(
    events: &std::sync::mpsc::Sender<BackendEvent>,
    tab_id: &str,
    generation: SftpGeneration,
    request_id: Option<u64>,
    sftp: &SftpSession,
    path: &str,
) -> Result<()> {
    let entries = list_dir_impl(sftp, path).await?;
    let _ = events.send(BackendEvent::SftpEntries {
        tab_id: tab_id.to_string(),
        generation: generation.0,
        request_id,
        path: path.to_string(),
        entries,
    });
    Ok(())
}

async fn connect_and_authenticate(
    session: &Session,
    proxy_config: &ConnectionProxyConfig,
) -> Result<Arc<russh::client::Handle<SftpClientHandler>>> {
    let config = Arc::new(client::Config {
        inactivity_timeout: None,
        keepalive_interval: Some(std::time::Duration::from_secs(5)),
        keepalive_max: 3,
        ..Default::default()
    });
    let addr = format!("{}:{}", session.host, session.port);
    let stream = crate::session::config::connect_proxy(session, proxy_config).await?;
    let handler = SftpClientHandler::new(&session.host, session.port)?;
    let mut handle = client::connect_stream(config, stream, handler)
        .await
        .with_context(|| format!("connect {addr} failed"))?;

    let authed = match session.auth {
        AuthMethod::Password => handle
            .authenticate_password(&session.user, &session.password)
            .await
            .context("password authentication failed")?
            .success(),
        AuthMethod::Key => {
            let has_explicit_key = session_has_explicit_key(session);

            if has_explicit_key {
                let keypair = load_session_private_key(session)?;
                let keys = private_keys_with_algs(keypair);
                let mut success = false;
                for key in keys {
                    match handle.authenticate_publickey(&session.user, key).await {
                        Ok(result) if result.success() => {
                            success = true;
                            break;
                        }
                        Ok(_) => {
                            tracing::debug!(
                                "[sftp] public key auth failed with algorithm, trying next"
                            );
                            continue;
                        }
                        Err(e) => {
                            tracing::debug!("[sftp] public key auth error: {:?}, trying next", e);
                            continue;
                        }
                    }
                }
                if !success {
                    return Err(anyhow!(
                        "public key authentication failed for {}@{}:{}",
                        session.user,
                        session.host,
                        session.port
                    ));
                }
                success
            } else {
                let passphrase = session.passphrase.trim();
                let passphrase = (!passphrase.is_empty()).then_some(passphrase);
                let success =
                    authenticate_with_default_keys(&mut handle, &session.user, passphrase).await?;
                if !success {
                    return Err(anyhow!(
                        "public key authentication failed for {}@{}:{} - no valid default key found in ~/.ssh/",
                        session.user,
                        session.host,
                        session.port
                    ));
                }
                success
            }
        }
        AuthMethod::Config => {
            // For Config auth, try the identity file from config entry, or default keys
            // Note: for Config auth, we never use inline key content
            let has_explicit_key = !session.private_key_path.trim().is_empty();

            if has_explicit_key {
                let keypair = load_session_private_key(session)?;
                let keys = private_keys_with_algs(keypair);
                let mut success = false;
                for key in keys {
                    match handle.authenticate_publickey(&session.user, key).await {
                        Ok(result) if result.success() => {
                            success = true;
                            break;
                        }
                        Ok(_) => {
                            tracing::debug!(
                                "[sftp] public key auth failed with algorithm, trying next"
                            );
                            continue;
                        }
                        Err(e) => {
                            tracing::debug!("[sftp] public key auth error: {:?}, trying next", e);
                            continue;
                        }
                    }
                }
                if !success {
                    return Err(anyhow!(
                        "ssh-config key authentication failed for {}@{}:{}",
                        session.user,
                        session.host,
                        session.port
                    ));
                }
                success
            } else {
                let passphrase = session.passphrase.trim();
                let passphrase = (!passphrase.is_empty()).then_some(passphrase);
                let success =
                    authenticate_with_default_keys(&mut handle, &session.user, passphrase).await?;
                if !success {
                    return Err(anyhow!(
                        "ssh-config authentication failed for {}@{}:{} - no valid default key found",
                        session.user,
                        session.host,
                        session.port
                    ));
                }
                success
            }
        }
    };

    if !authed {
        let _ = handle
            .disconnect(Disconnect::ByApplication, "auth failed", "")
            .await;
        return Err(anyhow!(
            "authentication failed: server rejected {} authentication for {}@{}:{}",
            match session.auth {
                AuthMethod::Password => "password",
                AuthMethod::Key => "public key",
                AuthMethod::Config => "ssh-config",
            },
            session.user,
            session.host,
            session.port
        ));
    }

    Ok(Arc::new(handle))
}

fn load_session_private_key(session: &Session) -> Result<PrivateKey> {
    let inline_key = normalize_inline_private_key(&session.private_key_inline);
    let key_path = expand_key_path(session.private_key_path.trim());
    let passphrase = session.passphrase.trim();
    let passphrase = (!passphrase.is_empty()).then_some(passphrase);
    let has_inline = !inline_key.is_empty();
    let has_path = key_path.is_some();

    if !has_inline && !has_path {
        return Err(anyhow!("private key content or path is required"));
    }

    let mut errors = Vec::new();

    if has_inline {
        match decode_secret_key(&inline_key, passphrase) {
            Ok(key) => return Ok(key),
            Err(err) => errors.push(format!("decode private key content: {err}")),
        }
    }

    if let Some(path) = key_path {
        match load_secret_key(path.as_path(), passphrase) {
            Ok(key) => return Ok(key),
            Err(err) => errors.push(format!("load key {}: {err}", path.display())),
        }
    }

    Err(anyhow!(errors.join("; ")))
}

fn expand_key_path(value: &str) -> Option<PathBuf> {
    if value.is_empty() {
        return None;
    }
    if value == "~" {
        return BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return BaseDirs::new().map(|dirs| dirs.home_dir().join(rest));
    }
    Some(Path::new(value).to_path_buf())
}

fn base_name(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string()
}

pub(crate) fn parent_dir(path: &str) -> Option<String> {
    if path == "/" || path.is_empty() {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    if let Some(idx) = trimmed.rfind('/') {
        if idx == 0 {
            Some("/".to_string())
        } else {
            Some(trimmed[..idx].to_string())
        }
    } else {
        Some("/".to_string())
    }
}

pub(crate) fn join_remote(parent: &str, child: &str) -> String {
    if parent == "/" {
        format!("/{child}")
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), child)
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

pub fn format_mtime(ts: u32) -> String {
    let dt: DateTime<Utc> = Utc
        .timestamp_opt(ts as i64, 0)
        .single()
        .unwrap_or_else(Utc::now);
    dt.format("%Y-%m-%d %H:%M").to_string()
}

async fn list_dir_impl(sftp: &SftpSession, path: &str) -> Result<Vec<RemoteEntry>> {
    let raw = sftp
        .read_dir(path)
        .await
        .with_context(|| format!("read_dir {path} failed"))?;

    let mut entries = raw
        .into_iter()
        .filter(|entry| {
            let name = entry.file_name();
            name != "." && name != ".."
        })
        .map(|entry| {
            let name = entry.file_name().to_string();
            let full_path = join_remote(path, &name);
            let meta = entry.metadata();
            let permissions = meta.permissions;
            let file_type = file_type_from_mode(permissions);
            let is_dir = file_type == RemoteFileType::Directory;
            let size = meta.size.unwrap_or(0);
            let modified = meta.mtime.unwrap_or(0);
            RemoteEntry {
                name,
                full_path,
                is_dir,
                file_type,
                permissions,
                size,
                modified,
            }
        })
        .collect::<Vec<_>>();

    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(entries)
}

async fn preview_impl(sftp: &SftpSession, path: &str) -> Result<PreviewData> {
    let metadata = sftp
        .metadata(path)
        .await
        .with_context(|| format!("metadata {path}"))?;
    let is_dir = metadata
        .permissions
        .map(|mode| (mode & 0o170_000) == 0o040_000)
        .unwrap_or(false);

    if is_dir {
        let entries = list_dir_impl(sftp, path).await?;
        let mut lines = vec![format!("Directory: {path}"), String::new()];
        for entry in entries.into_iter().take(200) {
            let kind = if entry.is_dir { "dir " } else { "file" };
            lines.push(format!("{kind}  {}", entry.name));
        }
        return Ok(PreviewData {
            path: path.to_string(),
            title: base_name(path),
            body: lines.join("\n"),
        });
    }

    let mut remote_file = sftp
        .open(path)
        .await
        .with_context(|| format!("open remote {path}"))?;
    let mut buffer = vec![0u8; 128 * 1024];
    let read = remote_file
        .read(&mut buffer)
        .await
        .context("read preview bytes")?;
    buffer.truncate(read);

    let nul_ratio = if buffer.is_empty() {
        0.0
    } else {
        buffer.iter().filter(|byte| **byte == 0).count() as f32 / buffer.len() as f32
    };
    let is_binary = nul_ratio > 0.01;
    let body = if is_binary {
        format!(
            "Binary file\npath: {path}\nsize: {}\npreview: unavailable in-app",
            format_bytes(metadata.size.unwrap_or(0)),
        )
    } else {
        String::from_utf8_lossy(&buffer).into_owned()
    };

    Ok(PreviewData {
        path: path.to_string(),
        title: base_name(path),
        body,
    })
}

async fn download_path_impl(
    handle: &russh::client::Handle<SftpClientHandler>,
    sftp: &SftpSession,
    remote: &str,
    local_dir: &Path,
    transfer: TransferContext<'_>,
) -> Result<String> {
    tokio::fs::create_dir_all(local_dir)
        .await
        .with_context(|| format!("create {}", local_dir.display()))?;

    // Check for cancellation after initial setup
    let state = transfer.flag.0.load(Ordering::SeqCst);
    if state == 2 {
        return Err(anyhow::anyhow!("transfer cancelled"));
    }

    let metadata = sftp
        .metadata(remote)
        .await
        .with_context(|| format!("metadata {remote}"))?;
    let is_dir = metadata
        .permissions
        .map(|mode| (mode & 0o170_000) == 0o040_000)
        .unwrap_or(false);

    if is_dir {
        let local_archive = local_dir.join(format!(
            ".ashell-{}-{}.tar.gz",
            base_name(remote),
            Uuid::new_v4()
        ));
        let extracted_to =
            download_remote_directory_archive(handle, sftp, remote, &local_archive, transfer)
                .await?;
        return Ok(t!("downloaded_folder", path = extracted_to.display()).to_string());
    }

    let local_path = local_dir.join(base_name(remote));
    download_file_impl(sftp, remote, &local_path, transfer).await?;
    Ok(t!("downloaded_file", path = local_path.display()).to_string())
}

async fn download_remote_directory_archive(
    handle: &russh::client::Handle<SftpClientHandler>,
    sftp: &SftpSession,
    remote_dir: &str,
    local_archive: &Path,
    transfer: TransferContext<'_>,
) -> Result<PathBuf> {
    let remote_archive = format!(
        "/tmp/ashell-{}-{}.tar.gz",
        base_name(remote_dir),
        Uuid::new_v4()
    );

    // Check for cancellation before creating remote archive
    let state = transfer.flag.0.load(Ordering::SeqCst);
    if state == 2 {
        return Err(anyhow::anyhow!("transfer cancelled"));
    }

    create_remote_archive(handle, remote_dir, &remote_archive).await?;

    let local_extract_root = local_archive
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(base_name(remote_dir));

    let archive_download = async {
        download_file_impl(sftp, &remote_archive, local_archive, transfer).await?;
        extract_archive_to(
            local_archive,
            local_archive.parent().unwrap_or_else(|| Path::new(".")),
        )
        .await?;
        tokio::fs::remove_file(local_archive)
            .await
            .with_context(|| format!("remove {}", local_archive.display()))?;
        Ok::<PathBuf, anyhow::Error>(local_extract_root)
    }
    .await;

    let cleanup_result = remove_remote_path(handle, &remote_archive).await;

    let extracted_to = archive_download?;
    if let Err(err) = cleanup_result {
        tracing::warn!("failed to clean remote archive {remote_archive}: {err:#}");
    }

    Ok(extracted_to)
}

async fn download_file_impl(
    sftp: &SftpSession,
    remote: &str,
    local: &Path,
    transfer: TransferContext<'_>,
) -> Result<()> {
    let mut remote_file = sftp
        .open(remote)
        .await
        .with_context(|| format!("open remote {remote}"))?;
    let local_parent = local.parent().unwrap_or_else(|| Path::new("."));
    let temporary = tempfile::Builder::new()
        .prefix(".jshell-download-")
        .tempfile_in(local_parent)
        .with_context(|| format!("create temporary download in {}", local_parent.display()))?;
    let (temporary_file, temporary_path) = temporary.into_parts();
    let mut local_file = tokio::fs::File::from_std(temporary_file);

    let total = sftp.metadata(remote).await.ok().and_then(|m| m.size);
    let mut transferred = 0u64;

    let mut buffer = vec![0u8; 128 * 1024];
    loop {
        transfer
            .flag
            .yield_if_paused(
                transfer.events,
                transfer.tab_id,
                transfer.id,
                transfer.generation,
                transferred,
                total,
            )
            .await?;
        let read = remote_file
            .read(&mut buffer)
            .await
            .context("read remote file")?;
        if read == 0 {
            break;
        }
        local_file
            .write_all(&buffer[..read])
            .await
            .with_context(|| format!("write {}", local.display()))?;

        transferred += read as u64;
        let _ = transfer.events.send(BackendEvent::TransferProgress {
            tab_id: transfer.tab_id.to_string(),
            generation: transfer.generation,
            id: transfer.id.to_string(),
            transferred,
            total,
            state: crate::terminal::TransferState::Running,
        });
    }
    local_file.flush().await.context("flush local file")?;
    local_file.sync_all().await.context("sync local file")?;
    transfer
        .flag
        .yield_if_paused(
            transfer.events,
            transfer.tab_id,
            transfer.id,
            transfer.generation,
            transferred,
            total,
        )
        .await?;
    drop(local_file);
    persist_local_download(temporary_path, local)?;

    let _ = transfer.events.send(BackendEvent::TransferProgress {
        tab_id: transfer.tab_id.to_string(),
        generation: transfer.generation,
        id: transfer.id.to_string(),
        transferred,
        total,
        state: crate::terminal::TransferState::Completed,
    });

    Ok(())
}

fn persist_local_download(temporary: tempfile::TempPath, destination: &Path) -> Result<()> {
    temporary
        .persist(destination)
        .map_err(|error| error.error)
        .with_context(|| format!("replace local {}", destination.display()))
}

async fn upload_paths_impl(
    sftp: &SftpSession,
    locals: &[String],
    remote_dir: &str,
    transfer: TransferContext<'_>,
) -> Result<String> {
    // Check for cancellation before starting
    let state = transfer.flag.0.load(Ordering::SeqCst);
    if state == 2 {
        return Err(anyhow::anyhow!("transfer cancelled"));
    }

    create_remote_dir_all(sftp, remote_dir).await?;
    let mut file_count = 0usize;
    let mut folder_count = 0usize;

    let mut total_bytes = 0u64;
    let mut files_to_upload = Vec::new();
    let mut dirs_to_create = Vec::new();

    for local in locals {
        let p = PathBuf::from(local);
        let root_metadata = tokio::fs::metadata(&p)
            .await
            .with_context(|| format!("read local metadata for {}", p.display()))?;
        if root_metadata.is_dir() {
            folder_count += 1;
            let root_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("folder");
            let remote_root = join_remote(remote_dir, root_name);
            dirs_to_create.push(remote_root.clone());

            for entry in WalkDir::new(&p) {
                let entry = entry?;
                let path = entry.path();
                if path == p {
                    continue;
                }

                let meta = tokio::fs::metadata(path)
                    .await
                    .with_context(|| format!("read local metadata for {}", path.display()))?;
                let relative = path.strip_prefix(&p)?;
                let remote_path = if relative.as_os_str().is_empty() {
                    remote_root.clone()
                } else {
                    let rel = relative
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().to_string())
                        .collect::<Vec<_>>()
                        .join("/");
                    join_remote(&remote_root, &rel)
                };

                if meta.is_dir() {
                    dirs_to_create.push(remote_path);
                } else {
                    total_bytes += meta.len();
                    files_to_upload.push((path.to_path_buf(), remote_path));
                }
            }
        } else {
            total_bytes += root_metadata.len();
            let file_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("file");
            files_to_upload.push((p.clone(), join_remote(remote_dir, file_name)));
            file_count += 1;
        }
    }

    // Check for cancellation before creating directories
    let state = transfer.flag.0.load(Ordering::SeqCst);
    if state == 2 {
        return Err(anyhow::anyhow!("transfer cancelled"));
    }

    // Create directories sequentially first
    for dir in dirs_to_create {
        // Check for cancellation between each directory creation
        let state = transfer.flag.0.load(Ordering::SeqCst);
        if state == 2 {
            return Err(anyhow::anyhow!("transfer cancelled"));
        }
        create_remote_dir_all(sftp, &dir).await?;
    }

    let transferred = Arc::new(AtomicU64::new(0));
    let mut futures = Vec::new();

    for (local_path, remote_path) in files_to_upload {
        let flag_clone = TransferStateFlag(Arc::clone(&transfer.flag.0));
        let events_clone = transfer.events.clone();
        let tab_id_clone = transfer.tab_id.to_string();
        let id_clone = transfer.id.to_string();
        let generation = transfer.generation;
        let transferred_clone = Arc::clone(&transferred);

        futures.push(async move {
            upload_file_impl(
                sftp,
                &local_path,
                &remote_path,
                TransferContext::new(
                    &flag_clone,
                    &events_clone,
                    &tab_id_clone,
                    &id_clone,
                    generation,
                ),
                transferred_clone,
                Some(total_bytes),
            )
            .await
        });
    }

    use futures::StreamExt as _;
    let mut stream = futures::stream::iter(futures).buffer_unordered(4);
    let mut first_error = None;
    while let Some(res) = stream.next().await {
        if let Err(error) = res
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }

    let _ = transfer.events.send(BackendEvent::TransferProgress {
        tab_id: transfer.tab_id.to_string(),
        generation: transfer.generation,
        id: transfer.id.to_string(),
        transferred: total_bytes,
        total: Some(total_bytes),
        state: crate::terminal::TransferState::Completed,
    });

    let summary = if file_count == 1 && folder_count == 0 {
        t!("uploaded_file").to_string()
    } else if file_count == 0 && folder_count == 1 {
        t!("uploaded_folder").to_string()
    } else if file_count > 0 && folder_count == 0 {
        t!("uploaded_n_files", files = file_count).to_string()
    } else if file_count == 0 && folder_count > 0 {
        t!("uploaded_n_folders", folders = folder_count).to_string()
    } else {
        t!(
            "uploaded_files_and_folders",
            files = file_count,
            folders = folder_count
        )
        .to_string()
    };
    Ok(summary)
}

async fn upload_file_impl(
    sftp: &SftpSession,
    local_file: &Path,
    remote_path: &str,
    transfer: TransferContext<'_>,
    transferred: Arc<AtomicU64>,
    total: Option<u64>,
) -> Result<()> {
    let mut local = tokio::fs::File::open(local_file)
        .await
        .with_context(|| format!("open local {}", local_file.display()))?;
    let operation_id = Uuid::new_v4().to_string();
    let temporary = temporary_upload_path(remote_path, &operation_id);
    let backup = backup_upload_path(remote_path, &operation_id);
    let (target_exists, permissions) = existing_remote_permissions(sftp, remote_path).await?;

    let result = async {
        let mut remote = sftp
            .create(&temporary)
            .await
            .with_context(|| format!("create remote temporary file {temporary}"))?;

        let mut buffer = vec![0u8; 128 * 1024];
        loop {
            let cur = transferred.load(Ordering::Relaxed);
            transfer
                .flag
                .yield_if_paused(
                    transfer.events,
                    transfer.tab_id,
                    transfer.id,
                    transfer.generation,
                    cur,
                    total,
                )
                .await?;
            let read = local.read(&mut buffer).await.context("read local file")?;
            if read == 0 {
                break;
            }
            remote
                .write_all(&buffer[..read])
                .await
                .with_context(|| format!("write remote temporary file {temporary}"))?;

            let new_cur = transferred.fetch_add(read as u64, Ordering::Relaxed) + read as u64;
            let _ = transfer.events.send(BackendEvent::TransferProgress {
                tab_id: transfer.tab_id.to_string(),
                generation: transfer.generation,
                id: transfer.id.to_string(),
                transferred: new_cur,
                total,
                state: crate::terminal::TransferState::Running,
            });
        }
        remote
            .flush()
            .await
            .context("flush remote temporary file")?;
        remote
            .sync_all()
            .await
            .context("sync remote temporary file")?;
        drop(remote);

        let current = transferred.load(Ordering::Relaxed);
        transfer
            .flag
            .yield_if_paused(
                transfer.events,
                transfer.tab_id,
                transfer.id,
                transfer.generation,
                current,
                total,
            )
            .await?;
        set_remote_permissions(sftp, &temporary, permissions).await?;
        commit_remote_replacement(sftp, &temporary, remote_path, &backup, target_exists).await
    }
    .await;

    if result.is_err() {
        cleanup_remote_temporary(sftp, &temporary).await;
    }
    result
}

fn temporary_upload_path(remote_path: &str, operation_id: &str) -> String {
    format!("{remote_path}.jshell-upload-{operation_id}.tmp")
}

fn backup_upload_path(remote_path: &str, operation_id: &str) -> String {
    format!("{remote_path}.jshell-upload-{operation_id}.bak")
}

async fn existing_remote_permissions(
    sftp: &SftpSession,
    remote_path: &str,
) -> Result<(bool, Option<u32>)> {
    match sftp.metadata(remote_path).await {
        Ok(metadata) => Ok((true, metadata.permissions)),
        Err(russh_sftp::client::error::Error::Status(status))
            if status.status_code == russh_sftp::protocol::StatusCode::NoSuchFile =>
        {
            Ok((false, None))
        }
        Err(error) => {
            Err(error).with_context(|| format!("read existing remote metadata for {remote_path}"))
        }
    }
}

async fn set_remote_permissions(
    sftp: &SftpSession,
    remote_path: &str,
    permissions: Option<u32>,
) -> Result<()> {
    if let Some(permissions) = permissions {
        let mut attributes = russh_sftp::protocol::FileAttributes::empty();
        attributes.permissions = Some(permissions);
        sftp.set_metadata(remote_path, attributes)
            .await
            .with_context(|| format!("preserve permissions on {remote_path}"))?;
    }
    Ok(())
}

async fn commit_remote_replacement(
    sftp: &SftpSession,
    temporary: &str,
    destination: &str,
    backup: &str,
    destination_exists: bool,
) -> Result<()> {
    match sftp.rename(temporary, destination).await {
        Ok(()) => return Ok(()),
        Err(error) if !destination_exists => return Err(error.into()),
        Err(_) => {}
    }

    sftp.rename(destination, backup)
        .await
        .with_context(|| format!("move existing remote file {destination} to backup {backup}"))?;
    if let Err(replace_error) = sftp.rename(temporary, destination).await {
        return match sftp.rename(backup, destination).await {
            Ok(()) => Err(replace_error.into()),
            Err(restore_error) => Err(anyhow!(
                "replace {destination} failed: {replace_error}; restore from {backup} failed: {restore_error}"
            )),
        };
    }
    if let Err(error) = sftp.remove_file(backup).await {
        tracing::warn!("failed to remove remote upload backup {backup}: {error}");
    }
    Ok(())
}

async fn cleanup_remote_temporary(sftp: &SftpSession, temporary: &str) {
    match sftp.remove_file(temporary).await {
        Ok(()) => {}
        Err(russh_sftp::client::error::Error::Status(status))
            if status.status_code == russh_sftp::protocol::StatusCode::NoSuchFile => {}
        Err(error) => tracing::warn!("failed to remove remote temporary file {temporary}: {error}"),
    }
}

async fn create_remote_dir_all(sftp: &SftpSession, remote_dir: &str) -> Result<()> {
    if remote_dir.is_empty() || remote_dir == "/" {
        return Ok(());
    }

    let mut current = String::from("/");
    for segment in remote_dir.split('/').filter(|segment| !segment.is_empty()) {
        current = join_remote(&current, segment);
        if let Err(create_error) = sftp.create_dir(&current).await {
            match sftp.metadata(&current).await {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => {
                    return Err(anyhow!(
                        "cannot create remote directory {current}: a non-directory already exists ({create_error})"
                    ));
                }
                Err(metadata_error) => {
                    return Err(anyhow!(
                        "create remote directory {current} failed: {create_error}; verify existing path failed: {metadata_error}"
                    ));
                }
            }
        }
    }
    Ok(())
}

async fn create_remote_archive(
    handle: &russh::client::Handle<SftpClientHandler>,
    remote_dir: &str,
    remote_archive: &str,
) -> Result<()> {
    let remote_dir = remote_dir.trim_end_matches('/');
    let parent = remote_parent(remote_dir);
    let name = base_name(remote_dir);
    let command = format!(
        "tar -C {} -czf {} {}",
        shell_quote(&parent),
        shell_quote(remote_archive),
        shell_quote(&name),
    );
    exec_remote_command(handle, &command)
        .await
        .with_context(|| format!("archive remote directory {remote_dir}"))?;
    Ok(())
}

async fn remove_remote_path(
    handle: &russh::client::Handle<SftpClientHandler>,
    remote_path: &str,
) -> Result<()> {
    let command = format!("rm -f {}", shell_quote(remote_path));
    exec_remote_command(handle, &command)
        .await
        .with_context(|| format!("remove remote temporary file {remote_path}"))?;
    Ok(())
}

async fn exec_remote_command(
    handle: &russh::client::Handle<SftpClientHandler>,
    command: &str,
) -> Result<()> {
    let result = tokio::time::timeout(REMOTE_COMMAND_TIMEOUT, async {
        let mut channel = handle
            .channel_open_session()
            .await
            .context("open remote exec session")?;
        channel
            .exec(true, command)
            .await
            .with_context(|| format!("exec remote command: {command}"))?;

        let mut stderr = Vec::new();
        let mut stdout = Vec::new();
        let mut output_bytes = 0usize;
        let mut exit_status = None;

        loop {
            // Yield to allow cancellation
            tokio::task::yield_now().await;

            if let Some(msg) = channel.wait().await {
                match msg {
                    russh::ChannelMsg::Data { data } => append_remote_command_output(
                        &mut stdout,
                        &data,
                        &mut output_bytes,
                        "remote command stdout",
                    )?,
                    russh::ChannelMsg::ExtendedData { data, .. } => append_remote_command_output(
                        &mut stderr,
                        &data,
                        &mut output_bytes,
                        "remote command stderr",
                    )?,
                    russh::ChannelMsg::ExitStatus { exit_status: code } => exit_status = Some(code),
                    russh::ChannelMsg::Close => break,
                    _ => {}
                }
            } else {
                break;
            }
        }
        finish_remote_command(command, exit_status, &stdout, &stderr)
    })
    .await;

    match result {
        Err(_) => Err(anyhow!("remote command timed out: {command}")),
        Ok(result) => result,
    }
}

fn append_remote_command_output(
    output: &mut Vec<u8>,
    data: &[u8],
    total_bytes: &mut usize,
    label: &str,
) -> Result<()> {
    if (*total_bytes).saturating_add(data.len()) > MAX_REMOTE_COMMAND_OUTPUT_BYTES {
        return Err(anyhow!(
            "{label} exceeds the {MAX_REMOTE_COMMAND_OUTPUT_BYTES} byte limit"
        ));
    }
    output.extend_from_slice(data);
    *total_bytes += data.len();
    Ok(())
}

fn finish_remote_command(
    command: &str,
    exit_status: Option<u32>,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<()> {
    let code = exit_status
        .ok_or_else(|| anyhow!("remote command closed without an exit status: {command}"))?;
    match code {
        0 => Ok(()),
        code => {
            let stderr = String::from_utf8_lossy(stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(stdout).trim().to_string();
            Err(anyhow!(
                "remote command exited with {code}: {}",
                if !stderr.is_empty() { stderr } else { stdout }
            ))
        }
    }
}

fn remote_parent(path: &str) -> String {
    if path == "/" {
        "/".to_string()
    } else {
        path.rsplit_once('/')
            .map(|(parent, _)| {
                if parent.is_empty() {
                    "/".to_string()
                } else {
                    parent.to_string()
                }
            })
            .unwrap_or_else(|| "/".to_string())
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Extracts a downloaded directory archive into `target_dir`.
///
/// Path traversal protection relies on the archive crates themselves and must
/// not be weakened by future edits: for zip, `entry.enclosed_name()` rejects
/// escaping names; for tar, `Archive::unpack` skips entries with `..`
/// components and its canonicalization check refuses writes that resolve
/// outside the destination (including through symlinks). See the regression
/// tests in `handle_tests::extract_*`.
async fn extract_archive_to(path: &Path, target_dir: &Path) -> Result<()> {
    let Some(file_name) = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
    else {
        return Ok(());
    };
    let archive_path = path.to_path_buf();
    let target_dir = target_dir.to_path_buf();

    tokio::task::spawn_blocking(move || -> Result<()> {
        fs::create_dir_all(&target_dir)
            .with_context(|| format!("create {}", target_dir.display()))?;

        if file_name.ends_with(".zip") {
            let file = fs::File::open(&archive_path)
                .with_context(|| format!("open {}", archive_path.display()))?;
            let mut zip = ZipArchive::new(file).context("read zip archive")?;
            for index in 0..zip.len() {
                let mut entry = zip.by_index(index).context("read zip entry")?;
                let Some(name) = entry.enclosed_name().map(|name| name.to_path_buf()) else {
                    continue;
                };
                let output = target_dir.join(name);
                if entry.is_dir() {
                    fs::create_dir_all(&output)?;
                } else {
                    if let Some(parent) = output.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let mut output_file = fs::File::create(&output)?;
                    std::io::copy(&mut entry, &mut output_file)?;
                }
            }
        } else if file_name.ends_with(".tar.gz") || file_name.ends_with(".tgz") {
            let file = fs::File::open(&archive_path)
                .with_context(|| format!("open {}", archive_path.display()))?;
            let decoder = GzDecoder::new(file);
            let mut archive = tar::Archive::new(decoder);
            archive
                .unpack(&target_dir)
                .context("unpack tar.gz archive")?;
        } else if file_name.ends_with(".tar") {
            let file = fs::File::open(&archive_path)
                .with_context(|| format!("open {}", archive_path.display()))?;
            let mut archive = tar::Archive::new(file);
            archive.unpack(&target_dir).context("unpack tar archive")?;
        }

        Ok(())
    })
    .await
    .context("extract archive task join failure")??;

    Ok(())
}

#[derive(Clone)]
struct SftpClientHandler {
    host_keys: HostKeyVerifier,
}

impl SftpClientHandler {
    fn new(host: &str, port: u16) -> Result<Self> {
        Ok(Self {
            host_keys: HostKeyVerifier::new(host, port)?,
        })
    }

    #[cfg(test)]
    fn with_known_hosts_path(host: &str, port: u16, path: PathBuf) -> Self {
        Self {
            host_keys: HostKeyVerifier::with_known_hosts_path(host, port, path),
        }
    }
}

impl Handler for SftpClientHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        self.host_keys.verify(server_public_key)?;
        Ok(true)
    }
}

async fn open_sftp_subsystem(
    handle: &Arc<russh::client::Handle<SftpClientHandler>>,
) -> Result<SftpSession> {
    await_sftp_auxiliary_setup(
        async {
            let channel = handle
                .channel_open_session()
                .await
                .context("open document sftp channel")?;
            channel
                .request_subsystem(true, "sftp")
                .await
                .context("request document sftp subsystem")?;
            open_sftp_session(channel.into_stream())
                .await
                .context("document sftp handshake")
        },
        SFTP_SETUP_TIMEOUT,
    )
    .await
}

async fn document_stat_impl(
    sftp: &SftpSession,
    path: &str,
) -> Result<crate::document::remote::RemoteMetadata> {
    let metadata = match sftp.metadata(path).await {
        Ok(metadata) => metadata,
        Err(russh_sftp::client::error::Error::Status(status))
            if status.status_code == russh_sftp::protocol::StatusCode::NoSuchFile =>
        {
            return Err(crate::document::remote::RemoteFileError::NotFound.into());
        }
        Err(error) => return Err(error.into()),
    };
    Ok(crate::document::remote::RemoteMetadata {
        size: metadata.size.unwrap_or(0),
        mtime: metadata.mtime.unwrap_or(0),
        permissions: metadata.permissions,
    })
}

async fn document_read_impl(
    sftp: &SftpSession,
    path: &str,
    range: Option<crate::document::remote::ByteRange>,
) -> Result<Vec<u8>> {
    let mut file = sftp.open(path).await?;
    let mut bytes = Vec::new();
    if let Some(range) = range {
        validate_document_read_size(range.length as u64)?;
        file.seek(std::io::SeekFrom::Start(range.offset)).await?;
        file.take(range.length as u64)
            .read_to_end(&mut bytes)
            .await?;
    } else {
        file.take(crate::document::EDITABLE_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .await?;
        validate_document_read_size(bytes.len() as u64)?;
    }
    Ok(bytes)
}

fn validate_document_read_size(size: u64) -> Result<()> {
    if size > crate::document::EDITABLE_MAX_BYTES {
        return Err(anyhow!(
            "document read exceeds the {} byte limit",
            crate::document::EDITABLE_MAX_BYTES
        ));
    }
    Ok(())
}

async fn document_write_atomic_impl(
    sftp: &SftpSession,
    path: &str,
    bytes: &[u8],
    permissions: Option<u32>,
    operation_id: &str,
) -> Result<crate::document::remote::RemoteMetadata> {
    let temporary = crate::document::remote::temporary_remote_path(path, operation_id);
    let backup = crate::document::remote::backup_remote_path(path, operation_id);
    let (destination_exists, existing_permissions) =
        existing_remote_permissions(sftp, path).await?;

    let result = async {
        let mut file = sftp.create(&temporary).await?;
        file.write_all(bytes).await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);

        set_remote_permissions(sftp, &temporary, permissions.or(existing_permissions)).await?;
        commit_remote_replacement(sftp, &temporary, path, &backup, destination_exists).await?;

        document_stat_impl(sftp, path).await
    }
    .await;

    if result.is_err() {
        cleanup_remote_temporary(sftp, &temporary).await;
    }
    result
}

#[cfg(test)]
mod handle_tests {
    use super::*;
    use crate::session::host_keys::HostKeyError;

    const TEST_HOST_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const CHANGED_HOST_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G1sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X";

    #[test]
    fn remote_basename_treats_backslash_as_filename_content() {
        assert_eq!(base_name(r"/dir/report\2026.txt"), r"report\2026.txt");
    }

    #[test]
    fn document_read_limit_accepts_boundary_and_rejects_next_byte() {
        assert!(validate_document_read_size(crate::document::EDITABLE_MAX_BYTES).is_ok());
        assert!(validate_document_read_size(crate::document::EDITABLE_MAX_BYTES + 1).is_err());
    }

    #[test]
    fn remote_command_output_is_bounded() {
        let mut output = vec![b'x'; MAX_REMOTE_COMMAND_OUTPUT_BYTES];
        let mut total_bytes = MAX_REMOTE_COMMAND_OUTPUT_BYTES;
        assert!(
            append_remote_command_output(&mut output, &[], &mut total_bytes, "test output").is_ok()
        );
        let error =
            append_remote_command_output(&mut output, b"x", &mut total_bytes, "test output")
                .expect_err("remote command output beyond the limit must fail");
        assert!(error.to_string().contains("test output exceeds"));
    }

    #[test]
    fn remote_command_without_exit_status_fails_closed() {
        let error = finish_remote_command("true", None, b"", b"")
            .expect_err("a command without an exit status is ambiguous");
        assert!(error.to_string().contains("without an exit status"));
    }

    #[test]
    fn remote_command_nonzero_status_prefers_stderr() {
        let error = finish_remote_command("false", Some(7), b"stdout", b"stderr")
            .expect_err("non-zero exit status must fail");
        let message = error.to_string();
        assert!(message.contains("exited with 7"));
        assert!(message.contains("stderr"));
        assert!(!message.contains("stdout"));
    }

    #[test]
    fn interrupted_deletes_are_drained_for_cleanup_results() {
        let mut active = HashMap::from([
            ("delete-1".to_string(), vec!["/tmp/a".to_string()]),
            (
                "delete-2".to_string(),
                vec!["/tmp/b".to_string(), "/tmp/c".to_string()],
            ),
        ]);

        let mut interrupted = take_interrupted_deletes(&mut active);
        interrupted.sort();
        assert!(active.is_empty());
        assert_eq!(
            interrupted,
            vec![
                vec!["/tmp/a".to_string()],
                vec!["/tmp/b".to_string(), "/tmp/c".to_string()],
            ]
        );
    }

    #[test]
    fn failed_local_download_commit_preserves_existing_target() {
        let root = tempfile::tempdir().expect("create temporary download root");
        let destination = root.path().join("existing-target");
        fs::create_dir(&destination).expect("create existing target directory");
        fs::write(destination.join("sentinel"), b"old contents").expect("write old target");

        let mut temporary = tempfile::NamedTempFile::new_in(root.path())
            .expect("create adjacent temporary download");
        std::io::Write::write_all(&mut temporary, b"new contents")
            .expect("write temporary download");
        let temporary = temporary.into_temp_path();
        let temporary_name = temporary.to_path_buf();

        assert!(persist_local_download(temporary, &destination).is_err());
        assert_eq!(
            fs::read(destination.join("sentinel")).expect("read preserved old target"),
            b"old contents"
        );
        assert!(!temporary_name.exists());
    }

    #[test]
    fn local_download_commit_replaces_existing_file() {
        let root = tempfile::tempdir().expect("create temporary download root");
        let destination = root.path().join("download.txt");
        fs::write(&destination, b"old contents").expect("write old target");
        let mut temporary = tempfile::NamedTempFile::new_in(root.path())
            .expect("create adjacent temporary download");
        std::io::Write::write_all(&mut temporary, b"new contents")
            .expect("write temporary download");

        persist_local_download(temporary.into_temp_path(), &destination)
            .expect("replace existing download");

        assert_eq!(fs::read(destination).unwrap(), b"new contents");
    }

    #[tokio::test]
    async fn sftp_handler_accepts_and_records_new_host_key() {
        let root = tempfile::tempdir().expect("create temporary known_hosts root");
        let path = root.path().join(".ssh").join("known_hosts");
        let mut handler =
            SftpClientHandler::with_known_hosts_path("files.example.test", 2222, path.clone());
        let key = russh::keys::ssh_key::PublicKey::from_openssh(TEST_HOST_KEY)
            .expect("parse test public key");

        assert!(
            russh::client::Handler::check_server_key(&mut handler, &key)
                .await
                .expect("new host key should be accepted")
        );
        let contents = fs::read_to_string(path).expect("read persisted known_hosts");
        assert_eq!(
            contents
                .matches("[files.example.test]:2222 ssh-ed25519 ")
                .count(),
            1,
        );
    }

    #[tokio::test]
    async fn sftp_handler_rejects_changed_host_key_without_modifying_file() {
        let root = tempfile::tempdir().expect("create temporary known_hosts root");
        let path = root.path().join("known_hosts");
        let original = format!("files.example.test {TEST_HOST_KEY}\n");
        fs::write(&path, &original).expect("write known_hosts");
        let mut handler =
            SftpClientHandler::with_known_hosts_path("files.example.test", 22, path.clone());
        let changed = russh::keys::ssh_key::PublicKey::from_openssh(CHANGED_HOST_KEY)
            .expect("parse changed host key");

        let error = russh::client::Handler::check_server_key(&mut handler, &changed)
            .await
            .expect_err("changed host key must be rejected");

        assert!(error.downcast_ref::<HostKeyError>().is_some());
        assert_eq!(
            fs::read_to_string(path).expect("read known_hosts"),
            original
        );
    }

    #[test]
    fn host_key_errors_require_manual_reconnect() {
        let root = tempfile::tempdir().expect("create temporary known_hosts root");
        let path = root.path().join("known_hosts");
        fs::write(&path, format!("example.test {TEST_HOST_KEY}\n")).expect("write known_hosts");
        let verifier = HostKeyVerifier::with_known_hosts_path("example.test", 22, path);
        let changed = russh::keys::ssh_key::PublicKey::from_openssh(CHANGED_HOST_KEY)
            .expect("parse changed host key");
        let error = anyhow::Error::new(
            verifier
                .verify(&changed)
                .expect_err("changed key must fail"),
        )
        .context("connect example.test:22 failed");

        assert_eq!(sftp_retry_policy(&error), SftpRetryPolicy::Manual);
    }

    #[test]
    fn ordinary_connection_errors_keep_backoff() {
        assert_eq!(
            sftp_retry_policy(&anyhow!("network reset")),
            SftpRetryPolicy::Backoff,
        );
    }

    #[tokio::test]
    async fn close_cancels_sftp_setup_before_handshake_finishes() {
        let (commands, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        commands.send(SftpCommand::Close).unwrap();

        let outcome = await_sftp_setup_or_close(
            std::future::pending::<anyhow::Result<()>>(),
            &mut receiver,
            Duration::from_secs(1),
        )
        .await
        .unwrap();

        assert!(matches!(outcome, SftpSetupOutcome::Cancelled));
    }

    #[tokio::test]
    async fn sftp_setup_timeout_uses_connection_backoff() {
        let (_commands, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let error = await_sftp_setup_or_close(
            std::future::pending::<anyhow::Result<()>>(),
            &mut receiver,
            Duration::from_millis(10),
        )
        .await
        .expect_err("pending setup must time out");

        assert!(error.to_string().contains("SFTP setup timed out"));
        assert_eq!(sftp_retry_policy(&error), SftpRetryPolicy::Backoff);
    }

    #[tokio::test]
    async fn auxiliary_sftp_setup_has_a_deadline() {
        let error = await_sftp_auxiliary_setup(
            std::future::pending::<anyhow::Result<()>>(),
            Duration::from_millis(10),
        )
        .await
        .expect_err("pending auxiliary setup must time out");

        assert!(error.to_string().contains("auxiliary SFTP setup timed out"));
    }

    #[tokio::test]
    async fn sftp_setup_preserves_non_close_command_order() {
        let (commands, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        commands
            .send(SftpCommand::ListDir {
                path: "/first".into(),
                request_id: None,
                expected_generation: None,
            })
            .unwrap();
        commands
            .send(SftpCommand::Preview("/second".into()))
            .unwrap();

        let outcome = await_sftp_setup_or_close(
            async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok::<(), anyhow::Error>(())
            },
            &mut receiver,
            Duration::from_secs(1),
        )
        .await
        .unwrap();

        let SftpSetupOutcome::Ready { mut pending, .. } = outcome else {
            panic!("setup should complete");
        };
        assert!(matches!(
            pending.pop_front(),
            Some(SftpCommand::ListDir { path, .. }) if path == "/first"
        ));
        assert!(matches!(
            pending.pop_front(),
            Some(SftpCommand::Preview(path)) if path == "/second"
        ));
        assert!(pending.is_empty());
    }

    #[test]
    fn cloned_handle_only_closes_after_last_owner_drops() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = SftpHandle::from_sender_for_test(tx);
        let document_handle = handle.clone();
        drop(handle);
        assert!(rx.try_recv().is_err());
        drop(document_handle);
        assert!(matches!(rx.try_recv(), Ok(SftpCommand::Close)));
    }

    #[test]
    fn document_requests_are_forwarded_with_payloads() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = SftpHandle::from_sender_for_test(tx);

        let (stat_reply, _stat_receive) = tokio::sync::oneshot::channel();
        handle.document_stat("/etc/app.conf".into(), stat_reply);
        assert!(matches!(
            rx.try_recv(),
            Ok(SftpCommand::DocumentStat { path, .. }) if path == "/etc/app.conf"
        ));

        let range = crate::document::remote::ByteRange {
            offset: 4096,
            length: 512,
        };
        let (read_reply, _read_receive) = tokio::sync::oneshot::channel();
        handle.document_read("/var/log/app.log".into(), Some(range), read_reply);
        assert!(matches!(
            rx.try_recv(),
            Ok(SftpCommand::DocumentRead { path, range: Some(actual), .. })
                if path == "/var/log/app.log" && actual == range
        ));

        let (write_reply, _write_receive) = tokio::sync::oneshot::channel();
        handle.document_write_atomic(
            "/etc/app.conf".into(),
            b"updated".to_vec(),
            Some(0o100640),
            "operation-1".into(),
            write_reply,
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(SftpCommand::DocumentWriteAtomic {
                path,
                bytes,
                permissions: Some(0o100640),
                operation_id,
                ..
            }) if path == "/etc/app.conf"
                && bytes == b"updated"
                && operation_id == "operation-1"
        ));
    }

    #[test]
    fn only_read_only_commands_are_replayable_after_disconnect() {
        assert!(
            SftpCommand::ListDir {
                path: "/etc".into(),
                request_id: None,
                expected_generation: None,
            }
            .is_replayable()
        );
        assert!(SftpCommand::Preview("/etc/app.conf".into()).is_replayable());
        assert!(!SftpCommand::ReconnectNow.is_replayable());

        let (stat_reply, _stat_receive) = tokio::sync::oneshot::channel();
        assert!(
            SftpCommand::DocumentStat {
                path: "/etc/app.conf".into(),
                reply: stat_reply,
            }
            .is_replayable()
        );

        let (write_reply, _write_receive) = tokio::sync::oneshot::channel();
        assert!(
            !SftpCommand::DocumentWriteAtomic {
                path: "/etc/app.conf".into(),
                bytes: b"updated".to_vec(),
                permissions: None,
                operation_id: "op-1".into(),
                reply: write_reply,
            }
            .is_replayable()
        );
        assert!(!SftpCommand::DeletePaths(vec!["/tmp/app".into()]).is_replayable());
        assert!(
            !SftpCommand::Download {
                remote: "/tmp/app".into(),
                local_dir: "/tmp".into(),
            }
            .is_replayable()
        );
    }

    #[test]
    fn queued_automatic_list_dir_replaces_only_older_automatic_list_dirs() {
        let generation = SftpGeneration(7);
        let (events, events_rx) = std::sync::mpsc::channel();
        let mut pending = VecDeque::new();
        pending.push_back(SftpCommand::ListDir {
            path: "/manual".into(),
            request_id: None,
            expected_generation: Some(generation.0),
        });
        pending.push_back(SftpCommand::ListDir {
            path: "/automatic-old".into(),
            request_id: Some(1),
            expected_generation: Some(generation.0),
        });
        pending.push_back(SftpCommand::Preview("/preview.txt".into()));
        pending.push_back(SftpCommand::ListDir {
            path: "/refresh".into(),
            request_id: None,
            expected_generation: None,
        });

        queue_pending_command(
            &mut pending,
            SftpCommand::ListDir {
                path: "/automatic-new".into(),
                request_id: Some(2),
                expected_generation: Some(generation.0),
            },
            "group-1",
            generation,
            &events,
        );

        assert!(matches!(
            pending.pop_front(),
            Some(SftpCommand::ListDir {
                path,
                request_id: None,
                expected_generation: Some(7),
            }) if path == "/manual"
        ));
        assert!(matches!(
            pending.pop_front(),
            Some(SftpCommand::Preview(path)) if path == "/preview.txt"
        ));
        assert!(matches!(
            pending.pop_front(),
            Some(SftpCommand::ListDir {
                path,
                request_id: None,
                expected_generation: None,
            }) if path == "/refresh"
        ));
        assert!(matches!(
            pending.pop_front(),
            Some(SftpCommand::ListDir {
                path,
                request_id: Some(2),
                expected_generation: Some(7),
            }) if path == "/automatic-new"
        ));
        assert!(pending.is_empty());
        assert!(events_rx.try_recv().is_err());
    }

    #[test]
    fn reconnect_discards_only_pending_automatic_list_dirs() {
        let mut pending = VecDeque::new();
        pending.push_back(SftpCommand::ListDir {
            path: "/manual".into(),
            request_id: None,
            expected_generation: Some(1),
        });
        pending.push_back(SftpCommand::ListDir {
            path: "/automatic".into(),
            request_id: Some(1),
            expected_generation: Some(1),
        });
        pending.push_back(SftpCommand::Preview("/preview.txt".into()));

        discard_pending_automatic_list_dirs(&mut pending);

        assert!(matches!(
            pending.pop_front(),
            Some(SftpCommand::ListDir {
                path,
                request_id: None,
                expected_generation: Some(1),
            }) if path == "/manual"
        ));
        assert!(matches!(
            pending.pop_front(),
            Some(SftpCommand::Preview(path)) if path == "/preview.txt"
        ));
        assert!(pending.is_empty());
    }

    #[test]
    fn list_dir_generation_guard_accepts_current_and_unbound_commands() {
        let generation = SftpGeneration(9);

        assert!(list_dir_generation_matches(Some(9), generation));
        assert!(!list_dir_generation_matches(Some(8), generation));
        assert!(list_dir_generation_matches(None, generation));
    }

    // ---- Archive extraction traversal regression tests --------------------
    //
    // Directory downloads extract a remotely created tar.gz on the local
    // machine, so escaping entries must never write outside the target
    // directory. tar 0.4.x guards `..` by skipping the entry and rejects
    // writes that canonicalize outside the destination (symlink escapes
    // included); zip is guarded by `enclosed_name()`. These tests pin that
    // behavior against future refactors of `extract_archive_to`.

    fn append_tar_text_entry<W: std::io::Write>(
        builder: &mut tar::Builder<W>,
        name: &str,
        contents: &str,
    ) {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).expect("set tar entry path");
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append(&header, contents.as_bytes())
            .expect("append tar entry");
    }

    /// `Header::set_path` refuses `..` components, so to craft a hostile
    /// fixture we set a validated placeholder and then patch the raw name
    /// field (checksum is recomputed afterwards). The reader under test must
    /// therefore defend itself; `Builder::append` does not re-validate.
    fn append_tar_entry_with_raw_name<W: std::io::Write>(
        builder: &mut tar::Builder<W>,
        name: &str,
        contents: &str,
    ) {
        let mut header = tar::Header::new_gnu();
        header
            .set_path("placeholder")
            .expect("set placeholder path");
        let name_bytes = name.as_bytes();
        assert!(
            name_bytes.len() < 100,
            "tar name field must fit in 100 bytes"
        );
        let bytes = header.as_mut_bytes();
        bytes[..name_bytes.len()].copy_from_slice(name_bytes);
        for slot in &mut bytes[name_bytes.len()..100] {
            *slot = 0;
        }
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append(&header, contents.as_bytes())
            .expect("append tar entry");
    }

    fn write_tar_with_escaping_and_regular_entries(path: &Path) {
        let file = fs::File::create(path).expect("create tar archive");
        let mut builder = tar::Builder::new(file);
        append_tar_entry_with_raw_name(&mut builder, "../evil.txt", "pwned");
        append_tar_text_entry(&mut builder, "keep.txt", "safe");
        builder.finish().expect("finish tar archive");
    }

    #[tokio::test]
    async fn extract_tar_skips_entries_with_parent_dir_components() {
        let root = tempfile::tempdir().expect("create temporary extraction root");
        let target = root.path().join("target");
        fs::create_dir_all(&target).expect("create target dir");
        let archive = root.path().join("payload.tar");
        write_tar_with_escaping_and_regular_entries(&archive);

        extract_archive_to(&archive, &target)
            .await
            .expect("extract tar archive");

        // The escaping entry must be skipped, never written outside target.
        assert!(!root.path().join("evil.txt").exists());
        assert!(!target.join("evil.txt").exists());
        assert_eq!(
            fs::read_to_string(target.join("keep.txt")).expect("read regular entry"),
            "safe",
        );
    }

    #[tokio::test]
    async fn extract_tar_gz_skips_entries_with_parent_dir_components() {
        let root = tempfile::tempdir().expect("create temporary extraction root");
        let target = root.path().join("target");
        fs::create_dir_all(&target).expect("create target dir");
        let archive = root.path().join("payload.tar.gz");
        let file = fs::File::create(&archive).expect("create tar.gz archive");
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        append_tar_entry_with_raw_name(&mut builder, "../evil.txt", "pwned");
        append_tar_text_entry(&mut builder, "keep.txt", "safe");
        builder.finish().expect("finish tar archive");
        builder
            .into_inner()
            .expect("unwrap gz encoder")
            .finish()
            .expect("finish gzip stream");

        extract_archive_to(&archive, &target)
            .await
            .expect("extract tar.gz archive");

        assert!(!root.path().join("evil.txt").exists());
        assert_eq!(
            fs::read_to_string(target.join("keep.txt")).expect("read regular entry"),
            "safe",
        );
    }

    #[tokio::test]
    async fn extract_tar_rejects_writes_through_symlinks_escaping_the_target() {
        let root = tempfile::tempdir().expect("create temporary extraction root");
        let target = root.path().join("target");
        fs::create_dir_all(&target).expect("create target dir");
        let outside = root.path().join("outside");
        fs::create_dir_all(&outside).expect("create outside dir");
        let archive = root.path().join("symlink.tar");

        let file = fs::File::create(&archive).expect("create tar archive");
        let mut builder = tar::Builder::new(file);

        // A symlink inside the target pointing at a sibling directory,
        // followed by a file written through it.
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_path("escape").expect("set symlink entry path");
        link.set_link_name(outside.to_str().expect("absolute symlink target"))
            .expect("set symlink target");
        link.set_size(0);
        link.set_mode(0o777);
        link.set_cksum();
        builder
            .append(&link, std::io::empty())
            .expect("append symlink entry");

        append_tar_text_entry(&mut builder, "escape/pwned.txt", "pwned");
        builder.finish().expect("finish tar archive");

        let result = extract_archive_to(&archive, &target).await;

        // The write through the escaping symlink must be refused outright.
        assert!(result.is_err());
        assert!(!outside.join("pwned.txt").exists());
    }

    #[tokio::test]
    async fn extract_zip_writes_entries_inside_the_target_directory() {
        let root = tempfile::tempdir().expect("create temporary extraction root");
        let target = root.path().join("target");
        fs::create_dir_all(&target).expect("create target dir");
        let archive = root.path().join("payload.zip");

        let file = fs::File::create(&archive).expect("create zip archive");
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("nested/hello.txt", zip::write::SimpleFileOptions::default())
            .expect("start zip entry");
        std::io::Write::write_all(&mut writer, b"zip contents").expect("write zip entry");
        writer.finish().expect("finish zip archive");

        extract_archive_to(&archive, &target)
            .await
            .expect("extract zip archive");

        assert_eq!(
            fs::read_to_string(target.join("nested").join("hello.txt"))
                .expect("read extracted zip entry"),
            "zip contents",
        );
        assert!(!root.path().join("hello.txt").exists());
    }

    // ---- shell_quote adversarial tests -------------------------------------
    //
    // Remote paths are embedded in `tar -C '...' -czf '...' '<name>'` /
    // `rm -f '<path>'` command lines. The quoting must make it impossible for
    // a path to terminate the surrounding single-quoted context.

    /// Parses the `'...'"'"'...'` single-quote escaping emitted by
    /// `shell_quote` back into the original string; `None` means the quoted
    /// form is malformed (i.e. the payload escaped its quoting context).
    fn unquote_single(quoted: &str) -> Option<String> {
        let mut chars = quoted.chars().peekable();
        if chars.next() != Some('\'') {
            return None;
        }
        let mut out = String::new();
        while let Some(c) = chars.next() {
            if c == '\'' {
                if chars.peek() == Some(&'"') {
                    for expected in ['"', '\'', '"', '\''] {
                        if chars.next() != Some(expected) {
                            return None;
                        }
                    }
                    out.push('\'');
                } else if chars.peek().is_none() {
                    return Some(out);
                } else {
                    return None;
                }
            } else {
                out.push(c);
            }
        }
        None
    }

    #[test]
    fn shell_quote_wraps_plain_names_in_single_quotes() {
        assert_eq!(shell_quote("hello.txt"), "'hello.txt'");
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("it's.txt"), "'it'\"'\"'s.txt'");
    }

    #[test]
    fn shell_quote_neutralizes_command_substitution_and_backticks() {
        assert_eq!(shell_quote("$(rm -rf /)"), "'$(rm -rf /)'");
        assert_eq!(shell_quote("`id`"), "'`id`'");
    }

    #[test]
    fn shell_quote_round_trips_arbitrary_input() {
        for input in [
            "plain.txt",
            "it's.txt",
            "$(rm -rf /)",
            "`id`",
            "a\nb;c&d|e>f<g",
            "with 'quotes' and \"double\"",
            "文件 名.txt",
            "'",
            "''",
            "trailing-slash/",
            "-rf --no-preserve-root",
        ] {
            let quoted = shell_quote(input);
            assert_eq!(
                unquote_single(&quoted).as_deref(),
                Some(input),
                "input: {input:?}, quoted: {quoted:?}",
            );
        }
    }
}
