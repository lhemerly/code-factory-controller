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
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
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
            } else {
                json!({"job_id":job})
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
