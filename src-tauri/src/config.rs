//! Configuration persistée de l'application (`config.json` dans `app_config_dir()`).
//!
//! Chargement tolérant : tout champ absent ou fichier absent retombe sur les valeurs
//! par défaut (`#[serde(default = ...)]`), afin qu'une future version puisse ajouter
//! des champs sans casser la lecture d'un `config.json` existant.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

const CONFIG_FILENAME: &str = "config.json";

fn default_true() -> bool {
    true
}

fn default_thresholds() -> Vec<u8> {
    vec![70, 85, 95]
}

fn default_poll_interval_secs() -> u64 {
    300
}

fn default_opacity() -> f64 {
    1.0
}

fn default_skin() -> String {
    "altimetre".to_string()
}

fn default_theme() -> String {
    "auto".to_string()
}

fn default_hud_corner() -> String {
    "top-right".to_string()
}

/// Activation par provider (permet de couper Claude ou Codex sans désinstaller).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersConfig {
    #[serde(default = "default_true")]
    pub claude: bool,
    #[serde(default = "default_true")]
    pub codex: bool,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        ProvidersConfig {
            claude: true,
            codex: true,
        }
    }
}

/// Configuration utilisateur de Vigie, persistée sur disque.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Seuils (en %) déclenchant une notification, triés croissant par convention.
    #[serde(default = "default_thresholds")]
    pub thresholds: Vec<u8>,
    /// Notifier aussi la réinitialisation d'une fenêtre de quota.
    #[serde(default)]
    pub reset_notifications: bool,
    /// Intervalle nominal (s) entre deux appels à l'API `oauth/usage` de Claude.
    #[serde(default = "default_poll_interval_secs")]
    pub claude_poll_interval_secs: u64,
    #[serde(default = "default_true")]
    pub always_on_top: bool,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub notifications_paused: bool,
    /// Opacité de la fenêtre.
    #[serde(default = "default_opacity")]
    pub opacity: f64,
    /// Fenêtre traversable par les clics souris : réversible via le tray.
    #[serde(default)]
    pub click_through: bool,
    /// Skin du widget (structure) : "altimetre" (défaut) ou "carnet".
    #[serde(default = "default_skin")]
    pub skin: String,
    /// Thème de couleurs : "auto" (suit l'OS), "light" ou "dark".
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Mode HUD minifié (une ligne ancrée dans un coin de l'écran). Persisté,
    /// contrairement au mode étendu qui reste un état front éphémère.
    #[serde(default)]
    pub hud: bool,
    /// Coin d'ancrage du HUD : "top-right" (défaut), "top-left", "bottom-right",
    /// "bottom-left".
    #[serde(default = "default_hud_corner")]
    pub hud_corner: String,
    /// Animations de présentation et du tray (les états restent lisibles sans elles).
    #[serde(default = "default_true")]
    pub animations: bool,
    #[serde(default)]
    pub providers: ProvidersConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            thresholds: default_thresholds(),
            reset_notifications: false,
            claude_poll_interval_secs: default_poll_interval_secs(),
            always_on_top: true,
            autostart: false,
            notifications_paused: false,
            opacity: default_opacity(),
            click_through: false,
            skin: default_skin(),
            theme: default_theme(),
            hud: false,
            hud_corner: default_hud_corner(),
            animations: true,
            providers: ProvidersConfig::default(),
        }
    }
}

fn config_path(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    Some(dir.join(CONFIG_FILENAME))
}

impl Config {
    /// Charge `config.json` depuis `app_config_dir()`. Si le fichier est absent ou
    /// illisible/invalide, écrit et retourne les valeurs par défaut. Ne panique jamais.
    pub fn load(app: &AppHandle) -> Config {
        if let Some(path) = config_path(app) {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(config) = serde_json::from_str::<Config>(&content) {
                    return config;
                }
            }
        }
        let defaults = Config::default();
        let _ = defaults.save(app);
        defaults
    }

    /// Écrit complètement un fichier voisin avant de remplacer la configuration.
    pub fn save(&self, app: &AppHandle) -> Result<(), String> {
        let path = config_path(app).ok_or("Dossier de configuration indisponible")?;
        self.save_to(&path)
    }

    fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let temporary = path.with_extension("json.tmp");
        let result = (|| -> std::io::Result<()> {
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.map_err(|e| e.to_string())
    }
}

/// Normalise le coin d'ancrage persisté : toute valeur inconnue retombe sur
/// "top-right". Fonction pure, utilisée par le positionnement du HUD.
pub fn normalize_hud_corner(corner: &str) -> &'static str {
    match corner {
        "top-left" => "top-left",
        "bottom-right" => "bottom-right",
        "bottom-left" => "bottom-left",
        _ => "top-right",
    }
}

/// État managé Tauri : configuration courante, partagée entre commandes IPC,
/// menu tray et logique de notification.
pub struct ConfigState(pub Arc<Mutex<Config>>);

impl ConfigState {
    pub fn new(config: Config) -> Self {
        ConfigState(Arc::new(Mutex::new(config)))
    }
}

/// Applique les effets de bord d'une nouvelle configuration : fenêtre toujours au
/// premier plan, click-through souris et entrée de démarrage automatique Windows.
/// Le reste (seuils, pause notifications, intervalle de poll, opacité, providers
/// actifs) est relu à la volée par les fonctions concernées (ou par le front via
/// `get_config`/l'event `config-updated`), sans action immédiate nécessaire ici.
pub fn apply(app: &AppHandle, config: &Config) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_always_on_top(config.always_on_top);
        let _ = window.set_ignore_cursor_events(config.click_through);
    }
    crate::autostart::reconcile(config);
}

#[tauri::command]
pub fn get_config(state: tauri::State<ConfigState>) -> Config {
    state.0.lock().unwrap().clone()
}

/// Fusion sous le même verrou que la sauvegarde : deux fenêtres ne perdent pas
/// leurs modifications indépendantes. Les champs providers sont fusionnés aussi.
pub fn merge_patch(current: &Config, patch: serde_json::Value) -> Result<Config, String> {
    let mut value = serde_json::to_value(current).map_err(|e| e.to_string())?;
    let object = patch
        .as_object()
        .ok_or("Configuration partielle invalide")?;
    let target = value.as_object_mut().unwrap();
    for (key, val) in object {
        if !target.contains_key(key) {
            return Err(format!("Réglage inconnu : {key}"));
        }
        if key == "providers" {
            let providers = val.as_object().ok_or("Providers invalides")?;
            for (id, enabled) in providers {
                if id != "claude" && id != "codex" {
                    return Err("Provider inconnu".into());
                }
                target.get_mut(key).unwrap()[id] = enabled.clone();
            }
        } else {
            target.insert(key.clone(), val.clone());
        }
    }
    let mut result: Config = serde_json::from_value(value).map_err(|e| e.to_string())?;
    result.thresholds.retain(|t| (1..=100).contains(t));
    result.thresholds.sort_unstable();
    result.thresholds.dedup();
    if result.thresholds.is_empty() {
        result.thresholds = default_thresholds();
    }
    result.opacity = result.opacity.clamp(0.3, 1.0);
    result.claude_poll_interval_secs = result.claude_poll_interval_secs.clamp(300, 86400);
    result.hud_corner = normalize_hud_corner(&result.hud_corner).into();
    Ok(result)
}

pub fn commit(
    app: &AppHandle,
    state: &ConfigState,
    mutate: impl FnOnce(Config) -> Result<Config, String>,
) -> Result<Config, String> {
    let updated = {
        let mut guard = state.0.lock().map_err(|_| "Configuration indisponible")?;
        let next = merge_patch(&mutate(guard.clone())?, serde_json::json!({}))?;
        next.save(app)?;
        *guard = next.clone();
        next
    };
    apply(app, &updated);
    crate::sync_tray_config(app, &updated);
    let _ = app.emit("config-updated", &updated);
    Ok(updated)
}

#[tauri::command]
pub fn patch_config(
    app: AppHandle,
    state: tauri::State<ConfigState>,
    patch: serde_json::Value,
) -> Result<Config, String> {
    commit(&app, &state, |current| merge_patch(&current, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_preserve_independent_changes_and_nested_providers() {
        let current = merge_patch(
            &Config::default(),
            serde_json::json!({"theme":"dark", "providers":{"codex":false}}),
        )
        .unwrap();
        let next = merge_patch(
            &current,
            serde_json::json!({"opacity":0.8, "providers":{"claude":false}}),
        )
        .unwrap();
        assert_eq!(next.theme, "dark");
        assert_eq!(next.opacity, 0.8);
        assert!(!next.providers.claude && !next.providers.codex);
        assert!(merge_patch(&next, serde_json::json!({"theme":null})).is_err());
        assert!(merge_patch(&next, serde_json::json!({"providers":{"unknown":false}})).is_err());
    }

    #[test]
    fn persisted_configuration_can_be_replaced_without_partial_json() {
        let path =
            std::env::temp_dir().join(format!("vigie-config-test-{}.json", std::process::id()));
        let initial = Config::default();
        initial.save_to(&path).unwrap();
        let next =
            merge_patch(&initial, serde_json::json!({"hud":true,"animations":false})).unwrap();
        next.save_to(&path).unwrap();
        let actual: Config =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(actual, next);
        assert!(!path.with_extension("json.tmp").exists());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn defaults_match_spec() {
        let c = Config::default();
        assert_eq!(c.thresholds, vec![70, 85, 95]);
        assert!(!c.reset_notifications);
        assert_eq!(c.claude_poll_interval_secs, 300);
        assert!(c.always_on_top);
        assert!(!c.autostart);
        assert!(!c.notifications_paused);
        assert_eq!(c.opacity, 1.0);
        assert!(!c.click_through);
        assert_eq!(c.skin, "altimetre");
        assert_eq!(c.theme, "auto");
        assert!(!c.hud);
        assert_eq!(c.hud_corner, "top-right");
        assert!(c.providers.claude);
        assert!(c.providers.codex);
    }

    #[test]
    fn hud_defaults_are_off_and_top_right() {
        let c = Config::default();
        assert!(!c.hud);
        assert_eq!(c.hud_corner, "top-right");
    }

    #[test]
    fn normalizes_unknown_hud_corner_to_top_right() {
        assert_eq!(normalize_hud_corner("top-left"), "top-left");
        assert_eq!(normalize_hud_corner("bottom-right"), "bottom-right");
        assert_eq!(normalize_hud_corner("bottom-left"), "bottom-left");
        assert_eq!(normalize_hud_corner("top-right"), "top-right");
        assert_eq!(normalize_hud_corner("n'importe quoi"), "top-right");
        assert_eq!(normalize_hud_corner(""), "top-right");
    }

    #[test]
    fn loads_legacy_config_without_hud_fields() {
        // Un config.json écrit par une version antérieure : les champs hud sont absents.
        let json = r#"{"thresholds":[70,85,95],"resetNotifications":false,
            "claudePollIntervalSecs":300,"alwaysOnTop":true,"autostart":true,
            "notificationsPaused":false,"opacity":1.0,"clickThrough":false,
            "skin":"altimetre","theme":"auto","providers":{"claude":true,"codex":true}}"#;
        let c: Config =
            serde_json::from_str(json).expect("un config.json ancien doit rester lisible");
        assert!(!c.hud);
        assert_eq!(c.hud_corner, "top-right");
    }

    #[test]
    fn round_trips_through_json_unchanged() {
        let c = Config::default();
        let json = serde_json::to_string(&c).expect("sérialisation");
        let back: Config = serde_json::from_str(&json).expect("désérialisation");
        assert_eq!(back, c);
    }

    #[test]
    fn tolerates_partial_json_via_serde_defaults() {
        let partial = r#"{"thresholds":[50,90],"autostart":false}"#;
        let c: Config = serde_json::from_str(partial).expect("JSON partiel doit parser");
        assert_eq!(c.thresholds, vec![50, 90]);
        assert!(!c.autostart);
        // Champs absents du JSON : valeurs par défaut.
        assert!(c.always_on_top);
        assert_eq!(c.claude_poll_interval_secs, 300);
        assert_eq!(c.skin, "altimetre");
        assert_eq!(c.theme, "auto");
        assert!(c.providers.claude);
        assert!(c.providers.codex);
    }

    #[test]
    fn tolerates_empty_json_object() {
        let c: Config = serde_json::from_str("{}").expect("objet vide doit parser");
        assert_eq!(c, Config::default());
    }

    #[test]
    fn serializes_with_camel_case_field_names() {
        let json = serde_json::to_string(&Config::default()).unwrap();
        assert!(json.contains("\"claudePollIntervalSecs\""));
        assert!(json.contains("\"resetNotifications\""));
        assert!(json.contains("\"alwaysOnTop\""));
        assert!(json.contains("\"notificationsPaused\""));
        assert!(json.contains("\"thresholds\""));
        assert!(json.contains("\"opacity\""));
        assert!(json.contains("\"clickThrough\""));
        assert!(json.contains("\"skin\""));
        assert!(json.contains("\"theme\""));
        assert!(json.contains("\"providers\""));
    }
}
