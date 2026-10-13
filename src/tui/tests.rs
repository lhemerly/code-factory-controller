use super::*;
use crate::model::Criterion;

fn app() -> App {
    let mut app = App::new("http://127.0.0.1:4317".into(), false);
    app.apply_overview(demo_overview());
    app
}
fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}
fn draft(app: &App) -> Draft {
    let mut draft = Draft::new(app.overview.as_ref().unwrap());
    draft.values[1] = "Implement useful errors".into();
    draft.values[2] = "Invalid input includes line number".into();
    draft.values[3] = "Fixture output matches".into();
    draft.values[4] = "src, tests".into();
    draft
}
#[test]
fn review_and_actions_require_explicit_confirmation_and_fresh_state() {
    let mut app = app();
    app.modal = Some(Modal::Draft(draft(&app)));
    let save = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert!(handle_key(&mut app, save).is_none());
    assert!(matches!(app.modal, Some(Modal::Review { .. })));
    assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).is_none());
    app.last_refresh = None;
    assert!(handle_key(&mut app, key('y')).is_none());
    app.last_refresh = Some(Instant::now());
    assert!(
        handle_key(&mut app, save).is_none(),
        "holding the review key must not submit"
    );
    assert!(matches!(handle_key(&mut app, key('y')), Some(Effect::Action(op,_)) if op=="admit"));
    assert!(
        matches!(app.modal, Some(Modal::Review { .. })),
        "keep input until confirmed"
    );
    app.modal = None;
    assert!(handle_key(&mut app, key('a')).is_none());
    assert!(matches!(app.modal, Some(Modal::Confirm { .. })));
    assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).is_none());
    assert!(app.modal.is_none());
}

#[test]
fn late_log_never_discards_input_and_admission_reconciles_by_exact_input() {
    let mut app = app();
    let draft = draft(&app);
    let input = draft.build(app.overview.as_ref().unwrap()).unwrap();
    app.modal = Some(Modal::Draft(draft.clone()));
    apply_log(
        &mut app,
        "log".into(),
        Ok(json!({"text":"result","truncated":false})),
    );
    assert!(matches!(app.modal, Some(Modal::Draft(_))));
    app.modal = Some(Modal::Review {
        input: input.clone(),
        draft: Some(draft),
    });
    assert!(handle_key(&mut app, key('y')).is_some());
    // Rejected/lost responses retain the full review; a refreshed matching
    // durable job resolves the admission without resubmitting the request.
    let mut overview = demo_overview();
    app.apply_overview(overview.clone());
    assert!(matches!(app.modal, Some(Modal::Review { .. })));
    let mut job = overview.jobs[0].clone();
    job.input = input;
    overview.jobs.push(job);
    app.apply_overview(overview);
    assert!(app.modal.is_none());
    assert!(app.admitting.is_none());
    assert!(app.notice.contains("admit confirmed"));
}

#[test]
fn old_network_snapshot_is_stale_and_fifo_import_is_nonblocking() {
    let mut app = app();
    app.apply_observed_overview(demo_overview(), Instant::now() - Duration::from_secs(11));
    assert!(!app.can_write());
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.json");
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(
            read_json(&path)
                .unwrap_err()
                .to_string()
                .contains("regular JSON file")
        );
    }
}
#[test]
fn drafting_validates_scope_checks_and_preserves_imported_criteria() {
    let app = app();
    let overview = app.overview.as_ref().unwrap();
    let mut input = draft(&app).build(overview).unwrap();
    input.acceptance_criteria.push(Criterion {
        behavior: "Runtime case".into(),
        expected: "Actual output".into(),
        check_ids: vec!["runtime".into()],
    });
    let copied = serde_json::from_value(json!(input)).unwrap();
    validate_input(&copied, overview).unwrap();
    assert_eq!(copied.acceptance_criteria.len(), 2);
    input.allowed_paths = vec!["../escape".into()];
    assert!(validate_input(&input, overview).is_err());
    input.allowed_paths = vec!["src".into()];
    input.acceptance_criteria[0].check_ids = vec!["invented".into()];
    assert!(validate_input(&input, overview).is_err());
}
#[test]
fn paste_is_data_and_terminal_controls_are_removed() {
    let mut app = app();
    app.modal = Some(Modal::Draft(draft(&app)));
    paste(&mut app, "\x1b[31mtest\u{202e}\x07");
    assert!(matches!(app.modal, Some(Modal::Draft(_))));
    assert_eq!(safe_text("a\x1b\x07\u{2066}b\n"), "ab\n");
    app.modal = None;
    paste(&mut app, "acry");
    assert!(app.modal.is_none());
}
#[test]
fn connection_and_demo_disable_mutations() {
    let mut app = app();
    app.demo = true;
    assert!(handle_key(&mut app, key('r')).is_none());
    assert!(app.modal.is_none());
    app.demo = false;
    app.last_refresh = Some(Instant::now() - Duration::from_secs(11));
    assert!(!app.can_write());
    app.apply_overview(demo_overview());
    app.busy = true;
    assert!(!app.can_write());
}
#[test]
fn operator_origin_never_leaks_auth_to_remote_proxy_or_redirect() {
    for url in [
        "https://127.0.0.1:4317",
        "http://example.com",
        "http://127.0.0.1/path",
        "http://user:secret@127.0.0.1",
        "http://127.0.0.1?x=1",
        "http://127.0.0.1#secret",
    ] {
        assert!(origin(url).is_err(), "{url}");
    }
    assert!(origin("http://127.0.0.1:4317").is_ok());
    assert!(origin("http://[::1]:4317").is_ok());
}
#[test]
fn refresh_preserves_selection_by_identity_and_missing_goal_is_not_completed() {
    let mut app = app();
    let selected = app.job().unwrap().input.job_id.clone();
    let mut overview = demo_overview();
    overview.jobs.reverse();
    app.apply_overview(overview);
    assert_eq!(app.job().unwrap().input.job_id, selected);
    app.goal = Some(json!({"status":"completed"}));
    read_goal(
        &mut app,
        Some(Path::new("/nonexistent-factory-goal-state.json")),
    );
    assert!(app.goal.is_none());
    assert!(app.goal_error.is_some());
}
