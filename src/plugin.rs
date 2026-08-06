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
            // Needed only by the install screen, which shells out to init.sh.
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

        // Without this the frame shows the full wasm path, which is both unwieldy
        // and leaks a home directory into a screenshot.
        host::rename_own_pane(PANE_TITLE);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            // A denial here is not fatal. `RunCommands` is in the same request
            // but only the install screen needs it, so the panel stays usable
            // either way and that screen reports the failure itself.
            Event::PermissionRequestResult(_) => {
                self.permissions_granted = true;
                // Only now can `run_command` reach the host: a status read fired
                // from `load` is dropped before the grant lands. The empty
                // screen needs this to decide whether to offer setup.
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

    /// Everything is drawn as a coordinate-positioned Zellij component.
    ///
    /// Nothing here uses `println!`. Components emit a DCS sequence that moves
    /// the cursor itself, so mixing the two puts plain lines wherever the last
    /// component happened to leave the cursor - which is how rows used to land
    /// on top of each other. Explicit `y` for every element instead.
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
        y = self.render_rows(self.install.list_items(), y, width);
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
        y = self.render_rows(self.install.setup_items(), y, width);
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
        self.render_rows(rows, y, width);
    }

    fn render_list(&self, rows: usize, width: usize) {
        let counts = self.counts();
        // Colour each count from the theme, matching the status it summarises.
        let head = format!(
            "zj-agent-mob   {} waiting \u{b7} {} working \u{b7} {} done",
            counts.0, counts.1, counts.2
        );
        let at = "zj-agent-mob   ".len();
        let w = counts.0.to_string().len();
        let wk = counts.1.to_string().len();
        let working_at = at + w + " waiting \u{b7} ".len();
        let done_at = working_at + wk + " working \u{b7} ".len();
        let head = Text::new(head)
            .color_range(Status::Waiting.color_level(), at..at + w)
            .color_range(Status::Working.color_level(), working_at..working_at + wk)
            .color_range(Status::Done.color_level(), done_at..done_at + counts.2.to_string().len());
        print_text_with_coordinates(head, 0, 0, None, None);
        let mut y = self.render_rule(1, width);

        // Needs two rows per agent, plus the chrome above and below.
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
        y = self.render_rows(items, y, width);
        y = self.render_rule(y, width);
        self.render_hints(ribbon::LIST_HINTS, y, width);
    }

    /// One `Text` per grid row, each at its own `y`. Returns the next free row.
    ///
    /// Width is deliberately `None`: a component given an explicit width is
    /// padded out to it, and a row that fills the pane wraps onto the next grid
    /// line - which is what used to make rows land on top of each other.
    fn render_rows(&self, rows: Vec<Text>, y: usize, _width: usize) -> usize {
        let n = rows.len();
        for (i, row) in rows.into_iter().enumerate() {
            print_text_with_coordinates(row, 0, y + i, None, None);
        }
        y + n
    }

    /// Title row. Returns the first free `y` below it.
    fn render_header(&self, subtitle: &str, width: usize) -> usize {
        let text = format!("zj-agent-mob   {}", subtitle);
        let at = "zj-agent-mob   ".len();
        print_text_with_coordinates(Text::new(text).color_range(DIM_LEVEL, at..), 0, 0, None, None);
        self.render_rule(1, width)
    }

    /// A horizontal rule. Returns the next free `y`.
    fn render_rule(&self, y: usize, width: usize) -> usize {
        let rule = "\u{2500}".repeat(width);
        print_text_with_coordinates(Text::new(rule).color_range(DIM_LEVEL, ..), 0, y, None, None);
        y + 1
    }

    /// An error or hint under the body. Returns the next free `y`.
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

    /// Footer key hints as a row of ribbons.
    fn render_hints(&self, hints: &[ribbon::Hint], y: usize, width: usize) {
        // Zellij drops whole ribbon segments that overflow rather than
        // truncating them, so a narrow pane silently loses a key. Plain dimmed
        // text keeps every key visible at the cost of the themed styling.
        if ribbon::ribbon_width(hints) > width {
            let line = truncate(&ribbon::plain_line(hints), width);
            print_text_with_coordinates(Text::new(line).color_range(DIM_LEVEL, ..), 0, y, Some(width), None);
            return;
        }
        let texts: Vec<Text> = hints
            .iter()
            .map(|h| Text::new(h.text()).color_range(0, h.key_range()))
            .collect();
        // `None` width: the coordinates variant applies the width to the FIRST
        // ribbon only, which then expands to fill it and shoves the rest to the
        // far edge. Unsized, each segment is drawn at its natural length.
        print!(
            "{}",
            serialize_ribbon_line_with_coordinates(&texts, 0, y, None, None)
        );
    }
}
