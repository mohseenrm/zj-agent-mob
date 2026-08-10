//! Keyboard: selection, jump-to-pane, two-step kill.

use zellij_tile::prelude::*;

use crate::host;
use crate::install::{SetupAction, Target};
use crate::state::State;
use crate::status::Status;
use crate::util::wrap;

impl State {
    /// The setup prompt owns the whole screen while it is up, so the agent-list
    /// keys below are unreachable and cannot fire.
    fn handle_setup_key(&mut self, key: KeyWithModifier) -> bool {
        match key.bare_key {
            BareKey::Char('j') | BareKey::Down => {
                self.install.move_setup_selection(1);
                true
            }
            BareKey::Char('k') | BareKey::Up => {
                self.install.move_setup_selection(-1);
                true
            }
            BareKey::Enter => {
                let a = self.install.setup_at_cursor();
                self.run_setup(a);
                true
            }
            BareKey::Esc => {
                self.run_setup(SetupAction::Quit);
                true
            }
            // The full install screen is the only route to the plugin row.
            BareKey::Char('i') => {
                self.install.open = true;
                self.install.refresh();
                true
            }
            BareKey::Char(c) => match SetupAction::ALL.into_iter().find(|a| a.hotkey() == c) {
                Some(a) => {
                    self.install.setup_selected = a as usize;
                    self.run_setup(a);
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    /// Translates `Quit` into hiding the panel.
    fn run_setup(&mut self, action: SetupAction) {
        if !self.install.run_setup(action) {
            self.hidden = true;
            host::hide_self();
        }
    }

    /// Separate from the agent list so a stray key here cannot kill a pane.
    fn handle_install_key(&mut self, key: KeyWithModifier) -> bool {
        match key.bare_key {
            BareKey::Char('j') | BareKey::Down => {
                self.install.move_selection(1);
                true
            }
            BareKey::Char('k') | BareKey::Up => {
                self.install.move_selection(-1);
                true
            }
            BareKey::Enter => {
                let t = self.install.target_at_cursor();
                self.install.toggle(t);
                true
            }
            BareKey::Char('r') => {
                self.install.refresh();
                true
            }
            BareKey::Char('q') | BareKey::Esc | BareKey::Char('i') => {
                self.install.open = false;
                true
            }
            BareKey::Char(c) => match Target::ALL.into_iter().find(|t| t.hotkey() == c) {
                Some(t) => {
                    self.install.selected = t as usize;
                    self.install.toggle(t);
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    /// A row can be killed if its session is still alive. Foreign rows go
    /// through the `zellij` CLI, which takes a session argument where the
    /// plugin's own shims act on the current session only.
    pub(crate) fn can_kill_selected(&self) -> bool {
        self.agents.get(self.selected).map(|a| a.session_alive).unwrap_or(false)
    }

    /// Whether the selected row is in another session, so the CLI route is the
    /// only one that can reach it.
    pub(crate) fn selected_is_foreign(&self) -> bool {
        self.agents
            .get(self.selected)
            .map(|a| !self.session_name.is_empty() && a.id.session != self.session_name)
            .unwrap_or(false)
    }

    /// Moves the agent cursor, wrapping at both ends, and disarms a pending kill.
    fn move_selection(&mut self, delta: isize) {
        if !self.agents.is_empty() {
            self.selected = wrap(self.selected, delta, self.agents.len());
        }
        self.kill_armed = None;
    }

    pub(crate) fn focus_selected(&mut self) {
        let Some(agent) = self.agents.get(self.selected) else {
            return;
        };
        let (id, tab, session_alive) = (agent.id.clone(), agent.tab, agent.session_alive);
        let foreign = id.session != self.session_name && !self.session_name.is_empty();

        if let Some(a) = self.agents.iter_mut().find(|a| a.id == id) {
            if a.status == Status::Done {
                a.status = Status::Idle;
                a.status_since = self.now;
            }
        }

        if foreign {
            // A dead session has no pane to land on; attaching resurrects it.
            let target = if session_alive { Some((id.pane_id, false)) } else { None };
            host::switch_session_with_focus(&id.session, tab.filter(|_| session_alive), target);
        } else {
            host::focus_terminal_pane(id.pane_id, true, false);
        }
        self.hidden = true;
        self.sort_agents();
        host::hide_self();
    }

    /// While a reply is being composed the panel is a text field, so every
    /// printable key is text rather than a shortcut.
    fn handle_reply_key(&mut self, key: KeyWithModifier) -> bool {
        match key.bare_key {
            BareKey::Esc => {
                self.reply = None;
                true
            }
            // The reply stays bound while it is sent: `send_reply` reads its id
            // to reach the agent it was composed for rather than the selection.
            BareKey::Enter => {
                let text = self.reply.as_ref().map(|r| r.text.clone()).unwrap_or_default();
                self.send_reply(&format!("{}\n", text));
                self.reply = None;
                true
            }
            BareKey::Backspace => {
                if let Some(r) = &mut self.reply {
                    r.text.pop();
                }
                true
            }
            BareKey::Char(c) => {
                if let Some(r) = &mut self.reply {
                    r.text.push(c);
                }
                true
            }
            _ => false,
        }
    }

    pub(crate) fn handle_key(&mut self, key: KeyWithModifier) -> bool {
        if self.install.open {
            return self.handle_install_key(key);
        }
        if self.showing_setup() {
            return self.handle_setup_key(key);
        }
        if self.reply.is_some() {
            return self.handle_reply_key(key);
        }
        match key.bare_key {
            // Opening refreshes: the state may have changed outside the panel.
            BareKey::Char('i') => {
                self.install.open = true;
                self.kill_armed = None;
                self.install.refresh();
                true
            }
            BareKey::Char('j') | BareKey::Down => {
                self.move_selection(1);
                true
            }
            BareKey::Char('k') | BareKey::Up => {
                self.move_selection(-1);
                true
            }
            BareKey::Enter => {
                self.focus_selected();
                true
            }
            BareKey::Char(c @ '1'..='9') => {
                let idx = (c as u8 - b'1') as usize;
                if idx < self.agents.len() {
                    self.selected = idx;
                    self.focus_selected();
                }
                true
            }
            // First x interrupts, second closes the pane.
            BareKey::Char('x') => {
                if let Some(agent) = self.agents.get(self.selected) {
                    // A dead session has no pane left to signal.
                    if !self.can_kill_selected() {
                        return false;
                    }
                    let id = agent.id.clone();
                    let foreign = self.selected_is_foreign();
                    if self.kill_armed.as_ref() == Some(&id) {
                        self.close_pane(&id, foreign);
                        self.agents.retain(|a| a.id != id);
                        self.kill_armed = None;
                        self.clamp_selection();
                        self.publish_summary();
                    } else {
                        self.interrupt_pane(&id, foreign);
                        self.kill_armed = Some(id);
                    }
                }
                true
            }
            // Approve / reject. `d` already means dismiss, so reject is `r`:
            // a mis-keyed dismiss must never answer a permission prompt.
            BareKey::Char('a') => self.answer_selected(true),
            BareKey::Char('r') => self.answer_selected(false),
            BareKey::Char('d') => {
                let now = self.now;
                if let Some(agent) = self.agents.get_mut(self.selected) {
                    if agent.status == Status::Done {
                        agent.status = Status::Idle;
                        agent.status_since = now;
                    }
                }
                self.sort_agents();
                true
            }
            // Uppercase, so the whole-fleet version cannot be hit by a slipped
            // finger reaching for the single-row one.
            BareKey::Char('D') => {
                self.dismiss_all_done();
                true
            }
            // Reply to an agent that is blocked on a question. Restricted to
            // rows that are actually waiting, in this session: `write_chars` is
            // session-local, so a foreign pane id would type into a stranger.
            BareKey::Char('y') => self.send_reply("y\n"),
            BareKey::Char('m') => self.begin_reply(),
            BareKey::Char('n') => self.spawn_agent(),
            BareKey::Char('q') | BareKey::Esc => {
                self.kill_armed = None;
                self.hidden = true;
                host::hide_self();
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::{InstallState, SetupAction, Target};
    use std::collections::BTreeMap;

    fn key(c: char) -> KeyWithModifier {
        KeyWithModifier::new(BareKey::Char(c))
    }

    fn state_with_one_agent() -> State {
        let mut s = State {
            permissions_granted: true,
            session_name: "mob".into(),
            live_sessions: vec!["mob".into()],
            ..Default::default()
        };
        let args: BTreeMap<String, String> = [("pane_id", "7"), ("status", "idle")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        s.handle_status(&args);
        s
    }

    #[test]
    fn i_opens_and_closes_the_install_screen() {
        let mut s = state_with_one_agent();
        assert!(!s.install.open);
        s.handle_key(key('i'));
        assert!(s.install.open);
        s.handle_key(key('i'));
        assert!(!s.install.open, "i toggles back out");
        s.handle_key(key('i'));
        s.handle_key(KeyWithModifier::new(BareKey::Esc));
        assert!(!s.install.open, "esc leaves the install screen");
    }

    /// `x` kills a pane in the list but selects Codex on the install screen.
    #[test]
    fn x_on_the_install_screen_does_not_touch_agents() {
        let mut s = state_with_one_agent();
        s.handle_key(key('i'));
        s.handle_key(key('x'));
        assert_eq!(s.agents.len(), 1, "x must not kill a pane from the install screen");
        assert_eq!(s.kill_armed, None, "x must not arm a kill from the install screen");
        assert_eq!(s.install.target_at_cursor(), crate::install::Target::Codex);
    }

    /// Otherwise a queued `x` closes a pane after navigating away.
    #[test]
    fn opening_install_screen_disarms_a_pending_kill() {
        let mut s = state_with_one_agent();
        s.handle_key(key('x'));
        assert_eq!(
            s.kill_armed,
            Some(crate::agent::AgentId {
                session: "mob".into(),
                pane_id: 7
            })
        );
        s.handle_key(key('i'));
        assert_eq!(s.kill_armed, None);
    }

    #[test]
    fn install_hotkeys_move_the_cursor_to_their_row() {
        let mut s = state_with_one_agent();
        s.handle_key(key('i'));
        for (c, expect) in [
            ('c', crate::install::Target::Claude),
            ('x', crate::install::Target::Codex),
            ('p', crate::install::Target::Plugin),
        ] {
            s.handle_key(key(c));
            assert_eq!(s.install.target_at_cursor(), expect, "key {:?}", c);
        }
    }

    /// From Unknown, toggling must not claim work is in flight.
    #[test]
    fn toggling_from_unknown_state_is_inert() {
        let mut s = state_with_one_agent();
        s.handle_key(key('i'));
        s.handle_key(key('c'));
        assert_eq!(s.install.state(crate::install::Target::Claude), InstallState::Unknown);
    }

    /// Empty list plus a status read saying nothing is hooked.
    fn state_needing_setup() -> State {
        let mut s = State {
            permissions_granted: true,
            ..Default::default()
        };
        let ctx: BTreeMap<String, String> = [(crate::install::CTX_KEY, crate::install::CTX_STATUS)]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        s.install
            .on_command_result(Some(0), "claude=absent\ncodex=absent\n", "", &ctx);
        s
    }

    #[test]
    fn setup_screen_shows_only_when_there_are_no_agents_and_no_hooks() {
        let mut s = state_needing_setup();
        assert!(s.showing_setup());

        // An agent reporting in proves the hooks work, whatever status said.
        s.handle_status(&args_map(&[("pane_id", "1"), ("status", "idle")]));
        assert!(!s.showing_setup());

        s.handle_status(&args_map(&[("pane_id", "1"), ("status", "ended")]));
        assert!(s.showing_setup(), "back to empty and unhooked");

        s.install.open = true;
        assert!(!s.showing_setup(), "the full install screen takes over");
    }

    #[test]
    fn setup_hotkeys_pick_their_action() {
        for (c, expect) in [
            ('1', SetupAction::Claude),
            ('2', SetupAction::Codex),
            ('3', SetupAction::Both),
        ] {
            let mut s = state_needing_setup();
            assert!(s.handle_key(key(c)), "key {:?} must be handled", c);
            assert_eq!(s.install.setup_at_cursor(), expect);
            assert!(s.install.setup_busy(), "an install must be in flight");
        }
    }

    #[test]
    fn setup_quit_hides_the_panel_without_installing() {
        let mut s = state_needing_setup();
        s.handle_key(key('q'));
        assert!(s.hidden);
        assert!(!s.install.setup_busy(), "quit must not install anything");
    }

    #[test]
    fn setup_enter_runs_the_highlighted_action() {
        let mut s = state_needing_setup();
        s.handle_key(key('j'));
        assert_eq!(s.install.setup_at_cursor(), SetupAction::Codex);
        s.handle_key(KeyWithModifier::new(BareKey::Enter));
        assert_eq!(s.install.state(Target::Codex), InstallState::Busy);
        assert_eq!(
            s.install.state(Target::Claude),
            InstallState::Absent,
            "only the highlighted target is touched"
        );
    }

    /// The setup screen must swallow `x` rather than kill a pane.
    #[test]
    fn setup_screen_swallows_list_keys() {
        let mut s = state_needing_setup();
        assert!(!s.handle_key(key('x')), "x is not a setup action");
        assert_eq!(s.kill_armed, None);
        assert!(!s.install.setup_busy());
    }

    #[test]
    fn i_still_reaches_the_full_install_screen_from_setup() {
        let mut s = state_needing_setup();
        s.handle_key(key('i'));
        assert!(s.install.open);
        assert!(!s.showing_setup());
    }

    fn args_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn state_with(rows: &[(u32, &str, &str)]) -> State {
        let mut s = State {
            permissions_granted: true,
            session_name: "mob".into(),
            live_sessions: vec!["mob".into(), "other".into()],
            ..Default::default()
        };
        for (pane, session, status) in rows {
            s.handle_status(&args_map(&[
                ("pane_id", &pane.to_string()),
                ("session", session),
                ("status", status),
            ]));
        }
        s
    }

    /// Acknowledging six finished agents one keypress at a time is its own
    /// chore, but the whole-fleet key must not be reachable by a slipped finger
    /// on the single-row one.
    #[test]
    fn capital_d_dismisses_every_done_row() {
        let mut s = state_with(&[(1, "mob", "done"), (2, "mob", "done"), (3, "mob", "waiting")]);
        s.handle_key(key('D'));
        assert_eq!(
            s.agents.iter().filter(|a| a.status == Status::Done).count(),
            0,
            "every done row is cleared"
        );
        assert_eq!(
            s.agents.iter().filter(|a| a.status == Status::Waiting).count(),
            1,
            "a waiting row is not a done row"
        );
    }

    #[test]
    fn lowercase_d_clears_only_the_selected_row() {
        let mut s = state_with(&[(1, "mob", "done"), (2, "mob", "done")]);
        s.selected = 0;
        s.handle_key(key('d'));
        assert_eq!(s.agents.iter().filter(|a| a.status == Status::Done).count(), 1);
    }

    /// Typing into a pane that is not at a prompt would land as stray input
    /// mid-turn, so the reply keys must be inert for every other state.
    #[test]
    fn reply_is_refused_unless_the_agent_is_blocked() {
        for (status, allowed) in [
            ("waiting", true),
            ("idlewait", true),
            ("working", false),
            ("done", false),
            ("idle", false),
        ] {
            let mut s = state_with(&[(1, "mob", status)]);
            s.selected = 0;
            assert_eq!(s.can_reply_selected(), allowed, "status {status}");
            assert_eq!(s.handle_key(key('y')), allowed, "y on {status}");

            // A fresh row: the `y` above answers the agent, which legitimately
            // makes it no longer repliable.
            let mut s = state_with(&[(1, "mob", status)]);
            s.selected = 0;
            s.handle_key(key('m'));
            assert_eq!(s.reply.is_some(), allowed, "m opened an editor on {status}");
        }
    }

    /// A dead session has no pane left to type into.
    #[test]
    fn reply_is_refused_once_the_session_is_gone() {
        let mut s = state_with(&[(3, "other", "waiting")]);
        s.selected = 0;
        assert!(s.can_reply_selected());
        s.apply_sessions(vec!["mob".into()]);
        assert!(!s.can_reply_selected(), "nothing is left to answer");
    }

    /// While composing, every printable key is text rather than a shortcut, or
    /// typing "next" would kill a pane on the x.
    #[test]
    fn the_reply_editor_swallows_list_shortcuts() {
        let mut s = state_with(&[(1, "mob", "waiting")]);
        s.selected = 0;
        s.handle_key(key('m'));
        for c in ['x', 'd', 'q', 'i', 'n'] {
            s.handle_key(key(c));
        }
        assert_eq!(s.reply.as_ref().map(|r| r.text.as_str()), Some("xdqin"));
        assert_eq!(s.kill_armed, None, "x must not arm a kill while typing");
        assert!(!s.install.open, "i must not open the install screen while typing");
        assert_eq!(s.agents.len(), 1);
    }

    #[test]
    fn escape_abandons_a_reply_without_sending() {
        let mut s = state_with(&[(1, "mob", "waiting")]);
        s.selected = 0;
        s.handle_key(key('m'));
        s.handle_key(key('h'));
        s.handle_key(KeyWithModifier::new(BareKey::Esc));
        assert!(s.reply.is_none());
        assert_eq!(s.agents[0].status, Status::Waiting, "the agent was not answered");
    }

    #[test]
    fn backspace_edits_the_reply() {
        let mut s = state_with(&[(1, "mob", "waiting")]);
        s.selected = 0;
        s.handle_key(key('m'));
        for c in ['y', 'e', 's'] {
            s.handle_key(key(c));
        }
        s.handle_key(KeyWithModifier::new(BareKey::Backspace));
        assert_eq!(s.reply.as_ref().map(|r| r.text.as_str()), Some("ye"));
    }

    /// Sending marks the agent as working: leaving it `waiting` would show a
    /// prompt that has already been answered.
    #[test]
    fn sending_a_reply_advances_the_row() {
        let mut s = state_with(&[(1, "mob", "waiting")]);
        s.selected = 0;
        s.handle_key(key('y'));
        assert_eq!(s.agents[0].status, Status::Working);
        assert_eq!(s.agents[0].detail.as_deref(), Some("replied from panel"));
        assert!(s.reply.is_none());
    }

    /// A row vanishing mid-compose must not leave the text aimed at whichever
    /// agent inherits that slot.
    #[test]
    fn a_reply_is_dropped_when_its_agent_goes_away() {
        let mut s = state_with(&[(1, "mob", "waiting")]);
        s.selected = 0;
        s.handle_key(key('m'));
        assert!(s.reply.is_some());
        s.handle_status(&args_map(&[("pane_id", "1"), ("status", "ended")]));
        assert!(s.reply.is_none(), "the target is gone, so the text must be too");
    }

    /// The dangerous case: `x` arms a kill, then an incoming pipe re-sorts the
    /// list so a different agent sits under the cursor, and `x` fires again.
    /// The second press must never close the newly-selected pane.
    #[test]
    fn a_resort_between_the_two_x_presses_cannot_kill_the_wrong_pane() {
        let mut s = state_with(&[(1, "mob", "working")]);
        s.selected = 0;
        let armed = s.agents[0].id.clone();
        s.handle_key(key('x'));
        assert_eq!(s.kill_armed.as_ref(), Some(&armed));

        s.handle_status(&args_map(&[("pane_id", "2"), ("status", "waiting")]));
        let cursor = s.agents[s.selected].id.clone();
        assert_ne!(cursor, armed, "precondition: the cursor moved");

        s.handle_key(key('x'));
        assert!(
            s.agents.iter().any(|a| a.id == cursor),
            "the innocent agent must survive the second x"
        );
    }

    /// The same shape for replies. Both agents are blocked, so nothing but the
    /// bound id distinguishes them: text must reach the agent it was written
    /// for, not whoever the re-sort put under the cursor.
    #[test]
    fn a_reply_goes_to_its_own_agent_not_whoever_holds_the_cursor() {
        let mut s = state_with(&[(5, "mob", "waiting")]);
        s.selected = 0;
        let target = s.agents[0].id.clone();
        s.handle_key(key('m'));
        s.handle_key(key('h'));

        // A lower pane id sorts first within the same status, taking the cursor.
        s.handle_status(&args_map(&[("pane_id", "2"), ("status", "waiting")]));
        let cursor = s.agents[s.selected].id.clone();
        assert_ne!(cursor, target, "precondition: the cursor moved off the target");

        s.handle_key(KeyWithModifier::new(BareKey::Enter));

        let replied = |s: &State, id: &crate::agent::AgentId| {
            s.agents
                .iter()
                .find(|a| &a.id == id)
                .map(|a| a.detail.as_deref() == Some("replied from panel"))
                .unwrap_or(false)
        };
        assert!(replied(&s, &target), "the composed-for agent must receive the reply");
        assert!(!replied(&s, &cursor), "the agent under the cursor must not be answered");
        assert_eq!(
            s.agents.iter().find(|a| a.id == cursor).map(|a| a.status),
            Some(Status::Waiting),
            "the innocent agent is still blocked"
        );
    }

    /// An agent that stopped waiting while the reply was being typed can no
    /// longer be answered: the keystrokes would land mid-turn.
    #[test]
    fn a_reply_is_dropped_if_its_agent_moved_on_while_typing() {
        let mut s = state_with(&[(1, "mob", "waiting")]);
        s.selected = 0;
        s.handle_key(key('m'));
        s.handle_key(key('h'));
        s.handle_status(&args_map(&[("pane_id", "1"), ("status", "working")]));
        s.handle_key(KeyWithModifier::new(BareKey::Enter));
        assert!(s.reply.is_none());
        assert_ne!(
            s.agents[0].detail.as_deref(),
            Some("replied from panel"),
            "an agent mid-turn must not be typed into"
        );
    }

    #[test]
    fn install_screen_swallows_navigation_keys_from_the_agent_list() {
        let mut s = state_with_one_agent();
        s.selected = 0;
        s.handle_key(key('i'));
        s.handle_key(key('j'));
        assert_eq!(s.selected, 0, "j moves the install cursor, not the agent cursor");
        assert_eq!(s.install.target_at_cursor(), crate::install::Target::Codex);
    }
}
