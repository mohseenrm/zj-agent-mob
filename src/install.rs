//! The install screen.
//!
//! WASI gives the plugin no `$HOME` and no filesystem, so it cannot read
//! `settings.json`. All state comes from shelling out to the installer copy at
//! `~/.config/zj-agent-mob/install.sh`, which reports `key=state` lines.

use std::collections::BTreeMap;

use zellij_tile::prelude::Text;

use crate::host;
use crate::style::{chars, DIM_LEVEL};
use crate::util::wrap;

/// `$HOME` is expanded by the shell, not us: the plugin has no environment.
pub(crate) const INSTALLER: &str = "$HOME/.config/zj-agent-mob/install.sh";

/// Tags `run_command` results so they are not confused with another command's.
pub(crate) const CTX_KEY: &str = "zj-agent-mob";
pub(crate) const CTX_STATUS: &str = "install-status";
pub(crate) const CTX_ACTION: &str = "install-action";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Target {
    #[default]
    Claude,
    Codex,
    Plugin,
}

impl Target {
    pub(crate) const ALL: [Target; 3] = [Target::Claude, Target::Codex, Target::Plugin];

    /// The `init.sh` target name, and the key it reports in `status` output.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Target::Claude => "claude",
            Target::Codex => "codex",
            Target::Plugin => "plugin",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Target::Claude => "Claude Code hooks",
            Target::Codex => "Codex hooks",
            Target::Plugin => "Plugin wasm",
        }
    }

    pub(crate) fn hotkey(self) -> char {
        match self {
            Target::Claude => 'c',
            Target::Codex => 'x',
            Target::Plugin => 'p',
        }
    }
}

/// Quick actions for the empty screen. Not the same list as `Target`: the
/// plugin wasm is already running, so offering to install it would be noise.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SetupAction {
    Claude,
    Codex,
    Both,
    Quit,
}

impl SetupAction {
    pub(crate) const ALL: [SetupAction; 4] = [
        SetupAction::Claude,
        SetupAction::Codex,
        SetupAction::Both,
        SetupAction::Quit,
    ];

    pub(crate) fn hotkey(self) -> char {
        match self {
            SetupAction::Claude => '1',
            SetupAction::Codex => '2',
            SetupAction::Both => '3',
            SetupAction::Quit => 'q',
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            SetupAction::Claude => "Install for Claude Code",
            SetupAction::Codex => "Install for Codex",
            SetupAction::Both => "Install for both",
            SetupAction::Quit => "Quit",
        }
    }

    /// Empty for `Quit`.
    fn targets(self) -> &'static [Target] {
        match self {
            SetupAction::Claude => &[Target::Claude],
            SetupAction::Codex => &[Target::Codex],
            SetupAction::Both => &[Target::Claude, Target::Codex],
            SetupAction::Quit => &[],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum InstallState {
    #[default]
    Unknown,
    Installed,
    Absent,
    /// A command is in flight for this target.
    Busy,
}

impl InstallState {
    fn icon(self) -> &'static str {
        match self {
            InstallState::Installed => "\u{2713}",
            InstallState::Absent => "\u{25cb}",
            InstallState::Busy => "\u{2219}",
            InstallState::Unknown => "?",
        }
    }

    fn text(self) -> &'static str {
        match self {
            InstallState::Installed => "installed",
            InstallState::Absent => "not installed",
            InstallState::Busy => "working...",
            InstallState::Unknown => "unknown",
        }
    }
}

/// State for the install screen. Lives on `State` and is inert until opened.
#[derive(Default)]
pub(crate) struct Install {
    pub(crate) open: bool,
    pub(crate) selected: usize,
    states: [InstallState; 3],
    pub(crate) error: Option<String>,
    pub(crate) missing_installer: bool,
    pub(crate) setup_selected: usize,
    /// Gates the setup screen so it cannot flash before the first status read.
    pub(crate) status_known: bool,
}

impl Install {
    pub(crate) fn state(&self, t: Target) -> InstallState {
        self.states[t as usize]
    }

    fn set(&mut self, t: Target, s: InstallState) {
        self.states[t as usize] = s;
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        self.selected = wrap(self.selected, delta, Target::ALL.len());
    }

    pub(crate) fn target_at_cursor(&self) -> Target {
        Target::ALL[self.selected.min(Target::ALL.len() - 1)]
    }

    /// Also how a missing installer is detected.
    pub(crate) fn refresh(&mut self) {
        self.error = None;
        self.dispatch_status();
    }

    /// Unlike `refresh`, leaves `error` intact so a failed action's message
    /// survives the re-read that follows it.
    fn dispatch_status(&self) {
        host::run_command(&["sh", "-c", &format!("{} status", INSTALLER)], ctx(CTX_STATUS, None));
    }

    pub(crate) fn toggle(&mut self, t: Target) {
        // Unknown means a refresh is in flight or the installer is missing, so
        // there is no direction to toggle in.
        let verb = match self.state(t) {
            InstallState::Installed => "uninstall",
            InstallState::Absent => "install",
            InstallState::Busy | InstallState::Unknown => return,
        };
        self.error = None;
        self.set(t, InstallState::Busy);
        host::run_command(
            &["sh", "-c", &format!("{} {} {}", INSTALLER, verb, t.key())],
            ctx(CTX_ACTION, Some(t)),
        );
    }

    /// True when neither agent is hooked, so nothing can ever report status.
    /// One agent installed is a deliberate choice, not a broken setup.
    pub(crate) fn needs_setup(&self) -> bool {
        self.status_known
            && !self.missing_installer
            && self.state(Target::Claude) != InstallState::Installed
            && self.state(Target::Codex) != InstallState::Installed
    }

    pub(crate) fn setup_busy(&self) -> bool {
        self.state(Target::Claude) == InstallState::Busy || self.state(Target::Codex) == InstallState::Busy
    }

    pub(crate) fn move_setup_selection(&mut self, delta: isize) {
        self.setup_selected = wrap(self.setup_selected, delta, SetupAction::ALL.len());
    }

    pub(crate) fn setup_at_cursor(&self) -> SetupAction {
        SetupAction::ALL[self.setup_selected.min(SetupAction::ALL.len() - 1)]
    }

    /// Returns false for `Quit`, which the caller handles by hiding the panel.
    pub(crate) fn run_setup(&mut self, action: SetupAction) -> bool {
        let targets = action.targets();
        if targets.is_empty() {
            return false;
        }
        // Two shells writing the same settings file would race.
        if self.setup_busy() {
            return true;
        }
        self.error = None;
        let keys: Vec<&str> = targets.iter().map(|t| t.key()).collect();
        for t in targets {
            self.set(*t, InstallState::Busy);
        }
        host::run_command(
            &["sh", "-c", &format!("{} install {}", INSTALLER, keys.join(" "))],
            ctx(CTX_ACTION, None),
        );
        true
    }

    pub(crate) fn setup_items(&self) -> Vec<Text> {
        SetupAction::ALL
            .into_iter()
            .enumerate()
            .map(|(i, a)| {
                let marker = if i == self.setup_selected { "\u{25b6}" } else { " " };
                let text = format!("{} {}  {}", marker, a.hotkey(), a.label());
                // Character offset: the cursor marker is multi-byte.
                let at = chars(marker) + 1;
                let text = Text::new(text).color_range(0, at..at + 1);
                if i == self.setup_selected {
                    text.selected()
                } else {
                    text
                }
            })
            .collect()
    }

    /// The message under the body, and whether it renders as an error.
    pub(crate) fn notes(&self) -> Option<(String, bool)> {
        if self.setup_busy() {
            Some(("installing...".to_string(), false))
        } else if let Some(err) = &self.error {
            Some((err.clone(), true))
        } else if self.missing_installer {
            Some((
                "Installer not found. Run ./init.sh from the repo once.".to_string(),
                false,
            ))
        } else {
            None
        }
    }

    /// Returns true if the panel should redraw.
    pub(crate) fn on_command_result(
        &mut self,
        exit_code: Option<i32>,
        stdout: &str,
        stderr: &str,
        context: &BTreeMap<String, String>,
    ) -> bool {
        match context.get(CTX_KEY).map(|s| s.as_str()) {
            Some(CTX_STATUS) => {
                self.status_known = true;
                if exit_code == Some(0) {
                    self.missing_installer = false;
                    self.apply_status(stdout);
                } else {
                    // `sh -c` exits 127 when the installer isn't there.
                    self.missing_installer = true;
                    for t in Target::ALL {
                        self.set(t, InstallState::Unknown);
                    }
                    if exit_code != Some(127) {
                        self.error = first_line(stderr);
                    }
                }
                true
            }
            Some(CTX_ACTION) => {
                if exit_code != Some(0) {
                    self.error = first_line(stderr).or_else(|| first_line(stdout));
                }
                // The installer can partially succeed, so re-read rather than
                // assume. Not `refresh()`, which would clear the error above.
                self.dispatch_status();
                true
            }
            _ => false,
        }
    }

    /// Parse `key=state` lines from `init.sh status`.
    fn apply_status(&mut self, stdout: &str) {
        for line in stdout.lines() {
            let Some((key, value)) = line.trim().split_once('=') else {
                continue;
            };
            let state = match value {
                "installed" => InstallState::Installed,
                "absent" => InstallState::Absent,
                _ => continue,
            };
            if let Some(t) = Target::ALL.into_iter().find(|t| t.key() == key) {
                self.set(t, state);
            }
        }
    }

    pub(crate) fn list_items(&self) -> Vec<Text> {
        Target::ALL
            .into_iter()
            .enumerate()
            .map(|(i, t)| {
                let st = self.state(t);
                let marker = if i == self.selected { "\u{25b6}" } else { " " };
                let text = format!(
                    "{} {}  {:<20} {} {}",
                    marker,
                    t.hotkey(),
                    t.label(),
                    st.icon(),
                    st.text()
                );
                // Character offsets: the cursor marker is multi-byte.
                let key_at = chars(marker) + 1;
                let icon_at = key_at + 1 + 2 + 20 + 1;
                let mut item = Text::new(text).color_range(0, key_at..key_at + 1);
                item = match st {
                    InstallState::Installed => item.success_color_range(icon_at..),
                    InstallState::Unknown => item.error_color_range(icon_at..),
                    InstallState::Absent | InstallState::Busy => item.color_range(DIM_LEVEL, icon_at..),
                };
                if i == self.selected {
                    item = item.selected();
                }
                item
            })
            .collect()
    }
}

fn ctx(kind: &str, target: Option<Target>) -> BTreeMap<String, String> {
    let mut c = BTreeMap::new();
    c.insert(CTX_KEY.to_string(), kind.to_string());
    if let Some(t) = target {
        c.insert("target".to_string(), t.key().to_string());
    }
    c
}

/// Installer errors are multi-line; the panel has room for the first line only.
fn first_line(s: &str) -> Option<String> {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.trim_start_matches("error: ").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testing::{is_selected, item_text};

    fn ctx_of(kind: &str) -> BTreeMap<String, String> {
        let mut c = BTreeMap::new();
        c.insert(CTX_KEY.to_string(), kind.to_string());
        c
    }

    #[test]
    fn parses_status_output() {
        let mut i = Install::default();
        i.on_command_result(
            Some(0),
            "claude=installed\ncodex=absent\nplugin=installed\nhook=installed\n",
            "",
            &ctx_of(CTX_STATUS),
        );
        assert_eq!(i.state(Target::Claude), InstallState::Installed);
        assert_eq!(i.state(Target::Codex), InstallState::Absent);
        assert_eq!(i.state(Target::Plugin), InstallState::Installed);
        assert!(!i.missing_installer);
    }

    #[test]
    fn garbage_status_lines_are_skipped() {
        let mut i = Install::default();
        i.on_command_result(
            Some(0),
            "claude=installed\nnonsense\nbogus=maybe\n=\n",
            "",
            &ctx_of(CTX_STATUS),
        );
        assert_eq!(i.state(Target::Claude), InstallState::Installed);
        assert_eq!(
            i.state(Target::Codex),
            InstallState::Unknown,
            "unmentioned targets stay unknown"
        );
    }

    /// 127 is an expected state with its own hint, not an error to dump raw.
    #[test]
    fn missing_installer_is_detected_without_an_error_message() {
        let mut i = Install::default();
        i.on_command_result(Some(127), "", "sh: install.sh: not found", &ctx_of(CTX_STATUS));
        assert!(i.missing_installer);
        assert!(i.error.is_none(), "127 gets the hint, not the raw stderr");
        for t in Target::ALL {
            assert_eq!(i.state(t), InstallState::Unknown);
        }
        let (note, is_error) = i.notes().expect("a missing installer needs a hint");
        assert!(note.contains("Run ./init.sh"));
        assert!(!is_error, "a missing installer is an expected state, not an error");
    }

    #[test]
    fn non_127_status_failure_surfaces_stderr() {
        let mut i = Install::default();
        i.on_command_result(Some(1), "", "error: jq is required\nmore detail", &ctx_of(CTX_STATUS));
        assert_eq!(
            i.error.as_deref(),
            Some("jq is required"),
            "strips prefix, first line only"
        );
    }

    #[test]
    fn failed_action_records_error() {
        let mut i = Install::default();
        i.on_command_result(Some(1), "", "error: plugin not built", &ctx_of(CTX_ACTION));
        assert_eq!(i.error.as_deref(), Some("plugin not built"));
    }

    #[test]
    fn unrelated_command_results_are_ignored() {
        let mut i = Install::default();
        let mut other = BTreeMap::new();
        other.insert("something".to_string(), "else".to_string());
        assert!(!i.on_command_result(Some(0), "claude=installed", "", &other));
        assert_eq!(i.state(Target::Claude), InstallState::Unknown);
    }

    #[test]
    fn toggle_marks_busy_only_from_a_known_state() {
        let mut i = Install::default();
        i.toggle(Target::Claude);
        assert_eq!(
            i.state(Target::Claude),
            InstallState::Unknown,
            "unknown state must not guess a direction"
        );

        i.on_command_result(Some(0), "claude=absent\n", "", &ctx_of(CTX_STATUS));
        i.toggle(Target::Claude);
        assert_eq!(i.state(Target::Claude), InstallState::Busy);

        // A second press while in flight must not fire another command.
        i.toggle(Target::Claude);
        assert_eq!(i.state(Target::Claude), InstallState::Busy);
    }

    #[test]
    fn setup_is_offered_only_when_neither_agent_is_hooked() {
        let mut i = Install::default();
        assert!(!i.needs_setup(), "no status read yet: stay quiet");

        i.on_command_result(Some(0), "claude=absent\ncodex=absent\n", "", &ctx_of(CTX_STATUS));
        assert!(i.needs_setup());

        i.on_command_result(Some(0), "claude=installed\ncodex=absent\n", "", &ctx_of(CTX_STATUS));
        assert!(!i.needs_setup(), "one agent hooked is a choice, not a broken setup");

        i.on_command_result(Some(127), "", "not found", &ctx_of(CTX_STATUS));
        assert!(!i.needs_setup(), "a missing installer cannot be fixed by these actions");
    }

    #[test]
    fn setup_selection_wraps_and_covers_four_actions() {
        let mut i = Install::default();
        assert_eq!(SetupAction::ALL.len(), 4);
        assert_eq!(i.setup_at_cursor(), SetupAction::Claude);
        i.move_setup_selection(-1);
        assert_eq!(i.setup_at_cursor(), SetupAction::Quit);
        i.move_setup_selection(1);
        assert_eq!(i.setup_at_cursor(), SetupAction::Claude);
    }

    /// A hotkey sets the cursor via `SetupAction as usize`.
    #[test]
    fn setup_discriminants_line_up_with_all() {
        for (i, a) in SetupAction::ALL.into_iter().enumerate() {
            assert_eq!(a as usize, i, "{:?} must index ALL", a);
        }
    }

    #[test]
    fn every_setup_action_has_a_distinct_hotkey() {
        let mut keys: Vec<char> = SetupAction::ALL.iter().map(|a| a.hotkey()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), SetupAction::ALL.len());
    }

    #[test]
    fn quit_runs_nothing_and_reports_it() {
        let mut i = Install::default();
        assert!(!i.run_setup(SetupAction::Quit), "quit is the caller's job");
        assert!(!i.setup_busy());
    }

    #[test]
    fn install_both_marks_both_targets_busy() {
        let mut i = Install::default();
        assert!(i.run_setup(SetupAction::Both));
        assert_eq!(i.state(Target::Claude), InstallState::Busy);
        assert_eq!(i.state(Target::Codex), InstallState::Busy);
        assert_eq!(i.state(Target::Plugin), InstallState::Unknown, "plugin is untouched");
    }

    /// Unlike the row toggle, setup must fire before any status read.
    #[test]
    fn setup_installs_without_a_prior_status_read() {
        let mut i = Install::default();
        assert!(i.run_setup(SetupAction::Claude));
        assert_eq!(i.state(Target::Claude), InstallState::Busy);
    }

    #[test]
    fn a_second_setup_press_while_busy_is_ignored() {
        let mut i = Install::default();
        i.run_setup(SetupAction::Claude);
        i.error = Some("stale".into());
        i.run_setup(SetupAction::Codex);
        assert_eq!(
            i.state(Target::Codex),
            InstallState::Unknown,
            "must not launch a second writer against the same settings file"
        );
        assert_eq!(i.error.as_deref(), Some("stale"), "the in-flight run is left alone");
    }

    #[test]
    fn setup_action_failure_surfaces_and_clears_busy() {
        let mut i = Install::default();
        i.run_setup(SetupAction::Both);
        i.on_command_result(Some(1), "", "error: jq is required", &ctx_of(CTX_ACTION));
        assert_eq!(i.error.as_deref(), Some("jq is required"));
        // The follow-up status read is what actually clears Busy.
        i.on_command_result(Some(0), "claude=absent\ncodex=absent\n", "", &ctx_of(CTX_STATUS));
        assert!(!i.setup_busy());
        assert!(i.needs_setup(), "still unhooked, so keep offering");
    }

    /// Pins the screen the user lands on, so a silent change fails here.
    #[test]
    fn setup_screen_renders_four_numbered_actions() {
        let i = Install::default();
        let texts: Vec<String> = i.setup_items().iter().map(item_text).collect();
        assert_eq!(
            texts,
            vec![
                "\u{25b6} 1  Install for Claude Code",
                "  2  Install for Codex",
                "  3  Install for both",
                "  q  Quit",
            ]
        );
    }

    #[test]
    fn exactly_one_setup_item_is_selected() {
        let mut i = Install::default();
        for want in 0..SetupAction::ALL.len() {
            i.setup_selected = want;
            let flags: Vec<bool> = i.setup_items().iter().map(is_selected).collect();
            assert_eq!(
                flags.iter().filter(|f| **f).count(),
                1,
                "exactly one row must be selected"
            );
            assert!(flags[want], "row {} must be the selected one", want);
        }
    }

    #[test]
    fn install_rows_show_state_for_every_target() {
        let mut i = Install::default();
        i.on_command_result(
            Some(0),
            "claude=installed\ncodex=absent\nplugin=absent\n",
            "",
            &ctx_of(CTX_STATUS),
        );
        let texts: Vec<String> = i.list_items().iter().map(item_text).collect();
        assert_eq!(texts.len(), Target::ALL.len());
        assert!(texts[0].contains("Claude Code hooks") && texts[0].contains("installed"));
        assert!(texts[1].contains("Codex hooks") && texts[1].contains("not installed"));
    }

    #[test]
    fn selection_wraps_both_directions() {
        let mut i = Install::default();
        i.move_selection(-1);
        assert_eq!(i.target_at_cursor(), Target::Plugin, "up from the top wraps to the end");
        i.move_selection(1);
        assert_eq!(i.target_at_cursor(), Target::Claude);
        for _ in 0..Target::ALL.len() {
            i.move_selection(1);
        }
        assert_eq!(
            i.target_at_cursor(),
            Target::Claude,
            "a full cycle returns to the start"
        );
    }

    #[test]
    fn every_target_has_a_distinct_hotkey() {
        let mut keys: Vec<char> = Target::ALL.iter().map(|t| t.hotkey()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), Target::ALL.len());
    }

    /// An embedded newline would desync every coordinate below the row.
    #[test]
    fn rows_are_single_line() {
        let mut i = Install::default();
        i.on_command_result(
            Some(0),
            "claude=installed\ncodex=absent\nplugin=absent\n",
            "",
            &ctx_of(CTX_STATUS),
        );
        for item in i.list_items().iter().chain(i.setup_items().iter()) {
            assert!(!item_text(item).contains('\n'));
        }
    }
}
