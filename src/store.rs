use crate::model::*;
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;
use std::{path::Path, sync::Mutex};
pub struct Store(pub Mutex<Connection>);
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let c = Connection::open(path)?;
        c.busy_timeout(std::time::Duration::from_secs(5))?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
 CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY, state TEXT NOT NULL, data TEXT NOT NULL);
 CREATE TABLE IF NOT EXISTS requests(id TEXT PRIMARY KEY, project TEXT NOT NULL, session TEXT NOT NULL, operation TEXT NOT NULL, key TEXT NOT NULL, payload TEXT NOT NULL, response TEXT NOT NULL, UNIQUE(project,session,operation,key));
 CREATE TABLE IF NOT EXISTS grants(token_hash TEXT PRIMARY KEY, job_id TEXT NOT NULL, session TEXT NOT NULL UNIQUE);
 CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY AUTOINCREMENT, job_id TEXT NOT NULL, revision INTEGER NOT NULL, at TEXT NOT NULL, state TEXT NOT NULL, reason TEXT);
 CREATE TABLE IF NOT EXISTS attempts(id TEXT PRIMARY KEY,job_id TEXT NOT NULL,data TEXT NOT NULL);
 CREATE TABLE IF NOT EXISTS receipts(id TEXT PRIMARY KEY,job_id TEXT NOT NULL,data TEXT NOT NULL);
 CREATE TABLE IF NOT EXISTS steps(job_id TEXT NOT NULL,phase TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(job_id,phase));
 CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);")?;
        Ok(Self(Mutex::new(c)))
    }
    pub fn transaction<T>(&self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let mut c = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("database lock poisoned"))?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let v = f(&tx)?;
        tx.commit()?;
        Ok(v)
    }
    pub fn load(tx: &Transaction, id: &str) -> Result<Job> {
        let data: String = tx
            .query_row("SELECT data FROM jobs WHERE id=?1", [id], |r| r.get(0))
            .context("job not found")?;
        Ok(serde_json::from_str(&data)?)
    }
    pub fn save(tx: &Transaction, job: &mut Job, reason: &str) -> Result<()> {
        job.revision += 1;
        tx.execute("INSERT INTO jobs(id,state,data) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET state=excluded.state,data=excluded.data",params![job.input.job_id,job.state,serde_json::to_string(job)?])?;
        if let Some(attempt) = &job.attempt {
            tx.execute("INSERT INTO attempts(id,job_id,data) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![attempt.attempt_id,job.input.job_id,serde_json::to_string(attempt)?])?;
        }
        for receipt in job.receipt.iter().chain(job.candidate_receipt.iter()) {
            tx.execute("INSERT INTO receipts(id,job_id,data) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![receipt.verification_id,job.input.job_id,serde_json::to_string(receipt)?])?;
        }
        if let Some(commit) = &job.integrated_commit {
            tx.execute("INSERT INTO steps(job_id,phase,data) VALUES(?1,'integration',?2) ON CONFLICT(job_id,phase) DO UPDATE SET data=excluded.data",params![job.input.job_id,serde_json::to_string(&serde_json::json!({"base":job.integration_base,"commit":commit}))?])?;
        }
        if let Some(delivery) = &job.delivery {
            tx.execute("INSERT INTO steps(job_id,phase,data) VALUES(?1,'delivery',?2) ON CONFLICT(job_id,phase) DO UPDATE SET data=excluded.data",params![job.input.job_id,serde_json::to_string(delivery)?])?;
        }
        tx.execute(
            "INSERT INTO events(job_id,revision,at,state,reason) VALUES(?1,?2,?3,?4,?5)",
            params![
                job.input.job_id,
                i64::try_from(job.revision)?,
                now(),
                job.state,
                reason
            ],
        )?;
        Ok(())
    }
    pub fn all(&self) -> Result<Vec<Job>> {
        self.transaction(|tx| {
            let mut s = tx.prepare("SELECT data FROM jobs ORDER BY rowid")?;
            let data = s
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            data.into_iter()
                .map(|d| Ok(serde_json::from_str(&d)?))
                .collect()
        })
    }
    pub fn request(tx: &Transaction, id: &str) -> Result<Option<(Value, Value)>> {
        let row: Option<(String, String)> = tx
            .query_row(
                "SELECT payload,response FROM requests WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(p, r)| Ok((serde_json::from_str(&p)?, serde_json::from_str(&r)?)))
            .transpose()
    }
    pub fn authorize(tx: &Transaction, token: &str, session: &str) -> Result<String> {
        Ok(tx.query_row(
            "SELECT job_id FROM grants WHERE token_hash=?1 AND session=?2",
            params![digest(token), session],
            |r| r.get(0),
        )?)
    }
}
