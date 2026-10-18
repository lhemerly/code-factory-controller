//! Read-only loopback dashboard. Refreshes only approved local state files.
pub mod domain;
pub mod loader;
pub mod render;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
};
pub fn refresh(root: &Path, as_of: Option<&str>, stale_after: f64) -> Result<Value> {
    domain::build_dashboard(&loader::load_state(root)?, as_of, stale_after)
}
#[derive(clap::Args, Debug, Clone)]
pub struct Options {
    #[arg(long)]
    pub state_root: Option<PathBuf>,
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    #[arg(long, default_value_t = 8077)]
    pub port: u16,
    #[arg(long, default_value_t = 900)]
    pub stale_after: u64,
    #[arg(long)]
    pub as_of: Option<String>,
    #[arg(long, conflicts_with = "print_html")]
    pub print_json: bool,
    #[arg(long, conflicts_with = "print_json")]
    pub print_html: bool,
    #[arg(long)]
    pub verbose: bool,
}
async fn response(
    method: axum::http::Method,
    uri: axum::http::Uri,
    axum::extract::State(options): axum::extract::State<Arc<Options>>,
) -> axum::response::Response {
    use axum::{
        http::{StatusCode, header},
        response::IntoResponse,
    };
    if method != axum::http::Method::GET {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, "GET"), (header::CONTENT_LENGTH, "0")],
            "",
        )
            .into_response();
    }
    let path = uri.path();
    if !matches!(path, "/" | "/index.html" | "/api/state") {
        return (
            StatusCode::NOT_FOUND,
            [
                (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            "not found\n",
        )
            .into_response();
    }
    let result = refresh(
        options.state_root.as_ref().unwrap(),
        options.as_of.as_deref(),
        options.stale_after as f64,
    );
    let (status, body, kind) = match result {
        Ok(d) => {
            if path == "/api/state" {
                (
                    StatusCode::OK,
                    serde_json::to_string_pretty(&d).unwrap(),
                    "application/json; charset=utf-8",
                )
            } else {
                (StatusCode::OK, render::html(&d), "text/html; charset=utf-8")
            }
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            render::error(&e.to_string()),
            "text/html; charset=utf-8",
        ),
    };
    (
        status,
        [
            (header::CONTENT_TYPE, kind),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}
pub async fn run_cli(mut options: Options) -> Result<()> {
    let root = options.state_root.clone().unwrap_or_else(|| {
        std::env::var_os("FACTORY_DASHBOARD_STATE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("data"))
    });
    options.state_root = Some(root.clone());
    if options.print_json || options.print_html {
        let d = refresh(&root, options.as_of.as_deref(), options.stale_after as f64)?;
        println!(
            "{}",
            if options.print_json {
                serde_json::to_string_pretty(&d)?
            } else {
                render::html(&d)
            }
        );
        return Ok(());
    }
    let host: IpAddr = if options.host == "localhost" {
        "127.0.0.1".parse()?
    } else {
        options
            .host
            .parse()
            .context("literal loopback address required")?
    };
    ensure!(host.is_loopback(), "dashboard serving requires loopback");
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(host, options.port)).await?;
    println!(
        "read-only dashboard on http://{}/ (loopback; refresh = file reads only, zero model tokens)",
        listener.local_addr()?
    );
    let app = axum::Router::new()
        .fallback(response)
        .with_state(Arc::new(options));
    axum::serve(listener, app).await?;
    Ok(())
}
