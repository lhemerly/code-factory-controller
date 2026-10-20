//! Operator interface. The controller remains the sole owner of job transitions.
pub mod data;
mod view;

use anyhow::{Context, Result, bail};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
};
use data::{
    App, Draft, JobEvent, Modal, Overview, SetupStage, SetupState, safe_text, save_config,
    validate_input,
};
use ratatui::{Terminal, backend::TestBackend};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

#[derive(Clone)]
struct Client {
    http: reqwest::Client,
    url: String,
    token: String,
}

fn origin(url: &str) -> Result<String> {
    let parsed = reqwest::Url::parse(url)?;
    if parsed.scheme() != "http"
        || !matches!(parsed.host_str(), Some("127.0.0.1" | "[::1]"))
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        bail!("Controller URL must be an HTTP loopback origin, for example http://127.0.0.1:4317");
    }
    Ok(parsed.as_str().trim_end_matches('/').into())
}

impl Client {
    fn new(url: &str) -> Result<Self> {
        let token = std::env::var("FACTORY_OPERATOR_TOKEN").context(
            "Set FACTORY_OPERATOR_TOKEN to the same operator capability used by the controller",
        )?;
        if token.len() < 32 {
            bail!("Operator token must contain at least 32 characters");
        }
        Ok(Self {
            url: origin(url)?,
            token,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(45))
                .build()?,
        })
    }

    async fn call(&self, operation: &str, body: Value) -> Result<Value> {
        let mut response = self
            .http
            .post(format!("{}/operator/{operation}", self.url))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Controller request interrupted or timed out"))?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .context("Controller response interrupted")?
        {
            if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
                bail!("Controller response exceeds 8 MiB");
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value =
            serde_json::from_slice(&bytes).context("Controller returned invalid JSON")?;
        if !status.is_success() {
            bail!(
                "{}: {}",
                status,
                safe_text(value["error"].as_str().unwrap_or("Request rejected"))
            );
        }
        Ok(value)
    }
}

#[derive(Debug)]
enum Effect {
    Action(String, Value),
    Log(String, String),
}
enum Update {
    Snapshot(u64, Instant, Result<(Overview, Vec<JobEvent>, u64)>),
    Action(String, Result<Value>),
    Log(String, Result<Value>),
}

#[derive(Deserialize)]
struct Events {
    events: Vec<JobEvent>,
    next_seq: u64,
}

/// Render a preview, or connect an interactive terminal to a running controller.
pub async fn run(
    url: String,
    goal_state: Option<PathBuf>,
    demo: bool,
    snapshot: bool,
) -> Result<()> {
    if !snapshot && (!io::stdin().is_terminal() || !io::stdout().is_terminal()) {
        bail!("The TUI needs an interactive terminal; use --demo --snapshot for a text preview");
    }
    let mut app = App::new(origin(&url)?, demo);
    let client = if demo { None } else { Some(Client::new(&url)?) };
    if demo {
        app.apply_overview(demo_overview());
        app.notice = "Preview · no controller actions".into();
    }
    read_goal(&mut app, goal_state.as_deref());
    if snapshot {
        if let Some(client) = client {
            app.apply_overview(serde_json::from_value(
                client.call("overview", json!({})).await?,
            )?);
        }
        let mut terminal = Terminal::new(TestBackend::new(124, 40))?;
        terminal.draw(|frame| view::draw(frame, &app))?;
        for y in 0..40 {
            let line: String = (0..124)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect();
            println!("{}", line.trim_end());
        }
        return Ok(());
    }
    let mut terminal = ratatui::try_init()?;
    let _restore = RestoreTerminal;
    execute!(io::stdout(), event::EnableBracketedPaste)?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut polling = false;
    let mut poll_at = Instant::now();
    let mut after_seq = 0;
    let mut generation = 0_u64;
    loop {
        while let Ok(update) = rx.try_recv() {
            match update {
                Update::Snapshot(observed_generation, observed, result) => {
                    polling = false;
                    if observed_generation != generation {
                        poll_at = Instant::now();
                        continue;
                    }
                    poll_at = Instant::now() + Duration::from_secs(2);
                    match result {
                        Ok((overview, events, next_seq)) => {
                            app.apply_observed_overview(overview, observed);
                            after_seq = next_seq;
                            app.events.extend(events);
                            if app.events.len() > 500 {
                                app.events.drain(..app.events.len() - 500);
                            }
                        }
                        Err(error) => app.error = Some(safe_text(&error.to_string())),
                    }
                }
                Update::Action(operation, result) => {
                    generation += 1;
                    app.busy = false;
                    // The next authoritative read must succeed before another write.
                    app.last_refresh = None;
                    app.notice = match result {
                        Ok(_) => format!("{operation} accepted · refreshing controller state"),
                        Err(error) => format!(
                            "{operation}: {error}. Inspect refreshed state before retrying; request outcome may be unknown."
                        ),
                    };
                    poll_at = Instant::now();
                }
                Update::Log(title, result) => {
                    apply_log(&mut app, title, result);
                }
            }
        }
        if !polling && Instant::now() >= poll_at {
            read_goal(&mut app, goal_state.as_deref());
            if let Some(client) = client.clone() {
                polling = true;
                let tx = tx.clone();
                tokio::spawn(async move {
                    let observed = Instant::now();
                    let result = async {
                        let overview =
                            serde_json::from_value(client.call("overview", json!({})).await?)?;
                        let events: Events = serde_json::from_value(
                            client
                                .call(
                                    "events",
                                    json!({"after_seq":after_seq,"limit":200,"tail":after_seq==0}),
                                )
                                .await?,
                        )?;
                        Ok((overview, events.events, events.next_seq))
                    }
                    .await;
                    let _ = tx.send(Update::Snapshot(generation, observed, result));
                });
            } else {
                app.last_refresh = Some(Instant::now());
                poll_at = Instant::now() + Duration::from_secs(2);
            }
        }
        terminal.draw(|frame| view::draw(frame, &app))?;
        if event::poll(Duration::from_millis(80))? {
            let effect = match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        break;
                    }
                    if app.modal.is_none() && key.code == KeyCode::Char('q') {
                        break;
                    }
                    handle_key(&mut app, key)
                }
                Event::Paste(text) => {
                    paste(&mut app, &text);
                    None
                }
                _ => None,
            };
            if let (Some(effect), Some(client)) = (effect, client.clone()) {
                app.busy = true;
                if matches!(&effect, Effect::Action(..)) {
                    generation += 1;
                    app.last_refresh = None;
                }
                let tx = tx.clone();
                tokio::spawn(async move {
                    let update = match effect {
                        Effect::Action(operation, body) => {
                            let result = client.call(&operation, body).await;
                            Update::Action(operation, result)
                        }
                        Effect::Log(job_id, reference) => {
                            let result = client
                                .call(
                                    "log",
                                    json!({"job_id":job_id,"artifact_reference":reference}),
                                )
                                .await;
                            Update::Log(reference, result)
                        }
                    };
                    let _ = tx.send(update);
                });
            }
        }
        // Let network tasks progress even when keyboard input arrives continuously.
        tokio::task::yield_now().await;
    }
    Ok(())
}

/// Guided, controller-less creation of a project config file.
pub fn run_setup(output: PathBuf) -> Result<()> {
    let mut state = SetupState::new(output.display().to_string());
    let mut terminal = ratatui::try_init()?;
    let _restore = RestoreTerminal;
    loop {
        terminal.draw(|frame| view::draw_setup(frame, &mut state))?;
        if event::poll(Duration::from_millis(80))?
            && let Event::Key(key) = event::read()?
            && handle_setup_key(&mut state, key)
        {
            break;
        }
    }
    Ok(())
}

fn handle_setup_key(state: &mut SetupState, key: KeyEvent) -> bool {
    if key.kind != KeyEventKind::Press {
        return false;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match &mut state.stage {
        SetupStage::Form => match key.code {
            KeyCode::Esc => return true,
            KeyCode::Tab => {
                let n = state.draft.field_count();
                state.draft.field = (state.draft.field + 1) % n;
            }
            KeyCode::BackTab => {
                let n = state.draft.field_count();
                state.draft.field = (state.draft.field + n - 1) % n;
            }
            KeyCode::Char('u') if ctrl => {
                if let Some(field) = state.draft.current_field() {
                    state.draft.set_field(&field, "");
                }
            }
            KeyCode::Char('n') if ctrl => state.draft.add_check(),
            KeyCode::Char('p') if ctrl => state.draft.add_profile(),
            KeyCode::Char('d') if ctrl => state.draft.delete_current(),
            KeyCode::Char('s') if ctrl => match state.draft.build() {
                Ok(config) => {
                    state.stage = SetupStage::Review {
                        config: Box::new(config),
                        error: None,
                    };
                    state.scroll = 0;
                }
                Err(error) => state.draft.error = Some(error.to_string()),
            },
            KeyCode::Backspace => {
                if let Some(field) = state.draft.current_field() {
                    let mut value = state.draft.field_value(&field);
                    value.pop();
                    state.draft.set_field(&field, &value);
                }
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                if let Some(field) = state.draft.current_field() {
                    let mut value = state.draft.field_value(&field);
                    value.push(c);
                    state.draft.set_field(&field, &value);
                }
            }
            _ => {}
        },
        SetupStage::Review { config, error } => match key.code {
            KeyCode::Esc => state.stage = SetupStage::Form,
            KeyCode::PageDown => state.scroll = state.scroll.saturating_add(8),
            KeyCode::PageUp => state.scroll = state.scroll.saturating_sub(8),
            KeyCode::Char('s') if ctrl => match save_config(config, Path::new(&state.output)) {
                Ok(()) => {
                    state.stage = SetupStage::Saved {
                        path: state.output.clone(),
                    };
                    state.scroll = 0;
                }
                Err(e) => *error = Some(e.to_string()),
            },
            _ => {}
        },
        SetupStage::Saved { .. } => match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            _ => {}
        },
    }
    false
}

struct RestoreTerminal;
impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), event::DisableBracketedPaste);
        ratatui::restore();
    }
}

fn apply_log(app: &mut App, title: String, result: Result<Value>) {
    app.busy = false;
    match result {
        Ok(log) if app.modal.is_none() => {
            app.scroll = 0;
            app.modal = Some(Modal::Log {
                title,
                text: safe_text(log["text"].as_str().unwrap_or("")),
                truncated: log["truncated"].as_bool().unwrap_or(false),
            });
        }
        Ok(_) => {
            app.notice =
                "Log retrieved while a dialog was open · close it and press l to view".into()
        }
        Err(error) => app.notice = format!("Could not read log: {error}"),
    }
}

fn read_goal(app: &mut App, path: Option<&Path>) {
    if let Some(path) = path {
        match read_json(path) {
            Ok(goal) => {
                app.goal = Some(goal);
                app.goal_error = None;
            }
            Err(error) => {
                app.goal = None;
                app.goal_error = Some(format!("Goal snapshot unavailable: {error}"));
            }
        }
    }
}

fn read_json(path: &Path) -> Result<Value> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).context("Cannot open JSON file")?;
    if !file.metadata()?.is_file() {
        bail!("Expected a regular JSON file");
    }
    let mut bytes = Vec::new();
    file.take(262145).read_to_end(&mut bytes)?;
    if bytes.len() > 262144 {
        bail!("JSON file exceeds 256 KiB");
    }
    serde_json::from_slice(&bytes).context("Invalid JSON")
}

fn paste(app: &mut App, text: &str) {
    let text = safe_text(text);
    match &mut app.modal {
        Some(Modal::Draft(draft)) if draft.field < 8 => {
            let limit = if draft.field == 1 { 8000 } else { 4000 };
            let text = if draft.field == 1 {
                text
            } else {
                text.replace(['\n', '\t'], " ")
            };
            if draft.values[draft.field].len() + text.len() <= limit {
                draft.values[draft.field].push_str(&text);
            } else {
                draft.error = Some(format!("Field limit is {limit} bytes"));
            }
        }
        Some(Modal::Import { path, .. }) => {
            let text = text.replace(['\n', '\t'], "");
            if path.len() + text.len() <= 4000 {
                path.push_str(&text);
            }
        }
        _ => {}
    }
}

fn handle_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    if matches!(key.code, KeyCode::PageDown | KeyCode::PageUp) {
        app.scroll = if key.code == KeyCode::PageDown {
            app.scroll.saturating_add(8)
        } else {
            app.scroll.saturating_sub(8)
        };
        return None;
    }
    if let Some(mut modal) = app.modal.take() {
        if key.code == KeyCode::Esc {
            if let Modal::Review {
                draft: Some(draft), ..
            } = modal
            {
                app.modal = Some(Modal::Draft(draft));
            }
            app.scroll = 0;
            return None;
        }
        let submit =
            key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Char('u') && key.modifiers.contains(KeyModifiers::CONTROL) {
            match &mut modal {
                Modal::Draft(draft) if draft.field < 8 => draft.values[draft.field].clear(),
                Modal::Import { path, .. } => path.clear(),
                _ => {}
            }
            app.modal = Some(modal);
            return None;
        }
        match &mut modal {
            Modal::Draft(draft) => {
                match key.code {
                    KeyCode::Tab => draft.field = (draft.field + 1) % 9,
                    KeyCode::BackTab => draft.field = (draft.field + 8) % 9,
                    KeyCode::Left | KeyCode::Right if draft.field == 8 => {
                        let count = app
                            .overview
                            .as_ref()
                            .map_or(0, |overview| overview.profiles.len());
                        if count > 0 {
                            draft.profile = (draft.profile
                                + if key.code == KeyCode::Right {
                                    1
                                } else {
                                    count - 1
                                })
                                % count;
                        }
                    }
                    KeyCode::Backspace if draft.field < 8 => {
                        draft.values[draft.field].pop();
                    }
                    KeyCode::Enter if draft.field != 1 => {
                        draft.field = (draft.field + 1) % 9;
                        app.modal = Some(modal);
                        return None;
                    }
                    _ => {}
                }
                if submit {
                    if let Some(overview) = &app.overview {
                        match draft.build(overview) {
                            Ok(input) => {
                                app.scroll = 0;
                                app.modal = Some(Modal::Review {
                                    input,
                                    draft: Some(draft.clone()),
                                });
                                return None;
                            }
                            Err(error) => draft.error = Some(error.to_string()),
                        }
                    }
                } else if draft.field < 8 {
                    let text = match key.code {
                        KeyCode::Char(c)
                            if !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                        {
                            Some(c.to_string())
                        }
                        KeyCode::Enter if draft.field == 1 => Some("\n".into()),
                        _ => None,
                    };
                    if let Some(text) = text {
                        app.modal = Some(modal);
                        paste(app, &text);
                        return None;
                    }
                }
            }
            Modal::Import { path, error } => {
                if key.code == KeyCode::Backspace {
                    path.pop();
                } else if key.code == KeyCode::Enter {
                    let result = (|| -> Result<_> {
                        let input = serde_json::from_value(read_json(Path::new(path))?)?;
                        validate_input(
                            &input,
                            app.overview
                                .as_ref()
                                .context("Connect to the controller first")?,
                        )?;
                        Ok(input)
                    })();
                    match result {
                        Ok(input) => {
                            app.scroll = 0;
                            app.modal = Some(Modal::Review { input, draft: None });
                            return None;
                        }
                        Err(e) => *error = Some(e.to_string()),
                    }
                } else if let KeyCode::Char(c) = key.code
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    app.modal = Some(modal);
                    paste(app, &c.to_string());
                    return None;
                }
            }
            Modal::Review { input, .. }
                if key.code == KeyCode::Char('y') && key.modifiers.is_empty() =>
            {
                if app.can_write() {
                    if let Some(overview) = &app.overview {
                        if let Err(error) = validate_input(input, overview) {
                            app.notice = format!("admit: {error}");
                        } else {
                            app.scroll = 0;
                            app.admitting = Some(input.clone());
                            app.notice =
                                "admit request pending · waiting for durable confirmation".into();
                            let effect = Effect::Action("admit".into(), json!(input));
                            app.modal = Some(modal);
                            return Some(effect);
                        }
                    }
                } else {
                    app.notice = "admit disabled while offline, busy, or in preview".into();
                }
            }
            Modal::Confirm { operation, job_id } if key.code == KeyCode::Char('y') => {
                if app.can_write() {
                    return Some(Effect::Action(operation.clone(), json!({"job_id":job_id})));
                }
                app.notice = "Action disabled while offline, busy, or in preview".into();
            }
            _ => {}
        }
        app.modal = Some(modal);
        return None;
    }
    match key.code {
        KeyCode::Char('?') => app.modal = Some(Modal::Help),
        KeyCode::Tab => {
            app.tab = (app.tab + 1) % 3;
            app.scroll = 0;
        }
        KeyCode::BackTab => {
            app.tab = (app.tab + 2) % 3;
            app.scroll = 0;
        }
        KeyCode::Char('1'..='3') => {
            if let KeyCode::Char(c) = key.code {
                app.tab = c as usize - '1' as usize;
                app.scroll = 0;
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.selected = (app.selected + 1).min(
                app.overview
                    .as_ref()
                    .map_or(0, |o| o.jobs.len().saturating_sub(1)),
            );
            app.scroll = 0;
            app.artifact_index = 0;
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.selected = app.selected.saturating_sub(1);
            app.scroll = 0;
            app.artifact_index = 0;
        }
        KeyCode::Char('[') => app.artifact_index = app.artifact_index.saturating_sub(1),
        KeyCode::Char(']') => {
            app.artifact_index =
                (app.artifact_index + 1).min(app.artifacts().len().saturating_sub(1))
        }
        KeyCode::Char('n') => {
            if let Some(overview) = &app.overview {
                app.modal = Some(Modal::Draft(Draft::new(overview)));
            }
        }
        KeyCode::Char('i') => {
            app.modal = Some(Modal::Import {
                path: String::new(),
                error: None,
            })
        }
        KeyCode::Char('l') if !app.busy && app.connected() && !app.demo => {
            if let (Some(job), Some(artifact)) =
                (app.job(), app.artifacts().get(app.artifact_index))
            {
                return Some(Effect::Log(
                    job.input.job_id.clone(),
                    artifact.reference.clone(),
                ));
            }
            app.notice = "No verification logs for the selected job yet".into();
        }
        KeyCode::Char(c @ ('r' | 'p' | 'a' | 'c' | 'x')) => {
            if app.can_write() {
                if let Some(job) = app.job() {
                    let operation = match c {
                        'r' => "run",
                        'p' if job.state == "paused" => "resume",
                        'p' => "pause",
                        'a' => "abort",
                        'c' => "cancel",
                        _ => "requeue",
                    };
                    app.modal = Some(Modal::Confirm {
                        operation: operation.into(),
                        job_id: job.input.job_id.clone(),
                    });
                }
            } else {
                app.notice = "Actions disabled while offline, busy, or in preview".into();
            }
        }
        _ => {}
    }
    None
}

pub fn demo_overview() -> Overview {
    let sha = "a".repeat(40);
    let profile = json!({"id":"trusted-tests","version":"1","timeout_seconds":120,"checks":[{"id":"unit","argv":["cargo","test","--locked"]},{"id":"runtime","argv":["./checks/runtime.sh"]}]});
    let jobs: Vec<Value> = [("parser-errors","running"),("runtime-output","verifying"),("configuration","done"),("edge-cases","blocked")].iter().enumerate().map(|(index,(name,state))| {
        let receipt = json!({"verification_id":"verification-001","candidate_commit":sha,"verification_profile_id":"trusted-tests","profile_version":"1","environment_id":"host-clean-checkout","outcome":"passed","commands":[{"command":"cargo test --locked","exit_status":0},{"command":"./checks/runtime.sh","exit_status":0}],"artifacts":[],"config_digest":"demo","clean_checkout":true,"feature_map":[{"behavior":"Configuration round trip","expected":"values preserved","verified":true,"checks":[{"check_id":"unit","exit_status":0,"commit":sha},{"check_id":"runtime","exit_status":0,"commit":sha}]}],"environment":{"platform":"linux"}});
        json!({"input":{"job_id":name,"base_commit":sha,"task_brief":if index==0 {"Improve parser errors with precise locations and actionable messages. Preserve valid input behavior."} else {"Implement the approved behavior and run the registered checks."},"acceptance_criteria":[{"behavior":"Useful errors for invalid input","check_ids":["unit","runtime"],"expected":"Location and reason match the fixtures"}],"allowed_paths":["src","tests"],"protected_paths":["checks"],"verification_profile_id":"trusted-tests","policy_version":"operator-v1"},"profile":profile,"state":state,"revision":index+3,"attempts":1,"attempt":{"attempt_id":"attempt-01","lease_id":"demo-lease","session_id":"session-01","worktree":"/tmp/code-factory-demo/worker","lease_expires_at":"2099-01-01T00:00:00Z","claimed":true,"dispatch_state":"sent","model":"opencode/big-pickle","message_id":"message-01","session_title":"Factory job","mode":"adapter","server_url":"http://127.0.0.1:4096"},"receipt":if *state=="done" {Some(receipt)}else{None},"candidate":null,"candidate_receipt":null,"integrated_commit":if *state=="done"{Some(&sha)}else{None},"integration_base":null,"delivery":if *state=="done"{Some(json!({"path":"/tmp/code-factory-demo/delivery/configuration.tar","sha256":"demo digest","commit":sha}))}else{None},"blocker":if *state=="blocked"{Some("Provider cooldown · waiting for an approved free model")}else{None},"next_retry_at":null,"outcome":null,"verification_pending":null})
    }).collect();
    serde_json::from_value(json!({"project_id":"example-factory","repository":"/work/project","base_commit":sha,"integration_ref":"refs/heads/factory/integration","max_workers":2,"max_attempts":3,"profiles":[profile],"policy":{"models":["opencode/big-pickle"],"free_only":true,"spend_ceiling":0},"jobs":jobs})).expect("static demo is valid")
}

#[cfg(test)]
mod tests;
