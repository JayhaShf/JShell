use gpui::{App, AppContext as _, Bounds, WindowOptions, point, px, size};
use gpui_component::Root;

use crate::Ashell;
use crate::session::config::ConfigStore;

pub(crate) struct StartupConfig {
    pub(crate) config: ConfigStore,
    pub(crate) error: Option<String>,
}

impl StartupConfig {
    pub(crate) fn load() -> Self {
        Self::from_result(ConfigStore::load())
    }

    fn from_result(result: anyhow::Result<ConfigStore>) -> Self {
        match result {
            Ok(config) => Self {
                config,
                error: None,
            },
            Err(err) => {
                let error = format!("{err:#}");
                tracing::error!(error = %error, "failed to load persistent configuration");
                Self {
                    config: ConfigStore::in_memory(),
                    error: Some(error),
                }
            }
        }
    }
}

pub(crate) fn load_startup_config_and_bind_workspace_keys(cx: &mut gpui::App) -> StartupConfig {
    let startup_config = StartupConfig::load();
    crate::app::keybinding_recorder::bind_workspace_keys_from_config(cx, &startup_config.config);
    startup_config
}

struct LocalMinutelyRoller {
    dir: std::path::PathBuf,
    prefix: String,
    current_minute: u32,
    file: Option<std::fs::File>,
}

impl LocalMinutelyRoller {
    fn new(dir: std::path::PathBuf, prefix: String) -> Self {
        Self {
            dir,
            prefix,
            current_minute: 60,
            file: None,
        }
    }

    fn rollover(&mut self, now: chrono::DateTime<chrono::Local>) -> std::io::Result<()> {
        use chrono::Timelike;
        let minute = now.minute();
        if self.current_minute != minute || self.file.is_none() {
            let filename = format!("{}-{}.log", self.prefix, now.format("%Y-%m-%d-%H-%M"));
            let path = self.dir.join(filename);
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            self.file = Some(file);
            self.current_minute = minute;

            // Cleanup old files keeping last 6
            if let Ok(entries) = std::fs::read_dir(&self.dir) {
                let mut files: Vec<_> = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().starts_with(&self.prefix))
                    .collect();
                files.sort_by_key(|e| {
                    e.metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                });
                if files.len() > 6 {
                    for file in files.iter().take(files.len() - 6) {
                        let _ = std::fs::remove_file(file.path());
                    }
                }
            }
        }
        Ok(())
    }
}

impl std::io::Write for LocalMinutelyRoller {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let now = chrono::Local::now();
        let _ = self.rollover(now);
        if let Some(f) = &mut self.file {
            f.write(buf)
        } else {
            Ok(buf.len())
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(f) = &mut self.file {
            f.flush()
        } else {
            Ok(())
        }
    }
}

pub(crate) fn init_logging() {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    let log_dir = directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().join(".config").join("jshell").join("log"))
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    std::fs::create_dir_all(&log_dir).ok();

    // Logs may contain hostnames and usernames; keep the directory private
    // like the config directory (init_logging runs before the config store
    // exists, so this cannot rely on its chmod).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&log_dir) {
            let mut perms = meta.permissions();
            perms.set_mode(0o700);
            let _ = std::fs::set_permissions(&log_dir, perms);
        }
    }

    let roller = LocalMinutelyRoller::new(log_dir.clone(), "jshell".to_string());

    let (non_blocking, _guard) = tracing_appender::non_blocking(roller);
    // Leak the guard so it lives for the entire duration of the app since GPUI's run might not return
    std::mem::forget(_guard);

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let stdout_layer = if cfg!(debug_assertions) {
        Some(
            tracing_subscriber::fmt::layer()
                .with_timer(tracing_subscriber::fmt::time::LocalTime::rfc_3339())
                .with_target(true),
        )
    } else {
        None
    };

    let file_layer = tracing_subscriber::fmt::layer()
        .with_timer(tracing_subscriber::fmt::time::LocalTime::rfc_3339())
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();
}

#[cfg(target_os = "macos")]
pub(crate) fn sync_macos_launch_environment() {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let Ok(output) = std::process::Command::new(&shell)
        .args(["-l", "-c", "env -0"])
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }

    for entry in output.stdout.split(|b| *b == 0) {
        if entry.is_empty() {
            continue;
        }
        let Some(eq) = entry.iter().position(|b| *b == b'=') else {
            continue;
        };
        let Ok(key) = std::str::from_utf8(&entry[..eq]) else {
            continue;
        };
        let Ok(value) = std::str::from_utf8(&entry[eq + 1..]) else {
            continue;
        };

        let should_import = matches!(
            key,
            "PATH"
                | "MANPATH"
                | "INFOPATH"
                | "LANG"
                | "LC_ALL"
                | "LC_CTYPE"
                | "SHELL"
                | "HOME"
                | "HOMEBREW_PREFIX"
                | "HOMEBREW_CELLAR"
                | "HOMEBREW_REPOSITORY"
                | "HTTP_PROXY"
                | "HTTPS_PROXY"
                | "ALL_PROXY"
                | "http_proxy"
                | "https_proxy"
                | "all_proxy"
        ) || key.starts_with("LC_");

        if should_import {
            unsafe {
                std::env::set_var(key, value);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn sync_macos_launch_environment() {}

pub(crate) fn open_main_window(cx: &mut App) {
    let startup_config = StartupConfig::load();
    open_main_window_with_config(cx, startup_config, None, None);
}

pub(crate) fn open_main_window_with_config(
    cx: &mut App,
    startup_config: StartupConfig,
    instance_events: Option<std::sync::mpsc::Receiver<()>>,
    signal_events: Option<std::sync::mpsc::Receiver<()>>,
) {
    let config = &startup_config.config;

    crate::session::config::initialize_env_proxy();

    let mut window_options = WindowOptions {
        titlebar: Some(gpui::TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: Some(gpui::point(px(9.0), px(9.0))),
        }),
        ..WindowOptions::default()
    };

    #[cfg(not(target_os = "macos"))]
    if let Ok(img) = image::load_from_memory(include_bytes!("../../assets/icons/jshell.png")) {
        window_options.icon = Some(std::sync::Arc::new(img.into_rgba8()));
    }

    if let Some(bounds) = config.window_bounds() {
        window_options.window_bounds = Some(match bounds {
            crate::session::config::SavedWindowBounds::Fullscreen {
                x,
                y,
                width,
                height,
            } => gpui::WindowBounds::Fullscreen(Bounds::new(
                point(px(*x), px(*y)),
                size(px(*width), px(*height)),
            )),
            crate::session::config::SavedWindowBounds::Maximized {
                x,
                y,
                width,
                height,
            } => gpui::WindowBounds::Maximized(Bounds::new(
                point(px(*x), px(*y)),
                size(px(*width), px(*height)),
            )),
            crate::session::config::SavedWindowBounds::Windowed {
                x,
                y,
                width,
                height,
            } => gpui::WindowBounds::Windowed(Bounds::new(
                point(px(*x), px(*y)),
                size(px(*width), px(*height)),
            )),
        });
    } else if let Some(display) = cx.displays().first().cloned() {
        let display_bounds = display.bounds();
        let width = display_bounds.size.width * 0.8;
        let height = display_bounds.size.height * 0.9;

        let x = display_bounds.origin.x + (display_bounds.size.width - width) / 2.0;

        #[cfg(target_os = "macos")]
        let y = display_bounds.origin.y;
        #[cfg(not(target_os = "macos"))]
        let y = display_bounds.origin.y + (display_bounds.size.height - height) / 2.0;

        window_options.window_bounds = Some(gpui::WindowBounds::Windowed(Bounds::new(
            point(x, y),
            size(width, height),
        )));
    }

    cx.open_window(window_options, move |window, cx| {
        window.activate_window();
        window.set_window_title("JShell");
        gpui_component::Theme::sync_system_appearance(Some(window), cx);
        let view =
            cx.new(|cx| Ashell::new(window, cx, startup_config, instance_events, signal_events));

        tracing::info!("[ui] main application window opened");
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);

        let view_clone = view.clone();
        window.on_window_should_close(cx, move |window: &mut gpui::Window, cx: &mut gpui::App| {
            let handle = window.window_handle();
            if !cx.windows().contains(&handle) {
                tracing::warn!(
                    "[ui] window not found in app during close, skipping save layout state."
                );
                return true;
            }
            if view_clone.read(cx).allow_window_close {
                view_clone.update(cx, |this, cx| {
                    this.close_detached_windows_for_shutdown(cx);
                    this.save_layout_state(window, cx);
                });
                return true;
            }
            if view_clone.read(cx).tray.is_some() && !view_clone.read(cx).closing_application {
                // Keep the window (and every session) alive in the background.
                view_clone.update(cx, |this, cx| {
                    this.hide_to_tray(window, cx);
                });
                return false;
            }
            if view_clone.read(cx).has_dirty_documents() {
                view_clone.update(cx, |this, cx| {
                    this.request_application_close(window, cx);
                });
                return false;
            }
            view_clone.update(cx, |this, cx| {
                this.close_detached_windows_for_shutdown(cx);
                this.save_layout_state(window, cx);
            });
            true
        });

        cx.new(|cx| Root::new(view, window, cx))
    })
    .expect("failed to open window");
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::StartupConfig;
    use crate::session::config::ConfigStore;

    #[test]
    fn startup_config_failure_uses_non_persistent_mode_and_keeps_error() {
        let startup = StartupConfig::from_result(Err(anyhow!("system secure storage is locked")));

        assert!(!startup.config.is_persistent());
        assert!(
            startup
                .error
                .as_deref()
                .is_some_and(|error| error.contains("locked"))
        );
    }

    #[test]
    fn startup_config_default_tmp_dir_does_not_require_loading_encrypted_config() {
        let path = ConfigStore::default_tmp_dir().expect("resolve default temporary directory");

        assert!(path.ends_with(std::path::Path::new("jshell").join("tmp")));
    }
}
