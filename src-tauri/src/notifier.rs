//! Notifications système (seuils de quota franchis, reset de fenêtre).
//!
//! La décision anti-spam (`evaluate_snapshot` / `evaluate_window`) est pure et
//! testable ; `check_and_notify` se charge de l'envoi réel.

use crate::config::{Config, ConfigState};
use crate::state::{ProviderSnapshot, QuotaWindow, Snapshot};
use chrono::{Local, TimeZone};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

/// Clé d'une notification de seuil déjà envoyée : (provider, kind, resetsAt, seuil).
type SentKey = (String, String, i64, u8);
/// Clé de suivi du dernier `resetsAt` connu pour une fenêtre : (provider, kind).
type ResetKey = (String, String);

/// Mémoire anti-spam du notifier : seuils déjà notifiés pour l'instance de fenêtre
/// courante (identifiée par son `resetsAt`), et dernier `resetsAt` vu par fenêtre.
#[derive(Debug, Default)]
pub struct NotifierData {
    sent: HashSet<SentKey>,
    last_reset: HashMap<ResetKey, i64>,
}

/// État managé Tauri : mémoire anti-spam partagée entre le watcher Codex et le
/// poller Claude.
pub struct NotifierState(pub Arc<Mutex<NotifierData>>);

impl Default for NotifierState {
    fn default() -> Self {
        NotifierState(Arc::new(Mutex::new(NotifierData::default())))
    }
}

/// "$ claude" -> "Claude", "$ codex" -> "Codex". Provider inconnu : préfixe
/// dépouillé de "$ " et capitalisé.
fn provider_display_name(prefix: &str) -> String {
    let stripped = prefix.strip_prefix("$ ").unwrap_or(prefix);
    match stripped {
        "claude" => "Claude".to_string(),
        "codex" => "Codex".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => other.to_string(),
            }
        }
    }
}

/// Libellé humain d'un type de fenêtre de quota.
fn kind_label(kind: &str) -> String {
    match kind {
        "5h" => "5h".to_string(),
        "weekly" => "hebdo".to_string(),
        "opus" => "Opus hebdo".to_string(),
        other => other.to_string(),
    }
}

/// Formate un epoch (s) en heure locale "HH:MM". Dégradation propre si la
/// conversion échoue (ne devrait pas arriver en pratique).
fn format_local_hhmm(epoch_secs: i64) -> String {
    match Local.timestamp_opt(epoch_secs, 0) {
        chrono::LocalResult::Single(dt) => dt.format("%H:%M").to_string(),
        _ => "--:--".to_string(),
    }
}

/// Évalue une fenêtre de quota pour un provider donné : met à jour `data` et
/// retourne les messages à notifier (reset éventuel, puis seuils franchis).
/// Fonction pure : aucun effet de bord OS, entièrement testable.
fn evaluate_window(
    data: &mut NotifierData,
    config: &Config,
    provider_id: &str,
    display_name: &str,
    window: &QuotaWindow,
) -> Vec<String> {
    let mut messages = Vec::new();
    let reset_key: ResetKey = (provider_id.to_string(), window.kind.clone());
    let previous_reset = data.last_reset.get(&reset_key).copied();

    match previous_reset {
        Some(prev) if window.resets_at > prev => {
            // La fenêtre a été réinitialisée : purge les seuils envoyés pour l'ancienne
            // instance (ancien `resetsAt`), afin qu'elle puisse re-notifier.
            data.sent.retain(|(id, kind, resets_at, _)| {
                !(id == provider_id && kind == &window.kind && *resets_at == prev)
            });
            if config.reset_notifications {
                messages.push(format!(
                    "Fenêtre {} {} réinitialisée",
                    kind_label(&window.kind),
                    display_name
                ));
            }
            data.last_reset.insert(reset_key, window.resets_at);
        }
        None => {
            // Première observation de cette fenêtre : on initialise sans notifier.
            data.last_reset.insert(reset_key, window.resets_at);
        }
        Some(_) => {
            // resetsAt inchangé (ou antérieur, ce qui ne devrait pas arriver) : rien à faire.
        }
    }

    let mut thresholds = config.thresholds.clone();
    thresholds.sort_unstable();
    let pct = window.used_percent.round() as i64;

    for t in thresholds {
        let sent_key: SentKey = (
            provider_id.to_string(),
            window.kind.clone(),
            window.resets_at,
            t,
        );
        if pct >= t as i64 && !data.sent.contains(&sent_key) {
            messages.push(format!(
                "{} {} : {}% — reset {}",
                display_name,
                kind_label(&window.kind),
                pct,
                format_local_hhmm(window.resets_at)
            ));
            data.sent.insert(sent_key);
        }
    }

    messages
}

fn provider_enabled(config: &Config, provider: &ProviderSnapshot) -> bool {
    match provider.id.as_str() {
        "claude" => config.providers.claude,
        "codex" => config.providers.codex,
        _ => true,
    }
}

/// Évalue tout le snapshot et retourne les messages à notifier. Pure (pas d'`AppHandle`).
fn evaluate_snapshot(data: &mut NotifierData, config: &Config, snapshot: &Snapshot) -> Vec<String> {
    if config.notifications_paused {
        return Vec::new();
    }
    let mut messages = Vec::new();
    for provider in &snapshot.providers {
        if !provider_enabled(config, provider) {
            continue;
        }
        let display_name = provider_display_name(&provider.prefix);
        for window in &provider.windows {
            messages.extend(evaluate_window(
                data,
                config,
                &provider.id,
                &display_name,
                window,
            ));
        }
    }
    messages
}

fn send_notification(app: &AppHandle, body: &str) {
    let result = app.notification().builder().title("Vigie").body(body).show();
    if let Err(e) = result {
        eprintln!("Vigie: échec envoi notification: {e}");
    }
}

/// Point d'entrée appelé après chaque mise à jour d'usage (watcher Codex, poller
/// Claude) : calcule les messages à envoyer puis les envoie réellement. Ne panique
/// jamais ; silencieux si la config ou l'état anti-spam ne sont pas managés.
pub fn check_and_notify(app: &AppHandle, snapshot: &Snapshot) {
    let config = match app.try_state::<ConfigState>() {
        Some(c) => c.0.lock().unwrap().clone(),
        None => return,
    };
    let notifier_state = match app.try_state::<NotifierState>() {
        Some(s) => s,
        None => return,
    };

    let messages = {
        let mut data = notifier_state.0.lock().unwrap();
        evaluate_snapshot(&mut data, &config, snapshot)
    };

    for message in messages {
        send_notification(app, &message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str, prefix: &str, windows: Vec<QuotaWindow>) -> ProviderSnapshot {
        ProviderSnapshot {
            id: id.to_string(),
            prefix: prefix.to_string(),
            windows,
            active: false,
            data_ts: 0,
            note: None,
            model: None,
        }
    }

    fn window(kind: &str, used_percent: f64, resets_at: i64) -> QuotaWindow {
        QuotaWindow {
            kind: kind.to_string(),
            used_percent,
            resets_at,
        }
    }

    fn snapshot_with(provider: ProviderSnapshot) -> Snapshot {
        Snapshot {
            providers: vec![provider],
            fetched_at: 0,
        }
    }

    #[test]
    fn provider_display_name_strips_prefix_and_maps_known_ids() {
        assert_eq!(provider_display_name("$ claude"), "Claude");
        assert_eq!(provider_display_name("$ codex"), "Codex");
    }

    #[test]
    fn kind_label_maps_known_kinds() {
        assert_eq!(kind_label("5h"), "5h");
        assert_eq!(kind_label("weekly"), "hebdo");
        assert_eq!(kind_label("opus"), "Opus hebdo");
    }

    #[test]
    fn first_observation_notifies_thresholds_but_not_reset() {
        // La toute première observation d'une fenêtre initialise `last_reset` sans
        // notifier de reset ; mais un seuil déjà franchi doit tout de même notifier.
        let mut data = NotifierData::default();
        let config = Config::default();
        let snapshot = snapshot_with(provider(
            "claude",
            "$ claude",
            vec![window("5h", 75.0, 1_000)],
        ));
        let messages = evaluate_snapshot(&mut data, &config, &snapshot);
        assert_eq!(messages.len(), 1, "le seuil 70% doit être notifié dès la 1ère observation");
        assert!(messages[0].contains("Claude"));
        assert!(messages[0].contains("75%"));
    }

    #[test]
    fn threshold_notifies_only_once_per_window_instance() {
        let mut data = NotifierData::default();
        let config = Config::default();
        let snap = snapshot_with(provider("claude", "$ claude", vec![window("5h", 75.0, 1_000)]));

        let first = evaluate_snapshot(&mut data, &config, &snap);
        assert_eq!(first.len(), 1, "70% doit être franchi une 1ère fois");

        // Même snapshot (même resetsAt, même pourcentage) : aucune notif supplémentaire.
        let second = evaluate_snapshot(&mut data, &config, &snap);
        assert!(second.is_empty(), "anti-spam : pas de re-notification pour un seuil déjà envoyé");
    }

    #[test]
    fn crossing_a_higher_threshold_notifies_again() {
        let mut data = NotifierData::default();
        let config = Config::default();

        let snap_75 = snapshot_with(provider("claude", "$ claude", vec![window("5h", 75.0, 1_000)]));
        let msgs_75 = evaluate_snapshot(&mut data, &config, &snap_75);
        assert_eq!(msgs_75.len(), 1);

        // Progression jusqu'à 90% (même resetsAt) : le seuil 85 doit notifier, mais pas 70
        // à nouveau.
        let snap_90 = snapshot_with(provider("claude", "$ claude", vec![window("5h", 90.0, 1_000)]));
        let msgs_90 = evaluate_snapshot(&mut data, &config, &snap_90);
        assert_eq!(msgs_90.len(), 1, "seul le nouveau seuil (85) doit notifier");
        assert!(msgs_90[0].contains("90%"));
    }

    #[test]
    fn reset_purges_sent_thresholds_and_rearms_notifications() {
        let mut data = NotifierData::default();
        let mut config = Config::default();
        config.reset_notifications = true;

        let snap_before = snapshot_with(provider(
            "claude",
            "$ claude",
            vec![window("5h", 95.0, 1_000)],
        ));
        let before = evaluate_snapshot(&mut data, &config, &snap_before);
        assert_eq!(before.len(), 3, "70/85/95 doivent tous notifier dès la 1ère observation");

        // Nouvelle instance de fenêtre (resetsAt plus grand) : reset détecté.
        let snap_after = snapshot_with(provider(
            "claude",
            "$ claude",
            vec![window("5h", 75.0, 2_000)],
        ));
        let after = evaluate_snapshot(&mut data, &config, &snap_after);

        assert!(
            after.iter().any(|m| m.contains("réinitialisée")),
            "un message de reset doit être émis: {after:?}"
        );
        assert!(
            after.iter().any(|m| m.contains("75%")),
            "le seuil 70 doit re-notifier après le reset (re-armé): {after:?}"
        );
    }

    #[test]
    fn reset_without_config_flag_does_not_send_reset_message() {
        let mut data = NotifierData::default();
        let config = Config::default(); // reset_notifications = false par défaut

        let snap_before = snapshot_with(provider("claude", "$ claude", vec![window("5h", 75.0, 1_000)]));
        evaluate_snapshot(&mut data, &config, &snap_before);

        let snap_after = snapshot_with(provider("claude", "$ claude", vec![window("5h", 75.0, 2_000)]));
        let after = evaluate_snapshot(&mut data, &config, &snap_after);

        assert!(
            !after.iter().any(|m| m.contains("réinitialisée")),
            "resetNotifications=false : aucun message de reset attendu"
        );
        assert!(after.iter().any(|m| m.contains("75%")), "le seuil doit tout de même re-notifier");
    }

    #[test]
    fn paused_config_suppresses_all_notifications() {
        let mut data = NotifierData::default();
        let mut config = Config::default();
        config.notifications_paused = true;

        let snap = snapshot_with(provider("claude", "$ claude", vec![window("5h", 99.0, 1_000)]));
        let messages = evaluate_snapshot(&mut data, &config, &snap);
        assert!(messages.is_empty(), "notificationsPaused doit tout couper");
    }

    #[test]
    fn disabled_provider_is_skipped() {
        let mut data = NotifierData::default();
        let mut config = Config::default();
        config.providers.codex = false;

        let snap = snapshot_with(provider("codex", "$ codex", vec![window("weekly", 99.0, 1_000)]));
        let messages = evaluate_snapshot(&mut data, &config, &snap);
        assert!(messages.is_empty(), "provider désactivé : aucune notification");
    }

    #[test]
    fn below_all_thresholds_notifies_nothing() {
        let mut data = NotifierData::default();
        let config = Config::default();
        let snap = snapshot_with(provider("claude", "$ claude", vec![window("5h", 10.0, 1_000)]));
        let messages = evaluate_snapshot(&mut data, &config, &snap);
        assert!(messages.is_empty());
    }
}
