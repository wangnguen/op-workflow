use super::*;

const COMPLETE_PLAN: &str = "## Nguyên nhân\na.ts:1\n## Phương án\nA\n## Đề xuất\nA\n## File sẽ sửa\na.ts\n## Kế hoạch test\nKiểm tra A\n## Rủi ro\na.ts:1\n\n```json\n{\"questions\":[],\"harness\":{\"recommend\":false,\"reason\":\"Đã có test\"},\"browserTest\":{\"recommend\":false}}\n```";

fn public_repo(s: &Setup) {
    git(
        &s.repo,
        &["remote", "add", "origin", "https://github.com/example/sample.git"],
    );
}

fn verified_ctx(s: &Setup, config: &Config) -> TaskCtx {
    let ctx = s.ctx(1, config);
    std::fs::write(
        ctx.file(BRIEF_FILE),
        "# Task\nIssue: https://github.com/example/sample/issues/1\n",
    )
    .unwrap();
    let binding =
        crate::task_repo::Binding::read(&Git::new(&s.repo), "https://github.com/example/sample/issues/1").unwrap();
    s.store
        .lock()
        .unwrap()
        .upsert_task_with_binding(
            &ctx.row,
            crate::task_repo::KEY,
            &serde_json::to_string(&binding).unwrap(),
        )
        .unwrap();
    ctx
}

fn report(job: &Job, outcome: &str) {
    use serde_json::{Value, json};
    let url = if job.agent == Provider::Claude {
        let config: Value = serde_json::from_str(&flag(job, "--mcp-config").unwrap()).unwrap();
        config["mcpServers"]["op_workflow"]["url"]
            .as_str()
            .unwrap()
            .to_owned()
    } else {
        job.args
            .iter()
            .find_map(|a| a.strip_prefix("mcp_servers.op_workflow.url=\""))
            .unwrap()
            .trim_end_matches('"')
            .to_owned()
    };
    let call = |name: &str, arguments: Value| -> Value {
        let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}});
        let mut response = ureq::post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", job.mcp_token.as_ref().unwrap().0),
            )
            .header("Content-Type", "application/json")
            .send(request.to_string().as_bytes())
            .unwrap();
        serde_json::from_str::<Value>(&response.body_mut().read_to_string().unwrap()).unwrap()["result"]
            .clone()
    };
    assert_eq!(
        call(
            "report_step_result",
            json!({"outcome":"mismatch","summary":"Test"})
        )["isError"],
        true
    );
    let contract = call("get_step_contract", json!({}));
    let data: Value = serde_json::from_str(contract["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(data["context"]["purpose"], "executeStep");
    let evidence: Vec<_> = data["criteria"].as_array().unwrap().iter().map(|c| {
        json!({"criterion":c["id"],"detail":"Checked public behavior","sources":[{"file":"a.ts","line":1}]})
    }).collect();
    let mut payload = json!({"outcome":outcome,"summary":"Checked","evidence":evidence});
    if outcome == "completed" && data["context"]["step"] == "analyze" {
        payload["resultText"] = json!(COMPLETE_PLAN);
        let mut invalid = payload.clone();
        invalid["evidence"][1]["sources"][0]["line"] = json!(1000);
        assert_eq!(call("report_step_result", invalid)["isError"], true);
    }
    assert_eq!(call("report_step_result", payload)["isError"], false);
    assert_eq!(
        call(
            "report_step_result",
            json!({"outcome":"mismatch","summary":"Again"})
        )["isError"],
        true
    );
}

#[test]
fn mcp_plan_has_one_session_and_still_requires_a_valid_verdict() {
    for provider in ["claude", "codex"] {
        let Some(s) = setup() else { return };
        public_repo(&s);
        let mut config = Config::default();
        config.steps.analyze.agent = Some(provider.into());
        let ctx = verified_ctx(&s, &config);
        let calls = AtomicUsize::new(0);
        let fake = Arc::new(Fake {
            script: Box::new(move |job, _, out| {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                match n {
                    0 => {}
                    1 => report(job, "mismatch"),
                    _ => report(job, "completed"),
                }
                if job.agent == Provider::Claude {
                    out(&result_line("done"));
                } else {
                    out(r#"{"type":"thread.started","thread_id":"test-thread"}"#);
                    out(r#"{"type":"item.completed","item":{"type":"agent_message","text":"done"}}"#);
                }
                ok()
            }),
            jobs: Mutex::new(Vec::new()),
            missing: false,
        });
        let flow = Flow::new(s.store.clone(), fake.clone(), None);
        flow.started(&ctx, None).unwrap();
        assert_eq!(
            run(&flow, &ctx, FlowAction::Run).error.unwrap().code,
            "mcp.missingVerdict"
        );
        assert_eq!(
            run(&flow, &ctx, FlowAction::Retry).error.unwrap().code,
            "mcp.mismatch"
        );
        let d = run(&flow, &ctx, FlowAction::Retry);
        assert_eq!(d.gate.unwrap().kind, GateKind::PlanReview);
        assert_eq!(fake.jobs.lock().unwrap().len(), 3, "No extra plan audit run");
        assert_eq!(
            ctx.read(PLAN_FILE).unwrap().trim(),
            text::plan_answer(COMPLETE_PLAN).plan.trim()
        );
        assert_eq!(
            flow.act(ctx.clone(), FlowAction::Finish { forced: true })
                .unwrap_err()
                .code,
            "flow.notReady"
        );
        let d = run(&flow, &ctx, FlowAction::ApprovePlan);
        assert_eq!(d.current, Some(StepKind::ReviewLocal), "{:?}", d.error);
    }
}

fn no_mcp_flow(s: &Setup, script: impl ScriptFn + 'static) -> (Flow, Arc<Fake>, TaskCtx) {
    public_repo(s);
    let mut config = Config::default();
    config.steps.verify_mcp = false;
    let ctx = verified_ctx(s, &config);
    let fake = Arc::new(Fake {
        script: Box::new(script),
        jobs: Mutex::new(Vec::new()),
        missing: false,
    });
    let flow = Flow::new(s.store.clone(), fake.clone(), None);
    flow.started(&ctx, None).unwrap();
    (flow, fake, ctx)
}

#[test]
fn disabled_mcp_keeps_plan_repo_and_finish_guards_and_applies_next_run() {
    for provider in ["claude", "codex"] {
        let Some(s) = setup() else { return };
        let (flow, fake, mut ctx) = no_mcp_flow(&s, |job, _, out| {
            if job.mcp_token.is_some() {
                report(job, "completed");
            } else {
                assert!(
                    !job.args
                        .iter()
                        .any(|a| a.contains("mcp_servers.") || a == "--mcp-config")
                );
                assert!(!prompt_of(job).contains("report_step_result"));
            }
            if job.agent == Provider::Claude {
                out(&result_line(COMPLETE_PLAN));
            } else {
                out(&serde_json::json!({"type":"item.completed","item":{"type":"agent_message","text":COMPLETE_PLAN}}).to_string());
            }
            ok()
        });
        ctx.config.steps.analyze.agent = Some(provider.into());
        assert_eq!(
            run(&flow, &ctx, FlowAction::Run).gate.unwrap().kind,
            GateKind::PlanReview
        );
        assert_eq!(
            flow.act(ctx.clone(), FlowAction::Finish { forced: true })
                .unwrap_err()
                .code,
            "flow.notReady"
        );
        git(
            &s.repo,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/example/other.git",
            ],
        );
        assert_eq!(
            run(&flow, &ctx, FlowAction::ApprovePlan).error.unwrap().code,
            "repo.changed"
        );
        assert_eq!(fake.jobs.lock().unwrap().len(), 1);
        git(
            &s.repo,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/example/sample.git",
            ],
        );
        std::fs::write(ctx.file(BRIEF_FILE), "Changed by hand").unwrap();
        assert_eq!(flow.detail(&ctx).error.unwrap().code, "plan.briefChanged");
        // Reanalysis keeps the old plan; the next run reads the new MCP setting.
        ctx.config.steps.verify_mcp = true;
        let d = run(
            &flow,
            &ctx,
            FlowAction::Reopen {
                step: Some(StepKind::Analyze),
                note: None,
            },
        );
        assert_eq!(d.gate.unwrap().kind, GateKind::PlanReview);
        let jobs = fake.jobs.lock().unwrap();
        assert_eq!(jobs.len(), 2);
        assert!(jobs.last().unwrap().mcp_token.is_some());
    }
}

#[test]
fn brief_changed_during_implementation_keeps_success_and_waits_before_next_step() {
    let Some(s) = setup() else { return };
    let brief_path = s.tasks.join("WP-1").join(BRIEF_FILE);
    let calls = AtomicUsize::new(0);
    let (flow, fake, mut ctx) = no_mcp_flow(&s, move |job, _, out| {
        let n = calls.fetch_add(1, Ordering::SeqCst);
        if n == 1 {
            std::fs::write(&brief_path, "Changed during implementation").unwrap();
            std::fs::write(job.cwd.join("a.ts"), "export const a = 2;\n").unwrap();
        }
        out(&result_line(if n == 0 { COMPLETE_PLAN } else { CLEAN }));
        ok()
    });
    let alerts = Arc::new(Mutex::new(Vec::new()));
    let sink = alerts.clone();
    flow.set_alert_sink(move |a| sink.lock().unwrap().push(a));
    ctx.config.steps.auto_advance = true;
    run(&flow, &ctx, FlowAction::Run);
    let d = run(&flow, &ctx, FlowAction::ApprovePlan);
    assert_eq!(d.error.unwrap().code, "plan.briefChanged");
    assert_eq!(
        d.steps
            .iter()
            .find(|s| s.kind == StepKind::Implement)
            .unwrap()
            .state,
        StepState::Done
    );
    assert_eq!(
        fake.jobs.lock().unwrap().len(),
        2,
        "No next AI step while the question is open"
    );
    assert!(
        alerts
            .lock()
            .unwrap()
            .iter()
            .any(|a| a.kind == AlertKind::Gate && a.title.contains("Brief đã đổi"))
    );
    assert!(d.runs.last().unwrap().state == RunState::Done);
    ctx.config.steps.auto_advance = false;
    let d = run(&flow, &ctx, FlowAction::KeepPlan);
    assert!(d.error.is_none());
    assert!(d.log.iter().any(|l| l.text == "Bạn giữ plan sau khi brief đổi"));
}

#[test]
fn refreshed_comments_ask_immediately_keep_preserves_gate_and_reanalyze_has_old_plan() {
    let Some(s) = setup() else { return };
    let (flow, fake, ctx) = no_mcp_flow(&s, |job, _, out| {
        let prompt = prompt_of(job);
        if prompt.contains("<previous_plan>") {
            assert!(prompt.contains("Brief có comment mới"));
            assert!(prompt.contains("New requirement"));
        }
        out(&result_line(if kind(job) == "analyze" {
            COMPLETE_PLAN
        } else {
            CLEAN
        }));
        ok()
    });
    run(&flow, &ctx, FlowAction::Run);
    run(&flow, &ctx, FlowAction::SelfCode);
    let brief = ctx.read(BRIEF_FILE).unwrap();
    std::fs::write(
        ctx.file(BRIEF_FILE),
        format!("{brief}\n## Comment trên OpenProject\n### 2026-11-19\nNew requirement\n"),
    )
    .unwrap();
    flow.brief_refreshed(&ctx);
    drop(flow);
    let flow = Flow::new(s.store.clone(), fake.clone(), None);
    let d = flow.detail(&ctx);
    assert_eq!(d.gate.as_ref().unwrap().kind, GateKind::PlanChanged);
    let message = d.gate.unwrap().message;
    assert!(message.contains("New requirement"));
    assert!(!message.contains("sửa tay"));
    assert_eq!(flow.card(&ctx).unwrap().step.state, StepState::WaitingUser);
    let d = run(&flow, &ctx, FlowAction::KeepPlan);
    assert_eq!(d.gate.unwrap().kind, GateKind::SelfCode);
    assert_eq!(fake.jobs.lock().unwrap().len(), 1);
    std::fs::write(
        ctx.file(PLAN_FILE),
        format!("{}\nManual addition", ctx.read(PLAN_FILE).unwrap()),
    )
    .unwrap();
    assert!(flow.detail(&ctx).error.unwrap().message.contains("sửa tay"));
    let d = run(
        &flow,
        &ctx,
        FlowAction::Reopen {
            step: Some(StepKind::Analyze),
            note: None,
        },
    );
    assert_eq!(d.gate.unwrap().kind, GateKind::PlanReview);
    assert_eq!(fake.jobs.lock().unwrap().len(), 2);
}

#[test]
fn budget_go_on_resumes_analysis_with_current_limits_and_fresh_retry_does_not_resume() {
    for fresh in [false, true] {
        let Some(s) = setup() else { return };
        let calls = AtomicUsize::new(0);
        let (flow, fake, mut ctx) = no_mcp_flow(&s, move |job, _, out| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                out(&init_line());
                out(
                    r#"{"type":"result","subtype":"error_max_budget_usd","is_error":true,"session_id":"budget-session","total_cost_usd":5.0,"result":"partial"}"#,
                );
            } else {
                assert_eq!(
                    flag(job, "--resume"),
                    (!fresh).then(|| "budget-session".to_owned())
                );
                assert_eq!(flag(job, "--max-budget-usd").as_deref(), Some("7"));
                assert_eq!(job.timeout, Duration::from_secs(31 * 60));
                if !fresh {
                    assert!(job.stdin.as_deref().unwrap().contains("## Nguyên nhân"));
                }
                out(&result_line(COMPLETE_PLAN));
            }
            ok()
        });
        let d = run(&flow, &ctx, FlowAction::Run);
        assert_eq!(d.error.as_ref().unwrap().code, "step.budget");
        assert!(!d.error.unwrap().message.contains("config.json"));
        assert!(d.can_go_on);
        // The saved session survives reopening the app.
        drop(flow);
        let flow = Flow::new(s.store.clone(), fake, None);
        ctx.config.steps.analyze.max_budget_usd = Some(7.0);
        ctx.config.steps.analyze.timeout_min = Some(31);
        assert_eq!(flow.detail(&ctx).continuation_budget_usd, Some(7.0));
        let d = run(
            &flow,
            &ctx,
            if fresh {
                FlowAction::Retry
            } else {
                FlowAction::GoOn { note: String::new() }
            },
        );
        assert_eq!(d.gate.unwrap().kind, GateKind::PlanReview, "{:?}", d.error);
    }
}

#[test]
fn automatic_next_step_reads_the_changed_mcp_switch_from_settings() {
    let Some(s) = setup() else { return };
    let path = s._tmp.path().join("settings.json");
    let saved_path = path.clone();
    let calls = AtomicUsize::new(0);
    let (flow, fake, ctx) = no_mcp_flow(&s, move |job, _, out| {
        let n = calls.fetch_add(1, Ordering::SeqCst);
        match n {
            0 => {
                assert!(job.mcp_token.is_none());
                out(&result_line(COMPLETE_PLAN));
            }
            1 => {
                assert!(job.mcp_token.is_none());
                std::fs::write(&saved_path, r#"{"steps":{"verifyMcp":true,"autoAdvance":true}}"#).unwrap();
                out(&result_line("Implemented"));
            }
            2 => {
                assert!(job.mcp_token.is_some());
                report(job, "blocked");
                out(&result_line(CLEAN));
            }
            _ => panic!("No step may run after a blocked verdict"),
        }
        ok()
    });
    std::fs::write(&path, r#"{"steps":{"verifyMcp":false}}"#).unwrap();
    flow.set_config_path(path);
    run(&flow, &ctx, FlowAction::Run);
    let d = run(&flow, &ctx, FlowAction::ApprovePlan);
    assert_eq!(d.error.unwrap().code, "mcp.blocked");
    assert_eq!(fake.jobs.lock().unwrap().len(), 3);
}

#[test]
fn disabled_mcp_keeps_post_run_repo_checks_and_selected_findings_guard() {
    let Some(s) = setup() else { return };
    let (flow, fake, ctx) = no_mcp_flow(&s, |job, _, out| {
        git(
            &job.cwd,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/example/other.git",
            ],
        );
        out(&result_line(COMPLETE_PLAN));
        ok()
    });
    let d = run(&flow, &ctx, FlowAction::Run);
    assert_eq!(d.error.unwrap().code, "repo.changed");
    let finding = FindingRow {
        wp_id: 1,
        fingerprint: "selected".into(),
        severity: "high".into(),
        title: "Selected".into(),
        file: "a.ts".into(),
        line: Some(1),
        source: "test".into(),
        state: "open".into(),
        reason: None,
        detail: None,
        suggestion: None,
        step: Some("reviewLocal".into()),
        round: None,
        chosen: true,
        seen_at: None,
    };
    s.store.lock().unwrap().upsert_finding(&finding).unwrap();
    let claim = Claim {
        step: StepKind::PushSummary,
        agent: AgentKind::Claude,
        stop: Arc::new(AtomicBool::new(false)),
        choice: ModelChoice::default(),
        mode: None,
        nested: false,
    };
    match run_step(&flow.inner, &ctx, &claim, &s.repo) {
        Err(e) => assert_eq!(e.code, "mcp.openFindings"),
        Ok(_) => panic!("Selected findings must block handoff"),
    }
    assert_eq!(fake.jobs.lock().unwrap().len(), 1);
}

#[test]
fn budget_continuation_of_a_custom_push_command_uses_the_saved_session() {
    let Some(s) = setup() else { return };
    let (flow, _, ctx) = no_mcp_flow(&s, |job, _, out| {
        assert!(
            !job.args
                .iter()
                .any(|arg| arg == "--no-session-persistence" || arg.starts_with('/'))
        );
        assert_eq!(flag(job, "--resume").as_deref(), Some("push-session"));
        assert!(job.stdin.as_deref().unwrap().contains("Go on with the same task"));
        out(&result_line(PUSH));
        ok()
    });
    flow.inner.with_saved(1, |s| {
        s.go_on = Some(GoOnRun {
            step: StepKind::PushSummary,
            agent: AgentKind::Claude,
            session: "push-session".into(),
            note: String::new(),
        })
    });
    let claim = Claim {
        step: StepKind::PushSummary,
        agent: AgentKind::Claude,
        stop: Arc::new(AtomicBool::new(false)),
        choice: ModelChoice::default(),
        mode: None,
        nested: false,
    };
    let job = Job {
        agent: Provider::Claude,
        args: claude::push_summary_args(None, 1.0),
        cwd: s.repo.clone(),
        stdin: None,
        timeout: Duration::from_secs(600),
        mcp_token: None,
    };
    assert!(
        flow.inner
            .run_claude(&ctx, StepKind::PushSummary, &job, &claim)
            .is_ok()
    );
}

#[test]
fn disabled_mcp_still_rejects_incomplete_analysis_and_failed_checks_after_a_fix() {
    let Some(s) = setup() else { return };
    let (flow, _, mut ctx) = no_mcp_flow(&s, |_, _, out| {
        out(&result_line("Incomplete plan"));
        ok()
    });
    assert_eq!(
        run(&flow, &ctx, FlowAction::Run).error.unwrap().code,
        "mcp.planIncomplete"
    );
    ctx.config.repos.insert(
        s.repo.to_string_lossy().into_owned(),
        RepoConfig {
            checks: vec!["npm test".into()],
            ..RepoConfig::default()
        },
    );
    std::fs::write(s.repo.join("a.ts"), "export const a = 2;\n").unwrap();
    flow.set_check_runner(Arc::new(FakeChecks {
        calls: AtomicUsize::new(0),
        answer: |_, _| Err("Missing test tool".into()),
    }));
    let claim = Claim {
        step: StepKind::Fix,
        agent: AgentKind::Claude,
        stop: Arc::new(AtomicBool::new(false)),
        choice: ModelChoice::default(),
        mode: None,
        nested: false,
    };
    match run_checks(&flow.inner, &ctx, &claim, &s.repo, "main", StepKind::Fix) {
        Err(e) => assert_eq!(e.code, "checks.failed"),
        Ok(_) => panic!("A fix cannot succeed while checks fail"),
    }
}

#[test]
fn capped_check_repair_resumes_its_ai_session_with_the_fix_setting() {
    let Some(s) = setup() else { return };
    let calls = AtomicUsize::new(0);
    let (flow, _, mut ctx) = no_mcp_flow(&s, move |job, _, out| {
        match calls.fetch_add(1, Ordering::SeqCst) {
            0 => out(&result_line(COMPLETE_PLAN)),
            1 => {
                std::fs::write(job.cwd.join("a.ts"), "export const a = 2;\n").unwrap();
                out(&result_line("Implemented"));
            }
            2 => out(
                r#"{"type":"result","subtype":"error_max_budget_usd","is_error":true,"session_id":"repair-session","total_cost_usd":2.0}"#,
            ),
            3 => {
                assert_eq!(flag(job, "--resume").as_deref(), Some("repair-session"));
                assert_eq!(flag(job, "--max-budget-usd").as_deref(), Some("3"));
                assert_eq!(job.timeout, Duration::from_secs(9 * 60));
                out(&result_line("Fixed"));
            }
            _ => panic!("Unexpected AI run"),
        }
        ok()
    });
    ctx.config.repos.insert(
        s.repo.to_string_lossy().into_owned(),
        RepoConfig {
            checks: vec!["npm test".into()],
            ..RepoConfig::default()
        },
    );
    flow.set_check_runner(Arc::new(FakeChecks {
        calls: AtomicUsize::new(0),
        answer: |n, _| Ok((Some(if n < 2 { 1 } else { 0 }), "test output".into())),
    }));
    run(&flow, &ctx, FlowAction::Run);
    let d = run(&flow, &ctx, FlowAction::ApprovePlan);
    assert_eq!(d.current, Some(StepKind::Checks));
    assert_eq!(d.error.unwrap().code, "step.budget");
    assert!(d.can_go_on);
    ctx.config.steps.fix.max_budget_usd = Some(3.0);
    ctx.config.steps.fix.timeout_min = Some(9);
    assert_eq!(flow.detail(&ctx).continuation_budget_usd, Some(3.0));
    let d = run(&flow, &ctx, FlowAction::GoOn { note: String::new() });
    assert!(d.error.is_none());
    assert_eq!(d.current, Some(StepKind::ReviewLocal));
}
