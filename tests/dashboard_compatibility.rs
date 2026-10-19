//! Focused migration boundaries; the frozen dashboard tests/oracle are unchanged.
use code_factory_controller::dashboard::domain::{build_time, build_tokens, parse_ts};
use serde_json::{Value, json};

fn cycle(tokens: Value) -> Value {
    json!({"files":{"result.json":{"usage":{"tokens":tokens,"cost":0,"accounting_status":"settled_zero"}}}})
}

#[test]
fn integer_token_fields_and_cross_cycle_sums_remain_exact() {
    let n = 9_007_199_254_740_993u64;
    let first = cycle(json!({"input":n,"output":n,"reasoning":n,"cache":{"read":n,"write":n}}));
    let second = cycle(json!({"input":1,"output":2,"reasoning":3,"cache":{"read":4,"write":5}}));
    let result = build_tokens(&[first, second]);
    assert_eq!(
        result["totals"],
        json!({"input":n+1,"output":n+2,"reasoning":n+3,"cache_read":n+4,"cache_write":n+5,"total":4*n+12,"generated":n+2})
    );
    assert_eq!(result["accounting"]["state"], "settled");
    let signed_boundary =
        build_tokens(&[cycle(json!({"input":i64::MAX})), cycle(json!({"input":1}))]);
    assert_eq!(signed_boundary["totals"]["input"], json!(1u64 << 63));
    assert_eq!(signed_boundary["totals"]["total"], json!(1u64 << 63));
}

#[test]
fn unrepresentable_totals_are_partial_without_rounding_or_wrapping() {
    let result = build_tokens(&[cycle(json!({"input":u64::MAX,"output":1}))]);
    assert_eq!(result["totals"]["input"], json!(u64::MAX));
    assert_eq!(result["totals"]["output"], 1);
    assert_eq!(result["totals"]["total"], Value::Null);
    assert_eq!(result["accounting"]["state"], "partial");
    assert!(
        result["accounting"]["note"]
            .as_str()
            .unwrap()
            .contains("unknown")
    );
    let field_overflow = build_tokens(&[
        cycle(json!({"input":u64::MAX})),
        cycle(json!({"input":1,"output":3})),
    ]);
    assert_eq!(field_overflow["totals"]["input"], Value::Null);
    assert_eq!(field_overflow["totals"]["output"], 3);
    assert_eq!(field_overflow["accounting"]["state"], "partial");
}

#[test]
fn html_displays_exact_unsigned_totals_and_signed_token_counts() {
    use code_factory_controller::dashboard::{domain::build_dashboard, render};
    let raw = json!({"mode":"live","launch":{"worker_slots":1},"dispatcher":{},"reads":[],"projects":[{"goal_id":"one","dir":"one","goal":{},"launch":{},"status":{},"dispatcher_goal":{},"cycles":[cycle(json!({"input":(1u64<<63)+100,"output":-100,"reasoning":0}))]}]});
    let model = build_dashboard(&raw, Some("2026-01-01T00:00:00Z"), 900.).unwrap();
    let html = render::html(&model);
    assert!(html.contains("9,223,372,036,854,775,808"));
    assert!(html.contains("-100"));
    assert!(!html.contains("-,100"));
}

#[test]
fn legacy_float_truncation_and_nonnumeric_fields_are_preserved() {
    let result = build_tokens(&[cycle(
        json!({"input":12.75,"output":-2.75,"reasoning":true,"cache":{"read":"10","write":null}}),
    )]);
    assert_eq!(
        result["totals"],
        json!({"input":12,"output":-2,"reasoning":0,"cache_read":0,"cache_write":0,"total":10,"generated":-2})
    );
    let unknown = build_tokens(&[json!({"files":{"result.json":{"status":"failed"}}})]);
    assert_eq!(unknown["totals"]["input"], Value::Null);
    assert_eq!(unknown["accounting"]["state"], "unknown");
}

#[test]
fn legacy_calendar_dates_are_midnight_utc_and_restore_recorded_time() {
    assert_eq!(
        parse_ts(&json!("2026-01-01")),
        parse_ts(&json!("2026-01-01T00:00:00Z"))
    );
    assert_eq!(
        parse_ts(&json!(" 2024-02-29 ")),
        parse_ts(&json!("2024-02-29T00:00:00+00:00"))
    );
    let cycles = [
        json!({"files":{"result.json":{"started_utc":"2026-01-01","ended_utc":"2026-01-01T00:01:00Z"}}}),
    ];
    let as_of = parse_ts(&json!("2026-01-01T01:00:00Z")).unwrap();
    let time = build_time(&cycles, &json!({}), as_of);
    assert_eq!(time["active_seconds"], 60.0);
    assert_eq!(time["elapsed_seconds"], 3600.0);
    assert_eq!(time["waiting_seconds"], 3540.0);
    assert_eq!(time["partial"], false);
}

#[test]
fn malformed_calendar_dates_stay_unknown() {
    for value in [
        "2026-02-29",
        "2026-13-01",
        "2026-1-1",
        "0000-01-01",
        "2026-01-01junk",
    ] {
        assert_eq!(parse_ts(&json!(value)), None, "{value}");
    }
    assert_eq!(
        parse_ts(&json!("2026-01-01T01:00:00+01:00")),
        parse_ts(&json!("2026-01-01"))
    );
}
