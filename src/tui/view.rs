use super::data::{App, ConfigField, Draft, FIELDS, Modal, SetupStage, SetupState, safe_text};
use crate::model::{Check, Receipt};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap,
    },
};

const BACKGROUND: Color = Color::Rgb(13, 20, 31);
const PANEL: Color = Color::Rgb(20, 30, 44);
const BORDER: Color = Color::Rgb(48, 65, 84);
const TEXT: Color = Color::Rgb(223, 232, 241);
const MUTED: Color = Color::Rgb(138, 157, 178);
const ACCENT: Color = Color::Rgb(94, 234, 212);
const AMBER: Color = Color::Rgb(251, 191, 88);
const RED: Color = Color::Rgb(251, 128, 133);
const SELECTED: Color = Color::Rgb(32, 66, 77);

fn panel(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .title(Line::from(format!(" {} ", safe_text(&title.into()))))
        .title_style(Style::default().fg(MUTED))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(PANEL).fg(TEXT))
}

fn line(value: impl AsRef<str>) -> Line<'static> {
    Line::from(safe_text(value.as_ref()))
}

fn colored(value: impl AsRef<str>, color: Color) -> Span<'static> {
    Span::styled(safe_text(value.as_ref()), Style::default().fg(color))
}

fn heading(value: &str) -> Line<'static> {
    Line::from(Span::styled(
        safe_text(value),
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    ))
}

fn pair(label: &str, value: impl AsRef<str>) -> Line<'static> {
    Line::from(vec![
        colored(format!("{label}  "), MUTED),
        colored(value, TEXT),
    ])
}

fn state_label(state: &str) -> (&str, Color) {
    match state {
        "done" => ("Delivered", ACCENT),
        "running" => ("Working", ACCENT),
        "verifying" => ("Verifying", ACCENT),
        "integrating" => ("Integrating", ACCENT),
        "queued" => ("Queued", MUTED),
        "paused" => ("Paused", AMBER),
        "blocked" => ("Blocked", RED),
        "retry_wait" => ("Retry wait", AMBER),
        "cancelled" | "canceled" => ("Cancelled", MUTED),
        "aborted" => ("Aborted", AMBER),
        other => (other, MUTED),
    }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BACKGROUND).fg(TEXT)),
        area,
    );
    if area.width < 58 || area.height < 17 {
        compact(frame, area, app);
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Min(6),
            Constraint::Length(3),
        ])
        .split(area);
    header(frame, rows[0], app);
    stats(frame, rows[1], app);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((area.width / 3).clamp(23, 37)),
            Constraint::Min(25),
        ])
        .split(rows[2]);
    jobs(frame, columns[0], app);
    detail(frame, columns[1], app);
    footer(frame, rows[3], app);
    if let Some(modal) = &app.modal {
        draw_modal(frame, app, modal);
    }
}

fn compact(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = vec![
        heading("CODE FACTORY"),
        line("Resize to at least 58 × 17"),
        line("q  Close view · controller keeps running"),
    ];
    if app.modal.is_some() {
        lines.push(line("Input is open · resize to continue · Esc back"));
    }
    lines.push(line(if app.demo {
        "DEMO · read only"
    } else if app.connected() {
        "LIVE"
    } else {
        "STALE · controls locked"
    }));
    if let Some(job) = app.job() {
        lines.push(pair(state_label(&job.state).0, &job.input.job_id));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(BACKGROUND)),
        area,
    );
}

fn header(frame: &mut Frame, area: Rect, app: &App) {
    let status = if app.demo {
        ("DEMO / READ ONLY", AMBER)
    } else if app.connected() {
        ("LIVE", ACCENT)
    } else {
        ("STALE / CONTROLS LOCKED", RED)
    };
    let name = app
        .overview
        .as_ref()
        .map(|o| o.project_id.as_str())
        .unwrap_or("Connecting…");
    let mut parts = vec![
        Span::styled(
            "  CODE FACTORY  ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        colored("/  ", MUTED),
        colored(name, TEXT),
    ];
    if app.busy {
        parts.push(colored("   ◌ Request in progress", AMBER));
    }
    let refresh = app
        .last_refresh
        .map(|at| format!(" · refreshed {}s ago", at.elapsed().as_secs()))
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(parts),
            Line::from(vec![
                colored(format!("  {}", status.0), status.1),
                colored(refresh, MUTED),
                colored(format!("  {}", app.endpoint), MUTED),
            ]),
        ]),
        area,
    );
}

fn stats(frame: &mut Frame, area: Rect, app: &App) {
    let all = app
        .overview
        .as_ref()
        .map(|o| o.jobs.as_slice())
        .unwrap_or(&[]);
    let active = all
        .iter()
        .filter(|j| matches!(j.state.as_str(), "running" | "verifying" | "integrating"))
        .count();
    let queued = all.iter().filter(|j| j.state == "queued").count();
    let delivered = all.iter().filter(|j| j.state == "done").count();
    let attention = all
        .iter()
        .filter(|j| {
            matches!(
                j.state.as_str(),
                "blocked" | "paused" | "retry_wait" | "aborted"
            )
        })
        .count();
    let slots = app.overview.as_ref().map(|o| o.max_workers).unwrap_or(0);
    let values = [
        ("ACTIVE", format!("{active} / {slots} slots"), ACCENT),
        ("QUEUED", queued.to_string(), TEXT),
        ("DELIVERED JOBS", delivered.to_string(), ACCENT),
        (
            "NEEDS ATTENTION",
            attention.to_string(),
            if attention > 0 { AMBER } else { MUTED },
        ),
    ];
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 4); 4])
        .split(area);
    for (index, (label, value, color)) in values.into_iter().enumerate() {
        frame.render_widget(
            Paragraph::new(Line::from(colored(value, color)))
                .block(panel(label))
                .alignment(Alignment::Center),
            columns[index],
        );
    }
}

fn jobs(frame: &mut Frame, area: Rect, app: &App) {
    let all = app
        .overview
        .as_ref()
        .map(|o| o.jobs.as_slice())
        .unwrap_or(&[]);
    if all.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                line(""),
                heading(" Your queue is clear"),
                line(""),
                line(" n  Create a job"),
                line(" i  Import a job file"),
                line(""),
                line(" Define the outcome,"),
                line(" then choose its checks."),
            ])
            .block(panel("JOBS")),
            area,
        );
        return;
    }
    let items = all
        .iter()
        .map(|job| {
            let (state, color) = state_label(&job.state);
            ListItem::new(vec![
                Line::from(Span::styled(
                    safe_text(&job.input.job_id),
                    Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                )),
                Line::from(vec![
                    colored(format!("{state}  "), color),
                    colored(format!("attempt {}", job.attempts), MUTED),
                ]),
                line(""),
            ])
        })
        .collect::<Vec<_>>();
    let mut state =
        ListState::default().with_selected(Some(app.selected.min(all.len().saturating_sub(1))));
    frame.render_stateful_widget(
        List::new(items)
            .block(panel(format!("JOBS  {}", all.len())))
            .highlight_style(Style::default().bg(SELECTED))
            .highlight_symbol("▌ "),
        area,
        &mut state,
    );
}

fn detail(frame: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);
    frame.render_widget(
        Tabs::new(["1 Overview", "2 Evidence", "3 Activity"])
            .select(app.tab.min(2))
            .style(Style::default().fg(MUTED).bg(PANEL))
            .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
            .divider("  ")
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(BORDER)),
            ),
        rows[0],
    );
    let lines = match app.tab {
        1 => evidence_lines(app),
        2 => event_lines(app),
        _ => overview_lines(app),
    };
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(TEXT).bg(PANEL))
            .block(Block::default().padding(ratatui::widgets::Padding::horizontal(2)))
            .wrap(Wrap { trim: false })
            .scroll((app.scroll, 0)),
        rows[1],
    );
}

fn overview_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![];
    if let Some(goal) = &app.goal {
        lines.push(heading("BROAD GOAL · local snapshot"));
        lines.push(pair(
            "Status",
            goal.get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown"),
        ));
        if let Some(unmet) = goal.get("unmet_evidence").and_then(|v| v.as_array()) {
            let text = unmet
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(Line::from(colored(
                if text.is_empty() {
                    "Unmet evidence: none listed".into()
                } else {
                    format!("Unmet evidence: {text}")
                },
                if unmet.is_empty() { MUTED } else { AMBER },
            )));
        }
        lines.push(Line::from(colored(
            "Delivered jobs do not establish goal completion.",
            MUTED,
        )));
        if let Some(remaining) = goal.get("remaining_work").and_then(|v| v.as_array()) {
            for item in remaining.iter().filter_map(|v| v.as_str()) {
                lines.push(line(format!("  • {item}")));
            }
        }
        lines.push(line(""));
    }
    if let Some(error) = &app.goal_error {
        lines.push(Line::from(colored(
            format!("Goal snapshot unavailable: {error}"),
            AMBER,
        )));
        lines.push(line(""));
    }
    let Some(job) = app.job() else {
        lines.extend([
            heading("YOUR NEXT IMPLEMENTATION"),
            line(""),
            line("Start with a concrete behavior and the result that proves it works."),
            line(""),
            line("Press n for guided input, or i to review an existing job file."),
            line(""),
            heading("CONTROLLER"),
        ]);
        if let Some(overview) = &app.overview {
            lines.extend([
                pair("Repository", &overview.repository),
                pair("Integration", &overview.integration_ref),
                pair("Profiles", overview.profiles.len().to_string()),
            ]);
        }
        return lines;
    };
    let (label, color) = state_label(&job.state);
    lines.push(heading(&job.input.job_id));
    lines.push(Line::from(vec![
        colored(label, color),
        colored(
            format!("   revision {}   attempt {}", job.revision, job.attempts),
            MUTED,
        ),
    ]));
    if let Some(blocker) = &job.blocker {
        lines.push(Line::from(colored(format!("Attention: {blocker}"), AMBER)));
    }
    if let Some(retry) = &job.next_retry_at {
        lines.push(pair("Retry after", retry));
    }
    if let Some(outcome) = &job.outcome {
        lines.push(pair("Outcome", outcome));
    }
    lines.push(line(""));
    lines.push(heading("TASK"));
    for text in safe_text(&job.input.task_brief).lines() {
        lines.push(line(text));
    }
    lines.push(line(""));
    lines.push(heading("ACCEPTANCE"));
    for criterion in &job.input.acceptance_criteria {
        lines.push(line(format!("• {}", criterion.behavior)));
        lines.push(pair("  Expect", &criterion.expected));
        lines.push(pair("  Checks", criterion.check_ids.join(", ")));
    }
    if let Some(attempt) = &job.attempt {
        lines.extend([
            line(""),
            heading("CURRENT ATTEMPT"),
            pair("Model", attempt.model.as_deref().unwrap_or("Not assigned")),
            pair("Dispatch", &attempt.dispatch_state),
            pair(
                "Session",
                attempt.session_id.as_deref().unwrap_or("Not bound"),
            ),
            pair("Lease ends", &attempt.lease_expires_at),
            pair("Worktree", attempt.worktree.to_string_lossy()),
        ]);
    }
    lines.extend([
        line(""),
        heading("SCOPE & DELIVERY"),
        pair("Base", &job.input.base_commit),
        pair(
            "Profile",
            format!("{} / {}", job.profile.id, job.profile.version),
        ),
        pair("Allowed", job.input.allowed_paths.join(", ")),
        pair(
            "Protected",
            if job.input.protected_paths.is_empty() {
                "None declared".into()
            } else {
                job.input.protected_paths.join(", ")
            },
        ),
    ]);
    if let Some(commit) = &job.integrated_commit {
        lines.push(pair("Integrated", commit));
    }
    if let Some(delivery) = &job.delivery {
        lines.push(heading("DELIVERY RECORD"));
        append_json(&mut lines, delivery);
    }
    lines
}

fn evidence_lines(app: &App) -> Vec<Line<'static>> {
    let Some(job) = app.job() else {
        return vec![
            heading("EVIDENCE"),
            line("Select a job to inspect its independent verification."),
        ];
    };
    let mut lines = vec![
        heading("VERIFICATION EVIDENCE"),
        line("Receipts bind checks to an exact commit."),
        line(""),
    ];
    if let Some(pending) = &job.verification_pending {
        lines.push(pair("In progress", pending));
    }
    if job.receipt.is_none() && job.candidate_receipt.is_none() {
        lines.push(Line::from(colored("No verification receipt yet", AMBER)));
        lines.push(line("Worker activity alone does not prove acceptance."));
        lines.push(line(""));
        lines.push(heading("REGISTERED CHECKS"));
        for check in &job.profile.checks {
            check_lines(&mut lines, check);
        }
    }
    let artifacts = app.artifacts();
    if let Some(artifact) = artifacts.get(app.artifact_index % artifacts.len().max(1)) {
        lines.push(heading("LOG VIEWER"));
        lines.push(pair(
            "Selected",
            format!(
                "{} / {}   [ ] choose · l open",
                app.artifact_index % artifacts.len() + 1,
                artifacts.len()
            ),
        ));
        lines.push(line(&artifact.reference));
        lines.push(pair("SHA256", &artifact.sha256));
        lines.push(line(""));
    }
    if let Some(receipt) = &job.receipt {
        receipt_lines(&mut lines, "CURRENT RECEIPT", receipt);
    }
    if let Some(receipt) = &job.candidate_receipt
        && job
            .receipt
            .as_ref()
            .is_none_or(|current| current.verification_id != receipt.verification_id)
    {
        receipt_lines(&mut lines, "CANDIDATE RECEIPT", receipt);
    }
    lines
}

fn receipt_lines(lines: &mut Vec<Line<'static>>, title: &str, receipt: &Receipt) {
    lines.push(heading(title));
    let success = receipt.outcome == "passed"
        && receipt
            .commands
            .iter()
            .all(|command| command.exit_status == 0);
    lines.push(Line::from(colored(
        format!(
            "{} · clean checkout: {}",
            receipt.outcome, receipt.clean_checkout
        ),
        if success && receipt.clean_checkout {
            ACCENT
        } else {
            AMBER
        },
    )));
    lines.push(pair("Commit", &receipt.candidate_commit));
    lines.push(pair(
        "Profile",
        format!(
            "{} / {}",
            receipt.verification_profile_id, receipt.profile_version
        ),
    ));
    lines.push(pair("Receipt", &receipt.verification_id));
    lines.push(line(""));
    for command in &receipt.commands {
        lines.push(Line::from(vec![
            colored(
                format!(
                    "{}  exit {}  ",
                    if command.exit_status == 0 {
                        "EXIT OK"
                    } else {
                        "FAIL"
                    },
                    command.exit_status
                ),
                if command.exit_status == 0 {
                    ACCENT
                } else {
                    RED
                },
            ),
            colored(&command.command, TEXT),
        ]));
    }
    lines.push(line(""));
    lines.push(heading("BEHAVIOR → CHECK → EVIDENCE"));
    if let Some(features) = receipt.feature_map.as_array() {
        for feature in features {
            let behavior = feature
                .get("behavior")
                .and_then(|v| v.as_str())
                .unwrap_or("Unnamed behavior");
            let verified = feature
                .get("verified")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            lines.push(Line::from(vec![
                colored(
                    if verified {
                        "CHECKED  "
                    } else {
                        "UNVERIFIED  "
                    },
                    if verified { ACCENT } else { AMBER },
                ),
                colored(behavior, TEXT),
            ]));
            if let Some(expected) = feature.get("expected").and_then(|v| v.as_str()) {
                lines.push(pair("  Expect", expected));
            }
            if let Some(checks) = feature.get("checks").and_then(|v| v.as_array()) {
                for check in checks {
                    let id = check
                        .get("check_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("check");
                    let exit = check
                        .get("exit_status")
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "unknown".into());
                    lines.push(pair(&format!("  {id}"), format!("exit {exit}")));
                    if let Some(verified) = check.get("verified").and_then(|v| v.as_bool()) {
                        lines.push(Line::from(colored(
                            if verified {
                                "    Evidence verified"
                            } else {
                                "    Evidence unverified"
                            },
                            if verified { ACCENT } else { AMBER },
                        )));
                    }
                    if let Some(error) = check.get("evidence_error").and_then(|v| v.as_str()) {
                        lines.push(Line::from(colored(format!("    {error}"), RED)));
                    }
                    if let Some(observations) = check.get("observations").filter(|v| !v.is_null()) {
                        lines.push(pair("    Observations", observations.to_string()));
                    }
                    if let Some(commit) = check.get("commit").and_then(|v| v.as_str()) {
                        lines.push(pair("    Commit", commit));
                    }
                    if let Some(reference) = check
                        .pointer("/evidence/reference")
                        .and_then(|v| v.as_str())
                    {
                        lines.push(pair("    Log", reference));
                    }
                }
            }
        }
    } else {
        append_json(lines, &receipt.feature_map);
    }
    lines.push(line(""));
    lines.push(heading("ENVIRONMENT"));
    lines.push(pair("ID", &receipt.environment_id));
    append_json(lines, &receipt.environment);
    lines.push(line(""));
}

fn event_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![
        heading("DURABLE ACTIVITY"),
        line("Controller transitions · newest first"),
        line(""),
    ];
    let selected = app.job().map(|job| job.input.job_id.as_str());
    let mut count = 0;
    for event in app
        .events
        .iter()
        .rev()
        .filter(|event| selected.is_none_or(|id| event.job_id == id))
    {
        let (state, color) = state_label(&event.state);
        lines.push(Line::from(vec![
            colored(format!("#{}  ", event.seq), MUTED),
            colored(state, color),
            colored(format!("  rev {}", event.revision), MUTED),
        ]));
        lines.push(Line::from(colored(&event.at, MUTED)));
        lines.push(line(&event.reason));
        lines.push(line(""));
        count += 1;
    }
    if count == 0 {
        lines.push(line("No recorded transitions for this selection yet."));
    }
    lines
}

fn footer(frame: &mut Frame, area: Rect, app: &App) {
    let shortcuts = if area.width >= 115 {
        " n New  i Import  r Run  p Pause/resume  a Abort  c Cancel  x Requeue  l Log  Tab Views  ↑↓ Jobs  ? Help  q Quit "
    } else {
        " n New  i Import  r Run  p Pause  Tab Views  ↑↓ Jobs  ? Help  q Quit "
    };
    let mut lines = vec![Line::from(colored(shortcuts, ACCENT))];
    if let Some(error) = &app.error {
        lines.push(Line::from(colored(format!(" {error}"), RED)));
    } else {
        lines.push(Line::from(colored(format!(" {}", app.notice), MUTED)));
    }
    lines.push(Line::from(colored(
        " PgUp / PgDn Scroll · quitting closes this view; controller jobs continue",
        MUTED,
    )));
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(BACKGROUND)),
        area,
    );
}

fn modal_rect(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let width = area.width.saturating_sub(4).min(max_width);
    let height = area.height.saturating_sub(2).min(max_height);
    Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    )
}

fn draw_modal(frame: &mut Frame, app: &App, modal: &Modal) {
    let (title, max_height) = match modal {
        Modal::Draft(_) => ("NEW JOB · define the work", 42),
        Modal::Import { .. } => ("IMPORT JOB", 13),
        Modal::Review { .. } => ("REVIEW · approved job contract", 42),
        Modal::Confirm { .. } => ("CONFIRM ACTION", 16),
        Modal::Log { .. } => ("VERIFICATION LOG", 42),
        Modal::Help => ("OPERATOR GUIDE", 31),
    };
    let area = modal_rect(frame.area(), 104, max_height);
    frame.render_widget(Clear, area);
    let border = panel(title).border_style(Style::default().fg(ACCENT));
    let inner = border.inner(area);
    frame.render_widget(border, area);
    match modal {
        Modal::Draft(draft) => draft_modal(frame, inner, app, draft),
        Modal::Import { path, error } => {
            let mut lines = vec![
                line("Load a local JSON job file; its full contract is reviewed before admission."),
                line(""),
                heading("File path"),
                line(format!("{path}▏")),
                line(""),
            ];
            if let Some(error) = error {
                lines.push(Line::from(colored(error, RED)));
            }
            lines.extend([
                line(""),
                Line::from(colored(
                    "Enter  Review file     Ctrl+U  Clear path     Esc  Back",
                    ACCENT,
                )),
            ]);
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
        Modal::Review { input, .. } => {
            let mut lines = vec![
                heading("This admits a queued job. Use Run to start execution."),
                line("Review the complete scope and the controller's registered checks."),
                line(""),
            ];
            if app.notice.starts_with("admit") {
                lines.push(Line::from(colored(&app.notice, AMBER)));
                lines.push(line(""));
            }
            append_json(&mut lines, &serde_json::to_value(input).unwrap_or_default());
            lines.extend([
                line(""),
                heading("REGISTERED VERIFICATION COMMANDS · read only"),
            ]);
            if let Some(profile) = app.overview.as_ref().and_then(|o| {
                o.profiles
                    .iter()
                    .find(|p| p.id == input.verification_profile_id)
            }) {
                lines.push(pair(
                    "Profile",
                    format!(
                        "{} / {} · timeout {}s",
                        profile.id, profile.version, profile.timeout_seconds
                    ),
                ));
                for check in &profile.checks {
                    check_lines(&mut lines, check);
                }
            }
            modal_body(
                frame,
                inner,
                lines,
                app.scroll,
                if app.can_write() {
                    "Ctrl+S  Admit queued job     Esc  Back     PgUp / PgDn  Scroll"
                } else {
                    "Writes unavailable · connection must be fresh     Esc  Back     PgUp / PgDn  Scroll"
                },
            );
        }
        Modal::Confirm { operation, job_id } => {
            let description = match operation.as_str() {
                "run" => {
                    "Start or continue this job through the controller. The configured worker can modify its assigned worktree and execute approved checks."
                }
                "pause" => {
                    "Pause scheduling for this job. Already dispatched external work may continue until it is reconciled."
                }
                "resume" => {
                    "Resume this paused job and allow the controller to continue its workflow."
                }
                "abort" => {
                    "Abort this attempt and revoke its worker grant. External processes may need separate cleanup."
                }
                "cancel" => {
                    "Mark this job cancelled and block further scheduling. External worker processes can continue; use Abort to stop an active attempt. Its durable history and evidence are retained."
                }
                "requeue" => {
                    "Return this job to the queue for another attempt under the same approved job contract."
                }
                _ => "Send the selected operation to the controller.",
            };
            modal_body(
                frame,
                inner,
                vec![
                    heading(&format!("{}  {}", operation.to_uppercase(), job_id)),
                    line(""),
                    line(description),
                    line(""),
                    Line::from(colored(
                        "The controller remains the authority for permitted state transitions.",
                        MUTED,
                    )),
                ],
                0,
                "y  Confirm     Esc  Back",
            );
        }
        Modal::Log {
            title,
            text,
            truncated,
        } => {
            let mut lines = vec![
                heading(title),
                Line::from(colored(
                    if *truncated {
                        "Tail excerpt · earlier output omitted · digest verified by controller"
                    } else {
                        "Complete log · digest verified by controller"
                    },
                    if *truncated { AMBER } else { MUTED },
                )),
                line(""),
            ];
            lines.extend(safe_text(text).lines().map(line));
            modal_body(
                frame,
                inner,
                lines,
                app.scroll,
                "PgUp / PgDn  Scroll     Esc  Close",
            );
        }
        Modal::Help => {
            let lines = vec![
                heading("INPUT"),
                pair(
                    "n",
                    "Create a guided job with a trusted verification profile",
                ),
                pair(
                    "i",
                    "Import a JSON job, including multiple acceptance criteria",
                ),
                pair("Tab / Shift+Tab", "Move between input fields"),
                pair("← / →", "Choose a registered verification profile"),
                pair("Ctrl+U", "Clear the current input field or import path"),
                pair("Ctrl+S", "Review input, then admit the reviewed queued job"),
                line(""),
                heading("OPERATE SELECTED JOB"),
                pair("r", "Run / continue"),
                pair("p", "Pause / resume"),
                pair("a / c / x", "Abort attempt / cancel job / requeue"),
                line("Every action requires confirmation; stale views cannot write."),
                line(""),
                heading("OBSERVE"),
                pair("↑ / ↓ or j / k", "Select a job"),
                pair("Tab or 1 / 2 / 3", "Overview / evidence / activity"),
                pair(
                    "[ / ] then l",
                    "Choose a verification artifact and open its log",
                ),
                pair("PgUp / PgDn", "Scroll details, review, or log"),
                pair("q", "Close the TUI; jobs keep running in the controller"),
                line(""),
                Line::from(colored(
                    "A delivered job proves its own approved checks. Broad goal acceptance is tracked separately.",
                    AMBER,
                )),
            ];
            modal_body(
                frame,
                inner,
                lines,
                app.scroll,
                "Esc  Close     PgUp / PgDn  Scroll",
            );
        }
    }
}

fn modal_body(frame: &mut Frame, area: Rect, lines: Vec<Line<'static>>, scroll: u16, hint: &str) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(area);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(vec![line(""), Line::from(colored(hint, ACCENT))]),
        rows[1],
    );
}

// --- Controller setup -------------------------------------------------------

fn section_of(field: &ConfigField) -> String {
    match field {
        ConfigField::ProjectId
        | ConfigField::Repository
        | ConfigField::DataDir
        | ConfigField::DeliveryDir
        | ConfigField::IntegrationRef => "PROJECT".into(),
        ConfigField::ProfileId(p)
        | ConfigField::ProfileVersion(p)
        | ConfigField::ProfileTimeout(p) => {
            format!("PROFILE {}", p + 1)
        }
        ConfigField::CheckId(p, c) | ConfigField::CheckArgv(p, c) => {
            format!("PROFILE {} · CHECK {}", p + 1, c + 1)
        }
        ConfigField::Models | ConfigField::Cooldown => {
            "POLICY · locked free-only, zero-spend, data-approved".into()
        }
        ConfigField::MaxWorkers
        | ConfigField::LeaseSeconds
        | ConfigField::MaxAttempts
        | ConfigField::OpencodeUrl => "LIMITS".into(),
    }
}

pub fn draw_setup(frame: &mut Frame, state: &mut SetupState) {
    match &state.stage {
        SetupStage::Form => draw_setup_form(frame, state),
        SetupStage::Review { .. } => draw_setup_review(frame, state),
        SetupStage::Saved { .. } => draw_setup_saved(frame, state),
    }
}

fn draw_setup_form(frame: &mut Frame, state: &mut SetupState) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BACKGROUND).fg(TEXT)),
        area,
    );
    if area.width < 40 || area.height < 12 {
        frame.render_widget(
            Paragraph::new(vec![
                heading("CONTROLLER SETUP"),
                line("Resize to at least 40 × 12"),
                line("Esc  Quit"),
            ])
            .style(Style::default().fg(TEXT).bg(BACKGROUND)),
            area,
        );
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(6),
            Constraint::Length(4),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(vec![
            heading("CONTROLLER SETUP"),
            pair("Saving to", &state.output),
            line(
                "Paths must be absolute. Policy is locked to free-only, zero-spend, data-approved.",
            ),
        ]),
        rows[0],
    );
    let draft = &state.draft;
    let fields = draft.fields();
    let mut lines: Vec<Line> = vec![];
    let mut active_line = 0usize;
    let mut last_section: Option<String> = None;
    for (index, field) in fields.iter().enumerate() {
        let section = section_of(field);
        if last_section.as_deref() != Some(section.as_str()) {
            lines.push(line(""));
            lines.push(heading(&section));
            last_section = Some(section);
        }
        let value = draft.field_value(field);
        let is_active = index == draft.field;
        if is_active {
            active_line = lines.len();
        }
        let label = colored(format!("  {}  ", field.label()), MUTED);
        let value_span = if is_active {
            colored(format!("{}▏", value), ACCENT)
        } else {
            colored(value, TEXT)
        };
        let row = Line::from(vec![label, value_span]);
        lines.push(if is_active {
            row.style(Style::default().bg(SELECTED))
        } else {
            row
        });
    }
    // Keep the active field visible.
    let inner_height = rows[1].height.saturating_sub(2) as usize;
    if active_line < state.scroll as usize {
        state.scroll = active_line as u16;
    } else if inner_height > 0 && active_line >= state.scroll as usize + inner_height {
        state.scroll = (active_line - inner_height + 1) as u16;
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("CONFIGURATION"))
            .wrap(Wrap { trim: false })
            .scroll((state.scroll, 0)),
        rows[1],
    );
    let error = state.draft.error.as_deref().unwrap_or(
        "Tab  Next field     Ctrl+S  Review     Ctrl+N  New check     Ctrl+P  New profile     Ctrl+D  Delete     Esc  Quit",
    );
    frame.render_widget(
        Paragraph::new(Line::from(colored(
            error,
            if state.draft.error.is_some() {
                RED
            } else {
                ACCENT
            },
        ))),
        rows[2],
    );
}

fn draw_setup_review(frame: &mut Frame, state: &mut SetupState) {
    let SetupStage::Review { config, error } = &state.stage else {
        return;
    };
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BACKGROUND).fg(TEXT)),
        area,
    );
    let json = serde_json::to_string_pretty(config).unwrap_or_default();
    let mut lines = vec![
        heading("REVIEW CONTROLLER CONFIG"),
        line("This is the exact JSON that will be written."),
        line(""),
    ];
    lines.extend(json.lines().map(line));
    lines.push(line(""));
    if let Some(error) = error {
        lines.push(Line::from(colored(error, RED)));
        lines.push(line(""));
    }
    lines.push(Line::from(colored(
        "Ctrl+S  Save to file     Esc  Back",
        ACCENT,
    )));
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("CONFIGURATION"))
            .wrap(Wrap { trim: false })
            .scroll((state.scroll, 0)),
        area,
    );
}

fn draw_setup_saved(frame: &mut Frame, state: &mut SetupState) {
    let SetupStage::Saved { path } = &state.stage else {
        return;
    };
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BACKGROUND).fg(TEXT)),
        area,
    );
    let lines = vec![
        heading("CONFIG SAVED"),
        line(""),
        pair("Path", path),
        line(""),
        line("Start the controller:"),
        line(format!(
            "  code-factory-controller serve --config {} --listen 127.0.0.1:4317",
            path
        )),
        line(""),
        Line::from(colored("Press q to quit", ACCENT)),
    ];
    frame.render_widget(Paragraph::new(lines).block(panel("DONE")), area);
}

fn draft_modal(frame: &mut Frame, area: Rect, app: &App, draft: &Draft) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(colored(
            format!(
                "Field {} / {} · Tab next · Shift+Tab previous · Enter adds a line in the brief",
                draft.field + 1,
                FIELDS.len()
            ),
            MUTED,
        ))),
        rows[0],
    );
    // Keep the active field visible; a long brief has its own tail viewport.
    let field_height = |index| if index == 1 { 5u16 } else { 3u16 };
    let mut first = 0usize;
    while first < draft.field
        && (first..=draft.field).map(field_height).sum::<u16>() > rows[1].height
    {
        first += 1;
    }
    let mut y = rows[1].y;
    for (index, label) in FIELDS.iter().enumerate().skip(first) {
        if y >= rows[1].bottom() {
            break;
        }
        let height = field_height(index).min(rows[1].bottom().saturating_sub(y));
        let field_area = Rect::new(rows[1].x, y, rows[1].width, height);
        let active = index == draft.field;
        let border = panel(format!("{}  {}", index + 1, label))
            .border_style(Style::default().fg(if active { ACCENT } else { BORDER }));
        let value = if index == 8 {
            app.overview
                .as_ref()
                .and_then(|overview| overview.profiles.get(draft.profile))
                .map(|profile| {
                    format!(
                        "‹ {} / {} · {} checks ›",
                        profile.id,
                        profile.version,
                        profile.checks.len()
                    )
                })
                .unwrap_or_else(|| "No registered profiles".into())
        } else {
            draft.values.get(index).cloned().unwrap_or_default()
        };
        let mut text = safe_text(&value);
        if active {
            text.push('▏');
        }
        let width = field_area.width.saturating_sub(2).max(1) as usize;
        let visual_lines: usize = text
            .lines()
            .map(|text| Line::from(text).width().max(1).div_ceil(width))
            .sum();
        let scroll = visual_lines
            .saturating_sub(field_area.height.saturating_sub(2).max(1) as usize)
            .min(u16::MAX as usize) as u16;
        frame.render_widget(
            Paragraph::new(text)
                .block(border)
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0)),
            field_area,
        );
        y = y.saturating_add(height);
    }
    let error = draft.error.as_deref().unwrap_or(
        "Each behavior is checked by the selected profile. Import JSON for multiple criteria.",
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(colored(
                error,
                if draft.error.is_some() { RED } else { MUTED },
            )),
            line(""),
            Line::from(colored(
                "Ctrl+S  Review job     Ctrl+U  Clear field     ← / →  Profile     Esc  Discard",
                ACCENT,
            )),
        ])
        .wrap(Wrap { trim: false }),
        rows[2],
    );
}

fn append_json(lines: &mut Vec<Line<'static>>, value: &serde_json::Value) {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "Unavailable".into());
    lines.extend(text.lines().map(line));
}

fn format_args_as_json(args: &[String]) -> String {
    serde_json::to_string(args).unwrap_or_else(|_| "Unavailable".into())
}

fn check_lines(lines: &mut Vec<Line<'static>>, check: &Check) {
    lines.push(pair(&check.id, format_args_as_json(&check.argv)));
    let value = serde_json::to_value(check).unwrap_or_default();
    if let Some(contract) = value.get("result_contract").filter(|v| !v.is_null()) {
        lines.push(pair("  Structured evidence", contract.to_string()));
    } else {
        lines.push(Line::from(colored("  Exit status only", AMBER)));
    }
}

#[cfg(test)]
mod tests {
    use super::super::data::Overview;
    use super::*;
    use crate::model::{Criterion, Job, JobInput, Profile};
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;
    use std::time::Instant;

    fn render(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(width.max(1) as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn app() -> App {
        let mut app = App::new("http://127.0.0.1:4317".into(), false);
        let profile = Profile {
            id: "test".into(),
            version: "1".into(),
            checks: vec![],
            timeout_seconds: 60,
        };
        let input = JobInput {
            job_id: "parser-contract".into(),
            base_commit: "a".repeat(40),
            task_brief: "Implement the parser".into(),
            acceptance_criteria: vec![Criterion {
                behavior: "Valid input is accepted".into(),
                check_ids: vec!["unit".into()],
                expected: "Tests pass".into(),
            }],
            allowed_paths: vec!["src".into()],
            protected_paths: vec!["tests".into()],
            verification_profile_id: profile.id.clone(),
            policy_version: "1".into(),
        };
        let job = Job {
            input,
            profile: profile.clone(),
            state: "done".into(),
            revision: 4,
            attempts: 1,
            attempt: None,
            receipt: None,
            candidate: None,
            candidate_receipt: None,
            integrated_commit: Some("b".repeat(40)),
            integration_base: None,
            delivery: None,
            blocker: None,
            next_retry_at: None,
            outcome: None,
            verification_pending: None,
        };
        app.apply_overview(Overview {
            project_id: "demo".into(),
            repository: "/tmp/demo".into(),
            base_commit: "a".repeat(40),
            integration_ref: "refs/heads/factory".into(),
            max_workers: 2,
            max_attempts: 3,
            profiles: vec![profile],
            policy: json!({}),
            jobs: vec![job],
        });
        app
    }

    #[test]
    fn delivered_job_and_unmet_goal_remain_distinct() {
        let mut app = app();
        app.goal = Some(json!({"status":"awaiting_plan","unmet_evidence":["runtime_execution"]}));
        let screen = render(&app, 120, 36);
        assert!(screen.contains("Delivered"));
        assert!(screen.contains("BROAD GOAL · local snapshot"));
        assert!(screen.contains("awaiting_plan"));
        assert!(screen.contains("Unmet evidence: runtime_execution"));
        assert!(screen.contains("Delivered jobs do not establish goal completion."));
    }

    #[test]
    fn stale_and_busy_states_are_visible_without_false_live_status() {
        let mut app = app();
        app.last_refresh = Some(Instant::now() - std::time::Duration::from_secs(11));
        app.busy = true;
        let screen = render(&app, 120, 36);
        assert!(screen.contains("STALE / CONTROLS LOCKED"));
        assert!(screen.contains("Request in progress"));
        assert!(!screen.contains("  LIVE"));
    }

    #[test]
    fn every_modal_renders_at_tiny_and_normal_sizes() {
        let mut app = app();
        let input = app.job().unwrap().input.clone();
        let draft = Draft::new(app.overview.as_ref().unwrap());
        let modals = [
            Modal::Draft(draft),
            Modal::Import {
                path: "/tmp/job.json".into(),
                error: Some("No such file".into()),
            },
            Modal::Review { input, draft: None },
            Modal::Confirm {
                operation: "run".into(),
                job_id: "parser-contract".into(),
            },
            Modal::Log {
                title: "checks".into(),
                text: "line\n".repeat(100),
                truncated: true,
            },
            Modal::Help,
        ];
        for modal in modals {
            app.modal = Some(modal);
            for (width, height) in [(1, 1), (30, 8), (58, 17), (80, 24), (120, 40)] {
                render(&app, width, height);
            }
        }
    }

    #[test]
    fn control_characters_are_not_forwarded_to_terminal() {
        let mut app = app();
        app.notice = "hello\x1b[2J\u{202e}hidden".into();
        app.overview.as_mut().unwrap().jobs[0].input.task_brief = "task\x07\u{2066}".into();
        let screen = render(&app, 120, 36);
        assert!(!screen.contains(['\x1b', '\x07', '\u{202e}', '\u{2066}']));
    }

    #[test]
    fn zero_exit_does_not_hide_failed_structured_evidence() {
        let receipt: Receipt = serde_json::from_value(json!({
            "verification_id":"verify-1", "candidate_commit":"a".repeat(40),
            "verification_profile_id":"runtime", "profile_version":"1",
            "environment_id":"clean-checkout", "outcome":"failed",
            "commands":[{"command":"trusted-check", "exit_status":0}],
            "artifacts":[], "config_digest":"hash", "clean_checkout":true,
            "environment":{}, "feature_map":[{
                "behavior":"Actual execution", "verified":false,
                "checks":[{"check_id":"runtime", "exit_status":0, "verified":false,
                    "observations":{"executions":0}, "evidence_error":"No execution observed"}]
            }]
        }))
        .unwrap();
        let mut lines = vec![];
        receipt_lines(&mut lines, "CURRENT RECEIPT", &receipt);
        let text = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("EXIT OK"));
        assert!(!text.contains("PASS"));
        assert!(text.contains("UNVERIFIED"));
        assert!(text.contains("No execution observed"));
        assert!(text.contains("\"executions\":0"));
    }

    #[test]
    fn admission_error_is_visible_above_the_retained_contract() {
        let mut app = app();
        let input = app.job().unwrap().input.clone();
        app.modal = Some(Modal::Review { input, draft: None });
        app.notice = "admit: Controller request interrupted. Inspect refreshed state.".into();
        let screen = render(&app, 120, 40);
        assert!(screen.contains("admit: Controller request interrupted."));
        assert!(screen.contains("REVIEW · approved job contract"));
    }

    #[test]
    fn setup_screens_render_at_tiny_and_normal_sizes() {
        use crate::model::ModelPolicy;
        use std::path::PathBuf;
        let config = crate::model::Config {
            project_id: "demo".into(),
            repository: PathBuf::from("/work/project"),
            data_dir: PathBuf::from("/work/data"),
            delivery_dir: PathBuf::from("/work/delivery"),
            integration_ref: "refs/heads/factory-integration".into(),
            profiles: vec![Profile {
                id: "test".into(),
                version: "1".into(),
                checks: vec![Check {
                    id: "unit".into(),
                    argv: vec!["cargo".into(), "test".into()],
                    result_contract: None,
                }],
                timeout_seconds: 60,
            }],
            policy: ModelPolicy {
                models: vec!["opencode/big-pickle".into()],
                free_only: true,
                spend_ceiling: 0.0,
                data_approved: true,
                cooldown_seconds: 60,
            },
            max_workers: 2,
            lease_seconds: 1800,
            max_attempts: 3,
            opencode_url: Some("http://127.0.0.1:4096".into()),
            verifier_executable: None,
            llama_cpp: None,
            agy: None,
        };
        for stage in 0..3 {
            let mut state = SetupState::new("controller.json".into());
            state.stage = match stage {
                1 => SetupStage::Review {
                    config: Box::new(config.clone()),
                    error: None,
                },
                2 => SetupStage::Saved {
                    path: "controller.json".into(),
                },
                _ => SetupStage::Form,
            };
            for (width, height) in [(1, 1), (30, 8), (58, 17), (80, 24), (120, 40)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| draw_setup(frame, &mut state))
                    .unwrap();
            }
        }
    }

    #[test]
    fn setup_form_shows_sections_and_active_field() {
        let mut state = SetupState::new("controller.json".into());
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal
            .draw(|frame| draw_setup(frame, &mut state))
            .unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .chunks(120)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(screen.contains("CONTROLLER SETUP"));
        assert!(screen.contains("PROJECT"));
        assert!(screen.contains("PROFILE 1"));
        assert!(screen.contains("POLICY"));
        assert!(screen.contains("LIMITS"));
        assert!(screen.contains("controller.json"));
    }
}
