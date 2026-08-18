use std::{
    ffi::OsString,
    io::{Read, Write},
    sync::mpsc::{self, Sender},
    thread,
};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::terminal::{BackendCommand, BackendEvent, BackendTx};

fn choose_local_shell(
    environment_shell: Option<OsString>,
    account_shell: Option<OsString>,
) -> OsString {
    environment_shell
        .filter(|shell| !shell.is_empty())
        .or_else(|| account_shell.filter(|shell| !shell.is_empty()))
        .unwrap_or_else(|| {
            if cfg!(windows) {
                OsString::from("powershell.exe")
            } else {
                OsString::from("/bin/sh")
            }
        })
}

#[cfg(unix)]
fn account_login_shell() -> Option<OsString> {
    use std::os::unix::ffi::OsStringExt as _;

    const MAX_PASSWD_BUFFER: usize = 1024 * 1024;
    let configured_size = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let mut buffer_size = if configured_size > 0 {
        configured_size as usize
    } else {
        16 * 1024
    }
    .clamp(1024, MAX_PASSWD_BUFFER);

    loop {
        let mut entry = unsafe { std::mem::zeroed::<libc::passwd>() };
        let mut result = std::ptr::null_mut();
        let mut buffer = vec![0u8; buffer_size];
        let status = unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                &mut entry,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };

        if status == 0 {
            if result.is_null() || entry.pw_shell.is_null() {
                return None;
            }
            let shell = unsafe { std::ffi::CStr::from_ptr(entry.pw_shell) };
            return Some(OsString::from_vec(shell.to_bytes().to_vec()))
                .filter(|shell| !shell.is_empty());
        }
        if status != libc::ERANGE || buffer_size == MAX_PASSWD_BUFFER {
            return None;
        }
        buffer_size = (buffer_size * 2).min(MAX_PASSWD_BUFFER);
    }
}

#[cfg(windows)]
fn account_login_shell() -> Option<OsString> {
    std::env::var_os("COMSPEC")
}

pub fn spawn_local_terminal(
    tab_id: String,
    generation: u32,
    cols: u16,
    rows: u16,
    events: Sender<BackendEvent>,
) -> Result<BackendTx> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("open local PTY")?;

    let shell = choose_local_shell(std::env::var_os("SHELL"), account_login_shell());

    let mut cmd = CommandBuilder::new(&shell);
    cmd.env(
        "TERM",
        std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()),
    );
    cmd.env(
        "COLORTERM",
        std::env::var("COLORTERM").unwrap_or_else(|_| "truecolor".into()),
    );
    cmd.env("TERM_PROGRAM", "JShell");
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    if let Ok(lang) = std::env::var("LANG") {
        cmd.env("LANG", lang);
    } else {
        cmd.env("LANG", "en_US.UTF-8");
    }
    if let Ok(home) = std::env::var("HOME") {
        cmd.env("HOME", home);
    }
    cmd.env("SHELL", shell);
    let mut child = pair.slave.spawn_command(cmd).context("spawn local shell")?;
    drop(pair.slave);

    let master = pair.master;
    let mut reader = master.try_clone_reader().context("clone PTY reader")?;
    let mut writer = master.take_writer().context("take PTY writer")?;
    let (cmd_tx, cmd_rx) = mpsc::channel::<BackendCommand>();

    let read_tab = tab_id.clone();
    let read_events = events.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = read_events.send(BackendEvent::Output {
                        tab_id: read_tab.clone(),
                        generation,
                        bytes: buf[..n].to_vec(),
                    });
                }
                Err(err) => {
                    let _ = read_events.send(BackendEvent::Closed {
                        tab_id: read_tab.clone(),
                        generation,
                        reason: format!("local read error: {err}"),
                    });
                    return;
                }
            }
        }
        let _ = read_events.send(BackendEvent::Closed {
            tab_id: read_tab,
            generation,
            reason: "local shell closed".into(),
        });
    });

    let write_tab = tab_id.clone();
    let write_events = events.clone();
    thread::spawn(move || {
        loop {
            match cmd_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(command) => match command {
                    BackendCommand::Input(bytes) => {
                        if let Err(err) = writer.write_all(&bytes) {
                            let _ = write_events.send(BackendEvent::Closed {
                                tab_id: write_tab.clone(),
                                generation,
                                reason: format!("local write error: {err}"),
                            });
                            break;
                        }
                        let _ = writer.flush();
                    }
                    BackendCommand::Resize { cols, rows } => {
                        let _ = master.resize(PtySize {
                            rows,
                            cols,
                            pixel_width: 0,
                            pixel_height: 0,
                        });
                    }
                    BackendCommand::Close => break,
                    BackendCommand::SampleMetrics | BackendCommand::LoadCommandHistory => {}
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Ok(Some(status)) = child.try_wait() {
                        let _ = write_events.send(BackendEvent::Closed {
                            tab_id: write_tab,
                            generation,
                            reason: format!("local shell exited: {status}"),
                        });
                        return;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    });

    let _ = events.send(BackendEvent::Status {
        tab_id,
        generation,
        text: "local shell ready".into(),
    });

    Ok(BackendTx::Local(cmd_tx))
}

#[cfg(test)]
mod tests {
    use super::choose_local_shell;
    use std::ffi::{OsStr, OsString};

    #[test]
    fn environment_shell_takes_precedence_over_the_account_shell() {
        let selected = choose_local_shell(
            Some(OsString::from("/custom/env-shell")),
            Some(OsString::from("/custom/account-shell")),
        );

        assert_eq!(selected, OsStr::new("/custom/env-shell"));
    }

    #[test]
    fn account_shell_is_used_when_the_environment_does_not_define_one() {
        let selected = choose_local_shell(None, Some(OsString::from("/custom/account-shell")));

        assert_eq!(selected, OsStr::new("/custom/account-shell"));
    }

    #[test]
    fn empty_shell_values_are_ignored() {
        let selected = choose_local_shell(
            Some(OsString::new()),
            Some(OsString::from("/custom/account-shell")),
        );

        assert_eq!(selected, OsStr::new("/custom/account-shell"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_fallback_is_always_available() {
        assert_eq!(choose_local_shell(None, None), OsStr::new("/bin/sh"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_fallback_is_always_available() {
        assert_eq!(choose_local_shell(None, None), OsStr::new("powershell.exe"));
    }
}
