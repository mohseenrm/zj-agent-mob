//! Zellij plugin lifecycle: permissions, event subscriptions, and rendering.

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

use crate::state::State;
use crate::status::Status;
use crate::style::{BLUE, BOLD, DIM, GREEN, GREY, RED, RESET};
use crate::TICK;

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.popup_on_waiting = configuration
            .get("popup_on_waiting")
            .map(|v| v != "false")
            .unwrap_or(true);

        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
        ]);
        subscribe(&[
            EventType::PaneUpdate,
            EventType::TabUpdate,
            EventType::Key,
            EventType::Timer,
            EventType::PermissionRequestResult,
        ]);
        set_selectable(true);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(status) => {
                self.permissions_granted = matches!(status, PermissionStatus::Granted);
                true
            }
            Event::Timer(_) => {
                self.timer_running = false;
                self.frame = self.frame.wrapping_add(1);
                self.now += TICK;
                self.arm_timer();
                // Only repaint if something is actually animating.
                self.agents.iter().any(|a| a.status == Status::Working)
            }
            Event::PaneUpdate(manifest) => {
                self.reconcile(manifest);
                true
            }
            Event::Key(key) => self.handle_key(key),
            _ => false,
        }
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        match pipe_message.name.as_str() {
            "agent-status" => self.handle_status(&pipe_message.args),
            "agent-label" => self.handle_label(&pipe_message.args),
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        if !self.permissions_granted {
            println!("zj-agent-mob needs permissions - press 'y' to grant");
            return;
        }

        let counts = self.counts();
        if self.agents.is_empty() {
            println!("{}zj-agent-mob{}   no agents in this session", BOLD, RESET);
            println!("{}{}{}", DIM, "\u{2500}".repeat(cols.min(72)), RESET);
            println!();
            println!("{}  Start claude or codex in a pane; hooks report status here.{}", GREY, RESET);
            println!("{}  Run ./init.sh once to install the hooks.{}", GREY, RESET);
            return;
        }

        println!(
            "{}zj-agent-mob{}   {}{} waiting{} \u{b7} {}{} working{} \u{b7} {}{} done{}",
            BOLD, RESET,
            if counts.0 > 0 { RED } else { GREY }, counts.0, RESET,
            if counts.1 > 0 { BLUE } else { GREY }, counts.1, RESET,
            if counts.2 > 0 { GREEN } else { GREY }, counts.2, RESET,
        );
        println!("{}{}{}", DIM, "\u{2500}".repeat(cols.min(72)), RESET);

        // Two lines per agent plus header/footer; collapse to one when short.
        let detail_lines = rows >= 4 + self.agents.len() * 2 && cols >= 60;
        let show_cwd = cols >= 50;

        for (i, agent) in self.agents.iter().enumerate() {
            let selected = i == self.selected;
            let icon = self.icon_for(agent);
            println!(
                "{}",
                agent.styled_row(i, selected, icon, self.now, cols, show_cwd)
            );
            if detail_lines {
                let armed = self.kill_armed == Some(agent.pane_id);
                println!("{}{}{}", DIM, agent.detail_line(armed, cols), RESET);
            }
        }

        println!("{}{}{}", DIM, "\u{2500}".repeat(cols.min(72)), RESET);
        println!(
            "{} \u{21b5} jump  1-9 quick  x kill  d dismiss  q hide{}",
            GREY, RESET
        );
    }
}
