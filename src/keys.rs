//! Keyboard: selection, jump-to-pane, two-step kill.

use zellij_tile::prelude::*;

use crate::host;
use crate::install::{SetupAction, Target};
use crate::state::{Find, State};
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
            // The name Zellij knows it by, not the sanitized filename key.
            let session = self.real_session(&id.session);
            host::switch_session_with_focus(&session, tab.filter(|_| session_alive), target);
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
                    // The panel truncates for display, so without a cap the
                    // payload could grow far past what the line shows and be
                    // typed into the agent in full.
                    if r.text.chars().count() < crate::MAX_REPLY_CHARS {
                        r.text.push(c);
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// The `/` prompt. Printable keys are query text, so none of the list
    /// shortcuts below are reachable while it is open: a stray `x` mid-search
    /// must never arm a kill. Movement is on Ctrl (and the arrows), fzf-style,
    /// because plain `j`/`k` are query characters here.
    fn handle_find_key(&mut self, key: KeyWithModifier) -> bool {
        if key.key_modifiers.contains(&KeyModifier::Ctrl) {
            match key.bare_key {
                BareKey::Char('j') | BareKey::Char('n') => self.find_move(1),
                BareKey::Char('k') | BareKey::Char('p') => self.find_move(-1),
                _ => {}
            }
            return true;
        }
        match key.bare_key {
            BareKey::Esc => {
                self.find = None;
                true
            }
            // The target id is resolved from the cursor before the prompt
            // closes, then re-looked-up to focus: a pipe between the last
            // keystroke and Enter may have re-sorted the list, and an index
            // held across that would jump into a stranger.
            BareKey::Enter => {
                let matches = self.find_matches();
                let target = self
                    .find_cursor_pos(&matches)
                    .map(|p| self.agents[matches[p]].id.clone());
                self.find = None;
                if let Some(id) = target {
                    if let Some(i) = self.agents.iter().position(|a| a.id == id) {
                        self.selected = i;
                        self.focus_selected();
                    }
                }
                true
            }
            BareKey::Down => {
                self.find_move(1);
                true
            }
            BareKey::Up => {
                self.find_move(-1);
                true
            }
            // On an already-empty query, closes the prompt, as vim's search
            // line does.
            BareKey::Backspace => {
                match &mut self.find {
                    Some(f) if !f.query.is_empty() => {
                        f.query.pop();
                        f.cursor = None;
                    }
                    _ => self.find = None,
                }
                true
            }
            BareKey::Char(c) => {
                if let Some(f) = &mut self.find {
                    // A query is a few remembered characters; a cap far past
                    // any useful length just bounds the per-keystroke rescore.
                    if f.query.chars().count() < 64 {
                        f.query.push(c);
                        // Whatever was highlighted no longer reflects the new
                        // narrowing; the best match does.
                        f.cursor = None;
                    }
                }
                true
            }
            _ => true,
        }
    }

    /// Moves the find cursor within the current matches, wrapping.
    fn find_move(&mut self, delta: isize) {
        let matches = self.find_matches();
        let Some(pos) = self.find_cursor_pos(&matches) else {
            return;
        };
        let next = wrap(pos, delta, matches.len());
        let id = self.agents[matches[next]].id.clone();
        if let Some(f) = &mut self.find {
            f.cursor = Some(id);
        }
    }

    /// The count started by `g`. Vim puts the count before the motion (`25G`),
    /// but a bare `1`-`9` already jumps on its own here, so the digits cannot
    /// lead: pressing `2` has focused the pane and hidden the panel before a
    /// second digit could arrive. `g` opens the count instead, and `gg` / `G`
    /// keep their real vim meanings.
    ///
    /// Any other key cancels rather than guessing, so a mistyped prefix cannot
    /// fire a jump at whatever row it happened to parse to.
    fn handle_jump_key(&mut self, key: KeyWithModifier) -> bool {
        match key.bare_key {
            BareKey::Char(c @ '0'..='9') => {
                if let Some(b) = &mut self.jump_buf {
                    // Four digits is far past any real fleet, and caps the parse.
                    if b.len() < 4 {
                        b.push(c);
                    }
                }
                true
            }
            BareKey::Backspace => {
                if let Some(b) = &mut self.jump_buf {
                    b.pop();
                }
                true
            }
            // `gg` with no count is vim's "first row".
            BareKey::Char('g') if self.jump_buf.as_deref() == Some("") => {
                self.jump_buf = None;
                self.jump_to_row(1);
                true
            }
            // `G` closes a count the way vim does (`25G`); Enter is the same
            // thing for anyone who does not think in vim.
            BareKey::Enter | BareKey::Char('G') | BareKey::Char('g') => {
                let n = self.jump_buf.take().and_then(|b| b.parse::<usize>().ok());
                if let Some(n) = n {
                    self.jump_to_row(n);
                }
                true
            }
            _ => {
                self.jump_buf = None;
                true
            }
        }
    }

    /// Selects and focuses a 1-based row. Out of range does nothing: clamping
    /// would jump somewhere the user did not name.
    fn jump_to_row(&mut self, n: usize) {
        if n >= 1 && n <= self.agents.len() {
            self.selected = n - 1;
            self.focus_selected();
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
        if self.find.is_some() {
            return self.handle_find_key(key);
        }
        if self.jump_buf.is_some() {
            return self.handle_jump_key(key);
        }
        // Any deliberate keypress means the last failure has been seen.
        self.action_error = None;
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
            // Opens a count for rows past the 1-9 fast path: `g25` then Enter,
            // or `g25G` for the vim spelling. `gg` is the first row.
            BareKey::Char('g') => {
                self.jump_buf = Some(String::new());
                self.kill_armed = None;
                true
            }
            // Vim's search prompt, fzf's narrowing: typing filters the list.
            // Inert on an empty list, which the empty screen renders anyway.
            BareKey::Char('/') => {
                if !self.agents.is_empty() {
                    self.find = Some(Find::default());
                    self.kill_armed = None;
                }
                true
            }
            // Vim's "last line". No count can reach it without knowing how many
            // rows there are, which is exactly what this saves you looking up.
            BareKey::Char('G') => {
                self.jump_to_row(self.agents.len());
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
            // First x interrupts, second closes the pane. The second press acts
            // on the ARMED agent, never on the selection: a pipe arriving
            // between the two re-sorts the list under the cursor, and closing
            // whatever landed there would kill an agent nobody confirmed.
            BareKey::Char('x') => {
                if let Some(armed) = self.kill_armed.clone() {
                    if self.agents.iter().any(|a| a.id == armed && a.session_alive) {
                        let foreign = !self.session_name.is_empty() && armed.session != self.session_name;
                        self.close_pane(&armed, foreign);
                        self.agents.retain(|a| a.id != armed);
                        self.kill_armed = None;
                        self.clamp_selection();
                        self.publish_summary();
                        return true;
                    }
                    // The armed agent is gone, so the confirmation is moot.
                    self.kill_armed = None;
                }
                if !self.can_kill_selected() {
                    return false;
                }
                if let Some(id) = self.agents.get(self.selected).map(|a| a.id.clone()) {
                    let foreign = self.selected_is_foreign();
                    self.interrupt_pane(&id, foreign);
                    self.kill_armed = Some(id);
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
            // Cycles urgency -> project -> session. The selection follows the
            // agent rather than the row index: re-sorting under a fixed index
            // would move the cursor onto whatever landed there.
            BareKey::Char('s') => {
                let id = self.agents.get(self.selected).map(|a| a.id.clone());
                self.grouping = self.grouping.next();
                self.sort_agents();
                if let Some(id) = id {
                    if let Some(i) = self.agents.iter().position(|a| a.id == id) {
                        self.selected = i;
                    }
                }
                self.kill_armed = None;
                true
            }
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

    fn agents(specs: &[(&str, &str, &str)]) -> State {
        let mut s = State {
            permissions_granted: true,
            session_name: "mob".into(),
            live_sessions: vec!["mob".into()],
            ..Default::default()
        };
        for (pane, cwd, status) in specs {
            let args: BTreeMap<String, String> = [("pane_id", *pane), ("cwd", *cwd), ("status", *status)]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            s.handle_status(&args);
        }
        s
    }

    #[test]
    fn s_cycles_the_grouping_mode() {
        let mut s = state_with_one_agent();
        assert_eq!(s.grouping, crate::state::Grouping::Urgency);
        s.handle_key(key('s'));
        assert_eq!(s.grouping, crate::state::Grouping::Project);
        s.handle_key(key('s'));
        assert_eq!(s.grouping, crate::state::Grouping::Session);
        s.handle_key(key('s'));
        assert_eq!(s.grouping, crate::state::Grouping::Urgency, "s wraps back round");
    }

    /// Re-sorting moves rows, so a selection tracked by index would land on
    /// whichever agent happened to take that slot.
    #[test]
    fn s_keeps_the_selection_on_the_same_agent() {
        let mut s = agents(&[
            ("1", "/w/aaa", "idle"),
            ("2", "/w/zzz", "waiting"),
            ("3", "/w/aaa", "working"),
        ]);
        s.selected = 2;
        let before = s.agents[2].id.clone();
        s.handle_key(key('s'));
        assert_eq!(s.agents[s.selected].id, before, "the cursor must follow the agent");
    }

    #[test]
    fn s_disarms_a_pending_kill() {
        let mut s = state_with_one_agent();
        s.kill_armed = Some(s.agents[0].id.clone());
        s.handle_key(key('s'));
        assert!(
            s.kill_armed.is_none(),
            "re-sorting must not leave a kill armed on a moved row"
        );
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

    /// The panel truncates the reply for display, so an uncapped buffer would
    /// send far more than the line ever showed.
    #[test]
    fn a_reply_is_capped_in_length() {
        let mut s = state_with(&[(1, "mob", "waiting")]);
        s.selected = 0;
        s.handle_key(key('m'));
        for _ in 0..(crate::MAX_REPLY_CHARS + 50) {
            s.handle_key(key('x'));
        }
        assert_eq!(
            s.reply.as_ref().map(|r| r.text.chars().count()),
            Some(crate::MAX_REPLY_CHARS)
        );
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
        assert_eq!(
            s.kill_armed, None,
            "the confirmation was spent on the armed agent, not re-armed on the cursor"
        );
    }

    /// The second press must close the ARMED pane, not whatever the cursor
    /// happens to hold. A re-sort between the presses is routine: any incoming
    /// `failed` outranks `working` and takes index 0.
    #[test]
    fn the_second_x_closes_the_armed_pane_after_a_resort() {
        let mut s = state_with(&[(5, "mob", "working")]);
        s.selected = 0;
        let armed = s.agents[0].id.clone();
        s.handle_key(key('x'));

        s.handle_status(&args_map(&[("pane_id", "9"), ("status", "failed")]));
        let cursor = s.agents[s.selected].id.clone();
        assert_ne!(cursor, armed, "precondition: a failed row took the cursor");

        s.handle_key(key('x'));
        assert!(
            !s.agents.iter().any(|a| a.id == armed),
            "the armed agent is the one that must be closed"
        );
        assert!(
            s.agents.iter().any(|a| a.id == cursor),
            "the agent under the cursor must be untouched"
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

    /// Rows past 9 have a number on screen that no single key can reach, so the
    /// count is the only way to get to them.
    #[test]
    fn g_then_digits_then_enter_jumps_past_row_nine() {
        let mut s = State {
            permissions_granted: true,
            session_name: "mob".into(),
            live_sessions: vec!["mob".into()],
            ..Default::default()
        };
        for i in 0..25u32 {
            s.handle_status(&args_map(&[("pane_id", &i.to_string()), ("status", "idle")]));
        }
        s.handle_key(key('g'));
        assert_eq!(s.jump_buf.as_deref(), Some(""), "g opens the count");
        s.handle_key(key('2'));
        s.handle_key(key('5'));
        assert_eq!(s.jump_buf.as_deref(), Some("25"));
        s.handle_key(KeyWithModifier::new(BareKey::Enter));
        assert_eq!(s.jump_buf, None, "committing closes the buffer");
        assert_eq!(s.selected, 24, "g25 selects the 25th row, 1-based");
    }

    /// A number past the end must not move the cursor to a row that is not
    /// there; clamping would jump somewhere the user did not name.
    #[test]
    fn a_count_past_the_end_of_the_list_does_nothing() {
        let mut s = state_with_agents(3);
        s.selected = 1;
        for c in ['g', '9', '9'] {
            s.handle_key(key(c));
        }
        s.handle_key(KeyWithModifier::new(BareKey::Enter));
        assert_eq!(s.selected, 1, "an out-of-range goto leaves the selection alone");
        assert_eq!(s.jump_buf, None);
    }

    /// While the count is open every key belongs to it, so `x` cannot kill.
    #[test]
    fn a_pending_count_swallows_other_keys_instead_of_acting_on_them() {
        let mut s = state_with_agents(3);
        s.handle_key(key('g'));
        s.handle_key(key('x'));
        assert_eq!(s.jump_buf, None, "a non-digit cancels the count");
        assert_eq!(s.kill_armed, None, "and must not arm a kill");
        assert_eq!(s.agents.len(), 3);
    }

    #[test]
    fn backspace_edits_the_count() {
        let mut s = state_with_agents(3);
        for c in ['g', '1', '2'] {
            s.handle_key(key(c));
        }
        s.handle_key(KeyWithModifier::new(BareKey::Backspace));
        assert_eq!(s.jump_buf.as_deref(), Some("1"));
    }

    fn state_with_agents(n: u32) -> State {
        let mut s = State {
            permissions_granted: true,
            session_name: "mob".into(),
            live_sessions: vec!["mob".into()],
            ..Default::default()
        };
        for i in 0..n {
            s.handle_status(&args_map(&[("pane_id", &i.to_string()), ("status", "idle")]));
        }
        s
    }

    /// Vim's own ordering: the count precedes the motion, and `G` closes it.
    #[test]
    fn a_count_closed_with_capital_g_is_the_vim_spelling() {
        let mut s = state_with_agents(30);
        for c in ['g', '2', '5', 'G'] {
            s.handle_key(key(c));
        }
        assert_eq!(s.selected, 24, "g25G lands on row 25");
        assert_eq!(s.jump_buf, None);
    }

    /// `gg` is the first row and `G` the last, as in vim.
    #[test]
    fn gg_goes_to_the_first_row_and_capital_g_to_the_last() {
        let mut s = state_with_agents(12);
        s.selected = 5;
        s.handle_key(key('G'));
        assert_eq!(s.selected, 11, "G with no count is the last row");
        assert_eq!(s.jump_buf, None, "G alone opens no count");

        s.handle_key(key('g'));
        s.handle_key(key('g'));
        assert_eq!(s.selected, 0, "gg is the first row");
        assert_eq!(s.jump_buf, None);
    }

    /// `G` on an empty list must not index a row that is not there.
    #[test]
    fn capital_g_on_an_empty_list_is_inert() {
        let mut s = State {
            permissions_granted: true,
            session_name: "mob".into(),
            ..Default::default()
        };
        assert!(s.handle_key(key('G')));
        assert_eq!(s.selected, 0);
        assert!(s.agents.is_empty());
    }

    /// A count of zero names no row, so it must not wrap to the last one.
    #[test]
    fn a_zero_count_does_nothing() {
        let mut s = state_with_agents(5);
        s.selected = 2;
        for c in ['g', '0'] {
            s.handle_key(key(c));
        }
        s.handle_key(KeyWithModifier::new(BareKey::Enter));
        assert_eq!(s.selected, 2, "g0 names no row");
    }

    /// The single-key path must survive the count being added around it.
    #[test]
    fn bare_digits_still_jump_immediately() {
        let mut s = state_with_agents(9);
        s.handle_key(key('4'));
        assert_eq!(s.selected, 3, "4 jumps to row 4 with no count");
        assert_eq!(s.jump_buf, None, "a bare digit opens no count");
    }

    /// The full sequence a user actually types, asserted on selection rather
    /// than on a pty: `focus_selected` hides the panel, so every keystroke
    /// after a jump lands in the pane underneath and cannot be read back.
    #[test]
    fn the_documented_jump_sequences_land_on_the_right_row() {
        for (keys, expect, why) in [
            (vec!['G'], 25usize, "G is the last row"),
            (vec!['g', 'g'], 0, "gg is the first row"),
            (vec!['g', '2', '2', 'G'], 21, "g22G is the vim spelling"),
            (vec!['4'], 3, "a bare digit still jumps"),
        ] {
            let mut s = state_with_agents(26);
            s.selected = 12;
            for c in keys.iter() {
                s.handle_key(key(*c));
            }
            assert_eq!(s.selected, expect, "{}", why);
            assert_eq!(s.jump_buf, None, "the count must be closed after {}", why);
        }
    }

    /// Out of range must leave the cursor alone rather than clamping to an end.
    #[test]
    fn an_out_of_range_count_leaves_the_selection_untouched() {
        let mut s = state_with_agents(26);
        s.selected = 12;
        for c in ['g', '9', '9'] {
            s.handle_key(key(c));
        }
        s.handle_key(KeyWithModifier::new(BareKey::Enter));
        assert_eq!(s.selected, 12, "g99 with 26 rows must not move the cursor");
        assert_eq!(s.jump_buf, None);
    }
}

#[cfg(test)]
mod find_tests {
    use super::*;
    use std::collections::BTreeMap;

    fn key(c: char) -> KeyWithModifier {
        KeyWithModifier::new(BareKey::Char(c))
    }

    fn ctrl(c: char) -> KeyWithModifier {
        KeyWithModifier::new(BareKey::Char(c)).with_ctrl_modifier()
    }

    /// Agents in session "mob", each with a cwd and a task to match against.
    fn fleet(specs: &[(&str, &str, &str)]) -> State {
        let mut s = State {
            permissions_granted: true,
            session_name: "mob".into(),
            live_sessions: vec!["mob".into()],
            ..Default::default()
        };
        for (pane, cwd, task) in specs {
            let args: BTreeMap<String, String> = [
                ("pane_id", *pane),
                ("cwd", *cwd),
                ("task", *task),
                ("status", "working"),
            ]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
            s.handle_status(&args);
        }
        s
    }

    fn type_query(s: &mut State, q: &str) {
        for c in q.chars() {
            s.handle_key(key(c));
        }
    }

    #[test]
    fn slash_opens_the_find_prompt_and_swallows_list_keys() {
        let mut s = fleet(&[("1", "/w/alpha", "one"), ("2", "/w/beta", "two")]);
        assert!(s.handle_key(key('/')));
        assert!(s.find.is_some());
        // `x` mid-search is a query character, never an armed kill.
        s.handle_key(key('x'));
        assert_eq!(s.kill_armed, None);
        assert_eq!(s.find.as_ref().unwrap().query, "x");
        s.handle_key(key('q'));
        assert!(!s.hidden, "q must not hide the panel while searching");
    }

    #[test]
    fn typing_narrows_to_the_matching_agents() {
        let mut s = fleet(&[
            ("1", "/w/feat-auth", "refactor tokens"),
            ("2", "/w/feat-parser", "grammar"),
            ("3", "/w/docs", "auth writeup"),
        ]);
        s.handle_key(key('/'));
        type_query(&mut s, "auth");
        let matched: Vec<u32> = s.find_matches().into_iter().map(|i| s.agents[i].pane_id()).collect();
        assert_eq!(matched.len(), 2, "{:?}", matched);
        assert!(matched.contains(&1) && matched.contains(&3), "{:?}", matched);
    }

    #[test]
    fn the_worktree_name_is_matchable() {
        let mut s = fleet(&[
            ("1", "/repos/zj/.worktrees/fuzzy-find", "typing"),
            ("2", "/repos/zj", "main checkout"),
        ]);
        s.handle_key(key('/'));
        type_query(&mut s, "fuzz");
        let m = s.find_matches();
        assert_eq!(m.len(), 1);
        assert_eq!(s.agents[m[0]].pane_id(), 1);
    }

    #[test]
    fn enter_focuses_the_match_under_the_cursor() {
        let mut s = fleet(&[("1", "/w/alpha", "one"), ("2", "/w/beta", "two")]);
        s.handle_key(key('/'));
        type_query(&mut s, "beta");
        s.handle_key(KeyWithModifier::new(BareKey::Enter));
        assert!(s.find.is_none());
        assert_eq!(s.agents[s.selected].pane_id(), 2);
        assert!(s.hidden, "Enter jumps to the pane and hides the panel");
    }

    #[test]
    fn a_resort_between_keystrokes_cannot_redirect_the_jump() {
        let mut s = fleet(&[("1", "/w/alpha-a", "task a"), ("2", "/w/alpha-b", "task b")]);
        s.handle_key(key('/'));
        type_query(&mut s, "alpha");
        // Pin the cursor on the second match.
        s.handle_key(ctrl('j'));
        let pinned = {
            let m = s.find_matches();
            s.agents[m[s.find_cursor_pos(&m).unwrap()]].id.clone()
        };
        // A pipe promotes the other agent to blocked, which re-sorts it first.
        let args: BTreeMap<String, String> = [("pane_id", "1"), ("status", "waiting")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        s.handle_status(&args);
        s.handle_key(KeyWithModifier::new(BareKey::Enter));
        assert_eq!(s.agents[s.selected].id, pinned);
    }

    #[test]
    fn esc_restores_the_full_list_and_the_prior_selection() {
        let mut s = fleet(&[("1", "/w/alpha", "one"), ("2", "/w/beta", "two")]);
        s.selected = 1;
        s.handle_key(key('/'));
        type_query(&mut s, "alpha");
        s.handle_key(KeyWithModifier::new(BareKey::Esc));
        assert!(s.find.is_none());
        assert_eq!(s.selected, 1);
        assert_eq!(s.find_matches().len(), 2, "no prompt means the whole list");
    }

    #[test]
    fn backspace_edits_the_query_and_closes_on_empty() {
        let mut s = fleet(&[("1", "/w/alpha", "one")]);
        s.handle_key(key('/'));
        type_query(&mut s, "ab");
        s.handle_key(KeyWithModifier::new(BareKey::Backspace));
        assert_eq!(s.find.as_ref().unwrap().query, "a");
        s.handle_key(KeyWithModifier::new(BareKey::Backspace));
        assert_eq!(s.find.as_ref().unwrap().query, "");
        s.handle_key(KeyWithModifier::new(BareKey::Backspace));
        assert!(s.find.is_none(), "backspace on an empty query closes the prompt");
    }

    #[test]
    fn a_query_matching_nothing_makes_enter_inert() {
        let mut s = fleet(&[("1", "/w/alpha", "one")]);
        s.handle_key(key('/'));
        type_query(&mut s, "zzz");
        assert!(s.find_matches().is_empty());
        s.handle_key(KeyWithModifier::new(BareKey::Enter));
        assert!(s.find.is_none());
        assert!(!s.hidden, "nothing was focused");
    }

    #[test]
    fn the_cursor_falls_back_to_the_best_match_when_its_agent_leaves() {
        let mut s = fleet(&[("1", "/w/alpha-a", "task a"), ("2", "/w/alpha-b", "task b")]);
        s.handle_key(key('/'));
        type_query(&mut s, "alpha");
        s.handle_key(ctrl('j'));
        let m = s.find_matches();
        let pinned = s.agents[m[s.find_cursor_pos(&m).unwrap()]].id.clone();
        s.agents.retain(|a| a.id != pinned);
        let m = s.find_matches();
        assert_eq!(s.find_cursor_pos(&m), Some(0));
    }

    #[test]
    fn ctrl_j_and_k_move_within_matches_and_wrap() {
        let mut s = fleet(&[("1", "/w/alpha-a", "task a"), ("2", "/w/alpha-b", "task b")]);
        s.handle_key(key('/'));
        type_query(&mut s, "alpha");
        let at = |s: &State| {
            let m = s.find_matches();
            s.agents[m[s.find_cursor_pos(&m).unwrap()]].pane_id()
        };
        let first = at(&s);
        s.handle_key(ctrl('j'));
        let second = at(&s);
        assert_ne!(first, second);
        s.handle_key(ctrl('j'));
        assert_eq!(at(&s), first, "moving past the end wraps");
        s.handle_key(ctrl('k'));
        assert_eq!(at(&s), second);
        // A ctrl-modified letter is movement, never query text.
        assert_eq!(s.find.as_ref().unwrap().query, "alpha");
    }

    #[test]
    fn slash_on_an_empty_list_is_inert() {
        let mut s = State {
            permissions_granted: true,
            session_name: "mob".into(),
            ..Default::default()
        };
        s.handle_key(key('/'));
        assert!(s.find.is_none());
    }

    #[test]
    fn typing_resets_the_cursor_to_the_best_match() {
        let mut s = fleet(&[("1", "/w/alpha-a", "task a"), ("2", "/w/alpha-b", "task b")]);
        s.handle_key(key('/'));
        type_query(&mut s, "alpha");
        s.handle_key(ctrl('j'));
        assert!(s.find.as_ref().unwrap().cursor.is_some());
        s.handle_key(key('-'));
        assert!(
            s.find.as_ref().unwrap().cursor.is_none(),
            "a narrower query invalidates the pinned row"
        );
    }
}
