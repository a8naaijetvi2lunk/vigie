//! Sources de quotas, et cache disque de leur dernière mesure connue.
pub mod claude;
pub mod codex;

use crate::state::QuotaWindow;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

// --- Cache disque -----------------------------------------------------------
// Dernière mesure d'un provider, affichée dès le démarrage. Claude en a besoin face au
// 429 ; Codex parce que l'événement le plus récent de son seau principal peut ne figurer
// ni en tête ni dans le dernier Mio des rollouts relus au démarrage.

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFile {
    windows: Vec<CacheWindow>,
    data_ts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheWindow {
    kind: String,
    used_percent: f64,
    resets_at: i64,
}

impl From<&QuotaWindow> for CacheWindow {
    fn from(w: &QuotaWindow) -> Self {
        CacheWindow {
            kind: w.kind.clone(),
            used_percent: w.used_percent,
            resets_at: w.resets_at,
        }
    }
}

impl From<CacheWindow> for QuotaWindow {
    fn from(w: CacheWindow) -> Self {
        QuotaWindow {
            kind: w.kind,
            used_percent: w.used_percent,
            resets_at: w.resets_at,
        }
    }
}

fn cache_path(app: &AppHandle, filename: &str) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    Some(dir.join(filename))
}

/// Charge la dernière donnée connue depuis le cache disque, si présente et lisible.
pub fn load_cache(app: &AppHandle, filename: &str) -> Option<(Vec<QuotaWindow>, i64)> {
    let path = cache_path(app, filename)?;
    let content = fs::read_to_string(path).ok()?;
    let cache: CacheFile = serde_json::from_str(&content).ok()?;
    let windows = cache.windows.into_iter().map(QuotaWindow::from).collect();
    Some((windows, cache.data_ts))
}

/// Écrit le cache disque après une mesure réussie. Échec silencieux (non bloquant).
pub fn save_cache(app: &AppHandle, filename: &str, windows: &[QuotaWindow], data_ts: i64) {
    let Some(path) = cache_path(app, filename) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let cache = CacheFile {
        windows: windows.iter().map(CacheWindow::from).collect(),
        data_ts,
    };
    if let Ok(json) = serde_json::to_string(&cache) {
        let _ = fs::write(path, json);
    }
}
