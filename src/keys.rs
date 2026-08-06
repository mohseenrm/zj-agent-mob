//! Keyboard: selection, jump-to-pane, two-step kill.

use zellij_tile::prelude::*;

use crate::host;
use crate::state::State;
use crate::status::Status;

impl State {
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
        match key.bare_key {
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
