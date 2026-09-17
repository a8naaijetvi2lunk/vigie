//! Démarrage automatique avec Windows via `HKCU\...\Run`.
//!
//! La cible est toujours le binaire de **release**, même quand l'app tourne
//! actuellement en mode debug (dev) : on ne veut pas qu'un raccourci de démarrage
//! Windows lance un build de développement.

use crate::config::Config;
use std::path::PathBuf;
use winreg::enums::*;
use winreg::RegKey;

const RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Vigie";

/// Remplace un segment `target\debug` (ou `target/debug`) par `target\release`
/// (resp. `target/release`) dans un chemin. Laisse tout autre chemin inchangé.
/// Fonction pure, testable sans accès disque ni registre.
pub fn debug_to_release(path: &str) -> String {
    path.replace(r"target\debug", r"target\release")
        .replace("target/debug", "target/release")
}

/// Chemin de l'exécutable à enregistrer pour l'autostart : dérivé de
/// `current_exe()`, forcé vers le build release.
fn target_exe() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    let as_str = current.to_string_lossy().to_string();
    Some(PathBuf::from(debug_to_release(&as_str)))
}

/// Ajoute (ou remplace) la valeur `Vigie` dans `HKCU\...\Run`. Échec silencieux.
pub fn enable() {
    let Some(exe) = target_exe() else {
        return;
    };
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok((key, _)) = hkcu.create_subkey(RUN_KEY_PATH) {
        let quoted = format!("\"{}\"", exe.display());
        let _ = key.set_value(VALUE_NAME, &quoted);
    }
}

/// Supprime la valeur `Vigie` de `HKCU\...\Run` si présente. Échec silencieux.
pub fn disable() {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey_with_flags(RUN_KEY_PATH, KEY_SET_VALUE) {
        let _ = key.delete_value(VALUE_NAME);
    }
}

/// Aligne l'état du registre sur `config.autostart`.
pub fn reconcile(config: &Config) {
    if config.autostart {
        enable();
    } else {
        disable();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_debug_target_to_release_backslash() {
        let debug_path = r"C:\Outils\vigie\src-tauri\target\debug\vigie.exe";
        let expected = r"C:\Outils\vigie\src-tauri\target\release\vigie.exe";
        assert_eq!(debug_to_release(debug_path), expected);
    }

    #[test]
    fn maps_debug_target_to_release_forward_slash() {
        let debug_path = "C:/Outils/vigie/src-tauri/target/debug/vigie.exe";
        let expected = "C:/Outils/vigie/src-tauri/target/release/vigie.exe";
        assert_eq!(debug_to_release(debug_path), expected);
    }

    #[test]
    fn leaves_release_path_unchanged() {
        let release_path = r"C:\Outils\vigie\src-tauri\target\release\vigie.exe";
        assert_eq!(debug_to_release(release_path), release_path);
    }

    #[test]
    fn leaves_unrelated_path_unchanged() {
        let other = r"C:\Program Files\Vigie\vigie.exe";
        assert_eq!(debug_to_release(other), other);
    }
}
