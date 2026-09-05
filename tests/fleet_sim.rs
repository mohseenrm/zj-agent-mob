//! Multi-agent simulation and modal-key safety.
//!
//! The unit suite drives one agent through one transition at a time. Real use
//! is a fleet: eight agents across three sessions, statuses moving under the
//! cursor while keys are being pressed. The defects that survive unit testing
//! live in that interleaving, so these drive whole event sequences and assert
//! on invariants that must hold after every single step.

use std::collections::BTreeMap;

use zj_agent_mob::testing::{key, Sim};

fn args(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// Every invariant the panel must satisfy no matter what has happened to it.
/// Checked after each step of every sequence below rather than only at the end,
/// so a failure names the step that broke it.
fn check_invariants(sim: &Sim, step: &str) {
    let n = sim.agent_count();

    assert!(
        sim.selected() == 0 || sim.selected() < n,
        "{}: selection {} out of range for {} agents",
        step,
        sim.selected(),
        n
    );

    let mut ids: Vec<_> = sim.agent_ids();
    let before = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(
        before,
        ids.len(),
        "{}: duplicate agent ids: {:?}",
        step,
        sim.agent_ids()
    );

    let ranks: Vec<u8> = sim.ranks().into_iter().map(|(_, r)| r).collect();
    assert!(
        ranks.windows(2).all(|w| w[0] <= w[1]),
        "{}: list is not sorted by urgency: {:?}",
        step,
        ranks
    );

    assert!(
        sim.reply_target().is_none_or(|id| sim.agent_ids().contains(&id)),
        "{}: a reply is bound to an agent with no row",
        step
    );
}

/// One agent's realistic turn: a prompt, a handful of tool calls, then a stop.
fn turn(sim: &mut Sim, session: &str, pane: &str, tools: usize) {
    sim.status(&args(&[
        ("session", session),
        ("pane_id", pane),
        ("status", "working"),
        ("cwd", "/w/proj"),
        ("session_id", "sid"),
        ("task", "do the thing"),
    ]));
    for i in 0..tools {
        sim.status(&args(&[
            ("session", session),
            ("pane_id", pane),
            ("status", "working"),
            ("session_id", "sid"),
            ("detail", &format!("Edit file{}.rs", i)),
        ]));
    }
    sim.status(&args(&[
        ("session", session),
        ("pane_id", pane),
        ("status", "done"),
        ("session_id", "sid"),
    ]));
}

#[test]
fn a_fleet_of_turns_keeps_every_invariant() {
    let mut sim = Sim::new("mob", &["mob", "work", "side"]);
    let fleet = [
        ("mob", "1"),
        ("mob", "2"),
        ("work", "1"),
        ("work", "7"),
        ("side", "3"),
        ("side", "4"),
        ("side", "5"),
    ];
    for (round, tools) in [(0usize, 1usize), (1, 4), (2, 2)] {
        for (session, pane) in fleet {
            turn(&mut sim, session, pane, tools);
            check_invariants(&sim, &format!("round {} {}:{}", round, session, pane));
        }
    }
    assert_eq!(sim.agent_count(), fleet.len());
}

/// Subagent counters are deltas applied to a stateless source, so a long
/// interleaved sequence is the only thing that shows whether they conserve.
#[test]
fn subagent_counters_return_to_zero_over_a_turn() {
    let mut sim = Sim::new("mob", &["mob"]);
    sim.status(&args(&[("pane_id", "1"), ("status", "working"), ("session_id", "s")]));

    for i in 0..6 {
        sim.status(&args(&[
            ("pane_id", "1"),
            ("status", ""),
            ("subagent_delta", "1"),
            ("agent_type", &format!("type{}", i % 3)),
        ]));
    }
    assert_eq!(sim.counters()[0].1, 6, "six subagents started");

    for _ in 0..6 {
        sim.status(&args(&[("pane_id", "1"), ("status", ""), ("subagent_delta", "-1")]));
    }
    let (_, subs, _, _) = sim.counters()[0].clone();
    assert_eq!(subs, 0, "every subagent that started also stopped");
    assert!(
        sim.subagent_types(0).is_empty(),
        "the type list must clear with the last subagent"
    );
}

/// A stray Stop with no matching Start must not underflow into a huge count.
#[test]
fn counters_saturate_rather_than_wrap() {
    let mut sim = Sim::new("mob", &["mob"]);
    sim.status(&args(&[("pane_id", "1"), ("status", "working"), ("session_id", "s")]));
    for _ in 0..5 {
        sim.status(&args(&[("pane_id", "1"), ("status", ""), ("subagent_delta", "-1")]));
        sim.status(&args(&[("pane_id", "1"), ("status", ""), ("task_done_delta", "-1")]));
    }
    let (_, subs, total, done) = sim.counters()[0].clone();
    assert_eq!((subs, total, done), (0, 0, 0), "deltas must floor at zero");
}

/// A new turn retires the previous turn's fan-out.
#[test]
fn a_new_turn_resets_the_counters() {
    let mut sim = Sim::new("mob", &["mob"]);
    sim.status(&args(&[("pane_id", "1"), ("status", "working"), ("session_id", "s")]));
    sim.status(&args(&[("pane_id", "1"), ("status", ""), ("subagent_delta", "3")]));
    sim.status(&args(&[("pane_id", "1"), ("status", ""), ("task_delta", "2")]));
    sim.status(&args(&[("pane_id", "1"), ("status", "done"), ("session_id", "s")]));
    sim.status(&args(&[("pane_id", "1"), ("status", "working"), ("session_id", "s")]));
    let (_, subs, total, done) = sim.counters()[0].clone();
    assert_eq!((subs, total, done), (0, 0, 0), "a new turn starts from nothing");
}

// ---------------------------------------------------------------------------
// Modal key safety
// ---------------------------------------------------------------------------

/// The keys that do something irreversible to an agent. None of them may fire
/// from a mode that does not own them.
const DESTRUCTIVE: [char; 6] = ['x', 'a', 'r', 'y', 'm', 'n'];

/// Every printable key, in every mode, must leave the fleet intact unless the
/// mode owns that key. `x` from the install screen is unit-tested; this is the
/// rest of the matrix, which is where a vim user's muscle memory lands.
#[test]
fn no_key_reaches_a_destructive_action_from_a_foreign_mode() {
    let keys: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
        .chars()
        .collect();

    for mode in ["install", "setup", "reply", "jump"] {
        for &c in &keys {
            let mut sim = Sim::new("mob", &["mob"]);
            sim.status(&args(&[
                ("pane_id", "1"),
                ("status", "waiting"),
                ("session_id", "s"),
                ("cwd", "/w/p"),
            ]));
            sim.status(&args(&[
                ("pane_id", "2"),
                ("status", "working"),
                ("session_id", "s2"),
                ("cwd", "/w/p"),
            ]));
            sim.enter_mode(mode);

            let before = sim.agent_count();
            sim.press(key(c));

            assert_eq!(
                sim.agent_count(),
                before,
                "mode={} key={:?} changed the agent count",
                mode,
                c
            );
            if DESTRUCTIVE.contains(&c) {
                assert!(
                    sim.kill_armed().is_none(),
                    "mode={} key={:?} armed a kill from a mode that does not own x",
                    mode,
                    c
                );
            }
            check_invariants(&sim, &format!("mode={} key={:?}", mode, c));
        }
    }
}

/// The second `x` acts on the ARMED agent, never on the selection: a pipe
/// arriving between the two presses re-sorts the list under the cursor.
#[test]
fn the_second_x_kills_the_agent_that_was_armed() {
    let mut sim = Sim::new("mob", &["mob"]);
    for (pane, status) in [("1", "idle"), ("2", "idle"), ("3", "idle")] {
        sim.status(&args(&[
            ("pane_id", pane),
            ("status", status),
            ("session_id", "s"),
            ("cwd", "/w/p"),
        ]));
    }
    sim.select(1);
    let armed = sim.agent_ids()[1].clone();
    sim.press(key('x'));
    assert_eq!(sim.kill_armed(), Some(armed.clone()), "the first x arms the selection");

    sim.status(&args(&[
        ("pane_id", "3"),
        ("status", "failed"),
        ("session_id", "s"),
        ("cwd", "/w/p"),
    ]));
    assert_ne!(
        sim.agent_ids()[sim.selected()],
        armed,
        "the re-sort must have moved a different agent under the cursor"
    );

    sim.press(key('x'));
    assert!(
        !sim.agent_ids().contains(&armed),
        "the armed agent is the one that must be closed"
    );
    assert_eq!(sim.agent_count(), 2, "exactly one agent was closed");
    check_invariants(&sim, "after the confirmed kill");
}

/// Anything that moves the cursor must disarm, or a queued `x` closes whatever
/// the cursor landed on.
#[test]
fn navigation_disarms_a_pending_kill() {
    for nav in ['j', 'k', 's', 'i', 'q'] {
        let mut sim = Sim::new("mob", &["mob"]);
        for pane in ["1", "2"] {
            sim.status(&args(&[
                ("pane_id", pane),
                ("status", "idle"),
                ("session_id", "s"),
                ("cwd", "/w/p"),
            ]));
        }
        sim.press(key('x'));
        assert!(sim.kill_armed().is_some(), "nav={:?} setup", nav);
        sim.press(key(nav));
        assert!(
            sim.kill_armed().is_none(),
            "{:?} left a kill armed after moving the cursor",
            nav
        );
    }
}

/// `G`, `gg` and `25G` are reflex for a vim user, including past the end.
#[test]
fn jump_counts_never_select_out_of_range() {
    let mut sim = Sim::new("mob", &["mob"]);
    for pane in 1..=12 {
        sim.status(&args(&[
            ("pane_id", &pane.to_string()),
            ("status", "idle"),
            ("session_id", "s"),
            ("cwd", "/w/p"),
        ]));
    }

    for seq in [
        "G", "gg", "g1\n", "g12\n", "g13\n", "g0\n", "g999\n", "g9999\n", "g99999\n", "gG", "gx", "g\n",
    ] {
        let mut s = sim.clone_state();
        for c in seq.chars() {
            s.press(if c == '\n' { key('\r') } else { key(c) });
        }
        check_invariants(&s, &format!("after {:?}", seq));
        assert!(
            s.selected() < s.agent_count(),
            "{:?} selected row {} of {}",
            seq,
            s.selected(),
            s.agent_count()
        );
    }
}

/// A count is capped so a very long digit run cannot overflow the parse.
#[test]
fn a_jump_count_is_bounded() {
    let mut sim = Sim::new("mob", &["mob"]);
    sim.status(&args(&[("pane_id", "1"), ("status", "idle"), ("session_id", "s")]));
    sim.press(key('g'));
    for _ in 0..40 {
        sim.press(key('9'));
    }
    sim.press(key('\r'));
    check_invariants(&sim, "after a 40-digit count");
}

/// While composing, every printable key is text rather than a shortcut.
#[test]
fn reply_mode_swallows_every_destructive_key() {
    let mut sim = Sim::new("mob", &["mob"]);
    sim.status(&args(&[
        ("pane_id", "1"),
        ("status", "waiting"),
        ("session_id", "s"),
        ("cwd", "/w/p"),
    ]));
    sim.press(key('m'));
    assert!(sim.reply_target().is_some(), "m opens the editor");

    for c in DESTRUCTIVE {
        sim.press(key(c));
    }
    assert!(sim.reply_target().is_some(), "the editor must still be open");
    assert_eq!(sim.agent_count(), 1, "no key may have killed the agent");
    assert!(sim.kill_armed().is_none());
    assert_eq!(sim.reply_text(), "xarymn", "every key landed as text");
}

/// The reply goes to the agent it was written for, whatever the cursor holds
/// by the time Enter is pressed.
#[test]
fn a_reply_follows_its_agent_through_a_resort() {
    let mut sim = Sim::new("mob", &["mob"]);
    sim.status(&args(&[
        ("pane_id", "1"),
        ("status", "waiting"),
        ("session_id", "s"),
        ("cwd", "/w/p"),
    ]));
    sim.status(&args(&[
        ("pane_id", "2"),
        ("status", "idle"),
        ("session_id", "s2"),
        ("cwd", "/w/p"),
    ]));
    let target = sim.agent_ids()[sim.selected()].clone();
    sim.press(key('m'));
    assert_eq!(sim.reply_target(), Some(target.clone()));

    sim.status(&args(&[
        ("pane_id", "2"),
        ("status", "failed"),
        ("session_id", "s2"),
        ("cwd", "/w/p"),
    ]));
    assert_eq!(
        sim.reply_target(),
        Some(target),
        "a re-sort must not redirect the text at another agent"
    );
}

/// An agent exiting mid-compose must take the reply with it.
#[test]
fn a_reply_is_dropped_when_its_agent_exits() {
    let mut sim = Sim::new("mob", &["mob"]);
    sim.status(&args(&[
        ("pane_id", "1"),
        ("status", "waiting"),
        ("session_id", "s"),
        ("cwd", "/w/p"),
    ]));
    sim.press(key('m'));
    assert!(sim.reply_target().is_some());
    sim.status(&args(&[("pane_id", "1"), ("status", "ended")]));
    assert!(
        sim.reply_target().is_none(),
        "text aimed at an exited agent must not survive to be sent elsewhere"
    );
    check_invariants(&sim, "after the agent exited mid-compose");
}

/// Blocked rows only. Typing into a working agent lands mid-turn as stray input.
#[test]
fn reply_keys_are_inert_unless_the_agent_is_blocked() {
    for status in ["working", "idle", "done", "failed", "compact"] {
        let mut sim = Sim::new("mob", &["mob"]);
        sim.status(&args(&[
            ("pane_id", "1"),
            ("status", status),
            ("session_id", "s"),
            ("cwd", "/w/p"),
        ]));
        sim.press(key('m'));
        assert!(
            sim.reply_target().is_none(),
            "m opened an editor for a {} agent",
            status
        );
        sim.press(key('y'));
        assert_eq!(sim.status_of(0), status, "y changed a {} agent", status);
    }
}

/// Selection must stay on screen through the moves a vim user makes.
#[test]
fn the_selection_survives_rows_arriving_and_leaving() {
    let mut sim = Sim::new("mob", &["mob"]);
    for pane in 1..=5 {
        sim.status(&args(&[
            ("pane_id", &pane.to_string()),
            ("status", "idle"),
            ("session_id", "s"),
            ("cwd", "/w/p"),
        ]));
    }
    sim.press(key('G'));
    check_invariants(&sim, "after G");

    for pane in [5, 4, 3] {
        sim.status(&args(&[("pane_id", &pane.to_string()), ("status", "ended")]));
        check_invariants(&sim, &format!("after pane {} ended", pane));
    }
    assert!(sim.selected() < sim.agent_count());

    for _ in 0..20 {
        sim.press(key('j'));
        check_invariants(&sim, "wrapping with j");
    }
    for _ in 0..20 {
        sim.press(key('k'));
        check_invariants(&sim, "wrapping with k");
    }
}

/// A dead session's rows go `unknown` rather than vanishing, and nothing may
/// then act on them.
#[test]
fn a_dead_session_disables_the_actions_that_need_a_process() {
    let mut sim = Sim::new("mob", &["mob", "work"]);
    sim.status(&args(&[
        ("session", "work"),
        ("pane_id", "1"),
        ("status", "waiting"),
        ("session_id", "s"),
        ("cwd", "/w/p"),
    ]));
    sim.select(0);
    sim.sessions(&["mob"]);

    assert_eq!(sim.status_of(0), "unknown", "a dead session's row is unknowable");
    let before = sim.agent_count();
    sim.press(key('x'));
    assert!(
        sim.kill_armed().is_none(),
        "x must be refused with no process to signal"
    );
    assert_eq!(sim.agent_count(), before, "the row is kept, not dropped");
    sim.press(key('m'));
    assert!(sim.reply_target().is_none(), "there is nothing to type into");
}

// ---------------------------------------------------------------------------
// The spool: cross-session status, and the two clocks that date it
// ---------------------------------------------------------------------------

const STALE_AFTER: f64 = 60.0;

fn spool(status: &str, sid: &str) -> Vec<(&'static str, String)> {
    vec![
        ("status", status.to_string()),
        ("session_id", sid.to_string()),
        ("cwd", "/w/proj".to_string()),
        ("tool", "claude".to_string()),
    ]
}

fn rec<'a>(
    session: &'a str,
    pane: u32,
    ts: f64,
    kv: &'a [(&'static str, String)],
) -> (&'a str, u32, f64, Vec<(&'a str, &'a str)>) {
    (session, pane, ts, kv.iter().map(|(k, v)| (*k, v.as_str())).collect())
}

/// The rule that makes the whole design safe: existence comes from the process
/// scan, never from a file. A leftover record must not resurrect a dead agent.
#[test]
fn a_spool_record_never_creates_a_row() {
    let mut sim = Sim::new("mob", &["mob", "work"]);
    let kv = spool("waiting", "s1");
    sim.scan(&[], &[rec("work", 1, 100.0, &kv)]);
    assert_eq!(sim.agent_count(), 0, "a record with no process behind it is not a row");

    sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 100.0, &kv)]);
    assert_eq!(
        sim.agent_count(),
        1,
        "the scan justifies the row, the record refines it"
    );
    assert_eq!(sim.status_of(0), "waiting");
}

/// A record from a previous agent on a recycled pane must not colour the
/// current one.
#[test]
fn a_recycled_pane_ignores_the_previous_agents_record() {
    let mut sim = Sim::new("mob", &["mob", "work"]);
    let old = spool("failed", "OLD-SESSION");
    sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 100.0, &old)]);
    assert_eq!(sim.status_of(0), "failed");

    let new = spool("working", "NEW-SESSION");
    sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 200.0, &new)]);
    assert_eq!(sim.status_of(0), "working", "the newer agent's own record applies");

    let stale = spool("failed", "OLD-SESSION");
    sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 100.0, &stale)]);
    assert_eq!(
        sim.status_of(0),
        "working",
        "the previous agent's own record must not reach back over the current one"
    );
}

/// A home row's hook pipes straight into this session, so a poll must never
/// overwrite it.
#[test]
fn the_spool_never_overwrites_a_home_row() {
    let mut sim = Sim::new("mob", &["mob"]);
    sim.status(&args(&[
        ("pane_id", "1"),
        ("status", "working"),
        ("session_id", "s1"),
        ("cwd", "/w/p"),
    ]));
    let kv = spool("failed", "s1");
    sim.scan(&[("mob", 1, "claude")], &[rec("mob", 1, 999.0, &kv)]);
    assert_eq!(
        sim.status_of(0),
        "working",
        "the pipe is authoritative for the panel's own session"
    );
}

/// A working row claims active progress, and silence contradicts that.
#[test]
fn a_quiet_working_row_decays_to_unknown_and_recovers() {
    let mut sim = Sim::new("mob", &["mob", "work"]);
    let kv = spool("working", "s1");
    sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 1000.0, &kv)]);
    assert_eq!(sim.status_of(0), "working");

    while sim.now() < STALE_AFTER + 5.0 {
        sim.tick();
    }
    assert_eq!(
        sim.status_of(0),
        "unknown",
        "a working row the panel can no longer vouch for must say so"
    );

    let fresh = spool("working", "s1");
    sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 5000.0, &fresh)]);
    assert_eq!(sim.status_of(0), "working", "a fresh record must bring the row back");
}

/// A blocked agent writes nothing while it waits, so its record stops
/// advancing. Decaying it to `unknown` would hide the one row the panel exists
/// to show.
#[test]
fn a_blocked_row_survives_its_own_silence() {
    for status in ["waiting", "idlewait", "done", "failed", "idle"] {
        let mut sim = Sim::new("mob", &["mob", "work"]);
        let kv = spool(status, "s1");
        sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 1000.0, &kv)]);
        let label = sim.status_of(0).to_string();

        for _ in 0..40 {
            for _ in 0..(5.0 / 0.25) as usize {
                sim.tick();
            }
            sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 1000.0, &kv)]);
        }
        assert_eq!(
            sim.status_of(0),
            label,
            "{} is a state silence predicts, so re-reading must re-confirm it",
            status
        );
    }
}

/// A re-read can only ever re-confirm what the row already holds. It must never
/// be able to change one, or an unchanging file would pin a stale status.
#[test]
fn re_reading_an_old_record_cannot_change_a_status() {
    let mut sim = Sim::new("mob", &["mob", "work"]);
    let old = spool("waiting", "s1");
    sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 1000.0, &old)]);
    assert_eq!(sim.status_of(0), "waiting");

    let newer = spool("done", "s1");
    sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 2000.0, &newer)]);
    assert_eq!(sim.status_of(0), "done");

    sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 1000.0, &old)]);
    assert_eq!(
        sim.status_of(0),
        "done",
        "a superseded record must not reach back and re-apply an older status"
    );
}

/// A fleet that goes entirely quiet freezes the record epoch. Without the
/// second term in `spool_age` the last record reads as current forever.
#[test]
fn a_frozen_epoch_still_ages() {
    let mut sim = Sim::new("mob", &["mob", "work"]);
    let kv = spool("working", "s1");
    sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 1000.0, &kv)]);

    for _ in 0..(STALE_AFTER * 3.0 / 0.25) as usize {
        sim.tick();
    }
    assert_eq!(
        sim.status_of(0),
        "unknown",
        "a working row must decay even though no newer record ever arrived"
    );
}

/// A host clock jump must not pin a row as permanently current.
#[test]
fn a_clock_jump_cannot_pin_a_row_as_current() {
    let mut sim = Sim::new("mob", &["mob", "work"]);
    let future = spool("working", "s1");
    sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 4_000_000_000.0, &future)]);
    assert_eq!(sim.status_of(0), "working");

    for _ in 0..(STALE_AFTER * 3.0 / 0.25) as usize {
        sim.tick();
    }
    assert_eq!(
        sim.status_of(0),
        "unknown",
        "a record dated far in the future must still age on the panel's own clock"
    );
}

/// The scan is the only thing that ever sees a foreign agent exit.
#[test]
fn the_scan_culls_a_foreign_row_that_has_gone() {
    let mut sim = Sim::new("mob", &["mob", "work"]);
    let kv = spool("working", "s1");
    sim.scan(
        &[("work", 1, "claude"), ("work", 2, "codex")],
        &[rec("work", 1, 100.0, &kv)],
    );
    assert_eq!(sim.agent_count(), 2);

    sim.scan(&[("work", 1, "claude")], &[]);
    assert_eq!(sim.agent_count(), 1, "the agent the scan stopped seeing is gone");
    assert_eq!(sim.agent_ids()[0], ("work".to_string(), 1));
}

/// A truncated read looks like "no agents anywhere" and would cull everything.
#[test]
fn an_incomplete_scan_changes_nothing() {
    let mut sim = Sim::new("mob", &["mob", "work"]);
    sim.parse_scan("SCAN work 1 claude\nSCANEND\n");
    assert_eq!(sim.agent_count(), 1);

    sim.parse_scan("SCAN work 1 claude\n");
    assert_eq!(
        sim.agent_count(),
        1,
        "a scan with no SCANEND is not evidence of absence"
    );
}

/// The filename is the authority on identity: a record claiming a different
/// pane than the file it lives in is malformed.
#[test]
fn a_mislabelled_record_is_dropped() {
    let mut sim = Sim::new("mob", &["mob", "work"]);
    sim.parse_scan(
        "SCAN work 1 claude\n\
         SPOOL /tmp/s/work.1:ts=100,pane_id=2,session=work,status=failed\n\
         SCANEND\n",
    );
    assert_eq!(sim.agent_count(), 1);
    assert_eq!(
        sim.status_of(0),
        "found",
        "a record whose pane disagrees with its filename must not apply"
    );
}

/// Half-written records are named `.tmp`; the rename into place publishes them.
#[test]
fn a_half_written_record_is_ignored() {
    let mut sim = Sim::new("mob", &["mob", "work"]);
    sim.parse_scan(
        "SCAN work 1 claude\n\
         SPOOL /tmp/s/work.1.1234.tmp:ts=100,pane_id=1,session=work,status=failed\n\
         SCANEND\n",
    );
    assert_eq!(sim.status_of(0), "found", "a .tmp record has not been published yet");
}

/// Panel beacons share the directory but are not agent records.
#[test]
fn a_panel_beacon_is_not_an_agent() {
    let mut sim = Sim::new("mob", &["mob", "work"]);
    sim.parse_scan("SCAN work 1 claude\nSPOOL /tmp/s/panel.work:work\nSCANEND\n");
    assert_eq!(sim.agent_count(), 1, "a beacon must never become a row");
}

/// Pane ids are only unique within a session, so identity is (session, pane).
#[test]
fn the_same_pane_in_two_sessions_is_two_agents() {
    let mut sim = Sim::new("mob", &["mob", "work", "side"]);
    let a = spool("waiting", "sa");
    let b = spool("working", "sb");
    sim.scan(
        &[("work", 1, "claude"), ("side", 1, "claude")],
        &[rec("work", 1, 100.0, &a), rec("side", 1, 100.0, &b)],
    );
    assert_eq!(sim.agent_count(), 2, "same pane id, different sessions, two rows");
    check_invariants(&sim, "two sessions sharing a pane id");
}

/// Quitting an agent and starting another in the same pane is routine. The row
/// followed the dead agent's last status for as long as the pane kept a
/// process on it, because the id guard rejected the new agent's records too.
#[test]
fn a_restarted_agent_takes_over_its_pane() {
    let mut sim = Sim::new("mob", &["mob", "work"]);
    let dead = spool("failed", "AGENT-ONE");
    sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 100.0, &dead)]);
    assert_eq!(sim.status_of(0), "failed");

    let fresh = spool("working", "AGENT-TWO");
    sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 200.0, &fresh)]);
    assert_eq!(
        sim.status_of(0),
        "working",
        "the pane's new agent owns the row, not the one that exited"
    );
    assert_eq!(sim.agent_count(), 1, "a restart reuses the row rather than adding one");
}

/// The previous agent's turn state must not be inherited by its successor.
#[test]
fn a_restart_clears_the_previous_agents_turn() {
    let mut sim = Sim::new("mob", &["mob", "work"]);
    let mut before = spool("working", "AGENT-ONE");
    before.push(("task", "the old task".to_string()));
    before.push(("detail", "Edit old.rs".to_string()));
    sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 100.0, &before)]);

    let after = spool("working", "AGENT-TWO");
    sim.scan(&[("work", 1, "claude")], &[rec("work", 1, 200.0, &after)]);
    let (_, subs, total, done) = sim.counters()[0].clone();
    assert_eq!(
        (subs, total, done),
        (0, 0, 0),
        "the new agent starts with none of the old one's progress"
    );
}

/// Sanitizing a session name is lossy, so two live sessions can share one key.
/// The spool is keyed by that name, so their records land in one file and each
/// agent's status overwrites the other's.
#[test]
fn two_sessions_that_sanitize_alike_do_not_share_a_row() {
    let mut sim = Sim::new("mob", &["mob", "my_session"]);
    let a = spool("working", "AAA");
    sim.scan(&[("my_session", 1, "claude")], &[rec("my_session", 1, 100.0, &a)]);
    assert_eq!(sim.agent_count(), 1);
    assert_eq!(sim.status_of(0), "working");

    let b = spool("waiting", "BBB");
    sim.scan(&[("my_session", 1, "claude")], &[rec("my_session", 1, 200.0, &b)]);
    assert_eq!(
        sim.agent_count(),
        1,
        "the scan sees one pane, so there can only be one row"
    );
}

/// A floating pane can be dragged down to almost nothing. Every render path
/// must survive it: the panel is allowed to be useless at four columns, but it
/// must not wrap, panic, or paint outside its range.
#[test]
fn the_header_survives_a_pane_squeezed_to_nothing() {
    let mut sim = Sim::new("mob", &["mob"]);
    for pane in 1..=3 {
        sim.status(&args(&[
            ("pane_id", &pane.to_string()),
            ("status", "waiting"),
            ("session_id", "s"),
            ("cwd", "/w/p"),
            ("task", "a task summary long enough to need cutting"),
        ]));
    }
    for width in 1usize..=12 {
        let head = sim.head_line(width);
        assert!(head.chars().count() <= width, "width={} produced {:?}", width, head);
    }
}

// ---------------------------------------------------------------------------
// Answering a permission prompt
// ---------------------------------------------------------------------------

/// The hook polls a verdict file for a bounded time and then falls through to
/// the agent's own prompt. A verdict written after that is read by nobody, so
/// the panel must stop offering to write one - otherwise `a` reports an
/// approval that never reached the agent.
#[test]
fn an_expired_prompt_can_no_longer_be_answered() {
    let mut sim = Sim::new("mob", &["mob"]);
    sim.status(&args(&[
        ("pane_id", "1"),
        ("status", "waiting"),
        ("session_id", "s"),
        ("cwd", "/w/p"),
        ("block", "tool"),
    ]));
    sim.ask(&args(&[
        ("pane_id", "1"),
        ("verdict_file", "/tmp/v.1"),
        ("tool_name", "Bash"),
        ("tool_arg", "rm -rf /"),
        ("timeout", "5"),
    ]));
    assert!(sim.has_ask(0), "the prompt is parked");
    assert!(sim.press(key('a')), "a answers it while the hook is still waiting");

    sim.ask(&args(&[
        ("pane_id", "1"),
        ("verdict_file", "/tmp/v.1"),
        ("tool_name", "Bash"),
        ("tool_arg", "rm -rf /"),
        ("timeout", "5"),
    ]));
    while sim.now() < 6.0 {
        sim.tick();
    }
    assert!(!sim.has_ask(0), "the prompt must not outlive the hook that parked it");
    assert!(
        !sim.press(key('a')),
        "a must be refused once nothing is reading the verdict file"
    );
}

/// The panel offers the approve keys only while a prompt can actually be
/// settled, so the footer must lose them at the same moment.
#[test]
fn an_expired_prompt_stops_being_offered() {
    let mut sim = Sim::new("mob", &["mob"]);
    sim.status(&args(&[
        ("pane_id", "1"),
        ("status", "waiting"),
        ("session_id", "s"),
        ("cwd", "/w/p"),
    ]));
    sim.ask(&args(&[
        ("pane_id", "1"),
        ("verdict_file", "/tmp/v.1"),
        ("tool_name", "Bash"),
        ("timeout", "3"),
    ]));
    assert!(sim.has_ask(0));
    while sim.now() < 4.0 {
        sim.tick();
    }
    assert!(!sim.has_ask(0), "the row must stop advertising an unanswerable prompt");
}

/// A hook predating the timeout field must still expire, on the documented
/// default rather than never.
#[test]
fn a_prompt_without_a_timeout_still_expires() {
    let mut sim = Sim::new("mob", &["mob"]);
    sim.status(&args(&[
        ("pane_id", "1"),
        ("status", "waiting"),
        ("session_id", "s"),
        ("cwd", "/w/p"),
    ]));
    sim.ask(&args(&[("pane_id", "1"), ("verdict_file", "/tmp/v.1")]));
    assert!(sim.has_ask(0));
    while sim.now() < 31.0 {
        sim.tick();
    }
    assert!(!sim.has_ask(0), "an unbounded prompt would never stop being offered");
}

/// The clock is what carries a prompt to its expiry, and a blocked row animates
/// nothing that would otherwise keep it running.
#[test]
fn a_parked_prompt_keeps_the_clock_running() {
    let mut sim = Sim::new("mob", &["mob"]);
    sim.status(&args(&[
        ("pane_id", "1"),
        ("status", "waiting"),
        ("session_id", "s"),
        ("cwd", "/w/p"),
    ]));
    sim.ask(&args(&[
        ("pane_id", "1"),
        ("verdict_file", "/tmp/v.1"),
        ("timeout", "10"),
    ]));
    let start = sim.now();
    for _ in 0..8 {
        sim.tick();
    }
    assert!(sim.now() > start, "the tick must advance while a prompt is parked");
}
