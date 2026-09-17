use serde::Serialize;

/// Une fenêtre de quota (5h, hebdo, ou autre) pour un provider donné.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub kind: String,
    pub used_percent: f64,
    pub resets_at: i64,
}

/// Snapshot d'un provider (Codex, Claude, ...).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSnapshot {
    pub id: String,
    pub prefix: String,
    pub windows: Vec<QuotaWindow>,
    pub active: bool,
    pub data_ts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Modèle utilisé par le provider, nom BRUT tel que lu dans les fichiers de
    /// session (ex. "claude-opus-5", "gpt-5.6-sol"). L'abréviation d'affichage est
    /// de la présentation : elle vit côté front (`shortModelName`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// État global de l'application, partagé entre le watcher et les commandes IPC.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub providers: Vec<ProviderSnapshot>,
    pub fetched_at: i64,
}

impl Default for Snapshot {
    fn default() -> Self {
        Snapshot {
            providers: Vec::new(),
            fetched_at: 0,
        }
    }
}

/// Au-delà, une mesure Claude a manqué au moins un tour de poll (5 min).
const STALE_SECS: i64 = 600;

impl ProviderSnapshot {
    /// Mesure périmée ? Même règle que `isStale` côté front (`src/lib/usage.ts`).
    ///
    /// Claude, interrogé toutes les 5 min, est périmé au-delà de 10 min. Codex ne change
    /// que quand il tourne : sa mesure reste valable jusqu'au reset d'une de ses fenêtres
    /// (angle mort : un usage de Codex sur une autre machine).
    pub fn is_stale(&self, now: i64) -> bool {
        if self.id == "codex" {
            self.windows.iter().any(|w| w.resets_at < now)
        } else {
            now - self.data_ts > STALE_SECS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str, data_ts: i64, resets_at: i64) -> ProviderSnapshot {
        ProviderSnapshot {
            id: id.into(),
            prefix: format!("$ {id}"),
            windows: vec![QuotaWindow {
                kind: "weekly".into(),
                used_percent: 94.0,
                resets_at,
            }],
            active: false,
            data_ts,
            note: None,
            model: None,
        }
    }

    #[test]
    fn claude_is_stale_after_ten_minutes() {
        let now = 1_800_000_000;
        assert!(!provider("claude", now - 600, now + 3600).is_stale(now));
        assert!(provider("claude", now - 601, now + 3600).is_stale(now));
    }

    #[test]
    fn codex_stays_valid_until_a_window_resets() {
        let now = 1_800_000_000;
        // Mesurée il y a un jour, sans reset depuis : c'est toujours la bonne valeur.
        assert!(!provider("codex", now - 86_400, now + 3600).is_stale(now));
        assert!(provider("codex", now - 86_400, now - 1).is_stale(now));
    }
}
