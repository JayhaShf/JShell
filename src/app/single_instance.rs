use std::{
    io::{self, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{Receiver, Sender},
    },
    time::{Duration, Instant},
};

use interprocess::local_socket::{GenericNamespaced, ListenerOptions, prelude::*};

const INSTANCE_SOCKET_NAME: &str = "jshell-single-instance";
const ACTIVATE_PAYLOAD: &[u8] = b"activate";
const ACTIVATE_ACK: &[u8] = b"activated";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(500);
const IO_POLL_INTERVAL: Duration = Duration::from_millis(5);
const MAX_ACTIVE_CONNECTIONS: usize = 8;
/// How many times to retry the bind/connect handshake before running unlocked.
const ACQUIRE_ATTEMPTS: usize = 3;

pub(crate) enum AcquireOutcome {
    /// This process is the first instance and receives activation requests
    /// from later launches.
    First(Receiver<()>),
    /// Another instance is already running and has been asked to show its window.
    Second,
}

fn instance_socket_name() -> String {
    #[cfg(unix)]
    {
        // Linux abstract sockets and the shared `/tmp` namespace are visible to
        // every user, so include the user id to keep instances apart.
        let uid = unsafe { libc::geteuid() };
        format!("{INSTANCE_SOCKET_NAME}-{uid}")
    }
    #[cfg(not(unix))]
    {
        INSTANCE_SOCKET_NAME.to_string()
    }
}

/// Ensures only one JShell instance runs per user session.
///
/// The first instance binds a platform-local socket (`\\.\pipe\` on Windows,
/// the abstract namespace on Linux, `/tmp/` elsewhere). Later launches connect
/// to it, ask the running instance to activate its window, and exit.
pub(crate) fn acquire() -> AcquireOutcome {
    for _ in 0..ACQUIRE_ATTEMPTS {
        let name = match instance_socket_name().to_ns_name::<GenericNamespaced>() {
            Ok(name) => name,
            Err(error) => {
                tracing::warn!("failed to build single instance socket name: {error}");
                return AcquireOutcome::First(closed_receiver());
            }
        };
        match ListenerOptions::new().name(name.clone()).create_sync() {
            Ok(listener) => {
                let (tx, rx) = std::sync::mpsc::channel();
                spawn_accept_loop(listener, tx);
                return AcquireOutcome::First(rx);
            }
            Err(bind_error) => {
                tracing::info!(
                    "single instance listener busy ({bind_error}), asking the running instance to activate"
                );
                match LocalSocketStream::connect(name.clone()) {
                    Ok(mut stream) => match request_activation(&mut stream) {
                        Ok(()) => return AcquireOutcome::Second,
                        Err(error) => {
                            tracing::warn!(
                                "single instance activation handshake failed ({error}), retrying"
                            );
                            std::thread::sleep(Duration::from_millis(200));
                        }
                    },
                    Err(connect_error) => {
                        tracing::warn!(
                            "failed to reach the running instance ({connect_error}), retrying"
                        );
                        remove_stale_socket();
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            }
        }
    }

    // The lock could not be established: run unlocked instead of failing to start.
    tracing::warn!("could not establish single instance lock, running unlocked");
    AcquireOutcome::First(closed_receiver())
}

fn spawn_accept_loop(listener: LocalSocketListener, tx: Sender<()>) {
    let active_connections = Arc::new(AtomicUsize::new(0));
    match std::thread::Builder::new()
        .name("jshell-instance".to_string())
        .spawn(move || {
            loop {
                match listener.accept() {
                    Ok(stream) => {
                        if let Err(error) = verify_peer_user(&stream) {
                            tracing::warn!("rejected single instance peer: {error}");
                            continue;
                        }
                        let Some(permit) = ConnectionPermit::try_acquire(&active_connections) else {
                            tracing::warn!(
                                "single instance connection limit reached; dropping activation request"
                            );
                            continue;
                        };
                        let tx = tx.clone();
                        if let Err(error) = std::thread::Builder::new()
                            .name("jshell-instance-connection".to_string())
                            .spawn(move || {
                                let _permit = permit;
                                if let Err(error) = handle_activation_connection(stream, &tx) {
                                    tracing::warn!(
                                        "single instance activation request failed: {error}"
                                    );
                                }
                            })
                        {
                            tracing::warn!(
                                "failed to spawn single instance connection thread: {error}"
                            );
                        }
                    }
                    Err(error) => {
                        tracing::warn!("single instance listener error: {error}");
                        break;
                    }
                }
            }
        }) {
        Ok(_) => {}
        Err(error) => {
            // The listener is dropped here, which releases the socket so later
            // launches can still bind it.
            tracing::warn!("failed to spawn single instance listener thread: {error}");
        }
    }
}

struct ConnectionPermit {
    active: Arc<AtomicUsize>,
}

impl ConnectionPermit {
    fn try_acquire(active: &Arc<AtomicUsize>) -> Option<Self> {
        active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < MAX_ACTIVE_CONNECTIONS).then_some(current + 1)
            })
            .ok()?;
        Some(Self {
            active: Arc::clone(active),
        })
    }
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

fn request_activation(stream: &mut LocalSocketStream) -> io::Result<()> {
    verify_peer_user(stream)?;
    stream.set_nonblocking(true)?;
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    write_all_until(stream, ACTIVATE_PAYLOAD, deadline)?;

    let mut ack = [0_u8; ACTIVATE_ACK.len()];
    read_exact_until(stream, &mut ack, deadline)?;
    if ack != ACTIVATE_ACK {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "single instance server returned an invalid acknowledgement",
        ));
    }
    Ok(())
}

fn handle_activation_connection(mut stream: LocalSocketStream, tx: &Sender<()>) -> io::Result<()> {
    stream.set_nonblocking(true)?;
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut payload = [0_u8; ACTIVATE_PAYLOAD.len()];
    read_exact_until(&mut stream, &mut payload, deadline)?;
    if payload != ACTIVATE_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "single instance client sent an invalid request",
        ));
    }
    tx.send(())
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "activation receiver closed"))?;
    write_all_until(&mut stream, ACTIVATE_ACK, deadline)
}

fn verify_peer_user(stream: &LocalSocketStream) -> io::Result<()> {
    #[cfg(unix)]
    {
        let peer_uid = stream.peer_creds()?.euid().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "single instance peer has no effective user id",
            )
        })?;
        let current_uid = unsafe { libc::geteuid() };
        if peer_uid != current_uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "single instance peer belongs to another user",
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = stream;
    Ok(())
}

fn read_exact_until<R: Read>(
    reader: &mut R,
    mut buffer: &mut [u8],
    deadline: Instant,
) -> io::Result<()> {
    while !buffer.is_empty() {
        ensure_before_deadline(deadline)?;
        match reader.read(buffer) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "single instance peer closed during handshake",
                ));
            }
            Ok(read) => buffer = &mut buffer[read..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => wait_for_io(deadline)?,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn write_all_until<W: Write>(
    writer: &mut W,
    mut buffer: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    while !buffer.is_empty() {
        ensure_before_deadline(deadline)?;
        match writer.write(buffer) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "single instance peer stopped accepting handshake data",
                ));
            }
            Ok(written) => buffer = &buffer[written..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => wait_for_io(deadline)?,
            Err(error) => return Err(error),
        }
    }
    loop {
        ensure_before_deadline(deadline)?;
        match writer.flush() {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => wait_for_io(deadline)?,
            Err(error) => return Err(error),
        }
    }
}

fn wait_for_io(deadline: Instant) -> io::Result<()> {
    let now = Instant::now();
    ensure_before_deadline(deadline)?;
    std::thread::sleep(IO_POLL_INTERVAL.min(deadline - now));
    Ok(())
}

fn ensure_before_deadline(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "single instance handshake timed out",
        ));
    }
    Ok(())
}

/// On platforms where the socket is a filesystem entry, a crashed instance can
/// leave a stale file behind that no longer accepts connections.
fn remove_stale_socket() {
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        let path = std::path::Path::new("/tmp").join(instance_socket_name());
        if std::fs::remove_file(&path).is_ok() {
            tracing::info!("removed stale single instance socket at {}", path.display());
        }
    }
}

fn closed_receiver() -> Receiver<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    drop(tx);
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_SOCKET_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn test_socket_name() -> String {
        format!(
            "jshell-instance-test-{}-{}",
            std::process::id(),
            TEST_SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[test]
    fn activation_requires_server_acknowledgement() {
        let socket_name = test_socket_name();
        let name = socket_name
            .to_ns_name::<GenericNamespaced>()
            .expect("build test socket name");
        let listener = ListenerOptions::new()
            .name(name.clone())
            .create_sync()
            .expect("bind test socket");
        let server = std::thread::spawn(move || {
            let mut stream = listener.accept().expect("accept test connection");
            verify_peer_user(&stream).expect("same-user test peer");
            stream
                .set_nonblocking(true)
                .expect("set test stream nonblocking");
            let mut payload = [0_u8; ACTIVATE_PAYLOAD.len()];
            read_exact_until(
                &mut stream,
                &mut payload,
                Instant::now() + Duration::from_secs(1),
            )
            .expect("read activation without acknowledging it");
        });

        let mut stream = LocalSocketStream::connect(name).expect("connect test client");
        let error = request_activation(&mut stream)
            .expect_err("a listener that sends no acknowledgement is not a running instance");

        assert!(matches!(
            error.kind(),
            io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset
        ));
        server.join().expect("join test server");
    }

    #[test]
    fn valid_activation_is_delivered_and_acknowledged() {
        let socket_name = test_socket_name();
        let name = socket_name
            .to_ns_name::<GenericNamespaced>()
            .expect("build test socket name");
        let listener = ListenerOptions::new()
            .name(name.clone())
            .create_sync()
            .expect("bind test socket");
        let (tx, rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let stream = listener.accept().expect("accept test connection");
            verify_peer_user(&stream).expect("same-user test peer");
            handle_activation_connection(stream, &tx).expect("handle activation request");
        });

        let mut stream = LocalSocketStream::connect(name).expect("connect test client");
        request_activation(&mut stream).expect("complete activation handshake");

        rx.recv_timeout(Duration::from_secs(1))
            .expect("activation event delivered");
        server.join().expect("join test server");
    }

    #[test]
    fn stalled_activation_handshake_times_out() {
        let socket_name = test_socket_name();
        let name = socket_name
            .to_ns_name::<GenericNamespaced>()
            .expect("build test socket name");
        let listener = ListenerOptions::new()
            .name(name.clone())
            .create_sync()
            .expect("bind test socket");
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let _stream = listener.accept().expect("accept test connection");
            release_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("release stalled test connection");
        });

        let mut stream = LocalSocketStream::connect(name).expect("connect test client");
        let error = request_activation(&mut stream)
            .expect_err("a peer that never acknowledges activation must time out");

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        release_tx.send(()).expect("release test server");
        server.join().expect("join test server");
    }

    #[test]
    fn connection_permits_enforce_and_release_the_limit() {
        let active = Arc::new(AtomicUsize::new(0));
        let permits = (0..MAX_ACTIVE_CONNECTIONS)
            .map(|_| ConnectionPermit::try_acquire(&active).expect("permit below limit"))
            .collect::<Vec<_>>();

        assert!(ConnectionPermit::try_acquire(&active).is_none());
        drop(permits);
        assert_eq!(active.load(Ordering::Acquire), 0);
        assert!(ConnectionPermit::try_acquire(&active).is_some());
    }
}
