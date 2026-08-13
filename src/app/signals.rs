use std::sync::mpsc::{self, Receiver};

/// Installs termination signal handling (SIGINT/SIGTERM on Unix) and returns a
/// channel that delivers a notification when the process is asked to shut down.
///
/// The main event pump drains the channel and routes the request through the
/// regular quit flow, so layout state is saved and the tray icon is cleaned up
/// instead of the process being killed abruptly.
pub(crate) fn install() -> Option<Receiver<()>> {
    #[cfg(unix)]
    {
        use signal_hook::consts::{SIGINT, SIGTERM};
        let mut signals = match signal_hook::iterator::Signals::new([SIGINT, SIGTERM]) {
            Ok(signals) => signals,
            Err(error) => {
                tracing::warn!("failed to install termination signal handlers: {error}");
                return None;
            }
        };
        let (tx, rx) = mpsc::channel();
        if std::thread::Builder::new()
            .name("jshell-signals".to_string())
            .spawn(move || {
                let handle = signals.handle();
                let mut forever = signals.forever();
                if let Some(signal) = forever.next() {
                    tracing::info!("received signal {signal}, shutting down gracefully");
                    let _ = tx.send(());
                    // Restore the default disposition so a second signal can
                    // force-kill the process if the graceful quit gets stuck.
                    handle.close();
                }
            })
            .is_err()
        {
            tracing::warn!("failed to spawn termination signal thread");
            return None;
        }
        Some(rx)
    }

    #[cfg(not(unix))]
    {
        None
    }
}
