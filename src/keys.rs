//! Keyboard: selection, jump-to-pane, two-step kill.

use zellij_tile::prelude::*;

use crate::host;
use crate::install::Target;
use crate::state::State;
use crate::status::Status;

impl State {
    /// Keys for the install screen. Separate from the agent list so a stray key
    /// there can never kill a pane.
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

    pub(crate) fn focus_selected(&mut self) {
        if let Some(agent) = self.agents.get(self.selected) {
            let pane_id = agent.pane_id;
            if let Some(a) = self.agents.iter_mut().find(|a| a.pane_id == pane_id) {
                if a.status == Status::Done {
                    a.status = Status::Idle;
                    a.status_since = self.now;
                }
            }
            host::focus_terminal_pane(pane_id, true, false);
            self.hidden = true;
            self.sort_agents();
            host::hide_self();
        }
    }

    pub(crate) fn handle_key(&mut self, key: KeyWithModifier) -> bool {
        if self.install.open {
            return self.handle_install_key(key);
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
                if !self.agents.is_empty() {
                    self.selected = (self.selected + 1) % self.agents.len();
                }
                self.kill_armed = None;
                true
            }
            BareKey::Char('k') | BareKey::Up => {
                if !self.agents.is_empty() {
                    self.selected = if self.selected == 0 {
                        self.agents.len() - 1
                    } else {
                        self.selected - 1
                    };
                }
                self.kill_armed = None;
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
                    let pane_id = agent.pane_id;
                    if self.kill_armed == Some(pane_id) {
                        host::close_terminal_pane(pane_id);
                        self.agents.retain(|a| a.pane_id != pane_id);
                        self.kill_armed = None;
                        self.clamp_selection();
                    } else {
                        host::send_sigint_to_pane_id(PaneId::Terminal(pane_id));
                        self.kill_armed = Some(pane_id);
                    }
                }
                true
            }
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
    use crate::install::InstallState;
    use std::collections::BTreeMap;

    fn key(c: char) -> KeyWithModifier {
        KeyWithModifier::new(BareKey::Char(c))
    }

    fn state_with_one_agent() -> State {
        let mut s = State {
            permissions_granted: true,
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
    /// The two handlers must stay separate.
    #[test]
    fn x_on_the_install_screen_does_not_touch_agents() {
        let mut s = state_with_one_agent();
        s.handle_key(key('i'));
        s.handle_key(key('x'));
        assert_eq!(s.agents.len(), 1, "x must not kill a pane from the install screen");
        assert_eq!(s.kill_armed, None, "x must not arm a kill from the install screen");
        assert_eq!(s.install.target_at_cursor(), crate::install::Target::Codex);
    }

    /// Opening the install screen must disarm a pending kill, so a queued `x`
    /// can't close a pane after the user has navigated away.
    #[test]
    fn opening_install_screen_disarms_a_pending_kill() {
        let mut s = state_with_one_agent();
        s.handle_key(key('x'));
        assert_eq!(s.kill_armed, Some(7));
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

    /// Toggling needs a known state; from Unknown it must be a no-op so a
    /// missing installer can't make the panel claim work is in flight.
    #[test]
    fn toggling_from_unknown_state_is_inert() {
        let mut s = state_with_one_agent();
        s.handle_key(key('i'));
        s.handle_key(key('c'));
        assert_eq!(s.install.state(crate::install::Target::Claude), InstallState::Unknown);
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
