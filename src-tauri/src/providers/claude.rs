use crate::config::ConfigState;
use crate::providers::{load_cache, save_cache};
use crate::state::{ProviderSnapshot, QuotaWindow};
use crate::util::{now_epoch, now_epoch_millis, parse_iso8601_utc};
use crate::AppState;
use chrono::{Local, TimeZone};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter, Manager};

const OAUTH_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const ANTHROPIC_BETA_HEADER: &str = "oauth-2025-04-20";
const CACHE_FILENAME: &str = "claude-usage-cache.json";
const LOG_FILENAME: &str = "vigie.log";
/// Au-delà, `vigie.log` devient `vigie.log.1` (une seule génération conservée).
const LOG_MAX_BYTES: u64 = 256 * 1024;

/// Intervalle nominal entre deux appels à l'API (contrainte anti-rate-limit : jamais < 5 min).
const POLL_INTERVAL_SECS: u64 = 300;
/// Pas de la boucle d'attente entre deux appels : délai maximal pour remarquer un token
/// renouvelé par Claude Code, ou la fin d'une attente traversée par une mise en veille.
const WAIT_STEP: Duration = Duration::from_secs(5);

/// Chemin des credentials OAuth Claude Code : `~/.claude/.credentials.json`.
fn credentials_path() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    Path::new(&home).join(".claude").join(".credentials.json")
}

/// Dossier des sessions Claude Code : `~/.claude/projects`.
pub fn projects_dir() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    Path::new(&home).join(".claude").join("projects")
}

/// Lit le token d'accès et sa date d'expiration (epoch millisecondes) depuis les
/// credentials Claude Code. Ne loggue jamais le contenu du token.
pub fn read_token() -> Option<(String, i64)> {
    let content = fs::read_to_string(credentials_path()).ok()?;
    let value: Value = serde_json::from_str(&content).ok()?;
    let oauth = value.get("claudeAiOauth")?;
    let access_token = oauth.get("accessToken")?.as_str()?.to_string();
    if access_token.is_empty() {
        return None;
    }
    let expires_at = oauth.get("expiresAt")?.as_i64()?;
    Some((access_token, expires_at))
}

/// Empreinte du token courant : son échéance, qui change à chaque renouvellement par
/// Claude Code. Le token lui-même n'est ni conservé ni journalisé.
fn token_fingerprint() -> Option<i64> {
    read_token().map(|(_, expires_at)| expires_at)
}

/// Erreurs distinguées lors de l'appel à `oauth/usage`.
#[derive(Debug)]
pub enum FetchError {
    RateLimited,
    Unauthorized,
    Other(String),
}

/// Parse tolérant du JSON de `oauth/usage` en fenêtres de quotas connues.
/// Une fenêtre absente ou `null` est simplement omise (dégradation propre).
fn parse_usage_windows(json: &Value) -> Vec<QuotaWindow> {
    const FIELDS: [(&str, &str); 3] = [
        ("five_hour", "5h"),
        ("seven_day", "weekly"),
        ("seven_day_opus", "opus"),
    ];

    let mut windows = Vec::new();
    for (field, kind) in FIELDS {
        let window = match json.get(field) {
            Some(w) if !w.is_null() => w,
            _ => continue,
        };
        let used_percent = window.get("utilization").and_then(|v| v.as_f64());
        let resets_at = window
            .get("resets_at")
            .and_then(|v| v.as_str())
            .and_then(parse_iso8601_utc);
        if let (Some(used_percent), Some(resets_at)) = (used_percent, resets_at) {
            windows.push(QuotaWindow {
                kind: kind.to_string(),
                used_percent,
                resets_at,
            });
        }
    }
    windows
}

/// Appelle `GET /api/oauth/usage` avec le token fourni. Distingue 429 (rate-limit),
/// 401/403 (token invalide) des autres erreurs, pour piloter le backoff côté appelant.
pub fn fetch_usage(token: &str) -> Result<Vec<QuotaWindow>, FetchError> {
    let response = ureq::get(OAUTH_USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", ANTHROPIC_BETA_HEADER)
        .timeout(Duration::from_secs(10))
        .call();

    match response {
        Ok(resp) => {
            let json: Value = resp
                .into_json()
                .map_err(|e| FetchError::Other(format!("réponse illisible: {e}")))?;
            Ok(parse_usage_windows(&json))
        }
        Err(ureq::Error::Status(429, _)) => Err(FetchError::RateLimited),
        Err(ureq::Error::Status(401, _)) | Err(ureq::Error::Status(403, _)) => {
            Err(FetchError::Unauthorized)
        }
        Err(ureq::Error::Status(code, _)) => Err(FetchError::Other(format!("HTTP {code}"))),
        Err(ureq::Error::Transport(t)) => Err(FetchError::Other(t.to_string())),
    }
}

// --- Backoff exponentiel + jitter (sur 429) ---------------------------------

/// Palier de backoff en minutes selon le nombre de 429 consécutifs (attempt >= 1).
/// Séquence : 5 → 10 → 20 → 40 → plafonné à 60.
fn backoff_base_minutes(attempt: u32) -> u64 {
    match attempt {
        0 | 1 => 5,
        2 => 10,
        3 => 20,
        4 => 40,
        _ => 60,
    }
}

/// Jitter pseudo-aléatoire (sans dépendance `rand`) basé sur les millisecondes courantes.
fn jitter_secs(max_jitter: u64) -> u64 {
    if max_jitter == 0 {
        return 0;
    }
    now_epoch_millis().unsigned_abs() % (max_jitter + 1)
}

fn backoff_duration(attempt: u32) -> Duration {
    let base_secs = backoff_base_minutes(attempt) * 60;
    let jitter = jitter_secs(base_secs / 10);
    Duration::from_secs(base_secs + jitter)
}

// --- Attente entre deux appels ----------------------------------------------

/// Échéance d'une attente atteinte ? Fonction pure. L'heure murale fait foi : sous
/// Windows, `thread::sleep` peut ne pas décompter le temps passé en veille, et une
/// attente de 5 min (60 en backoff) reprendrait alors où elle en était au réveil. Le
/// temps monotone couvre l'autre sens : une horloge murale reculée ne prolonge rien.
fn wait_elapsed(now: SystemTime, deadline: SystemTime, waited: Duration, duration: Duration) -> bool {
    now >= deadline || waited >= duration
}

/// Claude Code a-t-il écrit un nouveau token depuis le début de l'attente ? Fonction
/// pure. Une lecture ratée (fichier en cours d'écriture) ne réveille pas.
fn token_renewed(baseline: Option<i64>, current: Option<i64>) -> bool {
    current.is_some() && current != baseline
}

/// Attend `duration` par pas de `WAIT_STEP`, contre une échéance murale.
fn wait(duration: Duration) {
    let started = Instant::now();
    let deadline = SystemTime::now() + duration;
    while !wait_elapsed(SystemTime::now(), deadline, started.elapsed(), duration) {
        std::thread::sleep(WAIT_STEP);
    }
}

/// Comme `wait`, mais rend la main dès que Claude Code renouvelle son token. Vigie ne
/// rafraîchit jamais le token : la rotation du refresh token déconnecterait les
/// sessions Claude Code. `baseline` est l'échéance lue au tour de l'échec, pas relue
/// ici, pour ne pas manquer un renouvellement survenu entre les deux lectures.
/// Retourne `true` si l'attente a été écourtée par un nouveau token.
fn wait_for_new_token(duration: Duration, baseline: Option<i64>) -> bool {
    let started = Instant::now();
    let deadline = SystemTime::now() + duration;
    while !wait_elapsed(SystemTime::now(), deadline, started.elapsed(), duration) {
        if token_renewed(baseline, token_fingerprint()) {
            return true;
        }
        std::thread::sleep(WAIT_STEP);
    }
    false
}

// --- Journal -----------------------------------------------------------------

/// Journal des coupures et reprises de la source Claude, dans le dossier de données
/// (`vigie.log`). Le build release n'a pas de console : ce journal est la seule trace
/// des coupures. Jamais de token ici.
fn log_event(app: &AppHandle, message: &str) {
    eprintln!("Vigie: {message}");
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let path = dir.join(LOG_FILENAME);
    if fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX_BYTES) {
        let _ = fs::rename(&path, dir.join(format!("{LOG_FILENAME}.1")));
    }
    let line = format!("{} {message}\n", Local::now().format("%Y-%m-%d %H:%M:%S"));
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Journalise un changement d'état de la source. `outage` retient la note en cours et
/// le début de la coupure. Un token expiré qui se répète n'est écrit qu'une fois : une
/// coupure de plusieurs heures (Claude Code inactif) tient en deux lignes.
fn note_transition(
    app: &AppHandle,
    outage: &mut Option<(&'static str, i64)>,
    note: Option<&'static str>,
    detail: &str,
) {
    match note {
        None => {
            if let Some((previous, since)) = outage.take() {
                log_event(
                    app,
                    &format!(
                        "Claude : reconnecté après {} min ({previous})",
                        (now_epoch() - since) / 60
                    ),
                );
            }
        }
        Some(note) => {
            let repeat = matches!(outage, Some((previous, _)) if *previous == note);
            if !(repeat && note == TOKEN_EXPIRED_NOTE) {
                log_event(app, &format!("Claude : {note} — {detail}"));
            }
            let since = (*outage).map_or_else(now_epoch, |(_, since)| since);
            *outage = Some((note, since));
        }
    }
}

fn local_datetime(epoch_secs: i64) -> String {
    match Local.timestamp_opt(epoch_secs, 0) {
        chrono::LocalResult::Single(dt) => dt.format("%d/%m %H:%M").to_string(),
        _ => "?".to_string(),
    }
}

// --- Poller ------------------------------------------------------------------

const TOKEN_EXPIRED_NOTE: &str = "token_expired";
const RATE_LIMITED_NOTE: &str = "rate_limited";
const UNAVAILABLE_NOTE: &str = "unavailable";

fn make_snapshot(
    windows: Vec<QuotaWindow>,
    data_ts: i64,
    note: Option<String>,
) -> ProviderSnapshot {
    ProviderSnapshot {
        id: "claude".to_string(),
        prefix: "$ claude".to_string(),
        windows,
        active: false,
        data_ts,
        note,
        model: None,
    }
}

fn update_and_emit(app: &AppHandle, mut provider: ProviderSnapshot) {
    // Reflète l'activité dans le snapshot d'usage produit, à l'identique du
    // watcher Codex. Sans cela, chaque emit d'usage (poll, backoff, token expiré...)
    // réécraserait `active` à false via upsert et ferait clignoter la pastille en idle
    // jusqu'au prochain tick d'activité.
    let (active, model) = crate::sessions::provider_details(app, "claude");
    provider.active = active;
    // Même raison que `active` ci-dessus : l'upsert remplace l'entrée entière du
    // provider, donc le modèle doit être re-posé à chaque emit d'usage sous peine
    // de disparaître à chaque poll (5 min).
    provider.model = model;
    let snapshot = match app.try_state::<AppState>() {
        Some(state) => {
            let mut guard = state.0.lock().unwrap();
            crate::upsert_provider(&mut guard, provider);
            guard.clone()
        }
        None => return,
    };
    crate::record_snapshot(app, &snapshot);
    let _ = app.emit("usage-updated", &snapshot);
    crate::notifier::check_and_notify(app, &snapshot);
}

fn cached_windows_and_ts(app: &AppHandle) -> (Vec<QuotaWindow>, i64) {
    load_cache(app, CACHE_FILENAME).unwrap_or((Vec::new(), 0))
}

/// Publie la dernière mesure connue, marquée de la note d'échec.
fn emit_cached(app: &AppHandle, note: &str) {
    let (windows, data_ts) = cached_windows_and_ts(app);
    update_and_emit(app, make_snapshot(windows, data_ts, Some(note.to_string())));
}

/// Applique le plancher anti-rate-limit : l'intervalle effectif ne descend JAMAIS
/// sous `POLL_INTERVAL_SECS` (5 min), quelle que soit la valeur de config (édition
/// manuelle de `config.json`, que `Config::load` ne borne pas). Fonction pure, testable.
fn clamp_poll_interval(secs: u64) -> u64 {
    secs.max(POLL_INTERVAL_SECS)
}

/// Intervalle de poll courant (s), relu depuis la config à chaque itération
/// (réglable sans redémarrer l'app), borné par le plancher anti-rate-limit de 5 min.
/// Retombe sur `POLL_INTERVAL_SECS` si la config n'est pas (encore) managée.
fn poll_interval_secs(app: &AppHandle) -> u64 {
    let configured = match app.try_state::<ConfigState>() {
        Some(state) => state.0.lock().unwrap().claude_poll_interval_secs,
        None => POLL_INTERVAL_SECS,
    };
    clamp_poll_interval(configured)
}

fn poll_interval(app: &AppHandle) -> Duration {
    Duration::from_secs(poll_interval_secs(app))
}

fn claude_enabled(app: &AppHandle) -> bool {
    app.state::<ConfigState>().0.lock().unwrap().providers.claude
}

/// Lance le thread de polling Claude. Ne panique jamais : toute erreur réseau/parsing
/// est absorbée, journalisée sans le token, et le cache est conservé pour l'affichage.
pub fn spawn_poller(app: AppHandle) {
    std::thread::spawn(move || {
        let mut consecutive_429: u32 = 0;
        // Note en cours et début de la coupure ; `None` quand la source répond.
        let mut outage: Option<(&'static str, i64)> = None;

        // Affichage immédiat de la dernière donnée connue (stale-aware), avant le 1er fetch.
        if claude_enabled(&app) {
            let (windows, data_ts) = cached_windows_and_ts(&app);
            let note = if read_token().is_none() {
                Some(TOKEN_EXPIRED_NOTE.to_string())
            } else {
                None
            };
            update_and_emit(&app, make_snapshot(windows, data_ts, note));
        }

        loop {
            if !claude_enabled(&app) {
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
            let token_info = read_token();
            let expired = match &token_info {
                Some((_, expires_at)) => *expires_at < now_epoch_millis(),
                None => true,
            };

            if expired {
                consecutive_429 = 0;
                let detail = match &token_info {
                    Some((_, expires_at)) => format!(
                        "échéance du token dépassée ({}), en attente de Claude Code",
                        local_datetime(expires_at / 1000)
                    ),
                    None => "credentials absents ou illisibles".to_string(),
                };
                note_transition(&app, &mut outage, Some(TOKEN_EXPIRED_NOTE), &detail);
                emit_cached(&app, TOKEN_EXPIRED_NOTE);
                let expiry = token_info.as_ref().map(|(_, expires_at)| *expires_at);
                if wait_for_new_token(poll_interval(&app), expiry) {
                    log_event(&app, "Claude : token renouvelé par Claude Code, nouvel appel");
                }
                continue;
            }

            // `expired` est false ici, donc `token_info` est bien `Some`.
            let (token, expires_at) = token_info.expect("token présent (vérifié ci-dessus)");

            let result = fetch_usage(&token);
            drop(token); // pas de jeton en mémoire pendant les attentes qui suivent
            match result {
                Ok(windows) => {
                    consecutive_429 = 0;
                    note_transition(&app, &mut outage, None, "");
                    let data_ts = now_epoch();
                    save_cache(&app, CACHE_FILENAME, &windows, data_ts);
                    update_and_emit(&app, make_snapshot(windows, data_ts, None));
                    wait(poll_interval(&app));
                }
                Err(FetchError::RateLimited) => {
                    consecutive_429 += 1;
                    let pause = backoff_duration(consecutive_429);
                    note_transition(
                        &app,
                        &mut outage,
                        Some(RATE_LIMITED_NOTE),
                        &format!(
                            "HTTP 429 (tentative {consecutive_429}), pause {} min",
                            pause.as_secs() / 60
                        ),
                    );
                    emit_cached(&app, RATE_LIMITED_NOTE);
                    wait(pause);
                }
                Err(FetchError::Unauthorized) => {
                    consecutive_429 = 0;
                    note_transition(
                        &app,
                        &mut outage,
                        Some(TOKEN_EXPIRED_NOTE),
                        "HTTP 401/403, token refusé, en attente de Claude Code",
                    );
                    emit_cached(&app, TOKEN_EXPIRED_NOTE);
                    // Nouvel essai au tour suivant même sans nouveau token : un refus
                    // passager ne doit pas laisser Vigie bloqué jusqu'au prochain
                    // renouvellement, qui peut attendre des heures.
                    if wait_for_new_token(poll_interval(&app), Some(expires_at)) {
                        log_event(&app, "Claude : token renouvelé par Claude Code, nouvel appel");
                    }
                }
                Err(FetchError::Other(msg)) => {
                    consecutive_429 = 0;
                    note_transition(&app, &mut outage, Some(UNAVAILABLE_NOTE), &msg);
                    emit_cached(&app, UNAVAILABLE_NOTE);
                    wait(poll_interval(&app));
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture réelle : five_hour à 56 %, seven_day à 23 %, seven_day_opus absent (null).
    const FIXTURE_JSON: &str = r#"{
        "five_hour":       { "utilization": 56.0, "resets_at": "2026-07-17T14:30:00.407877+00:00" },
        "seven_day":       { "utilization": 23.0, "resets_at": "2026-07-21T17:00:00.407897+00:00" },
        "seven_day_opus":  null,
        "limits": []
    }"#;

    #[test]
    fn parses_five_hour_and_weekly_windows_and_ignores_null_opus() {
        let json: Value = serde_json::from_str(FIXTURE_JSON).expect("fixture JSON valide");
        let windows = parse_usage_windows(&json);

        assert_eq!(
            windows.len(),
            2,
            "seule five_hour et seven_day doivent produire une fenêtre"
        );

        let five_h = windows
            .iter()
            .find(|w| w.kind == "5h")
            .expect("fenêtre 5h absente");
        assert_eq!(five_h.used_percent, 56.0);
        assert!(five_h.resets_at > 0);

        let weekly = windows
            .iter()
            .find(|w| w.kind == "weekly")
            .expect("fenêtre weekly absente");
        assert_eq!(weekly.used_percent, 23.0);
        assert!(
            weekly.resets_at > five_h.resets_at,
            "le reset weekly doit être après le reset 5h"
        );

        assert!(
            windows.iter().all(|w| w.kind != "opus"),
            "seven_day_opus est null, aucune fenêtre opus ne doit être produite"
        );
    }

    #[test]
    fn parses_opus_window_when_present() {
        let json: Value = serde_json::from_str(
            r#"{
                "five_hour": { "utilization": 10.0, "resets_at": "2026-07-17T14:30:00+00:00" },
                "seven_day": { "utilization": 5.0, "resets_at": "2026-07-21T17:00:00+00:00" },
                "seven_day_opus": { "utilization": 42.0, "resets_at": "2026-07-21T17:00:00+00:00" }
            }"#,
        )
        .unwrap();
        let windows = parse_usage_windows(&json);
        let opus = windows
            .iter()
            .find(|w| w.kind == "opus")
            .expect("fenêtre opus absente");
        assert_eq!(opus.used_percent, 42.0);
    }

    #[test]
    fn poll_interval_never_drops_below_five_minute_floor() {
        // Valeurs dangereuses (édition manuelle de config.json) ramenées au plancher.
        assert_eq!(clamp_poll_interval(0), 300);
        assert_eq!(clamp_poll_interval(10), 300);
        assert_eq!(clamp_poll_interval(299), 300);
        // Le défaut et les valeurs supérieures passent inchangés.
        assert_eq!(clamp_poll_interval(300), 300);
        assert_eq!(clamp_poll_interval(600), 600);
    }

    #[test]
    fn backoff_sequence_follows_5_10_20_40_60_cap() {
        assert_eq!(backoff_base_minutes(1), 5);
        assert_eq!(backoff_base_minutes(2), 10);
        assert_eq!(backoff_base_minutes(3), 20);
        assert_eq!(backoff_base_minutes(4), 40);
        assert_eq!(backoff_base_minutes(5), 60);
        assert_eq!(
            backoff_base_minutes(100),
            60,
            "plafonné à 60 min quelque soit le nombre de tentatives"
        );
    }

    // --- Token expiré ---------------------------------------------------

    #[test]
    fn detects_expired_token_from_expires_at() {
        let expires_at_past: i64 = 1_700_000_000_000; // largement dans le passé (ms)
        let now = now_epoch_millis();
        assert!(
            expires_at_past < now,
            "la fixture doit représenter un token expiré"
        );
    }

    #[test]
    fn detects_valid_token_from_future_expires_at() {
        let expires_at_future = now_epoch_millis() + 60_000_000; // ~16h dans le futur
        let now = now_epoch_millis();
        assert!(
            expires_at_future > now,
            "la fixture doit représenter un token encore valide"
        );
    }

    // --- Attente -------------------------------------------------------------

    #[test]
    fn wait_ends_on_wall_clock_even_when_monotonic_time_lagged() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let duration = Duration::from_secs(300);
        let deadline = start + duration;
        // Réveil de veille : 1 h murale écoulée, 10 s seulement de temps monotone.
        assert!(wait_elapsed(
            start + Duration::from_secs(3600),
            deadline,
            Duration::from_secs(10),
            duration
        ));
        // Horloge murale reculée d'une heure : le temps monotone clôt l'attente.
        assert!(wait_elapsed(
            start - Duration::from_secs(3600),
            deadline,
            duration,
            duration
        ));
        // Ni l'un ni l'autre : l'attente continue.
        assert!(!wait_elapsed(
            start + Duration::from_secs(10),
            deadline,
            Duration::from_secs(10),
            duration
        ));
    }

    #[test]
    fn only_a_new_token_cuts_the_wait_short() {
        let old = Some(1_789_000_000_000);
        assert!(!token_renewed(old, old), "même échéance : pas de nouveau token");
        assert!(
            !token_renewed(old, None),
            "fichier illisible pendant son écriture : on patiente"
        );
        assert!(token_renewed(old, Some(1_789_028_800_000)));
        assert!(
            token_renewed(None, Some(1_789_028_800_000)),
            "credentials apparus en cours d'attente"
        );
    }
}
