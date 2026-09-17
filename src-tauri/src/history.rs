//! Historique SQLite des quotas : échantillons bruts (24 h) et agrégat journalier
//! (30 j). Best-effort : une erreur SQLite ne fait jamais paniquer l'application.
//!
//! Le `Mutex<Connection>` est séparé du `Mutex<Snapshot>` d'`AppState` : `record`
//! reçoit un `&Snapshot` déjà cloné, les deux verrous ne sont jamais imbriqués.

use crate::state::Snapshot;
use chrono::{Local, TimeZone};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};

const DB_FILENAME: &str = "history.db";

/// Rétention des échantillons bruts : 24h + marge.
const SAMPLE_RETENTION_SECS: i64 = 25 * 3600;
/// Rétention de l'agrégat journalier.
const DAILY_RETENTION_DAYS: i64 = 31;
/// Fenêtre d'affichage de `get_history`.
const HISTORY_WINDOW_SECS: i64 = 24 * 3600;
/// Fenêtre d'affichage de `get_heatmap`.
const HEATMAP_WINDOW_DAYS: i64 = 30;

/// État managé Tauri : connexion SQLite partagée entre le watcher Codex et le
/// poller Claude. `rusqlite::Connection` est `Send` mais pas `Sync`, d'où le `Mutex`.
pub struct HistoryState(pub Arc<Mutex<Connection>>);

/// Un échantillon brut, tel que retourné par `get_history`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub ts: i64,
    pub pct: f64,
}

/// Un point d'agrégat journalier, tel que retourné par `get_heatmap`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyPoint {
    pub day: String,
    pub pct: f64,
}

/// Jour local ("YYYY-MM-DD") d'un epoch donné. Dégradation propre (ne devrait pas
/// arriver en pratique) si la conversion échoue.
fn local_day(epoch: i64) -> String {
    match Local.timestamp_opt(epoch, 0) {
        chrono::LocalResult::Single(dt) => dt.format("%Y-%m-%d").to_string(),
        _ => "1970-01-01".to_string(),
    }
}

/// Crée les tables/index si absents. Idempotent : peut être rappelé sans effet
/// secondaire indésirable.
fn create_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS samples (
            id INTEGER PRIMARY KEY,
            ts INTEGER NOT NULL,
            provider_id TEXT NOT NULL,
            window_kind TEXT NOT NULL,
            used_percent REAL NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_samples_provider_kind_ts
            ON samples(provider_id, window_kind, ts);
        CREATE TABLE IF NOT EXISTS daily (
            day TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            used_percent_max REAL NOT NULL,
            sample_count INTEGER NOT NULL,
            PRIMARY KEY(day, provider_id)
        );
        CREATE TABLE IF NOT EXISTS observation_cursor (
            provider_id TEXT NOT NULL,
            window_kind TEXT NOT NULL,
            data_ts INTEGER NOT NULL,
            PRIMARY KEY(provider_id, window_kind)
        );",
    )
}

/// Ouvre (ou crée) `app_data_dir()/history.db`, dossier créé si absent.
/// `None` si le dossier de données ou le fichier sont inaccessibles.
fn open_file_connection(app: &AppHandle) -> Option<Connection> {
    let dir = app.path().app_data_dir().ok()?;
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!(
            "Vigie: échec création du dossier data ({}) pour l'historique: {e}",
            dir.display()
        );
        return None;
    }
    let path = dir.join(DB_FILENAME);
    match Connection::open(&path) {
        Ok(conn) => Some(conn),
        Err(e) => {
            eprintln!("Vigie: échec ouverture {} : {e}", path.display());
            None
        }
    }
}

/// Initialise l'état managé de l'historique : ouvre `history.db` (ou, en dernier
/// recours, une base en mémoire pour que l'appli reste fonctionnelle) et crée le
/// schéma. Ne panique jamais.
pub fn init(app: &AppHandle) -> HistoryState {
    let conn = open_file_connection(app).unwrap_or_else(|| {
        eprintln!("Vigie: historique en mémoire seulement (fichier indisponible)");
        Connection::open_in_memory().expect("SQLite en mémoire doit toujours réussir à s'ouvrir")
    });
    if let Err(e) = create_schema(&conn) {
        eprintln!("Vigie: échec création du schéma historique: {e}");
    }
    HistoryState(Arc::new(Mutex::new(conn)))
}

fn record_inner(conn: &Connection, snapshot: &Snapshot, now: i64) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    for provider in &snapshot.providers {
        if provider.windows.is_empty() || provider.data_ts <= 0 || provider.data_ts > now + 60 {
            continue;
        }
        let day = local_day(provider.data_ts);
        for window in &provider.windows {
            if !window.used_percent.is_finite() {
                continue;
            }
            let inserted = tx.execute(
                "INSERT INTO observation_cursor (provider_id, window_kind, data_ts)
                 VALUES (?1, ?2, ?3) ON CONFLICT(provider_id, window_kind)
                 DO UPDATE SET data_ts = excluded.data_ts
                 WHERE excluded.data_ts > observation_cursor.data_ts",
                params![provider.id, window.kind, provider.data_ts],
            )?;
            if inserted == 0 || provider.data_ts < now - DAILY_RETENTION_DAYS * 86400 {
                continue;
            }
            tx.execute(
                "INSERT INTO samples (ts, provider_id, window_kind, used_percent)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    provider.data_ts,
                    provider.id,
                    window.kind,
                    window.used_percent
                ],
            )?;
            tx.execute(
                "INSERT INTO daily (day, provider_id, used_percent_max, sample_count)
                 VALUES (?1, ?2, ?3, 1)
                 ON CONFLICT(day, provider_id) DO UPDATE SET
                     used_percent_max = MAX(used_percent_max, excluded.used_percent_max),
                     sample_count = sample_count + 1",
                params![day, provider.id, window.used_percent],
            )?;
        }
    }
    tx.commit()
}

/// Enregistre un échantillon par fenêtre réelle du snapshot (providers sans
/// fenêtre ignorés), et met à jour l'agrégat journalier (max du jour). Best-effort :
/// toute erreur SQLite est logguée, jamais propagée en panique.
pub fn record(conn: &Connection, snapshot: &Snapshot, now: i64) {
    if let Err(e) = record_inner(conn, snapshot, now) {
        eprintln!("Vigie: échec enregistrement historique: {e}");
    }
}

fn prune_inner(conn: &Connection, now: i64) -> rusqlite::Result<()> {
    let sample_cutoff = now - SAMPLE_RETENTION_SECS;
    conn.execute("DELETE FROM samples WHERE ts < ?1", params![sample_cutoff])?;

    let daily_cutoff_day = local_day(now - DAILY_RETENTION_DAYS * 24 * 3600);
    conn.execute(
        "DELETE FROM daily WHERE day < ?1",
        params![daily_cutoff_day],
    )?;
    Ok(())
}

/// Purge les échantillons de plus de 25h et l'agrégat journalier de plus de 31j.
/// Best-effort : toute erreur SQLite est logguée, jamais propagée en panique.
pub fn prune(conn: &Connection, now: i64) {
    if let Err(e) = prune_inner(conn, now) {
        eprintln!("Vigie: échec purge historique: {e}");
    }
}

fn query_samples_inner(
    conn: &Connection,
    provider_id: &str,
    window_kind: &str,
    since: i64,
) -> rusqlite::Result<Vec<(i64, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT ts, used_percent FROM samples
         WHERE provider_id = ?1 AND window_kind = ?2 AND ts >= ?3
         ORDER BY ts ASC",
    )?;
    let rows = stmt.query_map(params![provider_id, window_kind, since], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Échantillons `(ts, usedPercent)` triés par `ts` croissant, depuis `since_epoch`.
/// Retombe sur une liste vide (et logue) en cas d'erreur SQLite.
pub fn query_samples(
    conn: &Connection,
    provider_id: &str,
    window_kind: &str,
    since_epoch: i64,
) -> Vec<(i64, f64)> {
    match query_samples_inner(conn, provider_id, window_kind, since_epoch) {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("Vigie: échec lecture historique (samples): {e}");
            Vec::new()
        }
    }
}

fn query_daily_inner(
    conn: &Connection,
    provider_id: &str,
    since_day: &str,
) -> rusqlite::Result<Vec<(String, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT day, used_percent_max FROM daily
         WHERE provider_id = ?1 AND day >= ?2
         ORDER BY day ASC",
    )?;
    let rows = stmt.query_map(params![provider_id, since_day], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Agrégat journalier `(day, usedPercentMax)` des `HEATMAP_WINDOW_DAYS` derniers
/// jours, trié par jour croissant. Retombe sur une liste vide (et logue) en cas
/// d'erreur SQLite.
pub fn query_daily(conn: &Connection, provider_id: &str) -> Vec<(String, f64)> {
    let since_day = local_day(crate::util::now_epoch() - HEATMAP_WINDOW_DAYS * 24 * 3600);
    match query_daily_inner(conn, provider_id, &since_day) {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("Vigie: échec lecture historique (daily): {e}");
            Vec::new()
        }
    }
}

/// Commande IPC : échantillons des dernières 24h pour `providerId`/`windowKind`.
#[tauri::command]
pub fn get_history(
    state: tauri::State<HistoryState>,
    provider_id: String,
    window_kind: String,
) -> Vec<Sample> {
    let conn = state.0.lock().unwrap();
    let since = crate::util::now_epoch() - HISTORY_WINDOW_SECS;
    query_samples(&conn, &provider_id, &window_kind, since)
        .into_iter()
        .map(|(ts, pct)| Sample { ts, pct })
        .collect()
}

/// Commande IPC : agrégat journalier des 30 derniers jours pour `providerId`.
#[tauri::command]
pub fn get_heatmap(state: tauri::State<HistoryState>, provider_id: String) -> Vec<DailyPoint> {
    let conn = state.0.lock().unwrap();
    query_daily(&conn, &provider_id)
        .into_iter()
        .map(|(day, pct)| DailyPoint { day, pct })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ProviderSnapshot, QuotaWindow};

    /// DB en mémoire, schéma créé — jamais de fichier réel dans les tests.
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("connexion en mémoire");
        create_schema(&conn).expect("création du schéma");
        conn
    }

    fn snapshot_with(provider_id: &str, windows: Vec<QuotaWindow>, data_ts: i64) -> Snapshot {
        Snapshot {
            providers: vec![ProviderSnapshot {
                id: provider_id.to_string(),
                prefix: format!("$ {provider_id}"),
                windows,
                active: false,
                data_ts,
                note: None,
                model: None,
            }],
            fetched_at: 0,
        }
    }

    fn window(kind: &str, used_percent: f64) -> QuotaWindow {
        QuotaWindow {
            kind: kind.to_string(),
            used_percent,
            resets_at: 0,
        }
    }

    #[test]
    fn schema_creation_is_idempotent() {
        let conn = test_conn();
        create_schema(&conn).expect("un 2e appel ne doit pas échouer");
        create_schema(&conn).expect("un 3e appel ne doit pas échouer non plus");
    }

    #[test]
    fn record_then_query_samples_round_trips() {
        let conn = test_conn();
        let snap = snapshot_with("codex", vec![window("5h", 42.0)], 5_000);
        record(&conn, &snap, 5_000);

        let rows = query_samples(&conn, "codex", "5h", 0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], (5_000, 42.0));

        // Provider/kind différents : aucun résultat.
        assert!(query_samples(&conn, "claude", "5h", 0).is_empty());
        assert!(query_samples(&conn, "codex", "weekly", 0).is_empty());
    }

    #[test]
    fn record_multiple_windows_produces_one_sample_each() {
        let conn = test_conn();
        let snap = snapshot_with(
            "claude",
            vec![window("5h", 10.0), window("weekly", 20.0)],
            1_000,
        );
        record(&conn, &snap, 1_000);

        assert_eq!(query_samples(&conn, "claude", "5h", 0).len(), 1);
        assert_eq!(query_samples(&conn, "claude", "weekly", 0).len(), 1);
    }

    #[test]
    fn empty_windows_are_not_recorded() {
        let conn = test_conn();
        let snap = snapshot_with("codex", vec![], 1_000);
        record(&conn, &snap, 1_000);

        assert!(query_samples(&conn, "codex", "5h", 0).is_empty());
        assert!(query_daily(&conn, "codex").is_empty());
    }

    #[test]
    fn daily_upsert_keeps_the_max_and_counts_samples() {
        let conn = test_conn();
        // `query_daily` borne sur les 30 derniers jours par rapport à l'heure réelle :
        // on ancre les enregistrements sur `now_epoch()` pour rester dans cette fenêtre.
        let now = crate::util::now_epoch();

        record(
            &conn,
            &snapshot_with("codex", vec![window("5h", 30.0)], now),
            now,
        );
        record(
            &conn,
            &snapshot_with("codex", vec![window("5h", 80.0)], now + 1),
            now + 1,
        );
        record(
            &conn,
            &snapshot_with("codex", vec![window("5h", 50.0)], now + 2),
            now + 2,
        );

        let daily = query_daily(&conn, "codex");
        assert_eq!(daily.len(), 1, "un seul jour concerné");
        assert_eq!(
            daily[0].1, 80.0,
            "le max doit être conservé, pas la dernière valeur"
        );

        let count: i64 = conn
            .query_row(
                "SELECT sample_count FROM daily WHERE provider_id = 'codex'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 3,
            "sample_count doit s'incrémenter à chaque enregistrement"
        );
    }

    #[test]
    fn prune_removes_old_samples_and_keeps_recent() {
        let conn = test_conn();
        let now = 100_000;
        let old_ts = now - SAMPLE_RETENTION_SECS - 1;
        let recent_ts = now - 3_600;

        conn.execute(
            "INSERT INTO samples (ts, provider_id, window_kind, used_percent)
             VALUES (?1, 'codex', '5h', 10.0)",
            params![old_ts],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO samples (ts, provider_id, window_kind, used_percent)
             VALUES (?1, 'codex', '5h', 20.0)",
            params![recent_ts],
        )
        .unwrap();

        prune(&conn, now);

        let rows = query_samples(&conn, "codex", "5h", 0);
        assert_eq!(rows.len(), 1, "seul l'échantillon récent doit survivre");
        assert_eq!(rows[0].0, recent_ts);
    }

    #[test]
    fn repeated_or_older_provider_data_is_not_recorded_as_new() {
        let conn = test_conn();
        let now = crate::util::now_epoch();
        let mut snapshot = snapshot_with("codex", vec![window("weekly", 88.0)], now - 86400);
        snapshot
            .providers
            .extend(snapshot_with("claude", vec![window("5h", 40.0)], now).providers);
        record_inner(&conn, &snapshot, now).unwrap();
        record_inner(&conn, &snapshot, now + 300).unwrap();
        snapshot.providers[0].data_ts -= 500;
        record_inner(&conn, &snapshot, now + 600).unwrap();
        assert_eq!(
            query_samples(&conn, "codex", "weekly", 0),
            vec![(now - 86400, 88.0)]
        );
        assert_eq!(query_samples(&conn, "claude", "5h", 0), vec![(now, 40.0)]);
        assert_eq!(query_daily(&conn, "codex")[0].0, local_day(now - 86400));
        assert_eq!(
            conn.query_row("SELECT SUM(sample_count) FROM daily", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn missing_or_future_timestamps_are_not_recorded() {
        let conn = test_conn();
        for ts in [0, 2_000] {
            record_inner(
                &conn,
                &snapshot_with("codex", vec![window("weekly", 88.0)], ts),
                1_000,
            )
            .unwrap();
        }
        assert!(query_samples(&conn, "codex", "weekly", 0).is_empty());
    }

    #[test]
    fn prune_removes_old_daily_rows() {
        let conn = test_conn();
        let now = crate::util::now_epoch();
        let old_day = local_day(now - (DAILY_RETENTION_DAYS + 5) * 24 * 3600);
        let recent_day = local_day(now - 1 * 24 * 3600);

        conn.execute(
            "INSERT INTO daily (day, provider_id, used_percent_max, sample_count)
             VALUES (?1, 'codex', 99.0, 1)",
            params![old_day],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO daily (day, provider_id, used_percent_max, sample_count)
             VALUES (?1, 'codex', 55.0, 1)",
            params![recent_day],
        )
        .unwrap();

        prune(&conn, now);

        // query_daily borne déjà à 30j ; on vérifie ici la suppression physique.
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM daily", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            remaining, 1,
            "seule la ligne récente doit survivre à la purge"
        );
    }

    #[test]
    fn query_daily_bounds_to_30_days() {
        let conn = test_conn();
        let now = crate::util::now_epoch();
        let old_day = local_day(now - 40 * 24 * 3600);
        let recent_day = local_day(now - 5 * 24 * 3600);

        conn.execute(
            "INSERT INTO daily (day, provider_id, used_percent_max, sample_count)
             VALUES (?1, 'codex', 99.0, 1)",
            params![old_day],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO daily (day, provider_id, used_percent_max, sample_count)
             VALUES (?1, 'codex', 55.0, 1)",
            params![recent_day],
        )
        .unwrap();

        let daily = query_daily(&conn, "codex");
        assert_eq!(daily.len(), 1, "le jour vieux de 40j doit être exclu");
        assert_eq!(daily[0].0, recent_day);
    }
}
