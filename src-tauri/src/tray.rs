use std::{collections::HashMap, sync::Arc};

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

pub struct TauriEventSink {
    app: AppHandle,
    database: Database,
    last_notifications: Mutex<HashMap<(String, QualityState), i64>>,
}

impl TauriEventSink {
    pub fn new(app: AppHandle, database: Database) -> Arc<Self> {
        Arc::new(Self {
            app,
            database,
            last_notifications: Mutex::new(HashMap::new()),
        })
    }

    fn notify_transition(&self, event: &QualityTransitionEvent) {
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

        let language = active_language(settings.language);
        let key = match event.transition.to {
            QualityState::Unstable => "notification.unstable",
            QualityState::Disconnected => "notification.disconnected",
            QualityState::Stable => "notification.recovered",
            _ => return,
        };
        let body = message(language, key, &[("name", &event.target.name)]);
        let _ = self
            .app
            .notification()
            .builder()
            .title("LNPM")
            .body(body)
            .show();
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
        self.notify_transition(&event);
    }

    fn monitor_error(&self, target_id: Option<&str>, code: &str, detail: &str) {
        let _ = self.app.emit(
            "monitor-error",
            serde_json::json!({ "targetId": target_id, "code": code, "detail": detail }),
        );
    }
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
    let color = match state {
        QualityState::Stable => [40, 199, 111, 255],
        QualityState::Unstable => [245, 158, 11, 255],
        QualityState::Disconnected | QualityState::Error => [239, 68, 68, 255],
        _ => [107, 114, 128, 255],
    };
    let mut rgba = vec![0_u8; 32 * 32 * 4];
    for y in 0..32_i32 {
        for x in 0..32_i32 {
            let distance = (x - 16) * (x - 16) + (y - 16) * (y - 16);
            if distance <= 14 * 14 {
                put_pixel(&mut rgba, x, y, color);
            }
        }
    }
    let wave = [
        (5, 17),
        (9, 17),
        (12, 12),
        (15, 21),
        (19, 9),
        (22, 17),
        (27, 17),
    ];
    for segment in wave.windows(2) {
        draw_line(&mut rgba, segment[0], segment[1], [255, 255, 255, 255]);
    }
    Image::new_owned(rgba, 32, 32)
}

fn draw_line(rgba: &mut [u8], from: (i32, i32), to: (i32, i32), color: [u8; 4]) {
    let steps = (to.0 - from.0).abs().max((to.1 - from.1).abs()).max(1);
    for step in 0..=steps {
        let x = from.0 + (to.0 - from.0) * step / steps;
        let y = from.1 + (to.1 - from.1) * step / steps;
        put_pixel(rgba, x, y, color);
        put_pixel(rgba, x, y + 1, color);
    }
}

fn put_pixel(rgba: &mut [u8], x: i32, y: i32, color: [u8; 4]) {
    if !(0..32).contains(&x) || !(0..32).contains(&y) {
        return;
    }
    let index = (y as usize * 32 + x as usize) * 4;
    rgba[index..index + 4].copy_from_slice(&color);
}

#[allow(dead_code)]
fn _transition_is_recovery(transition: &StateTransition) -> bool {
    transition.to == QualityState::Stable
        && matches!(
            transition.from,
            QualityState::Unstable | QualityState::Disconnected
        )
}
