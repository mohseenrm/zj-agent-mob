//! Frame generator for `demo/tour.gif`.
//!
//! The tour is *staged*, not recorded: each frame is a fleet built in memory
//! and rendered through the panel's own row builders - `list_item`,
//! `detail_item`, `ask_rows`, `subagent_rows`, the real `ribbon` hint sets. No
//! Zellij session, no terminal recorder, no VHS tape. What lands in the GIF is
//! what the panel would draw for that fleet, because it is the same code that
//! draws it.
//!
//! Run it, then rasterise the frames:
//!
//! ```sh
//! cargo test --features tour tour::emit -- --nocapture
//! python3 scripts/demo/render-frames.py /tmp/zj-tour-frames demo/tour.gif
//! ```
//!
//! Staging rather than driving a live session is what makes the tour cover
//! states a live capture cannot reach on demand: a rate-limit failure, an agent
//! two sessions away going stale, a subagent fan-out mid-flight.

use std::fs;
use std::path::Path;

use crate::agent::{Agent, AgentId, RowCtx, Subagent};
use crate::plugin::{ask_rows, find_prompt_row, followup_row};
use crate::ribbon::{self, Hint};
use crate::state::Ask;
use crate::status::Status;
use crate::util::testing::item_text;

const COLS: usize = 96;
const HOME: &str = "mob";
const ICONS: &crate::icons::Icons = &crate::icons::NERD;

/// A fleet member, in the shorthand the scenes are written in.
struct Spec {
    tool: &'static str,
    status: Status,
    since: f64,
    repo: &'static str,
    wt: &'static str,
    branch: &'static str,
    session: &'static str,
    pane: u32,
    task: &'static str,
    detail: &'static str,
    turns: u32,
    model: &'static str,
    subs: Vec<(&'static str, f64, Option<f64>)>,
    block: Option<crate::agent::Block>,
    stale: bool,
    alive_session: bool,
    notified: bool,
    perm_mode: &'static str,
}

impl Default for Spec {
    fn default() -> Self {
        Spec {
            tool: "claude",
            status: Status::Working,
            since: 0.0,
            repo: "",
            wt: "",
            branch: "",
            session: HOME,
            pane: 1,
            task: "",
            detail: "",
            turns: 0,
            model: "",
            subs: Vec::new(),
            block: None,
            stale: false,
            alive_session: true,
            notified: false,
            perm_mode: "",
        }
    }
}

impl Spec {
    fn build(&self, now: f64) -> Agent {
        Agent {
            id: AgentId {
                session: self.session.into(),
                pane_id: self.pane,
            },
            tool: self.tool.into(),
            session_id: format!("s{}", self.pane),
            status: self.status,
            cwd: format!("/Users/x/src/{}", self.repo),
            task: (!self.task.is_empty()).then(|| self.task.to_string()),
            model: self.model.into(),
            followup_queued: false,
            detail: (!self.detail.is_empty()).then(|| self.detail.to_string()),
            turns: self.turns,
            status_since: now - self.since,
            last_report: now,
            spool_ts: 0.0,
            tab: None,
            pane_title: self.tool.into(),
            alive: true,
            perm_mode: self.perm_mode.into(),
            repo: self.repo.into(),
            wt: self.wt.into(),
            branch: self.branch.into(),
            subagents: self
                .subs
                .iter()
                .enumerate()
                .map(|(i, (kind, started, done))| Subagent {
                    id: format!("sub{}", i),
                    kind: (*kind).to_string(),
                    started: now - started,
                    done: done.map(|d| now - d),
                })
                .collect(),
            tasks_total: 0,
            tasks_done: 0,
            session_alive: self.alive_session,
            stale: self.stale,
            notified: self.notified,
            block: self.block,
        }
    }
}

/// One rendered frame: the lines, and how many frame-times it holds.
struct Frame {
    lines: Vec<String>,
    hold: usize,
}

/// Everything a scene needs that is not the fleet itself.
#[derive(Default, Clone)]
struct Scene {
    selected: usize,
    /// Row index -> the permission prompt parked under it.
    ask: Option<(usize, &'static str, &'static str)>,
    followup: Option<(usize, &'static str)>,
    find: Option<&'static str>,
    /// Row whose subagents are expanded by `o`.
    subs_open: Option<usize>,
    kill_armed: Option<usize>,
    /// Row indices sitting in the pinned block.
    pinned: Vec<usize>,
    /// Draw project headings over the unpinned rows, as `s` does.
    grouped: bool,
    /// Footer override; defaults to the set the panel would pick.
    hints: Option<&'static [Hint]>,
    note: Option<&'static str>,
    frame: usize,
    hold: usize,
}

fn icon_for(a: &Agent, frame: usize) -> &'static str {
    if !a.session_alive {
        return ICONS.gone;
    }
    match a.status {
        Status::Working | Status::Compact if a.stale => ICONS.spinner[0],
        Status::Working => ICONS.spinner[frame % ICONS.spinner.len()],
        Status::Compact => ICONS.compact,
        Status::Waiting => ICONS.waiting,
        Status::IdleWait => ICONS.idle_wait,
        Status::Failed => ICONS.failed,
        Status::Done => ICONS.done,
        Status::Idle => ICONS.idle,
        Status::Discovered => ICONS.discovered,
    }
}

/// The header, built the way `build_head` builds it: counts in rank order, a
/// zero failure count omitted, and the version chip right-aligned.
fn header(agents: &[Agent], version: &str) -> String {
    let mut failed = 0;
    let mut waiting = 0;
    let mut working = 0;
    let mut done = 0;
    let mut found = 0;
    for a in agents.iter().filter(|a| a.session_alive) {
        match a.status {
            Status::Failed => failed += 1,
            Status::Waiting | Status::IdleWait => waiting += 1,
            Status::Working | Status::Compact => working += 1,
            Status::Done => done += 1,
            Status::Discovered => found += 1,
            Status::Idle => {}
        }
    }
    let mut parts = Vec::new();
    if failed > 0 {
        parts.push(format!("{} failed", failed));
    }
    parts.push(format!("{} waiting", waiting));
    parts.push(format!("{} working", working));
    parts.push(format!("{} done", done));
    if found > 0 {
        parts.push(format!("{} found", found));
    }
    let head = format!("zj-agent-mob   {}", parts.join(" \u{b7} "));
    let tag = format!("v{}", version);
    let pad = COLS.saturating_sub(head.chars().count() + tag.chars().count());
    format!("{}{}{}", head, " ".repeat(pad), tag)
}

fn rule() -> String {
    "\u{2500}".repeat(COLS)
}

fn hint_line(hints: &[Hint]) -> String {
    ribbon::plain_line(hints)
}

/// Renders one scene into a frame, through the panel's real row builders.
fn render(specs: &[Spec], sc: &Scene, now: f64, version: &str) -> Frame {
    render_view(specs, specs, sc, now, version)
}

/// `all` is the whole fleet, which the header counts; `specs` is what the list
/// shows. They differ while `/` is narrowing: the panel filters the rows and
/// leaves the counts alone.
fn render_view(all: &[Spec], specs: &[Spec], sc: &Scene, now: f64, version: &str) -> Frame {
    let counted: Vec<Agent> = all.iter().map(|s| s.build(now)).collect();
    let agents: Vec<Agent> = specs.iter().map(|s| s.build(now)).collect();

    let id_width = agents
        .iter()
        .map(|a| {
            let foreign = a.session() != HOME;
            match foreign && a.repo.is_empty() {
                true => crate::style::chars(a.session()),
                false => crate::style::chars(&a.identity()),
            }
        })
        .max()
        .unwrap_or(10)
        .clamp(10, 24);

    let mut lines = vec![header(&counted, version), rule()];

    let group_key = |a: &Agent| match a.repo.is_empty() {
        false => a.repo.clone(),
        true => "scratch".to_string(),
    };
    let mut prev_group: Option<String> = None;
    for (i, a) in agents.iter().enumerate() {
        if i == 0 && !sc.pinned.is_empty() {
            lines.push(format!("  pinned ({})", sc.pinned.len()));
        }
        // Headings walk the unpinned rows only, and their counts exclude the
        // pinned members, exactly as `render_list` builds them.
        if sc.grouped && i >= sc.pinned.len() {
            let k = group_key(a);
            if prev_group.as_deref() != Some(k.as_str()) {
                let n = agents
                    .iter()
                    .enumerate()
                    .filter(|(j, b)| *j >= sc.pinned.len() && group_key(b) == k)
                    .count();
                lines.push(format!("  {} ({})", k, n));
                prev_group = Some(k);
            }
        }
        lines.push(item_text(&a.list_item(
            i,
            RowCtx {
                selected: i == sc.selected,
                icon: icon_for(a, sc.frame),
                now,
                cols: COLS,
                show_cwd: true,
                id_width,
                home: HOME,
                pinned: sc.pinned.contains(&i),
                icons: ICONS,
            },
        )));
        let foreign = a.session() != HOME;
        lines.push(item_text(&a.detail_item(
            sc.kill_armed == Some(i),
            now,
            foreign,
            COLS,
            ICONS,
        )));
        if sc.subs_open == Some(i) {
            lines.extend(a.subagent_rows(now, COLS, ICONS).iter().map(item_text));
        }
        if let Some((at, tool, arg)) = sc.ask {
            if at == i {
                let ask = Ask {
                    id: a.id.clone(),
                    verdict_file: String::new(),
                    tool_name: tool.to_string(),
                    tool_arg: arg.to_string(),
                    expires_at: now + 30.0,
                };
                lines.extend(ask_rows(&ask, COLS).iter().map(item_text));
            }
        }
        if let Some((at, text)) = sc.followup {
            if at == i {
                lines.push(item_text(&followup_row(text, COLS)));
            }
        }
    }

    // The pane does not shrink when a filter removes rows, so the frame keeps
    // its height and the footer stays where it was. Without this the GIF's
    // canvas - sized to the tallest frame - leaves a void under a narrowed
    // list, and the footer jumps up the screen mid-tour.
    let body_rows = all.len() * 2 + 4;
    while lines.len() < body_rows {
        lines.push(String::new());
    }

    lines.push(rule());
    if let Some(n) = sc.note {
        lines.push(format!("  {}", n));
    }
    if let Some(q) = sc.find {
        lines.push(item_text(&find_prompt_row(q, COLS)));
    } else {
        let hints = sc.hints.unwrap_or_else(|| {
            if sc.ask.is_some() {
                ribbon::ASK_HINTS
            } else if agents
                .get(sc.selected)
                .is_some_and(|a| matches!(a.status, Status::Waiting | Status::IdleWait))
            {
                ribbon::REPLY_HINTS
            } else {
                ribbon::LIST_HINTS
            }
        });
        lines.push(hint_line(hints));
    }
    Frame {
        lines,
        hold: sc.hold.max(1),
    }
}

/// A filtered view, scored by the panel's own fuzzy matcher so the tour
/// narrows exactly the way `/` narrows: best score first, ties by list order.
fn narrow<'a>(specs: &'a [Spec], q: &str) -> Vec<&'a Spec> {
    let mut scored: Vec<(u32, usize, &Spec)> = specs
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let fields: [(&str, u32); 6] = [
                (s.task, 4),
                (s.wt, 4),
                (s.repo, 2),
                (s.session, 2),
                (s.tool, 1),
                (s.status.label(), 1),
            ];
            fields
                .into_iter()
                .filter_map(|(f, w)| crate::find::score(q, f).map(|sc| sc * w))
                .max()
                .map(|sc| (sc, i, s))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, s)| s).collect()
}

fn write_frames(dir: &Path, frames: Vec<Frame>) {
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).expect("create frame dir");
    for (i, f) in frames.iter().enumerate() {
        let name = format!("{:04}", i);
        fs::write(dir.join(format!("{}.txt", name)), f.lines.join("\n") + "\n").expect("write frame");
        fs::write(dir.join(format!("{}.hold", name)), f.hold.to_string()).expect("write hold");
    }
    println!("wrote {} frames to {}", frames.len(), dir.display());
}

/// The fleet the tour runs on: seven agents across three sessions, two repos
/// and a linked worktree.
fn fleet() -> Vec<Spec> {
    vec![
        Spec {
            tool: "claude",
            status: Status::Waiting,
            since: 12.0,
            repo: "api",
            branch: "fix-auth",
            pane: 6,
            task: "Fix the failing auth test",
            detail: "needs approval: git push --force",
            turns: 7,
            model: "claude-opus-5",
            block: Some(crate::agent::Block::Tool),
            notified: true,
            ..Default::default()
        },
        Spec {
            tool: "claude",
            status: Status::Working,
            since: 41.0,
            repo: "api",
            pane: 5,
            task: "Port the hook suite to Rust",
            detail: "Edit tests/hook_e2e.rs",
            turns: 12,
            subs: vec![("explore", 18.0, None), ("review", 9.0, None)],
            ..Default::default()
        },
        Spec {
            tool: "codex",
            status: Status::Working,
            since: 130.0,
            repo: "web",
            session: "storefront",
            pane: 2,
            task: "Update the checkout flow",
            detail: "Bash pnpm test",
            turns: 5,
            ..Default::default()
        },
        Spec {
            tool: "claude",
            status: Status::Compact,
            since: 8.0,
            repo: "api",
            wt: "api-perf",
            branch: "perf/cache",
            pane: 9,
            task: "Cache the discovery scan",
            detail: "compacting context",
            turns: 22,
            ..Default::default()
        },
        Spec {
            tool: "codex",
            status: Status::Done,
            since: 95.0,
            repo: "web",
            session: "storefront",
            pane: 4,
            task: "Bump the lockfile",
            detail: "finished",
            turns: 3,
            ..Default::default()
        },
        Spec {
            tool: "claude",
            status: Status::Idle,
            since: 240.0,
            repo: "infra",
            session: "platform",
            pane: 1,
            task: "Terraform plan review",
            detail: "idle",
            turns: 1,
            ..Default::default()
        },
        Spec {
            tool: "claude",
            status: Status::Discovered,
            repo: "",
            session: "scratch",
            pane: 3,
            task: "",
            ..Default::default()
        },
    ]
}

#[test]
fn emit() {
    let dir = std::env::var("ZJ_TOUR_FRAMES").unwrap_or_else(|_| "/tmp/zj-tour-frames".into());
    let dir = Path::new(&dir);
    let version = crate::install::CURRENT_VERSION;
    let now = 1_000.0;
    let mut frames: Vec<Frame> = Vec::new();
    let f = fleet();

    // 1. The list. Seven agents, three sessions, sorted by urgency.
    for frame in 0..6 {
        frames.push(render(
            &f,
            &Scene {
                selected: 0,
                frame,
                hold: if frame == 0 { 14 } else { 2 },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    // 2. A permission prompt, answered from the panel.
    for frame in 0..4 {
        frames.push(render(
            &f,
            &Scene {
                selected: 0,
                ask: Some((0, "Bash", "git push --force origin fix-auth")),
                frame,
                hold: if frame == 0 { 16 } else { 3 },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    // 3. Approved. The row moves on without you leaving the panel.
    let mut approved = fleet();
    approved[0].status = Status::Working;
    approved[0].detail = "Bash git push --force";
    approved[0].block = None;
    approved[0].notified = false;
    approved[0].since = 1.0;
    for frame in 0..6 {
        frames.push(render(
            &approved,
            &Scene {
                selected: 0,
                frame,
                hold: if frame == 0 { 12 } else { 2 },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    // 4. Fuzzy find. Typing narrows the list; the header keeps counting the
    // whole fleet, the way the panel does.
    for (i, q) in ["", "c", "ch", "che", "chec", "check", "checko", "checkout", "checkout"]
        .iter()
        .enumerate()
    {
        let shown: Vec<Spec> = narrow(&approved, q).into_iter().map(|s| s.clone_spec()).collect();
        frames.push(render_view(
            &approved,
            &shown,
            &Scene {
                selected: 0,
                find: Some(q),
                frame: i,
                hold: match i {
                    0 => 8,
                    8 => 18,
                    _ => 4,
                },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    // 5. Subagent fan-out, expanded with `o`.
    for frame in 0..5 {
        frames.push(render(
            &approved,
            &Scene {
                selected: 1,
                subs_open: Some(1),
                frame,
                hold: if frame == 0 { 16 } else { 3 },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    // 6. Queue a follow-up for a working agent.
    for (i, typed) in ["", "now ", "now run ", "now run the tests"].iter().enumerate() {
        frames.push(render(
            &approved,
            &Scene {
                selected: 1,
                followup: Some((1, typed)),
                hints: Some(ribbon::FOLLOWUP_EDIT_HINTS),
                frame: i,
                hold: if i == 3 { 14 } else { 5 },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    // 7. A failure two sessions away sorts to the top.
    let mut failed = approved.clone_fleet();
    failed[2].status = Status::Failed;
    failed[2].detail = "rate limit reached";
    failed[2].since = 3.0;
    failed[2].notified = true;
    let failed = sort_by_urgency(failed);
    for frame in 0..6 {
        frames.push(render(
            &failed,
            &Scene {
                selected: 0,
                frame,
                hold: if frame == 0 { 16 } else { 2 },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    // 8. Pin the two you are watching; they hold the top while the rest churns.
    let (pinned_fleet, pinned_rows) = pin_to_top(failed.clone_fleet(), &[1, 3]);
    for frame in 0..6 {
        frames.push(render(
            &pinned_fleet,
            &Scene {
                selected: 0,
                pinned: pinned_rows.clone(),
                frame,
                hold: if frame == 0 { 18 } else { 2 },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    // 9. Kill, armed. Two-step so it is never accidental.
    for frame in 0..4 {
        frames.push(render(
            &failed,
            &Scene {
                selected: 0,
                kill_armed: Some(0),
                frame,
                hold: if frame == 0 { 16 } else { 3 },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    // 10. An update is available, and U installs it in place.
    for frame in 0..5 {
        frames.push(render(
            &failed,
            &Scene {
                selected: 0,
                note: Some("update available: v0.14.0 (press U)"),
                frame,
                hold: if frame == 0 { 18 } else { 3 },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    write_frames(dir, frames);
}

/// Urgency order, the way `sort_agents` ranks: status rank, then elapsed.
/// Keeps the pinned block first, then orders the rest by group the way
/// `sort_agents` does under `Grouping::Project`: each group ranked by its most
/// urgent *unpinned* member, and contiguous once ordered.
fn group_unpinned(v: &[Spec], pinned_n: usize) -> Vec<Spec> {
    let key = |s: &Spec| match s.repo.is_empty() {
        false => s.repo.to_string(),
        true => "scratch".to_string(),
    };
    let mut out: Vec<Spec> = v.iter().take(pinned_n).map(|s| s.clone_spec()).collect();
    let rest: Vec<&Spec> = v.iter().skip(pinned_n).collect();
    let mut keys: Vec<String> = Vec::new();
    for s in rest.iter() {
        let k = key(s);
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    keys.sort_by_key(|k| {
        rest.iter()
            .filter(|s| key(s) == *k)
            .map(|s| s.status.rank())
            .min()
            .unwrap_or(u8::MAX)
    });
    for k in keys {
        for s in rest.iter().filter(|s| key(s) == k) {
            out.push(s.clone_spec());
        }
    }
    out
}

/// Row index of the agent in pane `pane`, so a scene names the agent it means
/// rather than a position that shifts whenever the fleet re-sorts.
fn row_of(v: &[Spec], pane: u32) -> usize {
    v.iter().position(|s| s.pane == pane).expect("pane in fleet")
}

/// The pinned block, then everything else, each in urgency order - what
/// `sort_agents` produces once `State::pinned` is non-empty.
fn pin_to_top(v: Vec<Spec>, pinned: &[usize]) -> (Vec<Spec>, Vec<usize>) {
    let (mut top, mut rest): (Vec<Spec>, Vec<Spec>) = (Vec::new(), Vec::new());
    for (i, spec) in v.into_iter().enumerate() {
        match pinned.contains(&i) {
            true => top.push(spec),
            false => rest.push(spec),
        }
    }
    let n = top.len();
    let mut out = sort_by_urgency(top);
    out.extend(sort_by_urgency(rest));
    (out, (0..n).collect())
}

fn sort_by_urgency(mut v: Vec<Spec>) -> Vec<Spec> {
    v.sort_by(|a, b| {
        a.status
            .rank()
            .cmp(&b.status.rank())
            .then(b.since.partial_cmp(&a.since).unwrap())
    });
    v
}

impl Spec {
    fn clone_spec(&self) -> Spec {
        Spec {
            tool: self.tool,
            status: self.status,
            since: self.since,
            repo: self.repo,
            wt: self.wt,
            branch: self.branch,
            session: self.session,
            pane: self.pane,
            task: self.task,
            detail: self.detail,
            turns: self.turns,
            model: self.model,
            subs: self.subs.clone(),
            block: self.block,
            stale: self.stale,
            alive_session: self.alive_session,
            notified: self.notified,
            perm_mode: self.perm_mode,
        }
    }
}

trait CloneFleet {
    fn clone_fleet(&self) -> Vec<Spec>;
}

impl CloneFleet for Vec<Spec> {
    fn clone_fleet(&self) -> Vec<Spec> {
        self.iter().map(|s| s.clone_spec()).collect()
    }
}

/// Frames for the v1.0.0 release-notes GIF: the new icon set, then pinning.
///
/// Separate from `emit` so the full tour keeps its own pacing and length. Same
/// fleet, same row builders, so the two cannot drift apart.
#[test]
fn emit_release() {
    let dir = std::env::var("ZJ_RELEASE_FRAMES").unwrap_or_else(|_| "/tmp/zj-release-frames".into());
    let dir = Path::new(&dir);
    let version = crate::install::CURRENT_VERSION;
    let now = 1_000.0;
    let mut frames: Vec<Frame> = Vec::new();

    // A fleet that puts every new glyph on screen at once: a failure, a blocked
    // agent, a compaction, a fan-out, a finished run, an idle row and a scan hit.
    let base = sort_by_urgency(vec![
        Spec {
            tool: "codex",
            status: Status::Failed,
            since: 3.0,
            repo: "web",
            session: "storefront",
            pane: 2,
            task: "Update the checkout flow",
            detail: "rate limit reached",
            turns: 5,
            notified: true,
            ..Default::default()
        },
        Spec {
            tool: "claude",
            status: Status::Waiting,
            since: 40.0,
            repo: "api",
            pane: 7,
            task: "Fix the failing auth test",
            detail: "needs approval: Bash",
            block: Some(crate::agent::Block::Tool),
            turns: 7,
            model: "claude-opus-5",
            ..Default::default()
        },
        Spec {
            tool: "claude",
            status: Status::Working,
            since: 41.0,
            repo: "api",
            pane: 5,
            task: "Port the hook suite to Rust",
            detail: "Edit tests/hook_e2e.rs",
            turns: 12,
            subs: vec![("explore", 78.0, None), ("review", 12.0, None)],
            ..Default::default()
        },
        Spec {
            tool: "claude",
            status: Status::Compact,
            since: 8.0,
            repo: "api",
            wt: "api-perf",
            branch: "perf/cache",
            pane: 9,
            task: "Cache the discovery scan",
            detail: "compacting context",
            turns: 22,
            ..Default::default()
        },
        Spec {
            tool: "codex",
            status: Status::Done,
            since: 95.0,
            repo: "web",
            session: "storefront",
            pane: 4,
            task: "Bump the lockfile",
            detail: "finished",
            turns: 3,
            ..Default::default()
        },
        Spec {
            tool: "claude",
            status: Status::Idle,
            since: 240.0,
            repo: "infra",
            session: "platform",
            pane: 1,
            task: "Terraform plan review",
            detail: "idle",
            turns: 1,
            ..Default::default()
        },
        Spec {
            tool: "claude",
            status: Status::Discovered,
            repo: "",
            session: "scratch",
            pane: 3,
            task: "",
            ..Default::default()
        },
    ]);

    // 1. The fleet, so every status glyph is on screen before anything moves.
    for frame in 0..6 {
        frames.push(render(
            &base,
            &Scene {
                selected: 0,
                frame,
                hold: if frame == 0 { 20 } else { 3 },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    // 2. Cursor onto the agent you actually care about: the fan-out in `api`.
    let watched = row_of(&base, 5);
    for frame in 0..3 {
        frames.push(render(
            &base,
            &Scene {
                selected: watched,
                frame,
                hold: if frame == 0 { 14 } else { 3 },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    // 3. `p` lifts it into a block of its own, above even the failure.
    let (one, one_rows) = pin_to_top(base.clone_fleet(), &[row_of(&base, 5)]);
    for frame in 0..6 {
        frames.push(render(
            &one,
            &Scene {
                selected: 0,
                pinned: one_rows.clone(),
                frame,
                hold: if frame == 0 { 20 } else { 3 },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    // 4. A second pin, from another session: the block spans sessions.
    let (two, two_rows) = pin_to_top(base.clone_fleet(), &[row_of(&base, 5), row_of(&base, 4)]);
    for frame in 0..6 {
        frames.push(render(
            &two,
            &Scene {
                selected: 0,
                pinned: two_rows.clone(),
                frame,
                hold: if frame == 0 { 20 } else { 3 },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    // 5. The fleet churns underneath: the idle row blocks and the compaction
    // finishes, and the pinned block does not move.
    let mut churned = base.clone_fleet();
    for s in churned.iter_mut() {
        if s.pane == 1 {
            s.status = Status::Waiting;
            s.since = 4.0;
            s.detail = "needs approval: Write";
            s.block = Some(crate::agent::Block::Tool);
            s.notified = true;
        }
        if s.pane == 9 {
            s.status = Status::Working;
            s.since = 2.0;
            s.detail = "Bash cargo test";
        }
    }
    let pins = [row_of(&churned, 5), row_of(&churned, 4)];
    let (churned, churned_rows) = pin_to_top(churned, &pins);
    for frame in 0..8 {
        frames.push(render(
            &churned,
            &Scene {
                selected: 0,
                pinned: churned_rows.clone(),
                frame,
                hold: if frame == 0 { 22 } else { 3 },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    // 6. Grouped by project. The block is group zero, and each heading counts
    // only the rows beneath it - the pinned member has left its project.
    let grouped = group_unpinned(&churned, churned_rows.len());
    for frame in 0..6 {
        frames.push(render(
            &grouped,
            &Scene {
                selected: 0,
                pinned: churned_rows.clone(),
                grouped: true,
                frame,
                hold: if frame == 0 { 22 } else { 3 },
                ..Default::default()
            },
            now,
            version,
        ));
    }

    write_frames(dir, frames);
}
