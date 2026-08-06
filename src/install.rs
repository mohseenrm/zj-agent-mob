//! The install screen: hook install state, and the actions that change it.
//!
//! The plugin runs in WASI with no access to `$HOME`, so it cannot read
//! `settings.json` itself. Everything here is driven by shelling out to the
//! installer that `init.sh` copies to `~/.config/zj-agent-mob/install.sh`,
//! which reports state as `key=state` lines and performs the mutations.

use std::collections::BTreeMap;

use crate::host;
use crate::style::{BOLD, DIM, GREEN, GREY, RED, RESET, SEL_BG};
use crate::util::truncate;

/// Where `init.sh` puts its self-copy. `$HOME` is expanded by the shell, not us:
/// the plugin has no environment to read it from.
pub(crate) const INSTALLER: &str = "$HOME/.config/zj-agent-mob/install.sh";

/// Marks a `run_command` result as belonging to the install screen, so status
/// output is never confused with some other command's.
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

    /// The key that toggles this row.
    pub(crate) fn hotkey(self) -> char {
        match self {
            Target::Claude => 'c',
            Target::Codex => 'x',
            Target::Plugin => 'p',
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

    fn ansi(self) -> &'static str {
        match self {
            InstallState::Installed => GREEN,
            InstallState::Absent => GREY,
            InstallState::Busy => GREY,
            InstallState::Unknown => RED,
        }
    }
}

/// State for the install screen. Lives on `State` and is inert until opened.
#[derive(Default)]
pub(crate) struct Install {
    pub(crate) open: bool,
    pub(crate) selected: usize,
    states: [InstallState; 3],
    /// Last error from the installer, shown under the rows.
    pub(crate) error: Option<String>,
    /// True once the installer has been found to be missing.
    pub(crate) missing_installer: bool,
}

impl Install {
    pub(crate) fn state(&self, t: Target) -> InstallState {
        self.states[t as usize]
    }

    fn set(&mut self, t: Target, s: InstallState) {
        self.states[t as usize] = s;
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let n = Target::ALL.len() as isize;
        self.selected = (((self.selected as isize + delta) % n + n) % n) as usize;
    }

    pub(crate) fn target_at_cursor(&self) -> Target {
        Target::ALL[self.selected.min(Target::ALL.len() - 1)]
    }

    /// Ask the installer for current state. Also how we learn it is missing.
    pub(crate) fn refresh(&mut self) {
        self.error = None;
        self.dispatch_status();
    }

    /// Refresh without clearing `error`, so a failed action's message survives
    /// the state re-read that follows it.
    fn dispatch_status(&self) {
        host::run_command(&["sh", "-c", &format!("{} status", INSTALLER)], ctx(CTX_STATUS, None));
    }

    /// Install or uninstall one target, based on its current state.
    pub(crate) fn toggle(&mut self, t: Target) {
        // Unknown state means we don't know which direction to go; a refresh is
        // in flight or the installer is missing, so do nothing rather than guess.
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

    /// Handle a finished `run_command`. Returns true if the panel should redraw.
    pub(crate) fn on_command_result(
        &mut self,
        exit_code: Option<i32>,
        stdout: &str,
        stderr: &str,
        context: &BTreeMap<String, String>,
    ) -> bool {
        match context.get(CTX_KEY).map(|s| s.as_str()) {
            Some(CTX_STATUS) => {
                if exit_code == Some(0) {
                    self.missing_installer = false;
                    self.apply_status(stdout);
                } else {
                    // `sh -c` exits 127 when the installer isn't there. Anything
                    // else is a real failure worth surfacing verbatim.
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
                // Re-read rather than assuming the toggle landed: the installer
                // can succeed partially (e.g. hooks written, plugin not built).
                // Not `refresh()`: that would clear the error we just recorded.
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

    /// The screen body, one line per element. The caller emits the chrome.
    pub(crate) fn lines(&self, cols: usize) -> Vec<String> {
        let mut out = Vec::new();
        for (i, t) in Target::ALL.into_iter().enumerate() {
            let st = self.state(t);
            let marker = if i == self.selected { "\u{25b6}" } else { " " };
            let plain = format!(
                "{} {}  {:<20} {} {}",
                marker,
                t.hotkey(),
                t.label(),
                st.icon(),
                st.text()
            );
            let mut line = truncate(&plain, cols);
            // Colour after truncation so the escapes are never cut in half.
            line = line.replacen(st.icon(), &format!("{}{}{}", st.ansi(), st.icon(), RESET), 1);
            if i == self.selected {
                line = format!("{}{}{}", SEL_BG, line, RESET);
            }
            out.push(line);
        }
        if self.missing_installer {
            let hint = truncate("  Installer not found. Run ./init.sh from the repo once.", cols);
            out.push(String::new());
            out.push(format!("{}{}{}", GREY, hint, RESET));
        } else if let Some(err) = &self.error {
            out.push(String::new());
            out.push(format!("{}  {}{}", RED, truncate(err, cols.saturating_sub(2)), RESET));
        }
        out
    }

    pub(crate) fn header(&self) -> String {
        format!("{}zj-agent-mob{}   {}install{}", BOLD, RESET, DIM, RESET)
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

    /// 127 from `sh -c` means the installer isn't on disk. That is an expected
    /// state with its own hint, not an error to dump raw.
    #[test]
    fn missing_installer_is_detected_without_an_error_message() {
        let mut i = Install::default();
        i.on_command_result(Some(127), "", "sh: install.sh: not found", &ctx_of(CTX_STATUS));
        assert!(i.missing_installer);
        assert!(i.error.is_none(), "127 gets the hint, not the raw stderr");
        for t in Target::ALL {
            assert_eq!(i.state(t), InstallState::Unknown);
        }
        assert!(i.lines(80).iter().any(|l| l.contains("Run ./init.sh")));
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

    #[test]
    fn rows_stay_single_line_and_within_cols() {
        let mut i = Install::default();
        i.on_command_result(
            Some(0),
            "claude=installed\ncodex=absent\nplugin=absent\n",
            "",
            &ctx_of(CTX_STATUS),
        );
        for cols in [30usize, 40, 60, 80] {
            for line in i.lines(cols) {
                assert!(!line.contains('\n'), "lines must be single-line");
                let visible: String = strip_ansi(&line);
                assert!(
                    visible.chars().count() <= cols,
                    "cols={} produced {} visible chars: {:?}",
                    cols,
                    visible.chars().count(),
                    visible
                );
            }
        }
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for c2 in chars.by_ref() {
                    if c2 == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}
