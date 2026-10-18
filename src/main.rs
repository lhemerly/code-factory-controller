use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use code_factory_controller::{engine::Engine, model::*};
use serde_json::{Value, json};
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    time::Duration,
};
#[derive(Parser)]
#[command(version, about = "Durable controller for code factories")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Whole-goal acceptance or read-only receipt audit (JSON request on stdin).
    GoalAcceptance,
    /// Execute an approved durable backlog with whole-goal acceptance.
    Run(code_factory_controller::goal_runner::Options),
    #[command(hide = true)]
    AgySupervisor {
        #[arg(long)]
        plan: PathBuf,
    },
    #[command(hide = true)]
    VerifierSupervisor {
        #[arg(long)]
        plan: PathBuf,
    },
    /// Submit approved work and watch jobs, verification and delivery.
    Tui {
        #[arg(long, default_value = "http://127.0.0.1:4317")]
        url: String,
        /// Optional local goal-runner state file to display alongside job status.
        #[arg(long)]
        goal_state: Option<PathBuf>,
        /// Explore the interface without connecting or submitting work.
        #[arg(long)]
        demo: bool,
        /// Print a terminal preview instead of opening an interactive terminal.
        #[arg(long)]
        snapshot: bool,
    },
    /// Audit one persisted job and delivered archive offline; never dispatches work.
    Inspect {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        job: String,
    },
    /// Read or record a durable standing goal offline; never dispatches work.
    Goal {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        input: Option<PathBuf>,
    },
    /// Inspect installed agy version and existing policy without submitting inference.
    AgyPreflight {
        #[arg(long)]
        config: PathBuf,
    },
    Serve {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, default_value = "127.0.0.1:4317")]
        listen: SocketAddr,
    },
    Operator {
        operation: String,
        #[arg(long)]
        job: Option<String>,
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long, default_value = "http://127.0.0.1:4317")]
        url: String,
    },
    Schemas {
        #[arg(long, default_value = "schemas")]
        output: PathBuf,
    },
    /// Read-only audit of an owner-pinned contract-only permission-block resolution.
    PermissionBlockResolution {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        controller: PathBuf,
        #[arg(long)]
        next_sequence: u64,
    },
    /// Audit an explicit owner's stopped attempt; never resolve a native access denial.
    OwnerInterruption {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        controller: PathBuf,
        #[arg(long)]
        next_sequence: u64,
    },
    /// Owner-only reconciliation of a reserved identifier that never became a job.
    PreAdmissionRecovery {
        #[command(subcommand)]
        action: PreAdmissionAction,
    },
    /// Local host controls for spark code factory.
    SparkControl {
        action: String,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Read-only, fail-closed preflight for local Bonsai runtime.
    CheckBonsai {
        #[arg(long, default_value = "http://127.0.0.1:4096")]
        opencode_url: String,
    },
    /// Offline, non-authoritative ledger for explicitly reviewed sanitized summaries.
    TraceLedger {
        operation: String,
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        source_id: Option<String>,
        #[arg(long)]
        format: Option<String>,
        #[arg(long)]
        evidence_kind: Option<String>,
        #[arg(long)]
        reviewed_sanitized: bool,
    },
}

#[derive(Subcommand)]
enum PreAdmissionAction {
    Audit {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        directory: PathBuf,
        #[arg(long)]
        controller: PathBuf,
    },
    Reconcile {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        goal: String,
        #[arg(long)]
        owned_receipt: PathBuf,
    },
    AdvanceScope {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        goal: String,
        #[arg(long)]
        published_state: PathBuf,
        #[arg(long)]
        repair_input: String,
    },
}
fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Command::AgySupervisor { plan } = &cli.command {
        return code_factory_controller::supervisor::run_worker(plan);
    }
    if let Command::VerifierSupervisor { plan } = &cli.command {
        return code_factory_controller::supervisor::run(plan);
    }
    if let Command::GoalAcceptance = &cli.command {
        use std::io::Read;
        let mut input = Vec::new();
        std::io::stdin().take(16_777_217).read_to_end(&mut input)?;
        anyhow::ensure!(input.len() <= 16_777_216, "request exceeds limit");
        let result = code_factory_controller::goal_acceptance::request(
            &serde_json::from_slice(&input)?,
            &std::env::current_exe()?,
        )?;
        println!("{}", serde_json::to_string(&result)?);
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(cli))
}
async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Run(options) => {
            let code =
                code_factory_controller::goal_runner::run(options, &std::env::current_exe()?)
                    .await?;
            std::process::exit(code);
        }
        Command::GoalAcceptance => unreachable!("handled before runtime initialization"),
        Command::AgySupervisor { .. } => unreachable!("handled before runtime initialization"),
        Command::VerifierSupervisor { .. } => unreachable!("handled before runtime initialization"),
        Command::Tui {
            url,
            goal_state,
            demo,
            snapshot,
        } => {
            code_factory_controller::tui::run(url, goal_state, demo, snapshot).await?;
        }
        Command::Inspect { config, job } => {
            let engine = Engine::open(Config::load(&config)?)?;
            let saved = engine.reconcile_delivery(&job)?;
            println!("{}", serde_json::to_string_pretty(&engine.status(&saved))?);
        }
        Command::Goal { config, input } => {
            let engine = Engine::open(Config::load(&config)?)?;
            let proposed = input
                .map(|path| -> Result<serde_json::Value> {
                    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
                })
                .transpose()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&engine.standing_goal(proposed)?)?
            );
        }
        Command::AgyPreflight { config } => {
            let engine = Engine::open(Config::load(&config)?)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&engine.agy_preflight().await?)?
            );
        }
        Command::Serve { config, listen } => {
            if !matches!(listen.ip(),IpAddr::V4(ip)if ip.is_loopback())
                && !matches!(listen.ip(),IpAddr::V6(ip)if ip.is_loopback())
            {
                bail!("remote exposure is not supported");
            }
            let token = std::env::var("FACTORY_OPERATOR_TOKEN")
                .context("FACTORY_OPERATOR_TOKEN required")?;
            if token.len() < 32 {
                bail!("operator token must be at least 32 characters");
            }
            let engine = Engine::open(Config::load(&config)?)?;
            let app = code_factory_controller::http::router(engine.clone(), &token);
            let listener = tokio::net::TcpListener::bind(listen).await?;
            println!("controller listening on http://{}", listener.local_addr()?);
            let events = tokio::spawn(code_factory_controller::adapter::watch_events(
                engine.clone(),
            ));
            let scheduler = tokio::spawn(async move {
                loop {
                    let jobs = engine.store.all().unwrap_or_default();
                    for job in jobs {
                        if job.state == "done" {
                            engine.reconcile_delivery(&job.input.job_id).ok();
                            continue;
                        }
                        if let Err(e) = engine.adapter_tick(&job.input.job_id).await {
                            eprintln!("adapter reconciliation deferred: {e}");
                        }
                        if let Err(e) = engine.advance(&job.input.job_id).await {
                            eprintln!("durable step requires reconciliation: {e}");
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    tokio::select! { _ = engine.wake.notified() => {}, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
                }
            });
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    tokio::signal::ctrl_c().await.ok();
                })
                .await?;
            scheduler.abort();
            events.abort();
        }
        Command::Operator {
            operation,
            job,
            input,
            url,
        } => {
            let u = reqwest::Url::parse(&url)?;
            if u.scheme() != "http" || !matches!(u.host_str(), Some("127.0.0.1" | "[::1]")) {
                bail!("operator endpoint must be loopback");
            }
            let token =
                std::env::var("FACTORY_OPERATOR_TOKEN").context("operator token required")?;
            let body = if let Some(input) = input {
                serde_json::from_slice(&std::fs::read(input)?)?
            } else if let Some(job) = job {
                json!({"job_id":job})
            } else {
                json!({})
            };
            let result = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?
                .post(format!(
                    "{}/operator/{operation}",
                    url.trim_end_matches('/')
                ))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await?;
            let status = result.status();
            let body = result.text().await?;
            println!("{body}");
            if !status.is_success() {
                bail!("operator command failed");
            }
        }
        Command::Schemas { output } => {
            std::fs::create_dir_all(&output)?;
            for (name, schema) in [
                ("config", schemars::schema_for!(Config)),
                ("approved-job", schemars::schema_for!(JobInput)),
                ("worker-request", schemars::schema_for!(WorkerRequest)),
                ("verification-receipt", schemars::schema_for!(Receipt)),
            ] {
                std::fs::write(
                    output.join(format!("{name}.json")),
                    serde_json::to_string_pretty(&schema)? + "\n",
                )?;
            }
        }
        Command::PermissionBlockResolution {
            root,
            directory,
            controller,
            next_sequence,
        } => {
            let res = code_factory_controller::recovery::audit_permission_block_resolution(
                &root,
                &directory,
                &controller,
                next_sequence,
            )?;
            println!("{}", serde_json::to_string(&res)?);
        }
        Command::OwnerInterruption {
            root,
            directory,
            controller,
            next_sequence,
        } => {
            let res = code_factory_controller::recovery::audit_owner_interruption(
                &root,
                &directory,
                &controller,
                next_sequence,
            )?;
            println!("{}", serde_json::to_string(&res)?);
        }
        Command::PreAdmissionRecovery { action } => match action {
            PreAdmissionAction::Audit {
                root,
                directory,
                controller,
            } => {
                let res = code_factory_controller::recovery::audit_pre_admission(
                    &root,
                    &directory,
                    &controller,
                )?;
                println!("{}", serde_json::to_string(&res)?);
            }
            PreAdmissionAction::Reconcile {
                manifest,
                goal,
                owned_receipt,
            } => {
                let res = code_factory_controller::recovery::reconcile_pre_admission(
                    &manifest,
                    &goal,
                    &owned_receipt,
                )?;
                println!("{}", serde_json::to_string(&res)?);
            }
            PreAdmissionAction::AdvanceScope {
                manifest,
                goal,
                published_state,
                repair_input,
            } => {
                let res = code_factory_controller::recovery::advance_scope_base(
                    &manifest,
                    &goal,
                    &published_state,
                    &repair_input,
                )?;
                println!("{}", serde_json::to_string(&res)?);
            }
        },
        Command::SparkControl {
            action,
            root,
            reason,
        } => match action.as_str() {
            "status" => {
                let out = code_factory_controller::recovery::spark_status(&root)?;
                print!("{out}");
            }
            "pause" => {
                let out = code_factory_controller::recovery::spark_pause(&root, reason.as_deref())?;
                println!("{out}");
            }
            "resume" => {
                let r = reason.context(
                    "resume requires an explicit --reason after inspecting recovery evidence",
                )?;
                let out = code_factory_controller::recovery::spark_resume(&root, &r)?;
                println!("{out}");
            }
            other => bail!("unsupported spark action: {other}"),
        },
        Command::CheckBonsai { opencode_url } => {
            match code_factory_controller::check_bonsai::check(&opencode_url).await {
                Ok(val) => {
                    println!("{}", serde_json::to_string(&val)?);
                }
                Err(e) => {
                    println!(
                        "{}",
                        serde_json::to_string(&json!({
                            "passed": false,
                            "error": e.to_string(),
                        }))?
                    );
                    std::process::exit(1);
                }
            }
        }
        Command::TraceLedger {
            operation,
            db,
            input,
            source_id,
            format,
            evidence_kind,
            reviewed_sanitized,
        } => {
            let res = (|| -> Result<Value> {
                let mut document = None;
                if operation != "report" {
                    ensure!(
                        input.is_some()
                            && source_id.is_some()
                            && format.is_some()
                            && evidence_kind.is_some()
                            && reviewed_sanitized,
                        "explicit input, source identity, format, kind and sanitization review acknowledgment required"
                    );
                    let raw = code_factory_controller::trace_ledger::read_reviewed(
                        input.as_ref().unwrap(),
                    )?;
                    document = Some(code_factory_controller::trace_ledger::normalize(
                        &raw,
                        format.as_ref().unwrap(),
                        evidence_kind.as_ref().unwrap(),
                    )?);
                }
                let mut ledger = code_factory_controller::trace_ledger::Ledger::open(
                    &db,
                    operation != "import",
                )?;
                if operation == "report" {
                    ledger.report()
                } else if operation == "replay" {
                    ledger.replay(source_id.as_deref().unwrap(), document.as_ref().unwrap())
                } else if operation == "import" {
                    ledger.import_summary(source_id.as_deref().unwrap(), document.as_ref().unwrap())
                } else {
                    bail!("unsupported operation: {operation}")
                }
            })();
            match res {
                Ok(val) => {
                    println!("{}", code_factory_controller::trace_ledger::canonical(&val));
                }
                Err(_) => {
                    println!(
                        "{}",
                        code_factory_controller::trace_ledger::canonical(&json!({
                            "passed": false,
                            "error": "trace operation rejected; check schema, bounds, provenance and local file access",
                        }))
                    );
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}
