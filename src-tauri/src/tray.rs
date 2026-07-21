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

use crate::{
    domain::{
        AppSettings, DashboardSnapshot, QualityState, QualityTransitionEvent, StateTransition,
        unix_time_ms,
    },
    i18n::{Language, active_language, message, state_label, target_count, text},
    monitor::MonitorEventSink,
    storage::Database,
};

const TRAY_ID: &str = "lnpm-tray";
const NOTIFICATION_BATCH_WINDOW: Duration = Duration::from_millis(2_500);
static TRAY_ICON_SOURCE: OnceLock<(Vec<u8>, u32, u32)> = OnceLock::new();

#[derive(Debug, Clone)]
struct PendingNotification {
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
    database: Database,
    last_notifications: Mutex<HashMap<(String, QualityState), i64>>,
    pending_notifications: Arc<Mutex<PendingNotificationBatch>>,
}

impl TauriEventSink {
    pub fn new(app: AppHandle, database: Database) -> Arc<Self> {
        Arc::new(Self {
            app,
            database,
            last_notifications: Mutex::new(HashMap::new()),
            pending_notifications: Arc::new(Mutex::new(PendingNotificationBatch::default())),
        })
    }

    fn queue_notification(&self, event: &QualityTransitionEvent) {
        let Ok(settings) = self.database.load_settings() else {
            return;
        };
        let should_notify = matches!(
            event.transition.to,
            QualityState::Unstable | QualityState::Disconnected | QualityState::Stable
        ) && settings.notifications_enabled;
        if !should_notify {
            return;
        }

        let key = (event.target.id.clone(), event.transition.to);
        let now_ms = unix_time_ms();
        let mut notifications = self.last_notifications.lock();
        if notifications
            .get(&key)
            .is_some_and(|last| now_ms - *last < 15 * 60 * 1_000)
        {
            return;
        }
        notifications.insert(key, now_ms);
        drop(notifications);

        let mut pending = self.pending_notifications.lock();
        pending.replace(event);
        if pending.scheduled {
            return;
        }
        pending.scheduled = true;
        drop(pending);

        let app = self.app.clone();
        let database = self.database.clone();
        let pending = Arc::clone(&self.pending_notifications);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(NOTIFICATION_BATCH_WINDOW).await;
            let transitions = pending.lock().drain();
            if transitions.is_empty() {
                return;
            }
            let Ok(settings) = database.load_settings() else {
                return;
            };
            if !settings.notifications_enabled {
                return;
            }
            let body = notification_body(active_language(settings.language), &transitions);
            let _ = app.notification().builder().title("LNPM").body(body).show();
        });
    }
}

impl MonitorEventSink for TauriEventSink {
    fn dashboard_updated(&self, snapshot: DashboardSnapshot) {
        let _ = self.app.emit("dashboard-updated", &snapshot);
        if let Ok(settings) = self.database.load_settings() {
            update_tray(&self.app, &snapshot, &settings);
        }
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
    let quit = MenuItem::with_id(app, "quit", text(language, "tray.quit"), true, None::<&str>)?;
    Menu::with_items(app, &[&quick, &open, &pause, &quit])
}

pub fn handle_menu_event(app: &AppHandle, id: &str) {
    match id {
        "quick" => show_popup_window(app, None),
        "open" => show_main_window(app),
        "pause" => {
            if let Some(state) = app.try_state::<crate::commands::AppState>() {
                state.monitor.set_paused(!state.monitor.snapshot().paused);
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
        WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
            api.prevent_close();
            let _ = window.hide();
        }
        WindowEvent::Resized(_) if window.label() == "main" => {
            if window.is_minimized().unwrap_or(false) {
                let _ = window.hide();
            }
        }
        WindowEvent::Focused(false) if window.label() == "popup" => {
            let _ = window.hide();
        }
        _ => {}
    }
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn show_popup_window(app: &AppHandle, cursor: Option<PhysicalPosition<f64>>) {
    let Some(window) = app.get_webview_window("popup") else {
        return;
    };
    if let Some(cursor) = cursor
        && let Ok(size) = window.outer_size()
    {
        let x = (cursor.x - size.width as f64 / 2.0).max(0.0) as i32;
        let y = if cursor.y < 200.0 {
            (cursor.y + 24.0) as i32
        } else {
            (cursor.y - size.height as f64 - 12.0).max(0.0) as i32
        };
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
    let _ = window.show();
    let _ = window.set_focus();
}

fn update_tray(app: &AppHandle, snapshot: &DashboardSnapshot, settings: &AppSettings) {
    let state = aggregate_state(snapshot);
    let language = active_language(settings.language);
    let state_text = if state == QualityState::WarmingUp {
        text(language, "tray.starting")
    } else {
        state_label(language, state)
    };
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_icon(Some(status_icon(state)));
        let _ = tray.set_tooltip(Some(format!(
            "LNPM · {state_text} · {}",
            target_count(language, snapshot.targets.len())
        )));
    }
}

fn aggregate_state(snapshot: &DashboardSnapshot) -> QualityState {
    if snapshot.paused || snapshot.targets.is_empty() {
        return QualityState::Paused;
    }
    snapshot
        .targets
        .iter()
        .map(|target| target.state)
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

    #[test]
    fn formats_one_transition_with_the_existing_specific_message() {
        let transitions = vec![PendingNotification {
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
                target_name: "Office".into(),
                state: QualityState::Unstable,
            },
            PendingNotification {
                target_name: "Google".into(),
                state: QualityState::Unstable,
            },
            PendingNotification {
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
