#![allow(dead_code)]
use code_factory_controller::{engine::Engine, execution, model::*};
use serde_json::{Value, json};
use std::sync::Arc;
use tempfile::TempDir;
pub const TOKEN: &str = "fixture-worker-capability-with-32-characters";
pub struct Fixture {
    pub temp: TempDir,
    pub config: Config,
    pub engine: Arc<Engine>,
    pub base: String,
}
impl Fixture {
    pub async fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let repository = temp.path().join("repository");
        std::fs::create_dir(&repository).unwrap();
        execution::git(&repository, &["init", "-b", "main"])
            .await
            .unwrap();
        std::fs::write(repository.join("README.md"), "approved baseline\n").unwrap();
        execution::git(&repository, &["add", "."]).await.unwrap();
        execution::git(&repository, &["commit", "-m", "baseline"])
            .await
            .unwrap();
        let base = execution::git(&repository, &["rev-parse", "HEAD"])
            .await
            .unwrap();
        let config = Config {
            project_id: "fixture".into(),
            repository,
            data_dir: temp.path().join("controller"),
            delivery_dir: temp.path().join("delivery"),
            integration_ref: "refs/heads/factory-integration".into(),
            profiles: vec![Profile {
                id: "fixture-check".into(),
                version: "1".into(),
                timeout_seconds: 5,
                checks: vec![Check {
                    id: "result".into(),
                    argv: vec!["/usr/bin/test".into(), "-s".into(), "result.txt".into()],
                    result_contract: None,
                }],
            }],
            policy: ModelPolicy {
                models: vec!["opencode/big-pickle".into()],
                free_only: true,
                spend_ceiling: 0.0,
                data_approved: true,
                cooldown_seconds: 2,
            },
            max_workers: 2,
            lease_seconds: 3600,
            max_attempts: 3,
            opencode_url: None,
        };
        let engine = Engine::open(config.clone()).unwrap();
        Self {
            temp,
            config,
            engine,
            base,
        }
    }
    pub fn input(&self, id: &str) -> JobInput {
        JobInput {
            job_id: id.into(),
            base_commit: self.base.clone(),
            task_brief: "Produce the approved result file".into(),
            acceptance_criteria: vec![Criterion {
                behavior: "result exists".into(),
                check_ids: vec!["result".into()],
                expected: "nonempty result.txt".into(),
            }],
            allowed_paths: vec!["result.txt".into(), "extra.txt".into()],
            protected_paths: vec!["README.md".into()],
            verification_profile_id: "fixture-check".into(),
            policy_version: "1".into(),
        }
    }
    pub async fn assigned(&self, id: &str, session: &str, token: &str) -> Job {
        self.engine.admit(self.input(id)).await.unwrap();
        self.engine.prepare(id, "manual").await.unwrap();
        self.engine.bind(id, session, token).unwrap()
    }
    pub async fn commit(job: &Job, path: &str, content: &str) -> String {
        let w = &job.attempt.as_ref().unwrap().worktree;
        std::fs::write(w.join(path), content).unwrap();
        execution::git(w, &["add", path]).await.unwrap();
        execution::git(w, &["commit", "-m", "approved change"])
            .await
            .unwrap();
        execution::git(w, &["rev-parse", "HEAD"]).await.unwrap()
    }
    pub fn request(&self, session: &str, op: &str, key: &str, extra: Value) -> Value {
        let mut body = json!({"protocol_version":VERSION,"project_id":"fixture","session_id":session,"request_id":digest(serde_json::to_vec(&json!([VERSION,"fixture",session,op,key])).unwrap()),"idempotency_key":key});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        body
    }
    pub fn verify_request(&self, j: &Job, commit: &str, key: &str) -> Value {
        let a = j.attempt.as_ref().unwrap();
        self.request(a.session_id.as_ref().unwrap(),"factory_request_verification",key,json!({"job_id":j.input.job_id,"attempt_id":a.attempt_id,"lease_id":a.lease_id,"candidate_commit":commit,"verification_profile_id":j.profile.id}))
    }
    pub fn submit_request(&self, j: &Job, key: &str) -> Value {
        let a = j.attempt.as_ref().unwrap();
        let r = j.candidate_receipt.as_ref().unwrap();
        self.request(a.session_id.as_ref().unwrap(),"factory_submit_candidate",key,json!({"job_id":j.input.job_id,"attempt_id":a.attempt_id,"lease_id":a.lease_id,"candidate_commit":r.candidate_commit,"verification_id":r.verification_id,"artifact_manifest":r.artifacts,"change_summary":"Approved file produced"}))
    }
    pub async fn candidate(&self, id: &str, session: &str, token: &str, content: &str) -> Job {
        let j = self.assigned(id, session, token).await;
        let c = Self::commit(&j, "result.txt", content).await;
        let response = self
            .engine
            .worker(
                "factory_request_verification",
                token,
                self.verify_request(&j, &c, "verify"),
            )
            .await
            .unwrap();
        assert!(response.get("result").is_some(), "{response}");
        self.engine.advance(id).await.unwrap()
    }
    pub async fn accepted(&self, id: &str) -> Job {
        let j = self.candidate(id, "session", TOKEN, "verified\n").await;
        assert_eq!(j.receipt.as_ref().unwrap().outcome, "passed");
        let response = self
            .engine
            .worker(
                "factory_submit_candidate",
                TOKEN,
                self.submit_request(&j, "submit"),
            )
            .await
            .unwrap();
        assert_eq!(response["result"]["status"], "accepted_for_integration");
        self.engine.get(id).unwrap()
    }
}
