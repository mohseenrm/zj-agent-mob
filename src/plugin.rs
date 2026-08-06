//! Zellij lifecycle and rendering.

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

use crate::state::State;
use crate::status::Status;
use crate::style::DIM_LEVEL;
use crate::util::truncate;
use crate::{content_width, host, ribbon, PANE_TITLE, TICK};

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.popup_on_waiting = configuration
            .get("popup_on_waiting")
            .map(|v| v != "false")
            .unwrap_or(true);

        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            // Only the install screen needs this; it shells out to init.sh.
            PermissionType::RunCommands,
        ]);
        subscribe(&[
            EventType::PaneUpdate,
            EventType::TabUpdate,
            EventType::Key,
            EventType::Timer,
            EventType::PermissionRequestResult,
            EventType::RunCommandResult,
        ]);
        set_selectable(true);

        // Otherwise the frame shows the full wasm path, leaking $HOME.
        host::rename_own_pane(PANE_TITLE);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            // Denial is not fatal, so this ignores the result: only the install
            // screen needs RunCommands, and it reports its own failures.
            Event::PermissionRequestResult(_) => {
                self.permissions_granted = true;
                // `run_command` only reaches the host after the grant; a refresh
                // fired from `load` is silently dropped.
                self.install.refresh();
                true
            }
            Event::Timer(_) => {
                self.timer_running = false;
                self.frame = self.frame.wrapping_add(1);
                self.now += TICK;
                self.arm_timer();
                self.agents.iter().any(|a| a.status == Status::Working)
            }
            Event::PaneUpdate(manifest) => {
                self.reconcile(manifest);
                true
            }
            Event::Key(key) => self.handle_key(key),
            Event::RunCommandResult(exit_code, stdout, stderr, context) => {
                let out = String::from_utf8_lossy(&stdout);
                let err = String::from_utf8_lossy(&stderr);
                self.install.on_command_result(exit_code, &out, &err, &context)
            }
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

    /// Never use `println!` here. Components position the cursor themselves via
    /// a DCS sequence, so a plain line lands wherever the last one left it and
    /// rows collide. Every element gets an explicit `y`.
    fn render(&mut self, rows: usize, cols: usize) {
        if !self.permissions_granted {
            print_text_with_coordinates(
                Text::new("zj-agent-mob needs permissions - press 'y' to grant"),
                0,
                0,
                None,
                None,
            );
            return;
        }

        let width = content_width(cols);

        if self.install.open {
            self.render_install(width);
            return;
        }
        if self.showing_setup() {
            self.render_setup(width);
            return;
        }
        if self.agents.is_empty() {
            self.render_empty(width);
            return;
        }
        self.render_list(rows, width);
    }
}

impl State {
    fn render_install(&self, width: usize) {
        let mut y = self.render_header("install", width);
        y = self.render_rows(self.install.list_items(), y);
        y = self.render_rule(y, width);
        y = self.render_notes(self.install.notes(), y, width);
        self.render_hints(ribbon::INSTALL_HINTS, y, width);
    }

    fn render_setup(&self, width: usize) {
        let mut y = self.render_header("setup", width);
        print_text_with_coordinates(
            Text::new("  Hooks are not installed, so no agent can report status.").color_range(DIM_LEVEL, ..),
            0,
            y,
            None,
            None,
        );
        y += 2;
        y = self.render_rows(self.install.setup_items(), y);
        y = self.render_rule(y, width);
        y = self.render_notes(self.install.notes(), y, width);
        self.render_hints(ribbon::SETUP_HINTS, y, width);
    }

    fn render_empty(&self, width: usize) {
        let y = self.render_header("no agents in this session", width);
        let rows = vec![
            Text::new("  Start claude or codex in a pane; hooks report status here.").color_range(DIM_LEVEL, ..),
            Text::new("  Press i to check and install the hooks.").color_range(DIM_LEVEL, ..),
        ];
        self.render_rows(rows, y);
    }

    fn render_list(&self, rows: usize, width: usize) {
        let (waiting, working, done) = self.counts();
        let parts = [
            (waiting, "waiting", Status::Waiting),
            (working, "working", Status::Working),
            (done, "done", Status::Done),
        ];

        // Built up incrementally so each count's colour range tracks the digits
        // actually written; a two-digit count shifts everything after it.
        let mut head = "zj-agent-mob   ".to_string();
        let mut ranges = Vec::new();
        for (i, (n, label, status)) in parts.into_iter().enumerate() {
            if i > 0 {
                head.push_str(" \u{b7} ");
            }
            let digits = n.to_string();
            ranges.push((status.color_level(), head.len()..head.len() + digits.len()));
            head.push_str(&digits);
            head.push(' ');
            head.push_str(label);
        }
        let head = ranges
            .into_iter()
            .fold(Text::new(head), |t, (level, r)| t.color_range(level, r));
        print_text_with_coordinates(head, 0, 0, None, None);
        let mut y = self.render_rule(1, width);

        // A detail line per agent needs two rows each, plus header and footer.
        let detail_lines = rows >= 4 + self.agents.len() * 2 && width >= 60;
        let show_cwd = width >= 50;

        let mut items = Vec::new();
        for (i, agent) in self.agents.iter().enumerate() {
            let icon = self.icon_for(agent);
            items.push(agent.list_item(i, i == self.selected, icon, self.now, width, show_cwd));
            if detail_lines {
                items.push(agent.detail_item(self.kill_armed == Some(agent.pane_id), width));
            }
        }
        y = self.render_rows(items, y);
        y = self.render_rule(y, width);
        self.render_hints(ribbon::LIST_HINTS, y, width);
    }

    /// One row per `Text`, each at its own `y`. Returns the next free row.
    ///
    /// Passes width `None` throughout: a sized component is padded out to that
    /// width, and a row filling the pane wraps and eats the line below it.
    fn render_rows(&self, rows: Vec<Text>, y: usize) -> usize {
        let n = rows.len();
        for (i, row) in rows.into_iter().enumerate() {
            print_text_with_coordinates(row, 0, y + i, None, None);
        }
        y + n
    }

    /// Returns the next free `y`.
    fn render_header(&self, subtitle: &str, width: usize) -> usize {
        let text = format!("zj-agent-mob   {}", subtitle);
        let at = "zj-agent-mob   ".len();
        print_text_with_coordinates(Text::new(text).color_range(DIM_LEVEL, at..), 0, 0, None, None);
        self.render_rule(1, width)
    }

    /// Returns the next free `y`.
    fn render_rule(&self, y: usize, width: usize) -> usize {
        let rule = "\u{2500}".repeat(width);
        print_text_with_coordinates(Text::new(rule).color_range(DIM_LEVEL, ..), 0, y, None, None);
        y + 1
    }

    /// Returns the next free `y`.
    fn render_notes(&self, note: Option<(String, bool)>, y: usize, width: usize) -> usize {
        match note {
            Some((msg, is_error)) => {
                let text = Text::new(format!("  {}", truncate(&msg, width.saturating_sub(2))));
                let text = if is_error {
                    text.error_color_range(..)
                } else {
                    text.color_range(DIM_LEVEL, ..)
                };
                print_text_with_coordinates(text, 0, y, None, None);
                y + 1
            }
            None => y,
        }
    }

    fn render_hints(&self, hints: &[ribbon::Hint], y: usize, width: usize) {
        // Overflowing ribbons lose whole segments rather than truncating, so a
        // narrow pane would silently drop a key. Plain text keeps them all.
        if ribbon::ribbon_width(hints) > width {
            let line = truncate(&ribbon::plain_line(hints), width);
            print_text_with_coordinates(Text::new(line).color_range(DIM_LEVEL, ..), 0, y, Some(width), None);
            return;
        }
        // `selected` alternates the background so adjacent chips stay distinct.
        let texts: Vec<Text> = hints
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let t = Text::new(h.text()).color_range(0, h.key_range());
                if i % 2 == 1 {
                    t.selected()
                } else {
                    t
                }
            })
            .collect();
        // Width `None`: the sized variant stretches the FIRST ribbon to fill it
        // and shoves the rest to the far edge.
        print!("{}", serialize_ribbon_line_with_coordinates(&texts, 0, y, None, None));
    }
}
