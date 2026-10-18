//! External OpenCode V2 execution observation, never a dispatcher or acceptance ledger.
use crate::{engine::Engine, model::*, store::Store};
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{collections::HashSet, path::Path, time::Duration};

pub const MODE: &str = "opencode_v2";

struct Native {
    client: reqwest::Client,
    url: String,
}
impl Native {
    fn new(url: &str) -> Result<Self> {
        let u = reqwest::Url::parse(url)?;
        if u.scheme() != "http"
            || !matches!(u.host_str(), Some("127.0.0.1" | "[::1]"))
            || !u.username().is_empty()
            || u.password().is_some()
            || u.path() != "/"
            || u.query().is_some()
            || u.fragment().is_some()
        {
            bail!("V2 endpoint must be an approved loopback origin");
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()?,
            url: url.trim_end_matches('/').into(),
        })
    }
    async fn call(
        &self,
        path: &str,
        worktree: &Path,
        post: bool,
        query: &[(&str, &str)],
    ) -> Result<Value> {
        self.request(
            path,
            worktree,
            if post {
                reqwest::Method::POST
            } else {
                reqwest::Method::GET
            },
            query,
        )
        .await
    }
    async fn request(
        &self,
        path: &str,
        worktree: &Path,
        method: reqwest::Method,
        query: &[(&str, &str)],
    ) -> Result<Value> {
        let mut url = reqwest::Url::parse(&format!("{}{path}", self.url))?;
        url.query_pairs_mut().extend_pairs(query.iter().copied());
        let request = self.client.request(method.clone(), url);
        let request = if method == reqwest::Method::POST {
            request.json(&json!({}))
        } else {
            request
        };
        let mut request = request.header(
            "x-opencode-directory",
            worktree.to_str().context("V2 worktree")?,
        );
        if let Ok(password) = std::env::var("FACTORY_OPENCODE_PASSWORD") {
            request = request.basic_auth("opencode", Some(password));
        }
        let mut response = request.send().await?.error_for_status()?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(Value::Null);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 4_194_304 {
                bail!("V2 observation exceeds bound");
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
}

fn identity(info: &Value, session: &str, worktree: &Path) -> Result<Value> {
    if info["id"] != session
        || info["projectID"].as_str().is_none_or(|s| s.is_empty())
        || info["location"]["directory"].as_str() != worktree.to_str()
        || info.get("fork").is_some_and(|v| !v.is_null())
    {
        bail!("V2 session scope mismatch or unsupported fork");
    }
    Ok(
        json!({"id":info["id"],"projectID":info["projectID"],"location":info["location"],
        "parentID":info["parentID"],"metadata":info["metadata"]}),
    )
}
fn model_issue(
    config: &Config,
    model: &Value,
    absent_allowed: bool,
) -> Option<(&'static str, &'static str)> {
    if model.is_null() && absent_allowed {
        return None;
    }
    match (model["providerID"].as_str(), model["id"].as_str()) {
        (Some(provider), Some(id))
            if config.policy.models.contains(&format!("{provider}/{id}")) =>
        {
            None
        }
        (Some(_), Some(_)) => Some(("violating", "model outside approved free-model policy")),
        _ => Some(("unknown", "model identity missing or malformed")),
    }
}
fn accounting(config: &Config, info: &Value, messages: &[Value]) -> Value {
    let mut issues = vec![];
    let mut rows = vec![];
    for (label, value) in std::iter::once(("session".to_owned(), info)).chain(
        messages
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                matches!(
                    m["type"].as_str(),
                    Some("assistant" | "compaction" | "model-switched")
                )
            })
            .map(|(i, m)| (format!("message:{i}"), m)),
    ) {
        let selection = value["type"] == "model-switched";
        let mut known_zero = true;
        if !selection {
            match value["cost"].as_f64() {
                Some(0.0) => {}
                Some(n) if n > 0.0 => issues.push(
                    json!({"source":label,"status":"violating","reason":"known nonzero spend"}),
                ),
                _ => issues.push(
                    json!({"source":label,"status":"unknown","reason":"cost missing or malformed"}),
                ),
            }
            for (name, count) in [
                ("input", &value["tokens"]["input"]),
                ("output", &value["tokens"]["output"]),
                ("reasoning", &value["tokens"]["reasoning"]),
                ("cache.read", &value["tokens"]["cache"]["read"]),
                ("cache.write", &value["tokens"]["cache"]["write"]),
            ] {
                match count.as_f64().filter(|n| n.is_finite() && *n >= 0.0) {
                    Some(n) => known_zero &= n == 0.0,
                    None => {
                        known_zero = false;
                        issues.push(json!({"source":label,"status":"unknown","reason":format!("{name} usage missing or malformed")}));
                    }
                }
            }
        }
        // Only the session aggregate may omit a model for a measured deterministic zero-token fixture.
        if let Some((status, reason)) =
            model_issue(config, &value["model"], label == "session" && known_zero)
        {
            issues.push(json!({"source":label,"status":status,"reason":reason}));
        }
        rows.push(json!({"source":label,"id":value["id"],"type":value["type"],"model":value["model"],
            "cost":value["cost"],"tokens":value["tokens"],"time":value["time"],"error_present":value.get("error").is_some()}));
    }
    let status = if issues.iter().any(|i| i["status"] == "violating") {
        "violating"
    } else if issues.is_empty() {
        "settled_zero"
    } else {
        "unknown"
    };
    json!({"status":status,"issues":issues,"observations":rows})
}
fn require_settled(accounting: &Value) -> Result<()> {
    if accounting["status"] != "settled_zero" {
        bail!(
            "V2 accounting requires settlement: {}",
            accounting["status"]
        );
    }
    Ok(())
}
fn audit_messages(messages: &[Value], quiet: bool) -> Result<()> {
    if messages.len() > 4096 {
        bail!("V2 message observation exceeds bound");
    }
    for m in messages {
        match m["type"].as_str().context("V2 message type unknown")? {
            "assistant" => {
                if quiet && m["time"]["completed"].as_f64().is_none() {
                    bail!("V2 assistant completion unknown");
                }
                for content in m["content"]
                    .as_array()
                    .context("V2 assistant content unknown")?
                {
                    match content["type"].as_str() {
                        Some("text" | "reasoning") => {}
                        Some("tool")
                            if !quiet
                                || matches!(
                                    content["state"]["status"].as_str(),
                                    Some("completed" | "error")
                                ) => {}
                        _ => bail!("V2 tool activity unknown or nonterminal"),
                    }
                }
            }
            "model-switched" => {}
            "compaction" => {
                if quiet && !matches!(m["status"].as_str(), Some("completed" | "failed")) {
                    bail!("V2 compaction nonterminal");
                }
            }
            "shell"
                if quiet
                    && !matches!(m["status"].as_str(), Some("exited" | "timeout" | "killed")) =>
            {
                bail!("V2 shell nonterminal")
            }
            "location-switched" => bail!("V2 session location history requires host review"),
            "user" | "synthetic" | "system" | "skill" | "agent-switched" | "idle" | "shell" => {}
            _ => bail!("V2 message type unsupported"),
        }
    }
    Ok(())
}

fn audit_shells(shells: &Value, worktree: &Path) -> Result<()> {
    if shells["location"]["directory"].as_str() != worktree.to_str() {
        bail!("V2 shell observation scope mismatch");
    }
    let records = shells["data"]
        .as_array()
        .context("V2 shell observation unknown")?;
    if records.len() > 4096 {
        bail!("V2 shell observation exceeds bound");
    }
    for shell in records {
        if shell["id"].as_str().is_none_or(str::is_empty)
            || shell["cwd"].as_str().is_none_or(|cwd| {
                let path = Path::new(cwd);
                !path.is_absolute()
                    || !path.starts_with(worktree)
                    || path
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
            })
            || !matches!(
                shell["status"].as_str(),
                Some("exited" | "timeout" | "killed")
            )
            || shell["time"]["completed"].as_f64().is_none()
        {
            bail!("V2 active or unknown shell work");
        }
    }
    Ok(())
}

struct Observation {
    identity: Value,
    accounting: Value,
    evidence: Value,
}

impl Engine {
    fn v2_binding(&self, job: &Job) -> Result<Value> {
        let attempt = job.attempt.as_ref().context("V2 attempt")?;
        self.store.transaction(|tx| {
            let data: String = tx
                .query_row(
                    "SELECT value FROM metadata WHERE key=?1",
                    [format!("opencode-v2:{}", attempt.attempt_id)],
                    |r| r.get(0),
                )
                .context("V2 session is not host-bound")?;
            Ok(serde_json::from_str(&data)?)
        })
    }
    async fn observe_v2(
        &self,
        job: &Job,
        session: &str,
        binding: Option<&Value>,
        quiet: bool,
    ) -> Result<Observation> {
        tokio::time::timeout(
            Duration::from_secs(30),
            self.observe_v2_inner(job, session, binding, quiet),
        )
        .await
        .context("V2 observation deadline exceeded")?
    }
    async fn observe_v2_inner(
        &self,
        job: &Job,
        session: &str,
        binding: Option<&Value>,
        quiet: bool,
    ) -> Result<Observation> {
        if !valid_id(session) {
            bail!("Invalid V2 session ID");
        }
        let attempt = job.attempt.as_ref().context("V2 attempt")?;
        if attempt.mode != MODE {
            bail!("Not an external V2 attempt");
        }
        let runtime = Native::new(
            attempt
                .server_url
                .as_deref()
                .context("V2 endpoint missing")?,
        )?;
        let worktree = &attempt.worktree;
        if runtime.call("/api/info", worktree, false, &[]).await?["version"] != "2.0.29" {
            bail!("Unsupported V2 runtime");
        }
        let info = runtime
            .call(&format!("/api/session/{session}"), worktree, false, &[])
            .await?["data"]
            .clone();
        let observed = identity(&info, session, worktree)?;
        if info["time"]["updated"].as_f64().is_none() {
            bail!("V2 session update identity unknown");
        }
        if binding.is_some_and(|b| b != &observed) {
            bail!("V2 ownership binding changed");
        }
        // Enumerate all pages, across locations/projects, so a moved child cannot disappear.
        let mut cursor: Option<String> = None;
        let mut seen = HashSet::new();
        let mut found = false;
        for page in 0..32 {
            let mut query = vec![("limit", "100")];
            if let Some(c) = &cursor {
                query.push(("cursor", c));
            }
            let list = runtime
                .call("/api/session", worktree, false, &query)
                .await?;
            let entries = list["data"]
                .as_array()
                .context("V2 session inventory unknown")?;
            if entries.len() > 100 {
                bail!("V2 session page exceeds bound");
            }
            for entry in entries {
                let id = entry["id"]
                    .as_str()
                    .context("V2 inventory session ID unknown")?;
                if !seen.insert(id.to_owned()) {
                    bail!("V2 session pagination inconsistent");
                }
                if id == session {
                    found = true;
                    if identity(entry, session, worktree)? != observed {
                        bail!("V2 inventory ownership mismatch");
                    }
                }
                if entry["parentID"] == session {
                    bail!("V2 descendants require separate authority; unsupported in this slice");
                }
            }
            let paging = list["cursor"]
                .as_object()
                .context("V2 session pagination unknown")?;
            match paging.get("next") {
                None | Some(Value::Null) => break,
                Some(Value::String(c)) if !c.is_empty() && c.len() <= 4096 && page < 31 => {
                    cursor = Some(c.clone())
                }
                _ => bail!("V2 session inventory incomplete"),
            }
        }
        if !found {
            bail!("V2 bound session absent from complete inventory");
        }
        let exported = runtime
            .call(
                &format!("/api/experimental/session/{session}/export"),
                worktree,
                false,
                &[],
            )
            .await?;
        let exported = &exported["data"];
        if identity(&exported["info"], session, worktree)? != observed {
            bail!("V2 export ownership mismatch");
        }
        if exported["info"]["tokens"] != info["tokens"]
            || exported["info"]["cost"] != info["cost"]
            || exported["info"]["model"] != info["model"]
        {
            bail!("V2 export accounting changed");
        }
        let messages = exported["messages"]
            .as_array()
            .context("V2 transcript unknown")?;
        audit_messages(messages, quiet)?;
        if quiet {
            let active = runtime
                .call("/api/session/active", worktree, false, &[])
                .await?;
            let active = active["data"]
                .as_object()
                .context("V2 active execution observation unknown")?;
            if active.contains_key(session) {
                bail!("V2 session still active");
            }
            let inbox = runtime
                .call(
                    &format!("/api/session/{session}/inbox"),
                    worktree,
                    false,
                    &[],
                )
                .await?;
            if !inbox["data"]
                .as_array()
                .context("V2 inbox unknown")?
                .is_empty()
            {
                bail!("V2 pending inbox work");
            }
            let shells = runtime
                .call(
                    "/api/shell",
                    worktree,
                    false,
                    &[(
                        "location[directory]",
                        worktree.to_str().context("worktree")?,
                    )],
                )
                .await?;
            audit_shells(&shells, worktree)?;
        }
        // Re-observe identity and accounting after the bounded multi-request observation.
        let final_info = runtime
            .call(&format!("/api/session/{session}"), worktree, false, &[])
            .await?["data"]
            .clone();
        if identity(&final_info, session, worktree)? != observed
            || final_info["time"]["updated"] != info["time"]["updated"]
            || final_info["cost"] != info["cost"]
            || final_info["tokens"] != info["tokens"]
        {
            bail!("V2 session changed during observation");
        }
        let accounting = accounting(&self.config, &info, messages);
        let evidence = json!({"source":"opencode_v2_native_observation","runtime":"2.0.29","observed_at":now(),
            "identity":observed,"native_updated":info["time"]["updated"],
            "export_sha256":digest(serde_json::to_vec(exported)?),"inventory_count":seen.len(),"execution_record_count":messages.len(),
            "quiet_checks":quiet,"no_descendants":true,"native_outcome":info["outcome"]});
        Ok(Observation {
            identity: observed,
            accounting,
            evidence,
        })
    }
    pub async fn bind_v2(&self, id: &str, session: &str, token: &str) -> Result<Job> {
        self.bind_v2_observed(id, session, token, false).await
    }
    /// Attach only an independently observed empty session to a never-bound failed admission.
    /// No worker grant or execution/resume authority is created by this recovery operation.
    pub async fn recover_bind_v2(&self, id: &str, session: &str) -> Result<Job> {
        self.bind_v2_observed(id, session, "", true).await
    }
    async fn bind_v2_observed(
        &self,
        id: &str,
        session: &str,
        token: &str,
        recovery: bool,
    ) -> Result<Job> {
        if !valid_id(session) || (!recovery && token.len() < 32) {
            bail!("Invalid V2 session or capability");
        }
        let _gate = self.gate.lock().await;
        let job = self.get(id)?;
        if recovery {
            let attempt = job.attempt.as_ref().context("V2 recovery attempt")?;
            let prior = self
                .v2_recovery(&job)?
                .context("V2 recovery fence missing")?;
            if job.state != "blocked"
                || attempt.mode != MODE
                || !((attempt.session_id.is_none() && attempt.dispatch_state == "stop_unconfirmed")
                    || (attempt.session_id.as_deref() == Some(session)
                        && attempt.dispatch_state == "recovery_bound"))
                || prior["session_id"] != Value::Null
                || prior["execution_stop"]["status"] != "unconfirmed"
                || prior["accounting"]["status"] != "unobserved"
            {
                bail!("Recovery binding requires the exact never-bound failed admission");
            }
        } else if !job.active() || job.state != "running" {
            bail!("Inactive V2 attempt");
        }
        let observation = self.observe_v2(&job, session, None, true).await?;
        require_settled(&observation.accounting)?;
        if recovery {
            let rows = observation.accounting["observations"]
                .as_array()
                .context("V2 accounting observations")?;
            if rows.len() != 1 {
                bail!("Recovery accounting must cover exactly one session");
            }
            let runtime = Native::new(
                job.attempt
                    .as_ref()
                    .and_then(|a| a.server_url.as_deref())
                    .context("V2 endpoint")?,
            )?;
            let info = runtime
                .call(
                    &format!("/api/session/{session}"),
                    &job.attempt.as_ref().unwrap().worktree,
                    false,
                    &[],
                )
                .await?["data"]
                .clone();
            let denied = json!([{"action":"*","resource":"*","effect":"deny"}]);
            if info["permissions"] != denied
                || info["cost"] != rows[0]["cost"]
                || info["tokens"] != rows[0]["tokens"]
                || info["time"]["updated"] != observation.evidence["native_updated"]
                || identity(&info, session, &job.attempt.as_ref().unwrap().worktree)?
                    != observation.identity
                || observation.evidence["inventory_count"] != 1
                || observation.evidence["execution_record_count"] != 0
                || observation.identity["parentID"] != Value::Null
                || rows.len() != 1
                || ["input", "output", "reasoning"]
                    .iter()
                    .any(|key| rows[0]["tokens"][key].as_f64() != Some(0.0))
                || ["read", "write"]
                    .iter()
                    .any(|key| rows[0]["tokens"]["cache"][key].as_f64() != Some(0.0))
            {
                bail!(
                    "Recovery binding requires a sole empty, zero-token native session; preserve other history"
                );
            }
        }
        let observed = observation.identity;
        self.store.transaction(|tx| {
            let mut current=Store::load(tx,id)?;
            if current.revision!=job.revision { bail!("V2 binding state changed; reconcile"); }
            let attempt=current.attempt.as_mut().context("V2 attempt")?;
            if attempt.mode!=MODE || attempt.session_id.as_ref().is_some_and(|s|s!=session) { bail!("V2 session already bound"); }
            let key=format!("opencode-v2:{}",attempt.attempt_id);
            let existing:Option<String>=tx.query_row("SELECT value FROM metadata WHERE key=?1",[&key],|r|r.get(0)).optional()?;
            if existing.map(|s|serde_json::from_str::<Value>(&s)).transpose()?.is_some_and(|old|old!=observed) { bail!("V2 binding immutable"); }
            if !recovery {
                tx.execute("INSERT INTO grants(token_hash,job_id,session) VALUES(?1,?2,?3) ON CONFLICT(token_hash) DO UPDATE SET job_id=excluded.job_id,session=excluded.session WHERE grants.job_id=excluded.job_id AND grants.session=excluded.session",params![digest(token),id,session])?;
                let grant:String=tx.query_row("SELECT job_id FROM grants WHERE token_hash=?1 AND session=?2",params![digest(token),session],|r|r.get(0))?;
                if grant!=id { bail!("V2 capability already scoped"); }
            }
            attempt.session_id=Some(session.into());attempt.dispatch_state=if recovery {"recovery_bound"} else {"bound"}.into();
            tx.execute("INSERT INTO metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO NOTHING",params![key,serde_json::to_string(&observed)?])?;
            Store::save(tx,&mut current,"host V2 native session ownership bound")?;
            Ok(current)
        })
    }
    pub(crate) async fn check_v2(&self, job: &Job, quiet: bool) -> Result<()> {
        let Some(attempt) = job.attempt.as_ref().filter(|a| a.mode == MODE) else {
            return Ok(());
        };
        if self
            .v2_seal(job)?
            .is_some_and(|seal| seal["status"] != "sealed")
        {
            bail!("V2 completion seal unresolved; acceptance remains fenced");
        }
        let binding = self.v2_binding(job)?;
        let observation = self
            .observe_v2(
                job,
                attempt
                    .session_id
                    .as_deref()
                    .context("V2 session unbound")?,
                Some(&binding),
                quiet,
            )
            .await?;
        if let Some(seal) = self.v2_seal(job)? {
            Self::require_same_completion(&seal, &observation)?;
        }
        require_settled(&observation.accounting)
    }
    pub(crate) fn v2_seal(&self, job: &Job) -> Result<Option<Value>> {
        let Some(a) = job.attempt.as_ref().filter(|a| a.mode == MODE) else {
            return Ok(None);
        };
        let value: Option<Value> = self.store.transaction(|tx| {
            let value: Option<String> = tx
                .query_row(
                    "SELECT value FROM metadata WHERE key=?1",
                    [format!("opencode-v2-seal:{}", a.attempt_id)],
                    |r| r.get(0),
                )
                .optional()?;
            value
                .map(|v| serde_json::from_str(&v).map_err(Into::into))
                .transpose()
        })?;
        if let Some(seal) = &value {
            let stamp = seal["requested_at"]
                .as_str()
                .context("Completion seal timestamp")?;
            if seal["version"] != 1
                || seal["job_id"] != job.input.job_id
                || seal["attempt_id"] != a.attempt_id
                || seal["session_id"].as_str() != a.session_id.as_deref()
                || seal["expected_native_updated"]
                    .as_f64()
                    .is_none_or(|v| !v.is_finite() || v <= 0.)
                || !matches!(seal["status"].as_str(), Some("fencing" | "sealed"))
                || stamp.len() > 64
                || chrono::DateTime::parse_from_rfc3339(stamp).is_err()
                || uuid::Uuid::parse_str(seal["seal_id"].as_str().unwrap_or("")).is_err()
                || seal["binding"] != self.v2_binding(job)?
            {
                bail!("Completion seal identity/schema invalid; preserve admission fence");
            }
            if seal["status"] == "sealed" && seal["accounting"]["status"] != "settled_zero" {
                bail!("Completion seal accounting unresolved");
            }
        }
        Ok(value)
    }
    pub(crate) fn v2_seal_summary(seal: &Value) -> Value {
        let accounting = if seal["status"] == "sealed" {
            "settled_zero"
        } else {
            "unknown"
        };
        json!({"version":seal["version"],"seal_id":seal["seal_id"],
            "status":seal["status"],"job_id":seal["job_id"],
            "attempt_id":seal["attempt_id"],"session_id":seal["session_id"],
            "accounting_status":accounting,"expected_native_updated":seal["expected_native_updated"]})
    }
    fn require_same_completion(seal: &Value, observation: &Observation) -> Result<()> {
        if observation.evidence["native_outcome"] != "succeeded"
            || observation.evidence["native_updated"].as_f64()
                != seal["expected_native_updated"].as_f64()
            || (seal.get("before").is_some()
                && observation.evidence["export_sha256"] != seal["before"]["export_sha256"])
        {
            bail!("Native completion changed after admission fence; preserve unaccepted source");
        }
        Ok(())
    }
    /// Host-only completion fence. Never interrupts healthy work or invents usage.
    /// pstack's existing model/request hooks consume the now-empty admission list.
    pub async fn seal_v2(&self, id: &str, expected_native_updated: &Value) -> Result<Value> {
        if expected_native_updated
            .as_f64()
            .is_none_or(|v| !v.is_finite() || v <= 0.)
        {
            bail!("Native completion revision required");
        }
        // Two 30-second observations plus lock/storage work fit inside this
        // operation budget; the host control transport allows 120 seconds.
        tokio::time::timeout(
            Duration::from_secs(90),
            self.seal_v2_inner(id, expected_native_updated),
        )
        .await
        .context("Completion seal timed out; preserve and reconcile its durable fence")?
    }
    async fn seal_v2_inner(&self, id: &str, expected_native_updated: &Value) -> Result<Value> {
        let _gate = self.gate.lock().await;
        let mut job = self.get(id)?;
        let a = job
            .attempt
            .as_ref()
            .filter(|a| a.mode == MODE)
            .context("V2 attempt")?;
        let session = a.session_id.clone().context("V2 session unbound")?;
        let attempt = a.attempt_id.clone();
        let binding = self.v2_binding(&job)?;
        if !job.active()
            || job.state != "running"
            || job.candidate.is_some()
            || job.verification_pending.is_some()
            || job.integrated_commit.is_some()
        {
            bail!("Completion seal requires a live, unsubmitted V2 attempt");
        }
        let prior = self.v2_seal(&job)?;
        let mut seal = if let Some(prior) = prior {
            if prior["attempt_id"] != attempt
                || prior["session_id"] != session
                || prior["binding"] != binding
                || prior["expected_native_updated"].as_f64() != expected_native_updated.as_f64()
            {
                bail!("V2 completion seal identity changed");
            }
            prior
        } else {
            // Refuse to freeze a cached terminal outcome while a continuation,
            // inbox, shell, or unsettled usage is still observable.
            let observation = self
                .observe_v2(&job, &session, Some(&binding), true)
                .await?;
            require_settled(&observation.accounting)?;
            Self::require_same_completion(
                &json!({"expected_native_updated":expected_native_updated}),
                &observation,
            )?;
            let intent = json!({"version":1,"seal_id":new_id(),"job_id":id,
                "attempt_id":attempt,"session_id":session,"binding":binding,
                "status":"fencing","requested_at":now(),"before":observation.evidence,
                "expected_native_updated":expected_native_updated});
            self.store.transaction(|tx| {
                let mut current = Store::load(tx, id)?;
                if current.revision != job.revision
                    || !current.active()
                    || current.state != "running"
                {
                    bail!("Completion seal state changed; reconcile before candidate capture");
                }
                tx.execute(
                    "INSERT INTO metadata(key,value) VALUES(?1,?2)",
                    params![
                        format!("opencode-v2-seal:{attempt}"),
                        serde_json::to_string(&intent)?
                    ],
                )?;
                Store::save(
                    tx,
                    &mut current,
                    "native model admission fenced before candidate capture",
                )?;
                job = current;
                Ok(())
            })?;
            intent
        };
        // Re-observe after the durable fence; a race fails closed and retains
        // the intent. Repeating this read-only qualification never replays work.
        let observation = self
            .observe_v2(&job, &session, Some(&binding), true)
            .await?;
        require_settled(&observation.accounting)?;
        Self::require_same_completion(&seal, &observation)?;
        if seal["status"] == "sealed" {
            let current = self.get(id)?;
            if current.revision != job.revision || !current.active() || current.state != "running" {
                bail!("Completion seal replay lost its live lease/state");
            }
            return Ok(Self::v2_seal_summary(&seal));
        }
        seal["status"] = json!("sealed");
        seal["sealed_at"] = json!(now());
        seal["evidence"] = observation.evidence;
        seal["accounting"] = observation.accounting;
        self.store.transaction(|tx| {
            let mut current = Store::load(tx, id)?;
            if current.revision != job.revision || !current.active() || current.state != "running" {
                bail!("Completion seal state changed; preserve the inference fence");
            }
            tx.execute(
                "UPDATE metadata SET value=?2 WHERE key=?1",
                params![
                    format!("opencode-v2-seal:{attempt}"),
                    serde_json::to_string(&seal)?
                ],
            )?;
            Store::save(tx, &mut current, "native completion independently sealed")?;
            Ok(())
        })?;
        Ok(Self::v2_seal_summary(&seal))
    }
    pub(crate) fn v2_recovery(&self, job: &Job) -> Result<Option<Value>> {
        let Some(a) = job.attempt.as_ref().filter(|a| a.mode == MODE) else {
            return Ok(None);
        };
        self.store.transaction(|tx| {
            let value: Option<String> = tx
                .query_row(
                    "SELECT value FROM metadata WHERE key=?1",
                    [format!("opencode-v2-recovery:{}", a.attempt_id)],
                    |r| r.get(0),
                )
                .optional()?;
            value
                .map(|v| serde_json::from_str(&v).map_err(Into::into))
                .transpose()
        })
    }
    pub(crate) async fn abort_v2(&self, id: &str) -> Result<Job> {
        let stop_id = new_id();
        // Fence and expire the lease durably before waiting for an in-flight controller
        // effect or issuing any native request. A stale advance cannot save acceptance.
        let fenced=self.store.transaction(|tx| {
            let mut job=Store::load(tx,id)?;
            if job.state=="done" {bail!("delivered job cannot abort");}
            let a=job.attempt.as_mut().context("V2 attempt")?;
            if a.mode!=MODE {bail!("not an external V2 attempt");}
            let recovery=json!({"version":1,"job_id":id,"attempt_id":a.attempt_id,"session_id":a.session_id,
                "stop_id":stop_id,"fenced_at":now(),"execution_stop":{"status":"pending"},
                "accounting":{"status":"unobserved"}});
            a.dispatch_state="stop_pending".into();a.lease_expires_at=now();
            job.state="blocked".into();job.outcome=Some("aborted".into());
            job.blocker=Some("V2 execution stop pending; acceptance and reassignment fenced".into());
            tx.execute("INSERT INTO metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![format!("opencode-v2-recovery:{}",a.attempt_id),serde_json::to_string(&recovery)?])?;
            tx.execute("INSERT INTO metadata(key,value) VALUES(?1,?2)",params![format!("opencode-v2-stop:{stop_id}"),serde_json::to_string(&recovery)?])?;
            Store::save(tx,&mut job,"V2 abort fenced before external stop")?;Ok(job)
        })?;
        let _gate = self.gate.lock().await;
        let attempt = fenced.attempt.as_ref().context("V2 attempt")?;
        let latest = self.v2_recovery(&fenced)?.context("V2 recovery missing")?;
        if latest["stop_id"] != stop_id {
            bail!("V2 stop superseded by another operator request");
        }
        let result = self.interrupt_v2(&fenced).await;
        let mut record = latest;
        let error = match result {
            Ok((interruption, observation)) => {
                record["execution_stop"] = json!({"status":"confirmed","interruption":interruption,"evidence":observation.evidence});
                record["accounting"] = observation.accounting;
                None
            }
            Err(error) => {
                record["execution_stop"] =
                    json!({"status":"unconfirmed","reason":format!("{error:#}")});
                record["accounting"] =
                    json!({"status":"unobserved","reason":"No consistent execution-stop snapshot"});
                Some(error)
            }
        };
        record["observed_at"] = json!(now());
        let job = self.store.transaction(|tx| {
            let mut job = Store::load(tx, id)?;
            if job
                .attempt
                .as_ref()
                .is_none_or(|a| a.attempt_id != attempt.attempt_id)
            {
                bail!("V2 attempt changed during stop; no resolution");
            }
            let latest: String = tx.query_row(
                "SELECT value FROM metadata WHERE key=?1",
                [format!("opencode-v2-recovery:{}", attempt.attempt_id)],
                |r| r.get(0),
            )?;
            if serde_json::from_str::<Value>(&latest)?["stop_id"] != stop_id {
                bail!("V2 stop superseded; no stale resolution");
            }
            let stopped = record["execution_stop"]["status"] == "confirmed";
            let settled = record["accounting"]["status"] == "settled_zero";
            job.attempt.as_mut().unwrap().dispatch_state = if stopped && settled {
                "resolved"
            } else if stopped {
                "execution_stopped"
            } else {
                "stop_unconfirmed"
            }
            .into();
            job.state = "blocked".into();
            job.blocker = Some(if stopped && settled {
                "V2 execution stopped and zero accounting settled; operator may requeue".into()
            } else if stopped {
                format!(
                    "V2 execution stopped; accounting {}; acceptance and requeue remain fenced",
                    record["accounting"]["status"]
                )
            } else {
                "V2 execution stop unconfirmed; acceptance and requeue remain fenced".into()
            });
            for key in [
                format!("opencode-v2-recovery:{}", attempt.attempt_id),
                format!("opencode-v2-stop:{stop_id}"),
            ] {
                tx.execute(
                    "UPDATE metadata SET value=?2 WHERE key=?1",
                    params![key, serde_json::to_string(&record)?],
                )?;
            }
            Store::save(
                tx,
                &mut job,
                "V2 execution stop and accounting recorded separately",
            )?;
            Ok(job)
        })?;
        if let Some(error) = error {
            return Err(error.context("V2 stop remains unconfirmed; durable fence retained"));
        }
        Ok(job)
    }
    async fn interrupt_v2(&self, job: &Job) -> Result<(Value, Observation)> {
        let a = job.attempt.as_ref().context("V2 attempt")?;
        let session = a.session_id.as_deref().context("V2 session")?;
        let runtime = Native::new(a.server_url.as_deref().context("V2 endpoint")?)?;
        if runtime.call("/api/info", &a.worktree, false, &[]).await?["version"] != "2.0.29" {
            bail!("Unsupported V2 runtime");
        }
        let info = runtime
            .call(&format!("/api/session/{session}"), &a.worktree, false, &[])
            .await?["data"]
            .clone();
        if identity(&info, session, &a.worktree)? != self.v2_binding(job)? {
            bail!("V2 abort ownership changed");
        }
        let result = runtime
            .call(
                &format!("/api/session/{session}/interrupt"),
                &a.worktree,
                true,
                &[("resume", "false")],
            )
            .await?;
        if result["interrupted"].as_bool().is_none() {
            bail!("V2 interrupt observation unknown");
        }
        // Interrupting generation does not own or stop separately created background shells.
        // Only remove shells linked by ID to tool records in this exact bound session.
        let exported = runtime
            .call(
                &format!("/api/experimental/session/{session}/export"),
                &a.worktree,
                false,
                &[],
            )
            .await?;
        if identity(&exported["data"]["info"], session, &a.worktree)? != self.v2_binding(job)? {
            bail!("V2 shell-stop ownership changed");
        }
        let messages = exported["data"]["messages"]
            .as_array()
            .context("V2 shell-stop transcript unknown")?;
        audit_messages(messages, false)?;
        let owned: HashSet<&str> = messages
            .iter()
            .filter(|m| m["type"] == "assistant")
            .filter_map(|m| m["content"].as_array())
            .flatten()
            .filter(|c| c["type"] == "tool" && c["name"] == "shell")
            .filter_map(|c| c["state"]["metadata"]["shellID"].as_str())
            .collect();
        let location = a.worktree.to_str().context("V2 worktree")?;
        let shells = runtime
            .call(
                "/api/shell",
                &a.worktree,
                false,
                &[("location[directory]", location)],
            )
            .await?;
        if shells["location"]["directory"].as_str() != a.worktree.to_str() {
            bail!("V2 shell-stop scope mismatch");
        }
        let shells = shells["data"]
            .as_array()
            .context("V2 shell-stop inventory unknown")?;
        if shells.len() > 4096 {
            bail!("V2 shell-stop inventory exceeds bound");
        }
        for shell in shells.iter().filter(|s| s["status"] == "running") {
            let id = shell["id"].as_str().context("V2 shell-stop ID missing")?;
            let cwd = shell["cwd"].as_str().context("V2 shell-stop cwd missing")?;
            if !valid_id(id)
                || !owned.contains(id)
                || !Path::new(cwd).starts_with(&a.worktree)
                || Path::new(cwd)
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                bail!("V2 running shell has no bound-session ownership; no stop authority");
            }
            runtime
                .request(
                    &format!("/api/shell/{id}"),
                    &a.worktree,
                    reqwest::Method::DELETE,
                    &[("location[directory]", location)],
                )
                .await?;
        }
        let observation = self
            .observe_v2(job, session, Some(&self.v2_binding(job)?), true)
            .await?;
        Ok((result, observation))
    }
}
