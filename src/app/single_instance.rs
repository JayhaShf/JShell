use std::{
    io::{Read, Write},
    sync::mpsc::{Receiver, Sender},
    time::Duration,
};

use interprocess::local_socket::{GenericNamespaced, ListenerOptions, prelude::*};

const INSTANCE_SOCKET_NAME: &str = "jshell-single-instance";
const ACTIVATE_PAYLOAD: &[u8] = b"activate";
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
        let uid = unsafe { libc::getuid() };
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
                    Ok(mut stream) => {
                        let _ = stream.write_all(ACTIVATE_PAYLOAD);
                        let _ = stream.flush();
                        return AcquireOutcome::Second;
                    }
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
    match std::thread::Builder::new()
        .name("jshell-instance".to_string())
        .spawn(move || {
            loop {
                match listener.accept() {
                    Ok(stream) => {
                        // Handle each connection on a short-lived thread so a
                        // stalled peer cannot block further activation requests.
                        let tx = tx.clone();
                        std::thread::spawn(move || {
                            let mut buffer = [0u8; ACTIVATE_PAYLOAD.len()];
                            let mut stream = stream;
                            if stream.read_exact(&mut buffer).is_ok() && buffer == ACTIVATE_PAYLOAD
                            {
                                let _ = tx.send(());
                            }
                        });
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
