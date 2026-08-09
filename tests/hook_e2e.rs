//! End-to-end tests for `scripts/zj-agent-mob-hook.sh`.
//!
//! The hook is the seam between a real agent and the plugin: hook-event JSON
//! arrives on stdin, a `zellij pipe --args ...` call comes out. Everything in
//! between (event mapping, transcript reading, sanitizing, the status spool) is
//! untested by the unit suite, which starts from an already-parsed pipe message.
//!
//! `zellij` is stubbed with a script that records its argv, so these run
//! anywhere: no zellij, no agent, no pane. Each case feeds real-shaped JSON and
//! asserts on the emitted args.
//!
//! Args are parsed into a map rather than substring-matched. `--args` is
//! comma-separated `key=value`, so a substring check for `task=first line`
//! also passes when the value is `first lines`; a map comparison cannot.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A `zellij pipe` invocation the hook made.
#[derive(Debug, Clone)]
struct Pipe {
    name: String,
    args: BTreeMap<String, String>,
    plugin: String,
}

#[derive(Debug)]
struct Run {
    pipes: Vec<Pipe>,
    stdout: String,
    code: i32,
}

impl Run {
    /// The `agent-status` pipe, which is what a normal event emits.
    fn status_pipe(&self) -> Option<&Pipe> {
        self.pipes.iter().find(|p| p.name == "agent-status")
    }

    fn ask_pipe(&self) -> Option<&Pipe> {
        self.pipes.iter().find(|p| p.name == "agent-ask")
    }

    /// Field of the `agent-status` pipe. Panics if the hook stayed silent, so a
    /// test that expects a report fails loudly rather than comparing None.
    fn field(&self, key: &str) -> &str {
        let pipe = self
            .status_pipe()
            .unwrap_or_else(|| panic!("expected an agent-status pipe, got: {:?}", self.pipes));
        pipe.args
            .get(key)
            .unwrap_or_else(|| panic!("no `{key}` in args: {:?}", pipe.args))
    }

    /// True when the hook emitted nothing at all.
    fn silent(&self) -> bool {
        self.pipes.is_empty()
    }
}

/// Splits a `--args` string into pairs. Every segment must contain `=`: a comma
/// surviving inside a value leaves a fragment with no key, which the plugin
/// would read as a new field. Returned as an Err so tests can assert on it.
fn parse_args(raw: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for seg in raw.split(',') {
        match seg.split_once('=') {
            Some((k, v)) => {
                out.insert(k.to_string(), v.to_string());
            }
            None => return Err(seg.to_string()),
        }
    }
    Ok(out)
}

static CASE: AtomicU32 = AtomicU32::new(0);

/// A throwaway directory tree, removed on drop.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        // Tests run in parallel and several derive a path from TMPDIR (the
        // verdict file in particular), so the per-process counter is what keeps
        // two concurrent cases off the same file.
        let n = CASE.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("zj-hook-e2e-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create sandbox");
        Sandbox { root }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Some cases chmod a directory read-only to test degradation; restore
        // it so the cleanup can recurse.
        let spool = self.root.join("spool");
        if spool.is_dir() {
            let _ = fs::set_permissions(&spool, fs::Permissions::from_mode(0o700));
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

use std::os::unix::fs::PermissionsExt;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn hook_path() -> PathBuf {
    repo_root().join("scripts/zj-agent-mob-hook.sh")
}

/// Writes the `zellij` stub: records argv, one invocation per line.
fn write_stub(bin: &Path) {
    fs::create_dir_all(bin).expect("create bin dir");
    let stub = bin.join("zellij");
    fs::write(&stub, "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$ZJ_TEST_CAPTURE\"\n").expect("write stub");
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

/// One sandboxed hook installation. A single `Hook` can be run many times, so a
/// test can assert on how one event's spool record affects the next.
struct Hook {
    sandbox: Sandbox,
}

/// One invocation, with env overrides layered on the Hook's defaults. Borrows
/// the Hook so the sandbox outlives the run and its files stay inspectable.
struct Invocation<'a> {
    hook: &'a Hook,
    env: Vec<(String, String)>,
}

impl<'a> Invocation<'a> {
    fn env(mut self, k: &str, v: impl AsRef<Path>) -> Self {
        self.env
            .push((k.to_string(), v.as_ref().to_string_lossy().into_owned()));
        self
    }

    fn run(self, json: &str) -> Run {
        self.hook.exec(json, &self.env)
    }
}

impl Hook {
    fn new() -> Self {
        let sandbox = Sandbox::new();
        write_stub(&sandbox.path("bin"));
        Hook { sandbox }
    }

    /// Starts an invocation with one env override.
    fn env(&self, k: &str, v: impl AsRef<Path>) -> Invocation<'_> {
        Invocation {
            hook: self,
            env: Vec::new(),
        }
        .env(k, v)
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.sandbox.path(rel)
    }

    /// Runs the hook with no env overrides.
    fn run(&self, json: &str) -> Run {
        self.exec(json, &[])
    }

    /// Runs the hook with `json` on stdin. Defaults mimic a normal pane;
    /// anything in `env` overrides them.
    fn exec(&self, json: &str, env: &[(String, String)]) -> Run {
        let capture = self.sandbox.path("capture");
        let _ = fs::remove_file(&capture);
        fs::write(&capture, "").expect("init capture");

        let path_var = format!(
            "{}:{}",
            self.sandbox.path("bin").display(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
        );

        let mut cmd = Command::new("sh");
        cmd.arg(hook_path())
            .env_clear()
            .env("PATH", &path_var)
            .env("ZJ_TEST_CAPTURE", &capture)
            .env("ZELLIJ_PANE_ID", "3")
            .env("ZJ_AGENT_PLUGIN", "file:/plugin.wasm")
            // Keep the real spool and real HOME out of every run by default.
            .env("HOME", self.sandbox.path("home"))
            .env("ZJ_AGENT_SPOOL_DIR", self.sandbox.path("spool"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        for (k, v) in env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().expect("spawn hook");
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(json.as_bytes())
            .expect("write stdin");
        let out = child.wait_with_output().expect("wait hook");

        let raw = fs::read_to_string(&capture).unwrap_or_default();
        let pipes = raw.lines().filter(|l| !l.trim().is_empty()).map(parse_pipe).collect();

        Run {
            pipes,
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            code: out.status.code().unwrap_or(-1),
        }
    }
}

/// Parses one recorded `zellij` argv line into a Pipe.
fn parse_pipe(line: &str) -> Pipe {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let after = |flag: &str| -> String {
        toks.iter()
            .position(|t| *t == flag)
            .and_then(|i| toks.get(i + 1))
            .map(|s| s.to_string())
            .unwrap_or_default()
    };
    // --args is the last flag and its value may contain spaces, so take the
    // rest of the line rather than a single token.
    let args_raw = match line.find("--args ") {
        Some(at) => line[at + "--args ".len()..].trim().trim_matches('\'').to_string(),
        None => String::new(),
    };
    Pipe {
        name: after("--name"),
        plugin: after("--plugin"),
        args: parse_args(&args_raw).unwrap_or_default(),
    }
}

/// Convenience for the common case: default env, one event.
fn run(json: &str) -> Run {
    Hook::new().run(json)
}

fn ev(name: &str) -> String {
    serde_json::json!({ "hook_event_name": name }).to_string()
}

// ---------------------------------------------------------------------------
// event -> status mapping
// ---------------------------------------------------------------------------

#[test]
fn events_map_to_statuses() {
    let cases = [
        ("SessionStart", "idle"),
        ("UserPromptSubmit", "working"),
        ("Notification", "waiting"),
        ("PermissionRequest", "waiting"),
        ("Stop", "done"),
        ("SessionEnd", "ended"),
        ("PreToolUse", "working"),
        ("PostToolUse", "working"),
        ("StopFailure", "failed"),
        ("PreCompact", "compact"),
        ("PostCompact", "working"),
    ];
    for (event, want) in cases {
        let r = run(&ev(event));
        assert_eq!(r.field("status"), want, "{event} should map to {want}");
    }
}

// ---------------------------------------------------------------------------
// events that must stay silent
// ---------------------------------------------------------------------------

#[test]
fn unknown_and_malformed_events_are_ignored() {
    for json in [
        &ev("SomethingElse"),
        &r#"{"session_id":"x"}"#.to_string(),
        &String::new(),
        &"not json at all".to_string(),
    ] {
        let r = run(json);
        assert!(r.silent(), "expected silence for {json:?}, got {:?}", r.pipes);
    }
}

/// The pane id is what scopes monitoring to zellij; without it there is no pane
/// to report against, so an agent outside zellij must be invisible.
#[test]
fn no_pane_id_means_no_report() {
    let r = Hook::new().env("ZELLIJ_PANE_ID", "").run(&ev("Stop"));
    assert!(r.silent(), "got {:?}", r.pipes);
}

/// Documented as halving hook volume: tool events stop reporting entirely.
#[test]
fn heartbeat_off_silences_tool_events() {
    for event in ["PreToolUse", "PostToolUse"] {
        let r = Hook::new().env("ZJ_AGENT_HEARTBEAT", "0").run(&ev(event));
        assert!(r.silent(), "{event} should be silent, got {:?}", r.pipes);
    }
}

#[test]
fn heartbeat_off_still_reports_turn_boundaries() {
    let r = Hook::new().env("ZJ_AGENT_HEARTBEAT", "0").run(&ev("Stop"));
    assert_eq!(r.field("status"), "done");
}

/// The heartbeat switch has to cover the chatty fan-out events too, or turning
/// it off still leaves several hooks firing per second.
#[test]
fn heartbeat_off_silences_counter_events() {
    for event in [
        "SubagentStart",
        "SubagentStop",
        "TaskCreated",
        "TaskCompleted",
        "PostToolUseFailure",
    ] {
        let r = Hook::new().env("ZJ_AGENT_HEARTBEAT", "0").run(&ev(event));
        assert!(r.silent(), "{event} should be silent, got {:?}", r.pipes);
    }
}

// ---------------------------------------------------------------------------
// fields passed through
// ---------------------------------------------------------------------------

#[test]
fn identifying_fields_reach_the_args() {
    let r = run(&serde_json::json!({
        "hook_event_name": "Stop",
        "session_id": "sess-1",
        "cwd": "/home/me/api",
    })
    .to_string());
    assert_eq!(r.field("pane_id"), "3");
    assert_eq!(r.field("session_id"), "sess-1");
    assert_eq!(r.field("cwd"), "/home/me/api");
    assert_eq!(r.field("tool"), "claude", "claude is the default tool");
}

#[test]
fn the_tool_can_be_overridden() {
    let r = Hook::new().env("ZJ_AGENT_TOOL", "codex").run(&ev("Stop"));
    assert_eq!(r.field("tool"), "codex");
}

#[test]
fn the_plugin_path_can_be_overridden() {
    let r = Hook::new().env("ZJ_AGENT_PLUGIN", "file:/custom.wasm").run(&ev("Stop"));
    assert_eq!(r.status_pipe().unwrap().plugin, "file:/custom.wasm");
}

// ---------------------------------------------------------------------------
// claude transcript summaries
// ---------------------------------------------------------------------------

/// Writes a JSONL transcript and returns a Stop event pointing at it.
fn stop_with_transcript(h: &Hook, lines: &[serde_json::Value]) -> String {
    let tr = h.path("transcript.jsonl");
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(&tr, body).expect("write transcript");
    serde_json::json!({
        "hook_event_name": "Stop",
        "transcript_path": tr.to_string_lossy(),
    })
    .to_string()
}

#[test]
fn an_ai_title_is_preferred_over_the_last_prompt() {
    let h = Hook::new();
    let json = stop_with_transcript(
        &h,
        &[
            serde_json::json!({"type": "last-prompt", "lastPrompt": "the fallback prompt"}),
            serde_json::json!({"type": "ai-title", "aiTitle": "Add retry to webhook client"}),
        ],
    );
    assert_eq!(h.run(&json).field("task"), "Add retry to webhook client");
}

#[test]
fn the_task_falls_back_to_the_last_prompt() {
    let h = Hook::new();
    let json = stop_with_transcript(
        &h,
        &[serde_json::json!({"type": "last-prompt", "lastPrompt": "the fallback prompt"})],
    );
    assert_eq!(h.run(&json).field("task"), "the fallback prompt");
}

/// The newest title wins: the plugin shows current work, not the session's first.
#[test]
fn the_latest_ai_title_wins() {
    let h = Hook::new();
    let json = stop_with_transcript(
        &h,
        &[
            serde_json::json!({"type": "ai-title", "aiTitle": "older title"}),
            serde_json::json!({"type": "ai-title", "aiTitle": "newest title"}),
        ],
    );
    assert_eq!(h.run(&json).field("task"), "newest title");
}

/// Tool events fire constantly against multi-MB transcripts, so they must not
/// read them; the plugin treats an empty task as "leave unchanged".
#[test]
fn tool_events_send_an_empty_task() {
    let h = Hook::new();
    let tr = h.path("transcript.jsonl");
    fs::write(
        &tr,
        format!(
            "{}\n",
            serde_json::json!({"type": "ai-title", "aiTitle": "should not be read"})
        ),
    )
    .expect("write transcript");
    let json = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "transcript_path": tr.to_string_lossy(),
    })
    .to_string();
    assert_eq!(h.run(&json).field("task"), "");
}

#[test]
fn a_missing_transcript_is_survivable() {
    let json = serde_json::json!({
        "hook_event_name": "Stop",
        "transcript_path": "/no/such/file.jsonl",
    })
    .to_string();
    assert_eq!(run(&json).field("status"), "done");
}

/// A transcript whose tail is unparseable must not take the status report down.
#[test]
fn an_unparseable_transcript_still_reports_status() {
    let h = Hook::new();
    let tr = h.path("transcript.jsonl");
    fs::write(&tr, "garbage {{{ not json\n").expect("write transcript");
    let json = serde_json::json!({
        "hook_event_name": "Stop",
        "transcript_path": tr.to_string_lossy(),
    })
    .to_string();
    assert_eq!(h.run(&json).field("status"), "done");
}

// ---------------------------------------------------------------------------
// codex transcript summaries
// ---------------------------------------------------------------------------

/// Stop now takes its summary from the payload, so the rollout is read on the
/// turn-opening events instead.
#[test]
fn codex_reads_the_session_rollout() {
    let h = Hook::new();
    let dir = h.path("codex/sessions/2026/08/06");
    fs::create_dir_all(&dir).expect("create codex sessions");
    fs::write(
        dir.join("rollout-2026-08-06-sess-9.jsonl"),
        format!(
            "{}\n",
            serde_json::json!({
                "type": "event_msg",
                "payload": {"type": "user_message", "message": "Bump deps"}
            })
        ),
    )
    .expect("write rollout");

    let json = serde_json::json!({"hook_event_name": "UserPromptSubmit", "session_id": "sess-9"}).to_string();
    let r = h
        .env("ZJ_AGENT_TOOL", "codex")
        .env("CODEX_HOME", h.path("codex"))
        .run(&json);
    assert_eq!(r.field("task"), "Bump deps");
}

#[test]
fn codex_without_a_rollout_still_reports() {
    let h = Hook::new();
    fs::create_dir_all(h.path("codex/sessions")).expect("create codex sessions");
    let json = serde_json::json!({"hook_event_name": "Stop", "session_id": "absent"}).to_string();
    let r = h
        .env("ZJ_AGENT_TOOL", "codex")
        .env("CODEX_HOME", h.path("codex"))
        .run(&json);
    assert_eq!(r.field("status"), "done");
}

// ---------------------------------------------------------------------------
// sanitizing (--args is comma-separated, so commas and newlines break it)
// ---------------------------------------------------------------------------

/// Every comma in `--args` must separate two key=value pairs. A comma surviving
/// inside a value leaves a fragment with no `=`, which the plugin reads as a new
/// key. Asserting the invariant rather than a pair count keeps this honest as
/// fields are added.
#[test]
fn commas_in_the_task_are_stripped() {
    let h = Hook::new();
    let json = stop_with_transcript(
        &h,
        &[serde_json::json!({"type": "ai-title", "aiTitle": "fix a, b and c"})],
    );
    let r = h.run(&json);
    // parse_args returns Err on the first segment lacking `=`.
    let raw = r.status_pipe().expect("a pipe").args.clone();
    assert!(raw.contains_key("task"), "task survived as a field: {raw:?}");
    assert!(!r.field("task").contains(','), "comma survived: {}", r.field("task"));
}

#[test]
fn newlines_in_the_task_are_stripped() {
    let h = Hook::new();
    let json = stop_with_transcript(
        &h,
        &[serde_json::json!({"type": "ai-title", "aiTitle": "line one\nline two"})],
    );
    let r = h.run(&json);
    assert_eq!(r.pipes.len(), 1, "a newline split the args into two calls");
    assert!(!r.field("task").contains('\n'));
}

/// 60 chars is the documented cap; the panel truncates for display anyway.
#[test]
fn long_tasks_are_capped_at_60_chars() {
    let h = Hook::new();
    let long = "x".repeat(200);
    let json = stop_with_transcript(&h, &[serde_json::json!({"type": "ai-title", "aiTitle": long})]);
    let r = h.run(&json);
    assert!(r.field("task").len() <= 60, "got {} chars", r.field("task").len());
}

/// A quote or `$(...)` in a title must not escape into the shell: the hook evals
/// jq's @sh output, so this is the injection path that matters.
#[test]
fn shell_metacharacters_in_a_task_are_not_executed() {
    let h = Hook::new();
    let canary = h.path("pwned-task");
    let title = format!("it's $(touch {}) `id`", canary.display());
    let json = stop_with_transcript(&h, &[serde_json::json!({"type": "ai-title", "aiTitle": title})]);
    let r = h.run(&json);
    assert!(!canary.exists(), "command substitution ran");
    assert_eq!(r.field("status"), "done", "a quoted task still reports");
}

/// cwd is interpolated the same way and is attacker-influenced via directory names.
#[test]
fn shell_metacharacters_in_cwd_are_not_executed() {
    let h = Hook::new();
    let canary = h.path("pwned-cwd");
    let json = serde_json::json!({
        "hook_event_name": "Stop",
        "cwd": format!("/tmp/a b$(touch {})", canary.display()),
    })
    .to_string();
    h.run(&json);
    assert!(!canary.exists(), "command substitution ran");
}

/// Shell metacharacters in the notification message must not reach the shell.
#[test]
fn injection_through_the_notification_message_is_neutralised() {
    let h = Hook::new();
    let canary = h.path("pwned-notif");
    let json = serde_json::json!({
        "hook_event_name": "Notification",
        "notification_type": "permission_prompt",
        "message": format!("$(touch {})", canary.display()),
    })
    .to_string();
    let r = h.run(&json);
    assert_eq!(r.field("status"), "waiting");
    assert!(!canary.exists(), "command executed");
}

// ---------------------------------------------------------------------------
// the hook must never break an agent turn
// ---------------------------------------------------------------------------

/// Claude aborts nothing on a non-zero hook, but a hung or failing hook is still
/// a bad neighbour: the contract in the header is "always exit 0".
#[test]
fn the_hook_exits_zero_for_every_event() {
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "Notification",
        "Stop",
        "SessionEnd",
        "Bogus",
    ] {
        assert_eq!(run(&ev(event)).code, 0, "{event} exited non-zero");
    }
}

/// Even with zellij missing entirely, the hook must succeed silently.
#[test]
fn the_hook_exits_zero_when_zellij_is_absent() {
    let sandbox = Sandbox::new();
    // No stub written, and PATH deliberately excludes the sandbox bin.
    let mut cmd = Command::new("sh");
    cmd.arg(hook_path())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("ZELLIJ_PANE_ID", "3")
        .env("HOME", sandbox.path("home"))
        .env("ZJ_AGENT_SPOOL_DIR", sandbox.path("spool"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().expect("spawn");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(ev("Stop").as_bytes())
        .expect("write");
    assert_eq!(child.wait().expect("wait").code(), Some(0));
}

// ---------------------------------------------------------------------------
// debug logging
// ---------------------------------------------------------------------------

#[test]
fn debug_on_writes_a_hook_log() {
    let h = Hook::new();
    let home = h.path("home");
    fs::create_dir_all(&home).expect("create home");
    h.env("ZJ_AGENT_DEBUG", "1").run(&ev("Stop"));
    let log = home.join(".cache/zj-agent-mob/hook.log");
    assert!(log.exists(), "no log at {}", log.display());
    assert!(!fs::read_to_string(&log).unwrap().is_empty(), "log is empty");
}

#[test]
fn debug_off_writes_nothing() {
    let h = Hook::new();
    let home = h.path("home");
    fs::create_dir_all(&home).expect("create home");
    h.run(&ev("Stop"));
    assert!(
        !home.join(".cache/zj-agent-mob/hook.log").exists(),
        "log created without ZJ_AGENT_DEBUG=1"
    );
}

// ---------------------------------------------------------------------------
// richer payload fields
// ---------------------------------------------------------------------------

/// F1: the tool argument, not just the tool name.
#[test]
fn a_tool_argument_reaches_the_detail() {
    let cases = [
        (
            serde_json::json!({"file_path": "src/webhook.rs"}),
            "Edit",
            "Edit src/webhook.rs",
        ),
        (serde_json::json!({"command": "cargo test"}), "Bash", "Bash cargo test"),
        (serde_json::json!({}), "Glob", "Glob"),
    ];
    for (input, name, want) in cases {
        let json = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": name,
            "tool_input": input,
        })
        .to_string();
        assert_eq!(run(&json).field("detail"), want);
    }
}

/// A tool event with no `tool_input` key at all - a different jq path from an
/// empty object - still names the tool.
#[test]
fn a_tool_name_alone_becomes_the_detail() {
    let json = serde_json::json!({"hook_event_name": "PreToolUse", "tool_name": "Edit"}).to_string();
    assert_eq!(run(&json).field("detail"), "Edit");
}

/// PostToolUseFailure marks the row so a failed call is not read as progress.
#[test]
fn a_failed_tool_call_is_marked_in_the_detail() {
    let json = serde_json::json!({
        "hook_event_name": "PostToolUseFailure",
        "tool_name": "Bash",
        "tool_input": {"command": "cargo test"},
    })
    .to_string();
    assert_eq!(run(&json).field("detail"), "Bash cargo test (failed)");
}

/// F3: Stop carries the closing message, so no transcript read is needed.
#[test]
fn stop_takes_its_task_from_the_closing_message() {
    let json = serde_json::json!({
        "hook_event_name": "Stop",
        "last_assistant_message": "Found 3 issues in the render path",
    })
    .to_string();
    assert_eq!(run(&json).field("task"), "Found 3 issues in the render path");
}

#[test]
fn only_the_first_line_of_the_closing_message_is_used() {
    let json = serde_json::json!({
        "hook_event_name": "Stop",
        "last_assistant_message": "first line\nsecond line",
    })
    .to_string();
    assert_eq!(run(&json).field("task"), "first line");
}

/// F2: a real prompt is `waiting`; an idle nudge is not the same thing.
#[test]
fn a_permission_prompt_is_waiting_and_carries_its_text() {
    let json = serde_json::json!({
        "hook_event_name": "Notification",
        "notification_type": "permission_prompt",
        "message": "Bash wants to run rm -rf",
    })
    .to_string();
    let r = run(&json);
    assert_eq!(r.field("status"), "waiting");
    assert_eq!(r.field("detail"), "Bash wants to run rm -rf");
}

#[test]
fn an_idle_prompt_maps_to_idlewait() {
    let json = serde_json::json!({
        "hook_event_name": "Notification",
        "notification_type": "idle_prompt",
        "message": "waiting for input",
    })
    .to_string();
    assert_eq!(run(&json).field("status"), "idlewait");
}

/// F4: a stopped agent must not keep reporting `working`.
#[test]
fn a_failure_reports_its_reason() {
    let json = serde_json::json!({
        "hook_event_name": "StopFailure",
        "error_type": "rate_limit",
        "error_message": "rate limited, retry in 30s",
    })
    .to_string();
    let r = run(&json);
    assert_eq!(r.field("status"), "failed");
    assert_eq!(r.field("detail"), "rate limited retry in 30s");
}

#[test]
fn a_failure_with_no_message_falls_back_to_the_type() {
    let json = serde_json::json!({"hook_event_name": "StopFailure", "error_type": "overloaded"}).to_string();
    assert_eq!(run(&json).field("detail"), "overloaded");
}

/// F6
#[test]
fn compaction_names_its_trigger() {
    let json = serde_json::json!({"hook_event_name": "PreCompact", "trigger": "manual"}).to_string();
    assert_eq!(run(&json).field("detail"), "compacting context (manual)");
}

/// F7: `default` is the common case and must stay off the row.
#[test]
fn a_risky_permission_mode_is_forwarded() {
    let json = serde_json::json!({
        "hook_event_name": "UserPromptSubmit",
        "permission_mode": "bypassPermissions",
    })
    .to_string();
    assert_eq!(run(&json).field("perm_mode"), "bypassPermissions");
}

#[test]
fn the_default_permission_mode_is_suppressed() {
    let json = serde_json::json!({
        "hook_event_name": "UserPromptSubmit",
        "permission_mode": "default",
    })
    .to_string();
    assert_eq!(run(&json).field("perm_mode"), "");
}

/// F5 / F8: counter events carry a delta and no status, so they never overwrite
/// the parent pane's own state.
#[test]
fn subagent_start_sends_a_delta_and_no_status() {
    let json = serde_json::json!({"hook_event_name": "SubagentStart", "agent_type": "Explore"}).to_string();
    let r = run(&json);
    assert_eq!(r.field("subagent_delta"), "1");
    assert_eq!(r.field("agent_type"), "Explore");
    assert_eq!(r.field("status"), "", "a counter event must carry no status");
}

#[test]
fn the_counter_events_send_their_deltas() {
    let cases = [
        ("SubagentStop", "subagent_delta", "-1"),
        ("TaskCreated", "task_delta", "1"),
        ("TaskCompleted", "task_done_delta", "1"),
    ];
    for (event, key, want) in cases {
        assert_eq!(run(&ev(event)).field(key), want, "{event}");
    }
}

// ---------------------------------------------------------------------------
// answering permission prompts from the panel (opt-in)
// ---------------------------------------------------------------------------

fn permission_request() -> String {
    serde_json::json!({
        "hook_event_name": "PermissionRequest",
        "tool_name": "Bash",
        "tool_input": {"command": "rm -rf node_modules"},
    })
    .to_string()
}

/// The default must stay non-blocking: an unconfigured install can never have a
/// hook that waits on a panel the user may not even have open.
#[test]
fn approval_is_off_by_default() {
    let r = run(&permission_request());
    assert_eq!(r.field("status"), "waiting");
    assert!(r.ask_pipe().is_none(), "sent agent-ask without ZJ_AGENT_APPROVE=1");
}

/// With the flag on, the hook parks a prompt and waits for a verdict.
#[test]
fn approval_mode_sends_an_ask() {
    let h = Hook::new();
    let r = h
        .env("ZJ_AGENT_APPROVE", "1")
        .env("ZJ_AGENT_APPROVE_TIMEOUT", "1")
        .run(&permission_request());
    let ask = r.ask_pipe().expect("an agent-ask pipe");
    assert!(ask.args.contains_key("verdict_file"), "{:?}", ask.args);
    assert_eq!(
        ask.args.get("tool_arg").map(String::as_str),
        Some("rm -rf node_modules")
    );
}

/// Timing out must fall through to the agent's own prompt, not emit a decision.
#[test]
fn a_timeout_emits_no_decision() {
    let h = Hook::new();
    let r = h
        .env("ZJ_AGENT_APPROVE", "1")
        .env("ZJ_AGENT_APPROVE_TIMEOUT", "1")
        .run(&permission_request());
    assert!(r.stdout.trim().is_empty(), "got a decision: {}", r.stdout);
}

/// A verdict dropped by the panel becomes the documented decision JSON. The hook
/// clears the file first, so it is planted by a thread racing the poll loop.
fn approve_with(verdict: &str) -> String {
    let h = Hook::new();
    let tmp = h.path("tmp");
    fs::create_dir_all(&tmp).expect("create tmp");
    // The hook derives the verdict path from TMPDIR, the session and the pane.
    let vfile = tmp.join("zj-agent-mob").join("verdict.mob.3");

    let planter = {
        let vfile = vfile.clone();
        let verdict = verdict.to_string();
        std::thread::spawn(move || {
            // The hook creates the verdict directory, clears any stale file,
            // then polls once a second. Planting on a fixed sleep races that
            // clear - if it lands first the hook deletes the verdict and waits
            // out the full timeout. Wait for the directory to appear (the hook
            // has started) before writing.
            let dir = vfile.parent().expect("verdict dir");
            for _ in 0..100 {
                if dir.is_dir() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            // The clear happens immediately after the mkdir, so give it a beat
            // to land before planting the verdict it must not delete.
            std::thread::sleep(std::time::Duration::from_millis(300));
            let _ = fs::create_dir_all(dir);
            let _ = fs::write(&vfile, &verdict);
        })
    };

    let r = h
        .env("TMPDIR", &tmp)
        .env("ZELLIJ_SESSION_NAME", "mob")
        .env("ZJ_AGENT_APPROVE", "1")
        .env("ZJ_AGENT_APPROVE_TIMEOUT", "8")
        .run(&permission_request());
    planter.join().expect("planter");
    r.stdout
}

#[test]
fn an_allow_verdict_becomes_an_allow_decision() {
    assert!(approve_with("allow").contains(r#""behavior":"allow""#));
}

#[test]
fn a_deny_verdict_becomes_a_deny_decision() {
    assert!(approve_with("deny").contains(r#""behavior":"deny""#));
}

/// A blocking hook that can exit non-zero would fail the tool call outright.
#[test]
fn the_approval_path_still_exits_zero() {
    let h = Hook::new();
    let r = h
        .env("ZJ_AGENT_APPROVE", "1")
        .env("ZJ_AGENT_APPROVE_TIMEOUT", "1")
        .run(&permission_request());
    assert_eq!(r.code, 0, "non-zero exit would fail the tool call");
}

// ---------------------------------------------------------------------------
// cross-session identity
// ---------------------------------------------------------------------------

/// Pane ids repeat across sessions, so every message says which one it is from.
#[test]
fn the_report_names_its_session() {
    let r = Hook::new().env("ZELLIJ_SESSION_NAME", "mob").run(&ev("Stop"));
    assert_eq!(r.field("session"), "mob");
}

/// The name reaches a file path and a comma-separated arg string, so anything
/// that would split either is folded first. src/agent.rs mirrors this exactly.
#[test]
fn a_session_name_is_folded_to_safe_characters() {
    let cases = [("my session", "my_session"), ("a,b=c", "a_b_c"), ("../evil", ".._evil")];
    for (given, want) in cases {
        let r = Hook::new().env("ZELLIJ_SESSION_NAME", given).run(&ev("Stop"));
        assert_eq!(r.field("session"), want, "input {given:?}");
        assert!(!r.field("session").contains('/'), "a slash could traverse paths");
    }
}

/// The correctness fix: two sessions sharing a pane number must not share a
/// verdict file, or approving one answers the other's prompt.
#[test]
fn same_pane_in_two_sessions_gets_two_verdict_files() {
    let verdict_file = |session: &str| -> String {
        let h = Hook::new();
        let r = h
            .env("ZJ_AGENT_APPROVE", "1")
            .env("ZJ_AGENT_APPROVE_TIMEOUT", "1")
            .env("ZELLIJ_SESSION_NAME", session)
            .run(&permission_request());
        r.ask_pipe()
            .expect("an ask pipe")
            .args
            .get("verdict_file")
            .cloned()
            .unwrap_or_default()
    };
    let a = verdict_file("mob");
    let b = verdict_file("other");
    assert!(a.ends_with("verdict.mob.3"), "got {a}");
    assert_ne!(a, b, "both sessions got the same verdict file");
}

// ---------------------------------------------------------------------------
// The cross-session status spool.
//
// The pipe above only reaches the agent's own session, so a panel elsewhere
// reads status from these files instead. The write must be a plain filesystem
// side effect: no subprocess, and never able to block a turn.
// ---------------------------------------------------------------------------

/// Reads a spool record and splits it into fields.
fn record(path: &Path) -> BTreeMap<String, String> {
    let body = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let first = body.lines().next().unwrap_or_default();
    parse_args(first).unwrap_or_else(|frag| panic!("record fragment without a key: {frag:?} in {first}"))
}

#[test]
fn a_status_event_writes_a_spool_record() {
    let h = Hook::new();
    let json = serde_json::json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": "uuid-1",
        "cwd": "/Users/x/Projects/web",
    })
    .to_string();
    h.env("ZELLIJ_SESSION_NAME", "mob").run(&json);

    let file = h.path("spool/mob.3");
    assert!(file.exists(), "no record at {}", file.display());

    let rec = record(&file);
    assert!(rec.contains_key("ts"), "no timestamp: {rec:?}");
    assert_eq!(rec["pane_id"], "3");
    assert_eq!(rec["session"], "mob");
    assert_eq!(rec["status"], "working");
    // Pane ids are recycled, so this is what stops a stale record colouring a
    // new agent on the same pane.
    assert_eq!(rec["session_id"], "uuid-1");

    // The plugin keys identity off the filename, so a record must agree with it.
    assert_eq!(
        format!("{}.{}", rec["session"], rec["pane_id"]),
        "mob.3",
        "record disagrees with its filename"
    );

    // One line only: the plugin reads the first and treats extra as malformed.
    let body = fs::read_to_string(&file).unwrap();
    assert_eq!(body.lines().count(), 1, "record is not exactly one line: {body:?}");
}

/// On a shared /tmp another user must not be able to read prompt text.
///
/// The shell version had to probe GNU vs BSD `stat` here, because GNU `stat -f`
/// means "filesystem info" and exits 0, so a `-c || -f` fallback silently takes
/// the wrong branch. The mode is just a number through std.
#[test]
fn the_spool_directory_is_private() {
    let h = Hook::new();
    h.env("ZELLIJ_SESSION_NAME", "mob").run(&ev("UserPromptSubmit"));
    let mode = fs::metadata(h.path("spool")).expect("spool dir").permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "spool mode was {mode:o}");
}

/// The rename into place is what publishes a record, so a reader never sees a
/// partial one and no debris is left behind.
#[test]
fn no_partial_tmp_file_is_left_behind() {
    let h = Hook::new();
    h.env("ZELLIJ_SESSION_NAME", "mob").run(&ev("UserPromptSubmit"));
    let leftovers: Vec<_> = fs::read_dir(h.path("spool"))
        .expect("spool dir")
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// A newer event replaces the record rather than appending: the spool is a
/// snapshot per agent, so it cannot grow without bound.
#[test]
fn a_later_event_overwrites_the_record() {
    let h = Hook::new();
    h.env("ZELLIJ_SESSION_NAME", "mob").run(&ev("UserPromptSubmit"));
    let json = serde_json::json!({"hook_event_name": "Stop", "last_assistant_message": "all done"}).to_string();
    h.env("ZELLIJ_SESSION_NAME", "mob").run(&json);

    let file = h.path("spool/mob.3");
    assert_eq!(record(&file)["status"], "done");
    assert_eq!(
        fs::read_to_string(&file).unwrap().lines().count(),
        1,
        "overwriting appended instead"
    );
}

/// SessionEnd retires the agent, so the record must not outlive it and colour a
/// recycled pane id later.
#[test]
fn session_end_removes_the_record() {
    let h = Hook::new();
    h.env("ZELLIJ_SESSION_NAME", "mob").run(&ev("UserPromptSubmit"));
    assert!(h.path("spool/mob.3").exists(), "no record to remove");

    let json = serde_json::json!({"hook_event_name": "SessionEnd", "reason": "logout"}).to_string();
    h.env("ZELLIJ_SESSION_NAME", "mob").run(&json);
    assert!(!h.path("spool/mob.3").exists(), "record outlived the session");
}

/// Two sessions on the same pane number are different agents and need different
/// files, exactly like the verdict path.
#[test]
fn same_pane_in_two_sessions_gets_two_records() {
    let h = Hook::new();
    h.env("ZELLIJ_SESSION_NAME", "mob").run(&ev("UserPromptSubmit"));
    h.env("ZELLIJ_SESSION_NAME", "other").run(&ev("UserPromptSubmit"));
    assert!(h.path("spool/mob.3").exists(), "no mob record");
    assert!(h.path("spool/other.3").exists(), "no other record");
}

/// A session name that could split the args or escape the directory is folded
/// before it reaches a path.
#[test]
fn a_session_name_cannot_escape_the_spool_directory() {
    let h = Hook::new();
    h.env("ZELLIJ_SESSION_NAME", "../evil").run(&ev("UserPromptSubmit"));
    assert!(
        h.path("spool/.._evil.3").exists(),
        "folded name did not land in the spool"
    );
}

/// Opt-out must be complete: no directory, no file, no error.
#[test]
fn spool_off_writes_nothing_but_still_pipes() {
    let h = Hook::new();
    let r = h
        .env("ZJ_AGENT_SPOOL", "0")
        .env("ZELLIJ_SESSION_NAME", "mob")
        .run(&ev("UserPromptSubmit"));
    assert!(!h.path("spool").exists(), "spool created despite ZJ_AGENT_SPOOL=0");
    assert_eq!(r.field("status"), "working", "opting out must still pipe");
}

/// An unwritable spool must degrade to the pipe, never fail the turn: the hook's
/// contract is that the worst case is the normal experience.
#[test]
fn an_unwritable_spool_still_pipes() {
    let h = Hook::new();
    let spool = h.path("spool");
    fs::create_dir_all(&spool).expect("create spool");
    fs::set_permissions(&spool, fs::Permissions::from_mode(0o500)).expect("chmod");

    let r = h
        .env("ZJ_AGENT_SPOOL_DIR", spool.join("nested"))
        .env("ZELLIJ_SESSION_NAME", "mob")
        .run(&ev("UserPromptSubmit"));

    fs::set_permissions(&spool, fs::Permissions::from_mode(0o700)).expect("restore");
    assert_eq!(r.field("status"), "working");
}

/// Every agent event writes, not just turn boundaries, or a panel elsewhere
/// would only ever see the start and end of a turn.
#[test]
fn a_tool_event_spools_its_detail() {
    let h = Hook::new();
    let json = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "cargo test"},
    })
    .to_string();
    h.env("ZELLIJ_SESSION_NAME", "mob").run(&json);
    assert_eq!(record(&h.path("spool/mob.3"))["detail"], "Bash cargo test");
}

/// The heartbeat opt-out must suppress the spool write too, or the escape hatch
/// only halves the work it promises to remove.
#[test]
fn heartbeat_off_also_skips_the_spool() {
    let h = Hook::new();
    let json = serde_json::json!({"hook_event_name": "PreToolUse", "tool_name": "Bash"}).to_string();
    h.env("ZJ_AGENT_HEARTBEAT", "0")
        .env("ZELLIJ_SESSION_NAME", "mob")
        .run(&json);
    assert!(!h.path("spool/mob.3").exists(), "record written anyway");
}

/// A record is a whole snapshot, so an event that carries no session_id must
/// inherit the last known one rather than blanking it. Without this a recycled
/// pane id could inherit a dead agent's status, which the plugin cannot detect.
#[test]
fn an_event_without_identity_inherits_the_last_known() {
    let h = Hook::new();
    let first = serde_json::json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": "uuid-keep",
        "cwd": "/Users/x/Projects/web",
    })
    .to_string();
    h.env("ZELLIJ_SESSION_NAME", "mob").run(&first);

    let second = serde_json::json!({
        "hook_event_name": "Notification",
        "type": "permission_prompt",
        "message": "needs permission",
    })
    .to_string();
    h.env("ZELLIJ_SESSION_NAME", "mob").run(&second);

    let rec = record(&h.path("spool/mob.3"));
    assert_eq!(rec["session_id"], "uuid-keep");
    assert_eq!(rec["cwd"], "/Users/x/Projects/web");
    assert_eq!(rec["status"], "waiting", "the inheriting event keeps its own status");
}

/// Deltas describe one moment and are replayed on every poll, so a snapshot must
/// not carry them or the counts would inflate without bound.
#[test]
fn a_snapshot_carries_no_deltas() {
    let h = Hook::new();
    let sub = serde_json::json!({"hook_event_name": "SubagentStart", "agent_type": "Explore"}).to_string();
    h.env("ZELLIJ_SESSION_NAME", "mob").run(&sub);
    let up = serde_json::json!({"hook_event_name": "UserPromptSubmit", "session_id": "u"}).to_string();
    h.env("ZELLIJ_SESSION_NAME", "mob").run(&up);

    let rec = record(&h.path("spool/mob.3"));
    for key in ["subagent_delta", "task_delta", "task_done_delta"] {
        assert!(!rec.contains_key(key), "snapshot carries {key}: {rec:?}");
    }
}

/// The pane id reaches a file path and the args string. Zellij only ever sets a
/// bare integer, so anything else is a broken or planted environment.
#[test]
fn a_hostile_pane_id_reports_nothing() {
    for pane in ["1;rm -rf /", "../x"] {
        let h = Hook::new();
        let r = h
            .env("ZELLIJ_PANE_ID", pane)
            .env("ZELLIJ_SESSION_NAME", "mob")
            .run(&ev("UserPromptSubmit"));
        assert!(r.silent(), "pane {pane:?} reported: {:?}", r.pipes);
        assert!(!h.path("spool").exists(), "pane {pane:?} wrote a record");
    }
}

/// An agent outside zellij has no pane to attribute a record to.
#[test]
fn no_pane_id_writes_no_record() {
    let h = Hook::new();
    h.env("ZELLIJ_PANE_ID", "")
        .env("ZELLIJ_SESSION_NAME", "mob")
        .run(&ev("UserPromptSubmit"));
    assert!(!h.path("spool").exists(), "a record was written without a pane id");
}
