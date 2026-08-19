use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::Duration,
};

use parking_lot::Mutex;
use tauri::{
    App, AppHandle, Emitter, Manager, PhysicalPosition, Runtime, WindowEvent,
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::watch;

use crate::{
    commands::SharedSettings,
    domain::{
        AppSettings, DashboardSnapshot, QualityState, QualityTransitionEvent, StateTransition,
        unix_time_ms,
    },
    i18n::{Language, active_language, message, state_label, target_count, text},
    monitor::MonitorEventSink,
};

const TRAY_ID: &str = "lnpm-tray";
const NOTIFICATION_BATCH_WINDOW: Duration = Duration::from_millis(2_500);
const NOTIFICATION_REPEAT_GAP_MS: i64 = 15 * 60 * 1_000;
/// How long the tray writer waits for the tray icon to exist before looking again.
const TRAY_WAIT: Duration = Duration::from_millis(100);
static TRAY_ICON_SOURCE: OnceLock<(Vec<u8>, u32, u32)> = OnceLock::new();
/// The icon and tooltip the tray should show. A probe finishing every 200 ms would otherwise rebuild
/// and reassign both several times a second for no visible change.
static TRAY_UPDATES: OnceLock<watch::Sender<(QualityState, String)>> = OnceLock::new();

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowVisibility {
    label: String,
    visible: bool,
}

#[derive(Debug, Clone)]
struct PendingNotification {
    target_id: String,
    target_name: String,
    state: QualityState,
}

#[derive(Default)]
struct PendingNotificationBatch {
    scheduled: bool,
    transitions: HashMap<String, PendingNotification>,
}

impl PendingNotificationBatch {
    fn replace(&mut self, event: &QualityTransitionEvent) {
        self.transitions.insert(
            event.target.id.clone(),
            PendingNotification {
                target_id: event.target.id.clone(),
                target_name: event.target.name.clone(),
                state: event.transition.to,
            },
        );
    }

    fn drain(&mut self) -> Vec<PendingNotification> {
        self.scheduled = false;
        self.transitions.drain().map(|(_, event)| event).collect()
    }
}

pub struct TauriEventSink {
    app: AppHandle,
    settings: SharedSettings,
    last_notifications: Arc<Mutex<HashMap<(String, QualityState), i64>>>,
    pending_notifications: Arc<Mutex<PendingNotificationBatch>>,
}

impl TauriEventSink {
    pub fn new(app: AppHandle, settings: SharedSettings) -> Arc<Self> {
        Arc::new(Self {
            app,
            settings,
            last_notifications: Arc::new(Mutex::new(HashMap::new())),
            pending_notifications: Arc::new(Mutex::new(PendingNotificationBatch::default())),
        })
    }

    fn queue_notification(&self, event: &QualityTransitionEvent) {
        let settings = self.settings.lock().clone();
        let should_notify = matches!(
            event.transition.to,
            QualityState::Unstable | QualityState::Disconnected | QualityState::Stable
        ) && settings.notifications_enabled;
        if !should_notify {
            return;
        }

        let key = (event.target.id.clone(), event.transition.to);
        let now_ms = unix_time_ms();
        if self
            .last_notifications
            .lock()
            .get(&key)
            .is_some_and(|last| now_ms - *last < NOTIFICATION_REPEAT_GAP_MS)
        {
            return;
        }

        let mut pending = self.pending_notifications.lock();
        pending.replace(event);
        if pending.scheduled {
            return;
        }
        pending.scheduled = true;
        drop(pending);

        let app = self.app.clone();
        let shared_settings = Arc::clone(&self.settings);
        let pending = Arc::clone(&self.pending_notifications);
        let last_notifications = Arc::clone(&self.last_notifications);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(NOTIFICATION_BATCH_WINDOW).await;
            let transitions = pending.lock().drain();
            if transitions.is_empty() {
                return;
            }
            let settings = shared_settings.lock().clone();
            if !settings.notifications_enabled {
                return;
            }
            let body = notification_body(active_language(settings.language), &transitions);
            if app
                .notification()
                .builder()
                .title("LNPM")
                .body(body)
                .show()
                .is_err()
            {
                return;
            }
            // Only the states that were really shown may start their repeat gate. Recording it when
            // the notification was queued muted every state that the batch coalesced away.
            let shown_at_ms = unix_time_ms();
            let mut gate = last_notifications.lock();
            for transition in &transitions {
                gate.insert(
                    (transition.target_id.clone(), transition.state),
                    shown_at_ms,
                );
            }
            gate.retain(|_, last| shown_at_ms - *last < NOTIFICATION_REPEAT_GAP_MS);
        });
    }
}

impl MonitorEventSink for TauriEventSink {
    fn dashboard_updated(&self, snapshot: DashboardSnapshot) {
        let _ = self.app.emit("dashboard-updated", &snapshot);
        let settings = self.settings.lock().clone();
        update_tray(&self.app, &snapshot, &settings);
    }

    fn quality_transition(&self, event: QualityTransitionEvent) {
        let _ = self.app.emit("quality-transition", &event);
        self.queue_notification(&event);
    }

    fn monitor_error(&self, target_id: Option<&str>, code: &str, detail: &str) {
        let _ = self.app.emit(
            "monitor-error",
            serde_json::json!({ "targetId": target_id, "code": code, "detail": detail }),
        );
    }
}

fn notification_body(language: Language, transitions: &[PendingNotification]) -> String {
    if let [transition] = transitions {
        let key = match transition.state {
            QualityState::Unstable => "notification.unstable",
            QualityState::Disconnected => "notification.disconnected",
            QualityState::Stable => "notification.recovered",
            _ => return String::new(),
        };
        return message(language, key, &[("name", &transition.target_name)]);
    }

    let mut groups = Vec::new();
    for state in [
        QualityState::Disconnected,
        QualityState::Unstable,
        QualityState::Stable,
    ] {
        let mut names = transitions
            .iter()
            .filter(|transition| transition.state == state)
            .map(|transition| transition.target_name.as_str())
            .collect::<Vec<_>>();
        names.sort_by_key(|name| name.to_lowercase());
        if !names.is_empty() {
            groups.push(format!(
                "{} ({}): {}",
                state_label(language, state),
                names.len(),
                names.join(", ")
            ));
        }
    }
    let count = transitions.len().to_string();
    let items = groups.join("\n");
    message(
        language,
        "notification.multiple",
        &[("count", &count), ("items", &items)],
    )
}

pub fn build_tray<R: Runtime>(app: &App<R>, settings: &AppSettings) -> tauri::Result<()> {
    let language = active_language(settings.language);
    let menu = tray_menu(app, language)?;
    let tooltip = format!("LNPM · {}", text(language, "tray.starting"));
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(status_icon(QualityState::WarmingUp))
        .tooltip(tooltip)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .build(app)?;
    Ok(())
}

pub fn refresh_tray(
    app: &AppHandle,
    settings: &AppSettings,
    snapshot: &DashboardSnapshot,
) -> tauri::Result<()> {
    let language = active_language(settings.language);
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        tray.set_menu(Some(tray_menu(app, language)?))?;
    }
    update_tray(app, snapshot, settings);
    Ok(())
}

fn tray_menu<R: Runtime, M: Manager<R>>(app: &M, language: Language) -> tauri::Result<Menu<R>> {
    let quick = MenuItem::with_id(
        app,
        "quick",
        text(language, "tray.quickStatus"),
        true,
        None::<&str>,
    )?;
    let open = MenuItem::with_id(app, "open", text(language, "tray.open"), true, None::<&str>)?;
    let pause = MenuItem::with_id(
        app,
        "pause",
        text(language, "tray.pauseResume"),
        true,
        None::<&str>,
    )?;
    let check_updates = MenuItem::with_id(
        app,
        "check-updates",
        text(language, "tray.checkUpdates"),
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", text(language, "tray.quit"), true, None::<&str>)?;
    Menu::with_items(app, &[&quick, &open, &pause, &check_updates, &quit])
}

pub fn handle_menu_event(app: &AppHandle, id: &str) {
    match id {
        "quick" => show_popup_window(app, None),
        "open" => show_main_window(app),
        "pause" => {
            if let Some(state) = app.try_state::<crate::commands::AppState>() {
                // Pausing writes to the database, which must not happen on the event loop thread.
                let monitor = Arc::clone(&state.monitor);
                tauri::async_runtime::spawn_blocking(move || monitor.toggle_paused());
            }
        }
        "check-updates" => {
            if let Some(manager) = app.try_state::<crate::updater::UpdateManager>() {
                let manager = manager.inner().clone();
                tauri::async_runtime::spawn(async move { manager.check_manually().await });
            }
        }
        "quit" => {
            if app
                .try_state::<crate::updater::UpdateManager>()
                .is_some_and(|manager| manager.is_installing())
            {
                show_main_window(app);
            } else {
                app.exit(0);
            }
        }
        _ => {}
    }
}

pub fn handle_tray_event(app: &AppHandle, event: &TrayIconEvent) {
    match event {
        TrayIconEvent::Click {
            position,
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } => show_popup_window(app, Some(*position)),
        TrayIconEvent::DoubleClick {
            button: MouseButton::Left,
            ..
        } => show_main_window(app),
        _ => {}
    }
}

pub fn handle_window_event(window: &tauri::Window, event: &WindowEvent) {
    match event {
        WindowEvent::CloseRequested { api, .. }
            if window.label() == "main"
                && window
                    .app_handle()
                    .try_state::<crate::updater::UpdateManager>()
                    .is_some_and(|manager| manager.is_installing()) =>
        {
            api.prevent_close();
        }
        // Both windows live for the whole session behind the tray icon. Letting either be destroyed
        // (window chrome, Alt+F4) would leave the tray with nothing to show.
        WindowEvent::CloseRequested { api, .. }
            if window.label() == "main" || window.label() == "popup" =>
        {
            api.prevent_close();
            hide_window(window);
        }
        WindowEvent::Resized(_) if window.label() == "main" => {
            if window.is_minimized().unwrap_or(false) {
                hide_window(window);
            }
        }
        WindowEvent::Focused(false) if window.label() == "popup" => {
            hide_window(window);
        }
        _ => {}
    }
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        announce_visibility(&window, true);
    }
}

/// Tells a window's webview whether it is on screen. Hiding a Tauri window leaves the webview
/// running and still "visible" to the Page Visibility API, so the frontend cannot tell on its own
/// that its five-second refresh is going to waste.
pub fn announce_visibility<R: Runtime>(window: &tauri::WebviewWindow<R>, visible: bool) {
    // Addressed by label and carrying it in the payload: a plain `emit` fans out to every webview,
    // and both windows run the same frontend, so hiding the popup used to switch the main window
    // off. A label target matches window, webview and webview-window listeners alike, which a
    // `WebviewWindow` target would not.
    let label = window.label().to_string();
    let _ = window.emit_to(
        label.as_str(),
        "window-visibility",
        WindowVisibility {
            label: label.clone(),
            visible,
        },
    );
}

pub fn hide_window<R: Runtime>(window: &tauri::Window<R>) {
    let _ = window.hide();
    if let Some(webview) = window.get_webview_window(window.label()) {
        announce_visibility(&webview, false);
    }
}

pub fn show_popup_window(app: &AppHandle, cursor: Option<PhysicalPosition<f64>>) {
    let Some(window) = app.get_webview_window("popup") else {
        return;
    };
    if let Some(cursor) = cursor
        && let Ok(size) = window.outer_size()
    {
        let _ = window.set_position(popup_position(&window, cursor, size));
    }
    let _ = window.show();
    let _ = window.set_focus();
    announce_visibility(&window, true);
}

/// Places the popup next to the tray icon, kept inside the work area of the display the click
/// happened on. Clamping to `(0, 0)` instead put it on the primary monitor's top-left corner
/// whenever the tray sat on a second display or below a taskbar.
fn popup_position<R: Runtime>(
    window: &tauri::WebviewWindow<R>,
    cursor: PhysicalPosition<f64>,
    size: tauri::PhysicalSize<u32>,
) -> PhysicalPosition<i32> {
    let monitor = window
        .monitor_from_point(cursor.x, cursor.y)
        .ok()
        .flatten()
        .or_else(|| window.current_monitor().ok().flatten());
    let width = size.width as i32;
    let height = size.height as i32;
    let mut x = cursor.x as i32 - width / 2;
    let mut y = if let Some(monitor) = &monitor {
        let area = monitor.work_area();
        // Below the tray when it is at the top of the screen, above it otherwise.
        if cursor.y as i32 <= area.position.y + area.size.height as i32 / 2 {
            cursor.y as i32 + 24
        } else {
            cursor.y as i32 - height - 12
        }
    } else if cursor.y < 200.0 {
        cursor.y as i32 + 24
    } else {
        cursor.y as i32 - height - 12
    };
    if let Some(monitor) = &monitor {
        let area = monitor.work_area();
        let left = area.position.x;
        let top = area.position.y;
        let right = left + area.size.width as i32 - width;
        let bottom = top + area.size.height as i32 - height;
        x = x.clamp(left.min(right), right.max(left));
        y = y.clamp(top.min(bottom), bottom.max(top));
    } else {
        x = x.max(0);
        y = y.max(0);
    }
    PhysicalPosition::new(x, y)
}

fn update_tray(app: &AppHandle, snapshot: &DashboardSnapshot, settings: &AppSettings) {
    let state = aggregate_state(snapshot);
    let language = active_language(settings.language);
    let state_text = if state == QualityState::WarmingUp {
        text(language, "tray.starting")
    } else {
        state_label(language, state)
    };
    let tooltip = format!(
        "LNPM · {state_text} · {}",
        target_count(language, snapshot.targets.len())
    );
    // Handed to a single writer task rather than drawn here. `set_icon`/`set_tooltip` block until
    // the main thread services them, so holding a lock across them could deadlock the event loop,
    // and letting several probe threads draw concurrently could leave the icon a step behind. A
    // watch channel keeps only the newest value and one consumer applies it.
    let updates = TRAY_UPDATES.get_or_init(|| {
        let (sender, receiver) = watch::channel((state, tooltip.clone()));
        spawn_tray_writer(app.clone(), receiver);
        sender
    });
    updates.send_if_modified(|current| {
        let next = (state, tooltip);
        if *current == next {
            return false;
        }
        *current = next;
        true
    });
}

fn spawn_tray_writer(app: AppHandle, mut updates: watch::Receiver<(QualityState, String)>) {
    tauri::async_runtime::spawn(async move {
        loop {
            // The first status can be produced before the tray exists. Waiting instead of consuming
            // it matters, because an identical value is never sent twice.
            let Some(tray) = app.tray_by_id(TRAY_ID) else {
                tokio::time::sleep(TRAY_WAIT).await;
                continue;
            };
            let (state, tooltip) = updates.borrow_and_update().clone();
            let _ = tray.set_icon(Some(status_icon(state)));
            let _ = tray.set_tooltip(Some(&tooltip));
            if updates.changed().await.is_err() {
                return;
            }
        }
    });
}

fn aggregate_state(snapshot: &DashboardSnapshot) -> QualityState {
    if snapshot.paused || snapshot.targets.is_empty() {
        return QualityState::Paused;
    }
    // A single disabled target must not make the whole app report as paused: aggregate over the
    // targets that are actually being observed, and only fall back to Paused if none are.
    let mut observed = snapshot
        .targets
        .iter()
        .map(|target| target.state)
        .filter(|state| *state != QualityState::Paused)
        .peekable();
    if observed.peek().is_none() {
        return QualityState::Paused;
    }
    observed
        .max_by_key(|state| severity(*state))
        .unwrap_or(QualityState::WarmingUp)
}

fn severity(state: QualityState) -> u8 {
    match state {
        QualityState::Disconnected | QualityState::Error => 4,
        QualityState::Unstable => 3,
        QualityState::WarmingUp | QualityState::Unobserved => 2,
        QualityState::Paused => 1,
        QualityState::Stable => 0,
    }
}

fn status_icon(state: QualityState) -> Image<'static> {
    let (source, width, height) = TRAY_ICON_SOURCE.get_or_init(|| {
        let image = Image::from_bytes(include_bytes!("../icons/32x32.png"))
            .expect("embedded tray icon must be valid PNG");
        (image.rgba().to_vec(), image.width(), image.height())
    });
    let color = tray_icon_color(state);
    let mut rgba = source.clone();
    for pixel in rgba.chunks_exact_mut(4) {
        if pixel[3] > 0 {
            pixel[..3].copy_from_slice(&color);
        }
    }
    Image::new_owned(rgba, *width, *height)
}

fn tray_icon_color(state: QualityState) -> [u8; 3] {
    match state {
        QualityState::Stable => [23, 212, 196],
        QualityState::Unstable => [245, 158, 11],
        QualityState::Disconnected | QualityState::Error => [148, 163, 184],
        QualityState::Paused | QualityState::WarmingUp | QualityState::Unobserved => {
            [100, 116, 139]
        }
    }
}

#[allow(dead_code)]
fn _transition_is_recovery(transition: &StateTransition) -> bool {
    transition.to == QualityState::Stable
        && matches!(
            transition.from,
            QualityState::Unstable | QualityState::Disconnected
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{LiveTargetStatus, QualityMetrics, Target};

    fn status(state: QualityState) -> LiveTargetStatus {
        LiveTargetStatus {
            target: Target::new("Target", "1.1.1.1"),
            state,
            state_since_ms: 0,
            latest_sample: None,
            metrics: QualityMetrics::default(),
            reasons: Vec::new(),
        }
    }

    fn snapshot(states: &[QualityState]) -> DashboardSnapshot {
        DashboardSnapshot {
            now_ms: 0,
            paused: false,
            targets: states.iter().copied().map(status).collect(),
        }
    }

    #[test]
    fn a_disabled_target_does_not_report_the_whole_app_as_paused() {
        assert_eq!(
            aggregate_state(&snapshot(&[QualityState::Paused, QualityState::Stable])),
            QualityState::Stable
        );
        assert_eq!(
            aggregate_state(&snapshot(&[
                QualityState::Paused,
                QualityState::Stable,
                QualityState::Unstable
            ])),
            QualityState::Unstable
        );
        assert_eq!(
            aggregate_state(&snapshot(&[QualityState::Paused])),
            QualityState::Paused
        );
        assert_eq!(aggregate_state(&snapshot(&[])), QualityState::Paused);
    }

    #[test]
    fn formats_one_transition_with_the_existing_specific_message() {
        let transitions = vec![PendingNotification {
            target_id: "Google".into(),
            target_name: "Google".into(),
            state: QualityState::Unstable,
        }];

        assert_eq!(
            notification_body(Language::En, &transitions),
            "Google network quality is unstable"
        );
    }

    #[test]
    fn groups_multiple_transitions_into_one_localized_message() {
        let transitions = vec![
            PendingNotification {
                target_id: "Office".into(),
                target_name: "Office".into(),
                state: QualityState::Unstable,
            },
            PendingNotification {
                target_id: "Google".into(),
                target_name: "Google".into(),
                state: QualityState::Unstable,
            },
            PendingNotification {
                target_id: "Gateway".into(),
                target_name: "Gateway".into(),
                state: QualityState::Disconnected,
            },
        ];

        assert_eq!(
            notification_body(Language::En, &transitions),
            "3 network status changes\nDisconnected (1): Gateway\nUnstable (2): Google, Office"
        );
    }

    #[test]
    fn keeps_the_brand_silhouette_and_tints_it_for_each_tray_state() {
        let stable = status_icon(QualityState::Stable);
        let unstable = status_icon(QualityState::Unstable);
        let disconnected = status_icon(QualityState::Disconnected);

        assert_eq!((stable.width(), stable.height()), (32, 32));
        assert_eq!(
            stable
                .rgba()
                .chunks_exact(4)
                .map(|pixel| pixel[3])
                .collect::<Vec<_>>(),
            unstable
                .rgba()
                .chunks_exact(4)
                .map(|pixel| pixel[3])
                .collect::<Vec<_>>()
        );
        assert!(
            stable.rgba().chunks_exact(4).any(|pixel| {
                pixel[3] > 0 && pixel[..3] == tray_icon_color(QualityState::Stable)
            })
        );
        assert!(unstable.rgba().chunks_exact(4).any(|pixel| {
            pixel[3] > 0 && pixel[..3] == tray_icon_color(QualityState::Unstable)
        }));
        assert!(disconnected.rgba().chunks_exact(4).any(|pixel| {
            pixel[3] > 0 && pixel[..3] == tray_icon_color(QualityState::Disconnected)
        }));
    }
}
