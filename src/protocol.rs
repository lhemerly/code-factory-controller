use crate::{engine::Engine, execution, model::*, store::Store};
use anyhow::{Result, bail};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
impl Engine {
    pub fn capabilities(&self) -> Value {
        json!({"protocol_min":VERSION,"protocol_max":VERSION,"operations":OPERATIONS,"worker_adapters":["opencode"],"verification_profiles":self.config.profiles.iter().map(|p|&p.id).collect::<Vec<_>>()})
    }
    pub fn grant(&self, token: &str, session: &str) -> Result<Job> {
        self.store.transaction(|tx| {
            let id = Store::authorize(tx, token, session)?;
            let job = Store::load(tx, &id)?;
            if job
                .attempt
                .as_ref()
                .is_none_or(|a| a.session_id.as_deref() != Some(session))
            {
                bail!("revoked session");
            }
            Ok(job)
        })
    }
    pub fn reconcile(&self, token: &str, session: &str, request_id: &str) -> Result<Value> {
        self.grant(token, session)?;
        self.store.transaction(|tx| {
            if let Some((payload, response)) = Store::request(tx, request_id)? {
                if payload["session_id"] != session
                    || payload["project_id"] != self.config.project_id
                {
                    bail!("scope mismatch");
                }
                Ok(json!({"status":"complete","payload":payload,"response":response}))
            } else {
                Ok(json!({"status":"not_found"}))
            }
        })
    }
    pub async fn worker(&self, operation: &str, token: &str, payload: Value) -> Result<Value> {
        let request: WorkerRequest = serde_json::from_value(payload.clone())?;
        let fail = |code: &str| Ok(request.error(code));
        if request.protocol_version != VERSION {
            return fail("incompatible_version");
        }
        if request.project_id != self.config.project_id
            || !valid_id(&request.request_id)
            || !valid_id(&request.session_id)
        {
            return fail("policy_denied");
        }
        let bound = match self.grant(token, &request.session_id) {
            Ok(j) => j,
            Err(_) => return fail("policy_denied"),
        };
        if !OPERATIONS.contains(&operation) {
            return fail("unsupported_capability");
        }
        let mut allowed = vec!["protocol_version", "request_id", "project_id", "session_id"];
        match operation {
            "factory_status" => allowed.push("job_id"),
            "factory_next_task" => allowed.extend(["adapter_capabilities", "idempotency_key"]),
            "factory_request_verification" => allowed.extend([
                "job_id",
                "attempt_id",
                "lease_id",
                "candidate_commit",
                "verification_profile_id",
                "idempotency_key",
            ]),
            "factory_submit_candidate" => allowed.extend([
                "job_id",
                "attempt_id",
                "lease_id",
                "candidate_commit",
                "verification_id",
                "change_summary",
                "artifact_manifest",
                "idempotency_key",
            ]),
            _ => unreachable!(),
        }
        if payload
            .as_object()
            .is_none_or(|p| p.keys().any(|k| !allowed.contains(&k.as_str())))
        {
            return fail("validation_failed");
        }
        if request
            .job_id
            .as_ref()
            .is_some_and(|id| id != &bound.input.job_id)
        {
            return fail("policy_denied");
        }
        if operation == "factory_status" {
            return Ok(request.envelope(self.status(&bound)));
        }
        let key = match request.idempotency_key.as_ref() {
            Some(k) if valid_id(k) => k,
            _ => return fail("validation_failed"),
        };
        let expected = digest(serde_json::to_vec(&json!([
            VERSION,
            self.config.project_id,
            request.session_id,
            operation,
            key
        ]))?);
        if request.request_id != expected {
            return fail("validation_failed");
        }
        let _gate = self.gate.lock().await;
        if let Some((original, response)) = self
            .store
            .transaction(|tx| Store::request(tx, &request.request_id))?
        {
            return if original == payload {
                Ok(response)
            } else {
                fail("idempotency_conflict")
            };
        }
        let job = self.get(&bound.input.job_id)?;
        if !job.active() || !["running", "verifying"].contains(&job.state.as_str()) {
            return fail("stale_lease");
        }
        if self.check_v2(&job, false).await.is_err() {
            return fail("policy_denied");
        }
        if operation != "factory_next_task" {
            let a = job.attempt.as_ref().unwrap();
            if request.job_id.as_deref() != Some(&job.input.job_id)
                || request.attempt_id.as_deref() != Some(&a.attempt_id)
                || request.lease_id.as_deref() != Some(&a.lease_id)
            {
                return fail("stale_lease");
            }
            let candidate = match request.candidate_commit.as_deref() {
                Some(c) if valid_commit(c) => c,
                _ => return fail("validation_failed"),
            };
            if execution::scope(&self.config.repository, &job, candidate)
                .await
                .is_err()
            {
                return fail("policy_denied");
            }
        }
        self.store.transaction(|tx|{
   let existing:Option<String>=tx.query_row("SELECT id FROM requests WHERE project=?1 AND session=?2 AND operation=?3 AND key=?4",params![request.project_id,request.session_id,operation,key],|r|r.get(0)).optional()?;
   if existing.is_some(){return fail("idempotency_conflict");}
   let mut j=Store::load(tx,&job.input.job_id)?;if !j.active()||!["running","verifying"].contains(&j.state.as_str()){return fail("stale_lease");}
   let result=match operation {
    "factory_next_task"=>{if request.adapter_capabilities.as_ref().is_none_or(|c|c.len()>20||!c.iter().any(|s|s=="opencode")){return fail("unsupported_capability");}
if j.attempt.as_ref().unwrap().claimed {json!({"kind":"no_task"})}else{j.attempt.as_mut().unwrap().claimed=true;json!({"kind":"assignment","assignment":j.assignment(&self.config)})}},
    "factory_request_verification"=>{if request.verification_profile_id.as_deref()!=Some(&j.profile.id){return fail("unsupported_capability");}
if j.integrated_commit.is_some()||j.verification_pending.is_some(){return fail("policy_denied");}let verification_id=new_id();j.candidate=request.candidate_commit.clone();j.receipt=None;j.candidate_receipt=None;j.verification_pending=Some(verification_id.clone());j.state="verifying".into();j.blocker=None;json!({"verification_id":verification_id,"status":"queued"})},
    "factory_submit_candidate"=>{
     if request.change_summary.as_ref().is_none_or(|s|s.is_empty()||s.len()>4000)||request.artifact_manifest.as_ref().is_none_or(|a|a.len()>100){return fail("validation_failed");}
     let receipt=match j.candidate_receipt.as_ref(){Some(r)if r.outcome=="passed"&&Some(&r.verification_id)==request.verification_id.as_ref()&&Some(&r.candidate_commit)==request.candidate_commit.as_ref()=>r,_=>return fail("policy_denied")};
     if serde_json::to_value(&receipt.artifacts)?!=serde_json::to_value(&request.artifact_manifest)?{return fail("policy_denied");}
     for artifact in &receipt.artifacts{let file=artifact.reference.strip_prefix("artifact:").ok_or_else(||anyhow::anyhow!("artifact reference"))?;if !relative(file)||file.contains('/')||digest(std::fs::read(self.config.data_dir.join("artifacts").join(file))?)!=artifact.sha256{return fail("policy_denied");}}
     if j.integrated_commit.is_some()||j.verification_pending.is_some(){return fail("policy_denied");}j.outcome=Some("accepted_for_integration".into());j.state="verifying".into();json!({"status":"accepted_for_integration","reasons":[],"state_revision":j.revision+1})
    },_=>unreachable!()
   };
   Store::save(tx,&mut j,operation)?;let response=request.envelope(result);tx.execute("INSERT INTO requests(id,project,session,operation,key,payload,response) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![request.request_id,request.project_id,request.session_id,operation,key,serde_json::to_string(&payload)?,serde_json::to_string(&response)?])?;Ok(response)
  })
    }
}
