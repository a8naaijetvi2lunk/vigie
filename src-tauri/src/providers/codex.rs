use crate::state::{ProviderSnapshot, QuotaWindow};
use crate::util::parse_iso8601_utc;
use serde_json::Value;
use std::io::BufRead;
use std::path::{Path, PathBuf};

/// Dossier des sessions Codex : `~/.codex/sessions`.
pub fn codex_sessions_dir() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    Path::new(&home).join(".codex").join("sessions")
}

#[cfg(test)]
fn is_rollout_filename(name: &str) -> bool {
    name.starts_with("rollout-") && name.ends_with(".jsonl")
}

/// Retrouve le rollout-*.jsonl le plus récent (par date de modification) dans `dir`.
/// Sert au diagnostic manuel `reads_real_codex_rollout`.
#[cfg(test)]
pub fn latest_rollout(dir: &Path) -> Option<PathBuf> {
    if !dir.exists() {
        return None;
    }
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.file_name()
                    .to_str()
                    .map(is_rollout_filename)
                    .unwrap_or(false)
        })
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((modified, e.path().to_path_buf()))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

/// Classe une fenêtre de quota selon sa durée en minutes (tolérance large).
fn classify_window(window_minutes: i64) -> String {
    if (250..=350).contains(&window_minutes) {
        "5h".to_string()
    } else if (9000..=11000).contains(&window_minutes) {
        "weekly".to_string()
    } else {
        "other".to_string()
    }
}

/// Scanne un flux de lignes JSONL et retient le dernier event
/// `payload.type == "token_count"` porteur d'un `payload.rate_limits` non-null.
/// Parseur tolérant : toute ligne qui ne parse pas ou n'a pas la forme attendue
/// est silencieusement ignorée.
pub(crate) fn parse_snapshot_from_reader<R: BufRead>(reader: R) -> Option<ProviderSnapshot> {
    let mut last: Option<(Vec<QuotaWindow>, i64)> = None;
    let mut model: Option<String> = None;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let payload = match value.get("payload") {
            Some(p) => p,
            None => continue,
        };

        // Modèle courant : dernier `thread_settings_applied` du rollout. Lu dans la
        // même passe que les quotas — aucune lecture de fichier supplémentaire.
        let is_thread_settings = payload
            .get("type")
            .and_then(|t| t.as_str())
            .map(|t| t == "thread_settings_applied")
            .unwrap_or(false);
        if is_thread_settings {
            if let Some(m) = payload
                .get("thread_settings")
                .and_then(|s| s.get("model"))
                .and_then(|m| m.as_str())
            {
                model = Some(m.to_string());
            }
            continue;
        }

        let is_token_count = payload
            .get("type")
            .and_then(|t| t.as_str())
            .map(|t| t == "token_count")
            .unwrap_or(false);
        if !is_token_count {
            continue;
        }
        let rate_limits = match payload.get("rate_limits") {
            Some(rl) if !rl.is_null() => rl,
            _ => continue,
        };
        // Codex publie un seau par famille de modèles (`limit_id`) : "codex" (principal)
        // et d'autres, comme "codex_bengalfox" (Spark). Seul le principal compte, sinon
        // l'affichage saute d'un seau à l'autre. `limit_id` absent = ancien format.
        let main_bucket = rate_limits
            .get("limit_id")
            .and_then(Value::as_str)
            .map_or(true, |id| id == "codex");
        if !main_bucket {
            continue;
        }

        let ts = value
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(parse_iso8601_utc)
            .unwrap_or(0);

        let mut windows = Vec::new();
        for key in ["primary", "secondary"] {
            let window = match rate_limits.get(key) {
                Some(w) if !w.is_null() => w,
                _ => continue,
            };
            let used_percent = window.get("used_percent").and_then(|v| v.as_f64());
            let window_minutes = window.get("window_minutes").and_then(|v| v.as_i64());
            let resets_at = window.get("resets_at").and_then(|v| v.as_i64());
            if let (Some(used_percent), Some(window_minutes), Some(resets_at)) =
                (used_percent, window_minutes, resets_at)
            {
                windows.push(QuotaWindow {
                    kind: classify_window(window_minutes),
                    used_percent,
                    resets_at,
                });
            }
        }

        if !windows.is_empty() && ts > 0 {
            last = Some((windows, ts));
        }
    }

    let (windows, data_ts) = last?;
    Some(ProviderSnapshot {
        id: "codex".to_string(),
        prefix: "$ codex".to_string(),
        windows,
        active: false,
        data_ts,
        note: None,
        model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_weekly_window_from_token_count_event() {
        let line = r#"{"timestamp":"2026-07-17T06:03:24.764Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":93.0,"window_minutes":10080,"resets_at":1784834499},"secondary":null}}}"#;

        let snapshot = parse_snapshot_from_reader(Cursor::new(line))
            .expect("le parseur doit produire un ProviderSnapshot");

        assert_eq!(snapshot.id, "codex");
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].kind, "weekly");
        assert_eq!(snapshot.windows[0].used_percent, 93.0);
        assert_eq!(snapshot.windows[0].resets_at, 1784834499);
        assert_eq!(snapshot.data_ts, 1784268204); // 2026-07-17T06:03:24Z en epoch
    }

    #[test]
    fn ignores_lines_that_are_not_token_count() {
        let lines = "not json at all\n{\"type\":\"other\"}\n{\"timestamp\":\"2026-07-17T06:03:24.764Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"something_else\"}}";
        assert!(parse_snapshot_from_reader(Cursor::new(lines)).is_none());
    }

    #[test]
    fn classifies_window_kinds_by_tolerance() {
        assert_eq!(classify_window(300), "5h");
        assert_eq!(classify_window(250), "5h");
        assert_eq!(classify_window(350), "5h");
        assert_eq!(classify_window(10080), "weekly");
        assert_eq!(classify_window(9000), "weekly");
        assert_eq!(classify_window(11000), "weekly");
        assert_eq!(classify_window(60), "other");
    }

    #[test]
    fn captures_model_from_thread_settings_applied() {
        let lines = concat!(
            r#"{"timestamp":"2026-08-17T12:31:15.899Z","type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"model":"gpt-5.5","model_provider_id":"openai"}}}"#,
            "\n",
            r#"{"timestamp":"2026-08-17T12:40:00.000Z","type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"model":"gpt-5.6-sol","model_provider_id":"openai"}}}"#,
            "\n",
            r#"{"timestamp":"2026-07-17T06:03:24.764Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":93.0,"window_minutes":10080,"resets_at":1784834499},"secondary":null}}}"#,
        );

        let snapshot = parse_snapshot_from_reader(Cursor::new(lines))
            .expect("le parseur doit produire un ProviderSnapshot");

        // Le DERNIER thread_settings_applied gagne.
        assert_eq!(snapshot.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(
            snapshot.windows.len(),
            1,
            "les fenêtres ne doivent pas régresser"
        );
    }

    #[test]
    fn model_is_none_when_no_thread_settings_event() {
        let line = r#"{"timestamp":"2026-07-17T06:03:24.764Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":93.0,"window_minutes":10080,"resets_at":1784834499},"secondary":null}}}"#;

        let snapshot = parse_snapshot_from_reader(Cursor::new(line))
            .expect("le parseur doit produire un ProviderSnapshot");

        assert!(snapshot.model.is_none());
        assert_eq!(snapshot.windows[0].used_percent, 93.0);
    }

    #[test]
    fn ignores_other_quota_buckets_such_as_spark() {
        // Seau principal à 94 %, puis un tour sur GPT-5.3-Codex-Spark (seau distinct,
        // 0 %). L'affichage doit rester à 94 %.
        let lines = concat!(
            r#"{"timestamp":"2026-09-09T21:18:09.770Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex","limit_name":null,"primary":{"used_percent":94.0,"window_minutes":10080,"resets_at":1789496636},"secondary":null}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-10T05:22:09.000Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex_bengalfox","limit_name":"GPT-5.3-Codex-Spark","primary":{"used_percent":0.0,"window_minutes":300,"resets_at":1789035641},"secondary":{"used_percent":0.0,"window_minutes":10080,"resets_at":1789622441}}}}"#,
        );

        let snapshot = parse_snapshot_from_reader(Cursor::new(lines))
            .expect("le seau principal doit être retenu");

        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].kind, "weekly");
        assert_eq!(snapshot.windows[0].used_percent, 94.0);
        assert_eq!(
            Some(snapshot.data_ts),
            parse_iso8601_utc("2026-09-09T21:18:09Z")
        );
    }

    #[test]
    fn a_spark_only_rollout_yields_no_codex_quota() {
        let line = r#"{"timestamp":"2026-09-10T05:22:09.000Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex_bengalfox","primary":{"used_percent":0.0,"window_minutes":300,"resets_at":1789035641},"secondary":null}}}"#;
        assert!(parse_snapshot_from_reader(Cursor::new(line)).is_none());
    }
}
