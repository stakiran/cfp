use crate::collector::Record;
use rusqlite::{params, Connection};
use std::path::Path;

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        // WAL にしておくと、記録中でも別プロセスから読める
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS buckets (
                t             INTEGER PRIMARY KEY,  -- バケット開始 (Unix秒, UTC)
                key_char      INTEGER NOT NULL,
                key_space     INTEGER NOT NULL,
                key_bs        INTEGER NOT NULL,
                key_enter     INTEGER NOT NULL,
                key_tab       INTEGER NOT NULL,
                key_nav       INTEGER NOT NULL,
                key_shortcut  INTEGER NOT NULL,
                key_ime       INTEGER NOT NULL,
                key_esc       INTEGER NOT NULL,
                iki_lt100     INTEGER NOT NULL,
                iki_100_300   INTEGER NOT NULL,
                iki_300_1000  INTEGER NOT NULL,
                iki_ge1000    INTEGER NOT NULL,
                mouse_dist    REAL    NOT NULL,
                mouse_move_ms INTEGER NOT NULL,
                click_l       INTEGER NOT NULL,
                click_r       INTEGER NOT NULL,
                click_m       INTEGER NOT NULL,
                dblclick      INTEGER NOT NULL,
                drag_dist     REAL    NOT NULL,
                wheel_v       REAL    NOT NULL,
                wheel_h       REAL    NOT NULL,
                active_secs   INTEGER NOT NULL,
                max_idle_ms   INTEGER NOT NULL,
                hand_switches INTEGER NOT NULL
            );",
        )?;
        Ok(Self { conn })
    }

    pub fn insert(&self, r: &Record) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO buckets VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
            params![
                r.t,
                r.key_char,
                r.key_space,
                r.key_bs,
                r.key_enter,
                r.key_tab,
                r.key_nav,
                r.key_shortcut,
                r.key_ime,
                r.key_esc,
                r.iki[0],
                r.iki[1],
                r.iki[2],
                r.iki[3],
                r.mouse_dist,
                r.mouse_move_ms,
                r.click_l,
                r.click_r,
                r.click_m,
                r.dblclick,
                r.drag_dist,
                r.wheel_v,
                r.wheel_h,
                r.active_secs,
                r.max_idle_ms,
                r.hand_switches,
            ],
        )?;
        Ok(())
    }
}
