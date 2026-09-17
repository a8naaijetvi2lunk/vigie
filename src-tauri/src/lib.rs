mod autostart;
mod config;
mod history;
mod instance;
mod notifier;
mod providers;
mod sessions;
mod state;
mod tray_status;
mod util;

use std::sync::{Arc, Mutex};
use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager, WindowEvent};

use config::{Config, ConfigState};
use notifier::NotifierState;
use state::{ProviderSnapshot, Snapshot};

pub struct TrayChecks(Vec<(&'static str, CheckMenuItem<tauri::Wry>)>);

pub fn sync_tray_config(app: &tauri::AppHandle, config: &Config) {
    if let Some(checks) = app.try_state::<TrayChecks>() {
        for (key, item) in &checks.0 {
            let checked = match *key {
                "hud" => config.hud,
                "always_on_top" => config.always_on_top,
                "pause_notifications" => config.notifications_paused,
                "autostart" => config.autostart,
                _ => config.click_through,
            };
            let _ = item.set_checked(checked);
        }
    }
}

pub fn record_snapshot(app: &tauri::AppHandle, snapshot: &Snapshot) {
    if let Some(state) = app.try_state::<history::HistoryState>() {
        let conn = state.0.lock().unwrap();
        let now = util::now_epoch();
        history::record(&conn, snapshot, now);
        history::prune(&conn, now);
    }
}

fn show_sessions(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("sessions") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[tauri::command]
fn open_sessions(app: tauri::AppHandle) {
    show_sessions(&app);
}

/// État partagé de l'application (managé via `app.manage(...)`).
pub struct AppState(pub Arc<Mutex<Snapshot>>);

/// Position de la fenêtre `main` avant l'entrée en mode HUD, pour la restaurer à la
/// sortie. En RAM uniquement : si l'app démarre directement en HUD, il n'y a rien à
/// restaurer et `window-state` fait foi.
pub struct HudState(pub Mutex<Option<tauri::PhysicalPosition<i32>>>);

#[tauri::command]
fn get_snapshot(state: tauri::State<AppState>) -> Snapshot {
    state.0.lock().unwrap().clone()
}

/// Taille logique du HUD.
const HUD_SIZE: (f64, f64) = (232.0, 34.0);
/// Marge logique entre le HUD et les bords de l'écran.
const HUD_MARGIN: f64 = 12.0;

/// Coin d'ancrage → position PHYSIQUE de la fenêtre. Fonction pure, testable.
///
/// Tous les paramètres sont en pixels physiques : c'est indispensable pour rester
/// correct quand la mise à l'échelle Windows n'est pas à 100 % — cas courant sur les
/// petits écrans, précisément la cible de ce mode.
#[allow(clippy::too_many_arguments)]
pub fn hud_position(
    corner: &str,
    mon_x: i32,
    mon_y: i32,
    mon_w: u32,
    mon_h: u32,
    win_w: u32,
    win_h: u32,
    margin: u32,
) -> (i32, i32) {
    let left = mon_x + margin as i32;
    let right = mon_x + mon_w as i32 - win_w as i32 - margin as i32;
    let top = mon_y + margin as i32;
    let bottom = mon_y + mon_h as i32 - win_h as i32 - margin as i32;

    match config::normalize_hud_corner(corner) {
        "top-left" => (left, top),
        "bottom-left" => (left, bottom),
        "bottom-right" => (right, bottom),
        _ => (right, top),
    }
}

/// Bascule la fenêtre principale entre compact (320×180), étendu (420×620) et HUD
/// (232×34), en tailles logiques. 620 px contient le cas le plus haut.
/// En HUD, la position courante est mémorisée puis la fenêtre est ancrée au coin
/// configuré ; elle est restaurée à la sortie. Vue inconnue → compact.
#[tauri::command]
fn set_view(app: tauri::AppHandle, view: String) {
    // Une commande synchrone tourne sur le thread principal : y lire la fenêtre
    // (`outer_position`, `current_monitor`) attend la boucle d'événements et gèle
    // l'application. Un `std::thread` ne convient pas non plus, ces API Windows
    // exigent le thread principal. Le travail est donc posté sur la boucle.
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || set_view_inner(handle, view));
}

fn set_view_inner(app: tauri::AppHandle, view: String) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    let size = match view.as_str() {
        "expanded" => tauri::LogicalSize::new(420.0, 620.0),
        "hud" => tauri::LogicalSize::new(HUD_SIZE.0, HUD_SIZE.1),
        _ => tauri::LogicalSize::new(320.0, 180.0),
    };

    if view == "hud" {
        // Mémorise la position d'avant l'ancrage (une seule fois : ne pas écraser
        // une position déjà mémorisée par un re-rendu du front).
        if let Some(hud_state) = app.try_state::<HudState>() {
            let mut guard = hud_state.0.lock().unwrap();
            if guard.is_none() {
                *guard = window.outer_position().ok();
            }
        }
        let _ = window.set_size(size);
        anchor_hud(&app, &window);
        return;
    }

    let _ = window.set_size(size);
    // Sortie du HUD : restaure la position mémorisée, s'il y en a une.
    if let Some(hud_state) = app.try_state::<HudState>() {
        if let Some(position) = hud_state.0.lock().unwrap().take() {
            let _ = window.set_position(position);
        }
    }
}

/// Ancre la fenêtre au coin configuré de l'écran qui la porte. Silencieux si le
/// moniteur n'est pas déterminable (écran débranché à chaud) : le HUD reste alors
/// là où il est, sans erreur.
fn anchor_hud(app: &tauri::AppHandle, window: &tauri::WebviewWindow) {
    let corner = match app.try_state::<ConfigState>() {
        Some(state) => state.0.lock().unwrap().hud_corner.clone(),
        None => "top-right".to_string(),
    };

    let monitor = match window.current_monitor() {
        Ok(Some(m)) => m,
        _ => match window.primary_monitor() {
            Ok(Some(m)) => m,
            _ => return,
        },
    };

    let scale = monitor.scale_factor();
    let position = monitor.position();
    let size = monitor.size();
    let win_w = (HUD_SIZE.0 * scale).round() as u32;
    let win_h = (HUD_SIZE.1 * scale).round() as u32;
    let margin = (HUD_MARGIN * scale).round() as u32;

    let (x, y) = hud_position(
        &corner,
        position.x,
        position.y,
        size.width,
        size.height,
        win_w,
        win_h,
        margin,
    );
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
}

/// Ordre d'affichage fixe des providers : Claude d'abord, puis Codex, puis le reste.
fn provider_order(id: &str) -> u8 {
    match id {
        "claude" => 0,
        "codex" => 1,
        _ => 2,
    }
}

/// Remplace l'entrée du provider par `id` (ou l'insère si absente), sans toucher
/// aux autres providers, puis retrie et met à jour `fetched_at`. Utilisé par le
/// watcher Codex et le poller Claude pour fusionner leurs mises à jour respectives
/// dans l'état partagé sans s'écraser l'un l'autre.
pub fn upsert_provider(snapshot: &mut Snapshot, provider: ProviderSnapshot) {
    match snapshot.providers.iter_mut().find(|p| p.id == provider.id) {
        Some(existing) => *existing = provider,
        None => snapshot.providers.push(provider),
    }
    snapshot.providers.sort_by_key(|p| provider_order(&p.id));
    snapshot.fetched_at = util::now_epoch();
}

/// Applique `mutate` à la configuration managée, la persiste puis en applique les
/// effets de bord (fenêtre toujours au premier plan, autostart). Utilisé par les
/// entrées à cocher du menu tray.
fn update_config(app: &tauri::AppHandle, mutate: impl FnOnce(&mut Config)) {
    let Some(state) = app.try_state::<ConfigState>() else {
        return;
    };
    if let Err(error) = config::commit(app, &state, |mut current| {
        mutate(&mut current);
        Ok(current)
    }) {
        eprintln!("Vigie: configuration non enregistrée : {error}");
        sync_tray_config(app, &state.0.lock().unwrap());
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _instance = match instance::Instance::acquire() {
        Ok(Some(instance)) => instance,
        Ok(None) => {
            instance::activate_existing();
            return;
        }
        Err(error) => {
            eprintln!("Vigie: impossible de verrouiller l'instance : {error}");
            return;
        }
    };
    tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            set_view,
            config::get_config,
            config::patch_config,
            sessions::get_sessions,
            open_sessions,
            history::get_history,
            history::get_heatmap
        ])
        .setup(|app| {
            // Config chargée avant tout provider : pilote quels providers démarrer,
            // l'intervalle de poll Claude, et l'état initial du tray.
            let loaded_config = Config::load(app.handle());
            app.manage(ConfigState::new(loaded_config.clone()));
            app.manage(NotifierState::default());
            app.manage(history::init(app.handle()));
            app.manage(sessions::SessionsState::default());
            // Applique les effets de bord de la config chargée dès le démarrage
            // (always-on-top, click-through, autostart) — le reste est relu à la
            // volée par les fonctions concernées.
            config::apply(app.handle(), &loaded_config);

            let initial_snapshot = Snapshot::default();
            app.manage(AppState(Arc::new(Mutex::new(initial_snapshot.clone()))));
            app.manage(HudState(Mutex::new(None)));

            // HUD au démarrage : taille seule, pour éviter un flash en 320×180.
            // L'ancrage lit le moniteur, impossible avant le démarrage de la boucle
            // d'événements : le premier `set_view("hud")` du front s'en charge.
            if loaded_config.hud {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_size(tauri::LogicalSize::new(HUD_SIZE.0, HUD_SIZE.1));
                }
            }

            sessions::spawn(app.handle().clone());
            // Le poller dort quand Claude est désactivé et reprend à sa réactivation.
            providers::claude::spawn_poller(app.handle().clone());
            let _ = app.emit("usage-updated", &initial_snapshot);

            let toggle_item =
                MenuItem::with_id(app, "toggle", "Afficher/Masquer", true, None::<&str>)?;
            let settings_item =
                MenuItem::with_id(app, "settings", "Réglages…", true, None::<&str>)?;
            let always_on_top_item = CheckMenuItem::with_id(
                app,
                "always_on_top",
                "Toujours au premier plan",
                true,
                loaded_config.always_on_top,
                None::<&str>,
            )?;
            let pause_notifications_item = CheckMenuItem::with_id(
                app,
                "pause_notifications",
                "Pause notifications",
                true,
                loaded_config.notifications_paused,
                None::<&str>,
            )?;
            let autostart_item = CheckMenuItem::with_id(
                app,
                "autostart",
                "Démarrer avec Windows",
                true,
                loaded_config.autostart,
                None::<&str>,
            )?;
            let click_through_item = CheckMenuItem::with_id(
                app,
                "click_through",
                "Traverser les clics",
                true,
                loaded_config.click_through,
                None::<&str>,
            )?;
            let hud_item = CheckMenuItem::with_id(
                app,
                "hud",
                "Mode HUD",
                true,
                loaded_config.hud,
                None::<&str>,
            )?;
            let quit_item = MenuItem::with_id(app, "quit", "Quitter", true, None::<&str>)?;
            let sessions_item =
                MenuItem::with_id(app, "sessions", "Sessions actives…", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &toggle_item,
                    &sessions_item,
                    &settings_item,
                    &always_on_top_item,
                    &pause_notifications_item,
                    &autostart_item,
                    &click_through_item,
                    &hud_item,
                    &quit_item,
                ],
            )?;

            // Clones capturés par la closure d'event pour relire l'état visuel
            // (déjà basculé nativement par muda au clic) et le refléter en config.
            let always_on_top_item_for_event = always_on_top_item.clone();
            let pause_notifications_item_for_event = pause_notifications_item.clone();
            let autostart_item_for_event = autostart_item.clone();
            let click_through_item_for_event = click_through_item.clone();
            let hud_item_for_event = hud_item.clone();
            app.manage(TrayChecks(vec![
                ("hud", hud_item),
                ("always_on_top", always_on_top_item),
                ("pause_notifications", pause_notifications_item),
                ("autostart", autostart_item),
                ("click_through", click_through_item),
            ]));

            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        tauri::tray::TrayIconEvent::Click {
                            button: tauri::tray::MouseButton::Left,
                            button_state: tauri::tray::MouseButtonState::Up,
                            ..
                        }
                    ) {
                        show_sessions(tray.app_handle());
                    }
                })
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "sessions" => show_sessions(app),
                    "toggle" => {
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                    "settings" => {
                        // Fenêtre créée cachée au démarrage ; le hide-on-close global
                        // la masque à la fermeture → ici on la réaffiche + focus.
                        if let Some(window) = app.get_webview_window("settings") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "always_on_top" => {
                        let checked = always_on_top_item_for_event.is_checked().unwrap_or(true);
                        update_config(app, |c| c.always_on_top = checked);
                    }
                    "pause_notifications" => {
                        let checked = pause_notifications_item_for_event
                            .is_checked()
                            .unwrap_or(false);
                        update_config(app, |c| c.notifications_paused = checked);
                    }
                    "autostart" => {
                        let checked = autostart_item_for_event.is_checked().unwrap_or(true);
                        update_config(app, |c| c.autostart = checked);
                    }
                    "click_through" => {
                        // Le tray reste cliquable même en click-through (il ne fait pas
                        // partie de la fenêtre "main"), donc c'est le seul moyen de
                        // désactiver l'état une fois activé.
                        let checked = click_through_item_for_event.is_checked().unwrap_or(false);
                        update_config(app, |c| c.click_through = checked);
                    }
                    "hud" => {
                        // Seule sortie du mode HUD quand clickThrough est actif : la
                        // fenêtre ne reçoit plus les clics, le tray si.
                        let checked = hud_item_for_event.is_checked().unwrap_or(false);
                        update_config(app, |c| c.hud = checked);
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;
            tray_status::spawn(app.handle().clone());
            instance::listen(app.handle().clone());

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    // Écran principal 1920×1080 à l'origine, HUD 220×34, marge 12 (tout en physique).
    const MON: (i32, i32, u32, u32) = (0, 0, 1920, 1080);
    const WIN: (u32, u32) = (220, 34);

    #[test]
    fn hud_position_covers_the_four_corners() {
        let (x, y, w, h) = MON;
        assert_eq!(
            hud_position("top-left", x, y, w, h, WIN.0, WIN.1, 12),
            (12, 12)
        );
        assert_eq!(
            hud_position("top-right", x, y, w, h, WIN.0, WIN.1, 12),
            (1688, 12)
        );
        assert_eq!(
            hud_position("bottom-left", x, y, w, h, WIN.0, WIN.1, 12),
            (12, 1034)
        );
        assert_eq!(
            hud_position("bottom-right", x, y, w, h, WIN.0, WIN.1, 12),
            (1688, 1034)
        );
    }

    #[test]
    fn hud_position_respects_a_secondary_monitor_origin() {
        // Écran secondaire placé à droite du principal, origine x = 1920.
        let (x, y, w, h) = (1920, 0, 1280, 720);
        assert_eq!(
            hud_position("top-right", x, y, w, h, WIN.0, WIN.1, 12),
            (2968, 12)
        );
        assert_eq!(
            hud_position("bottom-left", x, y, w, h, WIN.0, WIN.1, 12),
            (1932, 674)
        );
    }

    #[test]
    fn hud_position_works_with_scaled_physical_sizes() {
        // Écran 150 % : 2880×1620 physiques, HUD 330×51 physiques, marge 18.
        assert_eq!(
            hud_position("top-right", 0, 0, 2880, 1620, 330, 51, 18),
            (2532, 18)
        );
    }

    #[test]
    fn hud_position_falls_back_to_top_right_on_unknown_corner() {
        let (x, y, w, h) = MON;
        assert_eq!(
            hud_position("nawak", x, y, w, h, WIN.0, WIN.1, 12),
            hud_position("top-right", x, y, w, h, WIN.0, WIN.1, 12)
        );
    }
}
