//! Index local : métadonnées seulement, lectures incrémentales et debounce borné.
use crate::{config::ConfigState, state::ProviderSnapshot, AppState};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, UNIX_EPOCH},
};
use tauri::{Emitter, Manager};

const TAIL: u64 = 1024 * 1024;
const ACTIVE_SECS: i64 = 30;
/// Relecture des fichiers suivis sans event fichier. Sous Windows, les écritures d'un
/// writer qui garde son handle ouvert (Codex) ne remontent pas forcément au watcher :
/// ce tick est la vraie cadence de rafraîchissement, d'où 2 s (≈ 256 stats, négligeable).
const TICK: Duration = Duration::from_secs(2);
const RECENT_SECS: i64 = 86400;
const MAX_FILES: usize = 128;
/// Intervalle du parcours complet des dossiers de sessions.
const SCAN_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub provider_id: String,
    pub project: String,
    pub model: Option<String>,
    pub started_at: Option<i64>,
    pub last_active_at: i64,
    pub active: bool,
}

#[derive(Default)]
pub struct SessionsState(pub Mutex<Vec<Session>>);

#[tauri::command]
pub fn get_sessions(state: tauri::State<SessionsState>) -> Vec<Session> {
    state.0.lock().unwrap().clone()
}

/// Conserve une ligne incomplète entre deux écritures, y compris en UTF-8.
#[derive(Default)]
struct Lines {
    pending: Vec<u8>,
    skipping: bool,
}
impl Lines {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Value> {
        let mut values = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                if !self.skipping {
                    if let Ok(value) = serde_json::from_slice(&self.pending) {
                        values.push(value);
                    }
                }
                self.pending.clear();
                self.skipping = false;
            } else if !self.skipping {
                if self.pending.len() >= TAIL as usize {
                    self.pending.clear();
                    self.skipping = true;
                } else {
                    self.pending.push(byte);
                }
            }
        }
        values
    }
}

struct Tracked {
    session: Session,
    offset: u64,
    modified: Option<std::time::SystemTime>,
    lines: Lines,
    quota: Option<ProviderSnapshot>,
    /// Horodatage racine le plus récent parmi les lignes lues. Sous Windows, le mtime
    /// d'un rollout tenu ouvert par Codex reste figé : l'activité se base donc sur le
    /// plus récent des deux (horodatage des lignes, mtime).
    last_event_at: i64,
}

/// Écarte les caractères de contrôle, invisibles ou de sens d'écriture, qui pourraient
/// maquiller un libellé lu dans un journal.
fn is_displayable(c: char) -> bool {
    !c.is_control()
        && !matches!(
            c,
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}'
        )
}

fn project_name(cwd: &str) -> String {
    cwd.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("Projet inconnu")
        .chars()
        .filter(|c| is_displayable(*c))
        .take(100)
        .collect()
}

impl Tracked {
    fn new(path: &Path, provider: &str) -> Self {
        Self {
            session: Session {
                id: format!(
                    "{provider}:{}",
                    path.file_stem().unwrap_or_default().to_string_lossy()
                ),
                provider_id: provider.into(),
                project: "Projet inconnu".into(),
                model: None,
                started_at: None,
                last_active_at: 0,
                active: false,
            },
            offset: 0,
            modified: None,
            lines: Lines::default(),
            quota: None,
            last_event_at: 0,
        }
    }
    fn accept(&mut self, value: Value) {
        let payload = value.get("payload").unwrap_or(&value);
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        if let Some(ts) = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(crate::util::parse_iso8601_utc)
        {
            self.last_event_at = self.last_event_at.max(ts);
        }
        if let Some(cwd) = payload
            .get("cwd")
            .or_else(|| value.get("cwd"))
            .and_then(Value::as_str)
        {
            self.session.project = project_name(cwd);
        }
        if kind == "session_meta" {
            self.session.started_at = payload
                .get("timestamp")
                .or_else(|| value.get("timestamp"))
                .and_then(Value::as_str)
                .and_then(crate::util::parse_iso8601_utc);
        } else if self.session.started_at.is_none() && self.offset == 0 {
            self.session.started_at = value
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(crate::util::parse_iso8601_utc);
        }
        let model = payload
            .get("thread_settings")
            .and_then(|s| s.get("model"))
            .or_else(|| {
                if kind == "turn_context" {
                    payload.get("model")
                } else {
                    None
                }
            })
            .or_else(|| value.get("message").and_then(|m| m.get("model")));
        if let Some(model) = model
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty() && !m.starts_with('<'))
        {
            self.session.model = Some(model.chars().filter(|c| is_displayable(*c)).take(100).collect());
        }
        if self.session.provider_id == "codex"
            && payload.get("type").and_then(Value::as_str) == Some("token_count")
        {
            if let Ok(line) = serde_json::to_vec(&value) {
                if let Some(quota) =
                    crate::providers::codex::parse_snapshot_from_reader(std::io::Cursor::new(line))
                {
                    if self
                        .quota
                        .as_ref()
                        .map_or(true, |q| quota.data_ts >= q.data_ts)
                    {
                        self.quota = Some(quota);
                    }
                }
            }
        }
    }
    fn refresh(&mut self, path: &Path) -> std::io::Result<()> {
        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        let modified = metadata.modified().ok();
        if metadata.len() == self.offset && modified == self.modified {
            return Ok(());
        }
        if metadata.len() < self.offset
            || (metadata.len() == self.offset && modified != self.modified)
        {
            *self = Self::new(path, &self.session.provider_id.clone());
        }
        let mtime = modified
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if self.offset == 0 && metadata.len() > TAIL {
            let mut head = Vec::new();
            // session_meta peut contenir un long contexte : conserver une ligne
            // complète jusqu'à 1 Mio pour en extraire cwd et timestamp.
            (&mut file).take(TAIL).read_to_end(&mut head)?;
            for value in Lines::default().feed(&head) {
                self.accept(value);
            }
            self.offset = metadata.len().saturating_sub(TAIL);
            self.lines.skipping = true; // première ligne du tail tronquée
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        (&mut file).take(4 * TAIL).read_to_end(&mut bytes)?;
        for value in self.lines.feed(&bytes) {
            self.accept(value);
        }
        self.offset += bytes.len() as u64;
        self.modified = modified;
        // Le plus récent du mtime et des horodatages lus, borné à maintenant : un event
        // écrit dans la même seconde que le tick ne doit pas produire un écart négatif,
        // que `publish` traiterait comme idle.
        self.session.last_active_at = mtime
            .max(self.last_event_at)
            .min(crate::util::now_epoch());
        Ok(())
    }
}

fn relevant(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "jsonl")
}

/// Un parcours complet est-il dû ? Oui s'il n'a jamais eu lieu.
///
/// ⚠️ Pas de `Instant::now() - SCAN_INTERVAL` comme valeur initiale : sous Windows,
/// `Instant` part du boot et la soustraction panique tant que l'uptime est inférieur
/// à 5 min, ce qui arrive au lancement par l'autostart.
fn scan_due(last_scan: Option<Instant>, now: Instant) -> bool {
    last_scan.map_or(true, |t| now.duration_since(t) >= SCAN_INTERVAL)
}

/// Timeout glissant 800 ms, mais publication au plus tard 2 s après le 1er event.
fn debounce_wait(start: Instant, now: Instant) -> Duration {
    Duration::from_millis(800).min(Duration::from_secs(2).saturating_sub(now.duration_since(start)))
}

fn publish(app: &tauri::AppHandle, files: &HashMap<PathBuf, Tracked>) {
    let now = crate::util::now_epoch();
    let config = app.state::<ConfigState>().0.lock().unwrap().clone();
    let enabled = |id: &str| {
        if id == "claude" {
            config.providers.claude
        } else {
            config.providers.codex
        }
    };
    let mut sessions: Vec<_> = files
        .values()
        .filter(|f| enabled(&f.session.provider_id) && now - f.session.last_active_at < RECENT_SECS)
        .map(|f| {
            let mut s = f.session.clone();
            s.active = (0..ACTIVE_SECS).contains(&(now - s.last_active_at));
            s
        })
        .collect();
    sessions.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then(b.last_active_at.cmp(&a.last_active_at))
            .then(a.id.cmp(&b.id))
    });
    sessions.truncate(64);
    let session_state = app.state::<SessionsState>();
    let changed = {
        let mut guard = session_state.0.lock().unwrap();
        if *guard == sessions {
            false
        } else {
            *guard = sessions.clone();
            true
        }
    };
    if changed {
        let _ = app.emit("sessions-updated", &sessions);
    }

    let state = app.state::<AppState>();
    let (snapshot, usage_changed, state_changed) = {
        let mut guard = state.0.lock().unwrap();
        let mut usage_changed = false;
        let mut state_changed = false;
        if enabled("codex") {
            if let Some(latest) = files
                .values()
                .filter_map(|f| f.quota.as_ref())
                .max_by_key(|q| q.data_ts)
            {
                if guard
                    .providers
                    .iter()
                    .find(|p| p.id == "codex")
                    .map_or(true, |p| latest.data_ts > p.data_ts)
                {
                    crate::upsert_provider(&mut guard, latest.clone());
                    usage_changed = true;
                }
            }
        }
        for p in &mut guard.providers {
            let active = sessions.iter().any(|s| s.provider_id == p.id && s.active);
            let model = sessions
                .iter()
                .find(|s| s.provider_id == p.id && s.model.is_some())
                .and_then(|s| s.model.clone());
            if p.active != active || (model.is_some() && p.model != model) {
                state_changed = true;
            }
            p.active = active;
            if model.is_some() {
                p.model = model;
            }
        }
        (guard.clone(), usage_changed, state_changed)
    };
    if usage_changed {
        crate::record_snapshot(app, &snapshot);
        if let Some(codex) = snapshot.providers.iter().find(|p| p.id == "codex") {
            crate::providers::save_cache(app, CODEX_CACHE_FILENAME, &codex.windows, codex.data_ts);
        }
    }
    if usage_changed || state_changed {
        let _ = app.emit("usage-updated", &snapshot);
    }
    if usage_changed {
        crate::notifier::check_and_notify(app, &snapshot);
    }
}

pub fn provider_details(app: &tauri::AppHandle, id: &str) -> (bool, Option<String>) {
    let state = app.state::<SessionsState>();
    let sessions = state.0.lock().unwrap();
    (
        sessions.iter().any(|s| s.provider_id == id && s.active),
        sessions
            .iter()
            .find(|s| s.provider_id == id && s.model.is_some())
            .and_then(|s| s.model.clone()),
    )
}

/// Cache disque de la dernière mesure Codex (seau principal).
const CODEX_CACHE_FILENAME: &str = "codex-usage-cache.json";

/// Pose la dernière mesure Codex connue avant le premier parcours. Au démarrage, seuls
/// la tête et le dernier Mio de chaque rollout sont relus, et l'événement le plus récent
/// du seau principal peut se trouver ailleurs. Une mesure plus récente lue ensuite la
/// remplace : `publish` n'accepte qu'un `data_ts` supérieur.
fn seed_codex_from_cache(app: &tauri::AppHandle) {
    if !app.state::<ConfigState>().0.lock().unwrap().providers.codex {
        return;
    }
    let Some((windows, data_ts)) = crate::providers::load_cache(app, CODEX_CACHE_FILENAME) else {
        return;
    };
    let snapshot = {
        let state = app.state::<AppState>();
        let mut guard = state.0.lock().unwrap();
        crate::upsert_provider(
            &mut guard,
            ProviderSnapshot {
                id: "codex".into(),
                prefix: "$ codex".into(),
                windows,
                active: false,
                data_ts,
                note: None,
                model: None,
            },
        );
        guard.clone()
    };
    let _ = app.emit("usage-updated", &snapshot);
}

pub fn spawn(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        seed_codex_from_cache(&app);
        let roots = [
            (crate::providers::codex::codex_sessions_dir(), "codex"),
            (crate::providers::claude::projects_dir(), "claude"),
        ];
        let (tx, rx) = std::sync::mpsc::sync_channel(1024);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event {
                    if !matches!(event.kind, notify::EventKind::Access(_)) {
                        let _ = tx.try_send(event);
                    }
                }
            })
            .ok();
        let mut watched = HashSet::new();
        let mut files: HashMap<PathBuf, Tracked> = HashMap::new();
        let mut scan_at: Option<Instant> = None;
        loop {
            if scan_due(scan_at, Instant::now()) {
                for (root, provider) in &roots {
                    if root.exists() && !watched.contains(root) {
                        if let Some(watcher) = &mut watcher {
                            if notify::Watcher::watch(
                                watcher,
                                root,
                                notify::RecursiveMode::Recursive,
                            )
                            .is_ok()
                            {
                                watched.insert(root.clone());
                            }
                        }
                    }
                    let mut candidates: Vec<_> = walkdir::WalkDir::new(root)
                        .into_iter()
                        .filter_map(Result::ok)
                        .filter(|e| e.file_type().is_file() && relevant(e.path()))
                        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.into_path())))
                        .collect();
                    candidates.sort_by(|a, b| b.0.cmp(&a.0));
                    for (_, path) in candidates.into_iter().take(MAX_FILES) {
                        files
                            .entry(path.clone())
                            .or_insert_with(|| Tracked::new(&path, provider));
                    }
                }
                scan_at = Some(Instant::now());
            }
            // Stat sur les seuls fichiers suivis : aucun parcours de dossiers au tick.
            files.retain(|path, file| file.refresh(path).is_ok());
            publish(&app, &files);
            let now = crate::util::now_epoch();
            // Conserve un quota de secours, même si sa session a plus de 24 h.
            let freshest_quota = files
                .values()
                .filter_map(|f| f.quota.as_ref())
                .map(|q| q.data_ts)
                .max();
            files.retain(|_, f| {
                now - f.session.last_active_at < RECENT_SECS
                    || f.quota
                        .as_ref()
                        .is_some_and(|q| Some(q.data_ts) == freshest_quota)
            });
            if let Ok(first) = rx.recv_timeout(TICK) {
                let start = Instant::now();
                let mut paths: HashSet<PathBuf> = first.paths.into_iter().collect();
                loop {
                    let wait = debounce_wait(start, Instant::now());
                    if wait.is_zero() {
                        break;
                    }
                    match rx.recv_timeout(wait) {
                        Ok(event) => paths.extend(event.paths),
                        Err(_) => break,
                    }
                }
                for path in paths.into_iter().filter(|p| relevant(p)) {
                    if let Some((_, provider)) =
                        roots.iter().find(|(root, _)| path.starts_with(root))
                    {
                        files
                            .entry(path.clone())
                            .or_insert_with(|| Tracked::new(&path, provider));
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn file_cursor_handles_append_partial_lines_and_truncation() {
        let path =
            std::env::temp_dir().join(format!("vigie-session-test-{}.jsonl", std::process::id()));
        std::fs::write(&path, b"{\"cwd\":\"D:/Vigie\"}\n").unwrap();
        let mut tracked = Tracked::new(&path, "claude");
        tracked.refresh(&path).unwrap();
        let offset = tracked.offset;
        tracked.refresh(&path).unwrap();
        assert_eq!(tracked.offset, offset);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"{\"message\":{\"model\":\"test").unwrap();
        tracked.refresh(&path).unwrap();
        assert!(tracked.session.model.is_none());
        file.write_all(b"-model\"}}\n").unwrap();
        tracked.refresh(&path).unwrap();
        assert_eq!(tracked.session.model.as_deref(), Some("test-model"));
        drop(file);
        std::fs::write(&path, b"{\"cwd\":\"D:/Next\"}\n").unwrap();
        tracked.refresh(&path).unwrap();
        assert_eq!(tracked.session.project, "Next");
        assert!(tracked.session.model.is_none());
        std::fs::remove_file(path).unwrap();
    }
    /// Sous Windows, un writer qui garde son handle ouvert (Codex) laisse le mtime figé :
    /// l'activité doit suivre les horodatages des lignes, pas le système de fichiers.
    #[test]
    fn activity_follows_line_timestamps_when_mtime_is_frozen() {
        let path = std::env::temp_dir()
            .join(format!("vigie-activity-test-{}.jsonl", std::process::id()));
        // Event horodaté 2026-07-17T06:03:24Z = 1 784 268 204 ; mtime posé 1 000 000 s avant.
        std::fs::write(
            &path,
            r#"{"timestamp":"2026-07-17T06:03:24.764Z","type":"event_msg","payload":{"type":"task_started"}}
"#,
        )
        .unwrap();
        let frozen = UNIX_EPOCH + Duration::from_secs(1_784_268_204 - 1_000_000);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(frozen)
            .unwrap();
        let mut tracked = Tracked::new(&path, "codex");
        tracked.refresh(&path).unwrap();
        assert_eq!(
            tracked.session.last_active_at, 1_784_268_204,
            "l'horodatage de la ligne doit primer sur un mtime figé"
        );

        // Un mtime plus récent que les lignes garde la main (repli fichier). Même
        // taille + mtime différent = réinitialisation du curseur, relue depuis le début.
        let newer = UNIX_EPOCH + Duration::from_secs(1_785_000_000);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(newer)
            .unwrap();
        tracked.refresh(&path).unwrap();
        assert_eq!(tracked.session.last_active_at, 1_785_000_000);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn activity_falls_back_to_mtime_without_line_timestamp() {
        let path = std::env::temp_dir()
            .join(format!("vigie-activity-mtime-test-{}.jsonl", std::process::id()));
        std::fs::write(
            &path,
            r#"{"type":"bridge-session","cwd":"D:/Vigie"}
"#,
        )
        .unwrap();
        let mtime = UNIX_EPOCH + Duration::from_secs(1_785_000_000);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        let mut tracked = Tracked::new(&path, "claude");
        tracked.refresh(&path).unwrap();
        assert_eq!(tracked.session.last_active_at, 1_785_000_000);
        std::fs::remove_file(path).unwrap();
    }
    /// Reproduction sur le poste, hors suite automatique :
    /// `cargo test -- --ignored --nocapture reads_real_codex_rollout`
    /// Affiche mtime, dernier horodatage lu et lastActiveAt du rollout Codex le plus récent
    /// — choisi par mtime : un rollout fermé récemment peut passer devant la session
    /// vivante, c'est un diagnostic, pas une garantie.
    #[test]
    #[ignore]
    fn reads_real_codex_rollout() {
        let Some(path) = crate::providers::codex::latest_rollout(
            &crate::providers::codex::codex_sessions_dir(),
        ) else {
            println!("aucun rollout Codex sur ce poste");
            return;
        };
        let mut tracked = Tracked::new(&path, "codex");
        tracked.refresh(&path).unwrap();
        let mtime = std::fs::metadata(&path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        println!("rollout : {}", path.display());
        println!(
            "mtime {mtime} | dernier event {} | lastActiveAt {} | now {}",
            tracked.last_event_at,
            tracked.session.last_active_at,
            crate::util::now_epoch()
        );
    }
    #[test]
    fn partial_utf8_line_is_kept_until_newline() {
        let mut lines = Lines::default();
        let bytes = "{\"cwd\":\"é\"}\n".as_bytes();
        assert!(lines.feed(&bytes[..9]).is_empty());
        assert_eq!(lines.feed(&bytes[9..])[0]["cwd"], "é");
    }
    #[test]
    fn malformed_lines_do_not_hide_next_event() {
        assert_eq!(Lines::default().feed(b"oops\n{\"a\":1}\n").len(), 1);
    }
    #[test]
    fn scan_is_due_at_start_then_every_interval() {
        let t0 = Instant::now();
        assert!(scan_due(None, t0), "premier parcours immédiat, même juste après le boot");
        assert!(!scan_due(Some(t0), t0));
        assert!(!scan_due(Some(t0), t0 + Duration::from_secs(299)));
        assert!(scan_due(Some(t0), t0 + SCAN_INTERVAL));
    }
    #[test]
    fn debounce_has_a_hard_deadline() {
        let start = Instant::now();
        assert_eq!(
            debounce_wait(start, start + Duration::from_millis(1800)),
            Duration::from_millis(200)
        );
        assert!(debounce_wait(start, start + Duration::from_secs(3)).is_zero());
    }
    #[test]
    fn labels_drop_invisible_and_bidi_characters() {
        assert_eq!(project_name("C:/Dev/a\u{202E}b\u{200B}c\u{7}"), "abc");
    }
    #[test]
    fn captures_metadata_without_conversation_content() {
        let mut file = Tracked::new(Path::new("session.jsonl"), "codex");
        file.accept(serde_json::json!({"type":"session_meta","payload":{"cwd":"C:\\Dev\\Vigie","timestamp":"2026-09-07T10:00:00Z"}}));
        file.accept(serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5.6-sol"}}));
        assert_eq!(file.session.project, "Vigie");
        assert_eq!(file.session.model.as_deref(), Some("gpt-5.6-sol"));
        assert!(file.session.started_at.is_some());
    }
}
