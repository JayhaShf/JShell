use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
};

use rust_i18n::t;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

#[cfg(not(target_os = "linux"))]
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconEvent};

pub(crate) enum TrayRequest {
    ToggleWindow,
    /// Always restore/activate the window, regardless of how it was minimized.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    ShowWindow,
    Quit,
}

const TOGGLE_WINDOW_ID: &str = "tray-toggle-window";
const QUIT_ID: &str = "tray-quit";

/// `muda`'s global event handler can only be installed once per process, so
/// requests are forwarded through a swappable sender that is replaced each
/// time the tray is recreated.
static REQUEST_TX: Mutex<Option<Sender<TrayRequest>>> = Mutex::new(None);

fn install_global_handlers() {
    use std::sync::Once;
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        MenuEvent::set_event_handler(Some(|event: MenuEvent| {
            let request = if event.id() == &MenuId::new(TOGGLE_WINDOW_ID) {
                Some(TrayRequest::ToggleWindow)
            } else if event.id() == &MenuId::new(QUIT_ID) {
                Some(TrayRequest::Quit)
            } else {
                None
            };
            if let Some(request) = request {
                send_request(request);
            }
        }));
        #[cfg(not(target_os = "linux"))]
        TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                // Left-click always restores the window so it never gets stuck
                // re-minimizing after the taskbar was used to minimize it.
                send_request(TrayRequest::ShowWindow);
            }
        }));
    });
}

fn send_request(request: TrayRequest) {
    if let Ok(mut tx) = REQUEST_TX.lock()
        && let Some(tx) = tx.as_mut()
    {
        let _ = tx.send(request);
    }
}

fn set_request_tx(tx: Sender<TrayRequest>) {
    *REQUEST_TX.lock().unwrap_or_else(|error| error.into_inner()) = Some(tx);
}

fn clear_request_tx() {
    *REQUEST_TX.lock().unwrap_or_else(|error| error.into_inner()) = None;
}

fn build_menu() -> anyhow::Result<Menu> {
    let toggle_item = MenuItem::with_id(
        MenuId::new(TOGGLE_WINDOW_ID),
        t!("tray_toggle_window"),
        true,
        None,
    );
    let quit_item = MenuItem::with_id(MenuId::new(QUIT_ID), t!("tray_quit"), true, None);
    let menu = Menu::new();
    menu.append(&toggle_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit_item)?;
    Ok(menu)
}

fn load_tray_icon() -> anyhow::Result<Icon> {
    let image = image::load_from_memory(include_bytes!("../../assets/icons/jshell.png"))
        .map_err(|error| anyhow::anyhow!("failed to decode tray icon: {error}"))?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height)
        .map_err(|error| anyhow::anyhow!("failed to build tray icon: {error}"))
}

/// Owns the platform tray icon and delivers user requests through a channel
/// that the main event pump drains on the UI thread.
pub(crate) struct TrayController {
    #[cfg(not(target_os = "linux"))]
    _icon: TrayIcon,
    rx: Receiver<TrayRequest>,
    #[cfg(target_os = "linux")]
    shutdown: Arc<AtomicBool>,
    #[cfg(target_os = "linux")]
    _thread: Option<std::thread::JoinHandle<()>>,
}

impl TrayController {
    #[cfg(not(target_os = "linux"))]
    pub(crate) fn new() -> anyhow::Result<Self> {
        install_global_handlers();
        let (tx, rx) = mpsc::channel();
        set_request_tx(tx);
        let icon = load_tray_icon()?;
        let icon = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("JShell")
            .with_menu(Box::new(build_menu()?))
            .build()
            .map_err(|error| anyhow::anyhow!("failed to create system tray icon: {error}"))?;
        Ok(Self { _icon: icon, rx })
    }

    /// On Linux the tray icon must live on the thread that pumps the GTK event
    /// loop, so it is created and owned by a dedicated thread.
    #[cfg(target_os = "linux")]
    pub(crate) fn new() -> anyhow::Result<Self> {
        install_global_handlers();
        let (tx, rx) = mpsc::channel();
        set_request_tx(tx);
        let icon = load_tray_icon()?;
        let (ready_tx, ready_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = shutdown.clone();
        let thread = std::thread::Builder::new()
            .name("jshell-tray".to_string())
            .spawn(move || {
                if !gtk::is_initialized()
                    && let Err(error) = gtk::init()
                {
                    let _ = ready_tx.send(Err(anyhow::anyhow!(
                        "failed to initialize GTK for the system tray: {error}"
                    )));
                    return;
                }
                let menu = match build_menu() {
                    Ok(menu) => menu,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                let icon = match TrayIconBuilder::new()
                    .with_icon(icon)
                    .with_tooltip("JShell")
                    .with_menu(Box::new(menu))
                    .build()
                {
                    Ok(icon) => icon,
                    Err(error) => {
                        let _ = ready_tx.send(Err(anyhow::anyhow!(
                            "failed to create system tray icon: {error}"
                        )));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                // Pump the thread's main context so both GTK and D-Bus sources
                // are processed: the StatusNotifier registration and menu events
                // are delivered over D-Bus, which `gtk::events_pending()` ignores.
                let main_context = gtk::glib::MainContext::default();
                while !thread_shutdown.load(Ordering::Relaxed) {
                    main_context.iteration(false);
                    std::thread::sleep(std::time::Duration::from_millis(8));
                }
                drop(icon);
            })
            .map_err(|error| anyhow::anyhow!("failed to spawn tray thread: {error}"))?;

        ready_rx
            .recv()
            .map_err(|_| anyhow::anyhow!("tray thread exited before creating the icon"))??;

        Ok(Self {
            rx,
            shutdown,
            _thread: Some(thread),
        })
    }

    pub(crate) fn try_recv(&self) -> Option<TrayRequest> {
        self.rx.try_recv().ok()
    }
}

impl Drop for TrayController {
    fn drop(&mut self) {
        clear_request_tx();
        #[cfg(target_os = "linux")]
        {
            self.shutdown.store(true, Ordering::Relaxed);
            if let Some(thread) = self._thread.take() {
                let _ = thread.join();
            }
        }
    }
}
