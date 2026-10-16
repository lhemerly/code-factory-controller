use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use code_factory_controller::{engine::Engine, model::*};
use serde_json::json;
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
}
fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Command::AgySupervisor { plan } = &cli.command {
        return code_factory_controller::supervisor::run_worker(plan);
    }
    if let Command::VerifierSupervisor { plan } = &cli.command {
        return code_factory_controller::supervisor::run(plan);
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(cli))
}
async fn run(cli: Cli) -> Result<()> {
    match cli.command {
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
    }
    Ok(())
}
