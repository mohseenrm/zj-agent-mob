//! Finding agents that have not reported.
//!
//! The panel only learns about an agent when its hook fires, so a reload or an
//! idle agent leaves the screen empty while agents are sitting right there. This
//! scans process environments instead: every agent inherits `ZELLIJ_PANE_ID` and
//! `ZELLIJ_SESSION_NAME` from its pane's pty, whether it was launched by the
//! layout or typed into an existing shell.

use std::collections::BTreeMap;

use crate::host;

pub(crate) const CTX_SCAN: &str = "discover-scan";

/// Executable basenames treated as agents. Matched against the process's own
/// name, not its command line: an agent started by typing `claude` into a shell
/// is a child of that shell, so a command-line pattern anchored at the start
/// misses it.
const TOOLS: [&str; 2] = ["claude", "codex"];

/// One `ps` for every process, filtered in awk.
///
/// `ps axeww` is required to get environments. The POSIX `-e` and the BSD `e`
/// collide silently on macOS: `ps -e eww` still exits 0 and still prints a
/// process list, just with no environment at all, so the scan finds nothing.
pub(crate) fn scan_script(tools: &[&str]) -> String {
    let guard = tools
        .iter()
        .map(|t| format!("cmd != \"{}\"", t))
        .collect::<Vec<_>>()
        .join(" && ");
    format!(
        r#"ps axeww -o pid=,command= 2>/dev/null | awk -v want="$1" '
{{
  cmd = $2; sub(/.*\//, "", cmd)
  if ({guard}) next
  pane = ""; sess = ""
  for (i = 3; i <= NF; i++) {{
    if ($i ~ /^ZELLIJ_PANE_ID=/)      pane = substr($i, 16)
    if ($i ~ /^ZELLIJ_SESSION_NAME=/) sess = substr($i, 21)
  }}
  if (pane != "" && sess == want) print pane, cmd
}}' | sort -un"#
    )
}

/// Session name is passed as a positional arg so it is never parsed by a shell.
pub(crate) fn dispatch(session: &str) {
    let mut ctx = BTreeMap::new();
    ctx.insert(crate::install::CTX_KEY.to_string(), CTX_SCAN.to_string());
    host::run_command(&["sh", "-c", &scan_script(&TOOLS), "sh", session], ctx);
}

/// A `pane_id tool` pair from the scan.
pub(crate) struct Found {
    pub(crate) pane_id: u32,
    pub(crate) tool: String,
}

/// Ignores anything malformed rather than failing the whole scan: a partial
/// list is more useful than none, and `ps` output is not a stable contract.
pub(crate) fn parse(stdout: &str) -> Vec<Found> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pane_id = parts.next()?.parse::<u32>().ok()?;
            let tool = parts.next()?;
            TOOLS.contains(&tool).then(|| Found {
                pane_id,
                tool: tool.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pane_and_tool_pairs() {
        let found = parse("2 claude\n3 codex\n11 claude\n");
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].pane_id, 2);
        assert_eq!(found[0].tool, "claude");
        assert_eq!(found[1].tool, "codex");
    }

    #[test]
    fn ignores_malformed_and_unknown_lines() {
        let found = parse("2 claude\nnotanumber claude\n4\n\n5 nvim\n6 codex\n");
        let ids: Vec<u32> = found.iter().map(|f| f.pane_id).collect();
        assert_eq!(ids, vec![2, 6], "only well-formed known tools survive");
    }

    #[test]
    fn empty_output_finds_nothing() {
        assert!(parse("").is_empty());
    }

    /// The awk guard is negated, so every tool must be joined with `&&`: an `||`
    /// there would match nothing at all.
    #[test]
    fn script_filters_on_every_tool() {
        let s = scan_script(&["claude", "codex"]);
        assert!(s.contains(r#"cmd != "claude" && cmd != "codex""#), "{}", s);
        assert!(s.contains("axeww"), "BSD form is required for environments");
    }

    /// Runs the real script through `sh` against a stubbed `ps`, so the awk
    /// program is executed rather than pattern-matched. The awk is the part that
    /// can silently return nothing, which is indistinguishable from "no agents".
    mod script {
        use super::*;
        use std::io::Write;
        use std::process::Command;

        /// Lines are `pid command ENV=...`, matching `ps axeww -o pid=,command=`.
        ///
        /// `tag` keeps each case's stub in its own directory: tests run in
        /// parallel, and a shared path means one case's `ps` answers another's.
        fn run(tag: &str, ps_output: &str, session: &str) -> String {
            let dir = std::env::temp_dir().join(format!("zj-scan-{}-{}", std::process::id(), tag));
            std::fs::create_dir_all(&dir).unwrap();
            let ps = dir.join("ps");
            let mut f = std::fs::File::create(&ps).unwrap();
            write!(f, "#!/bin/sh\ncat <<'EOF'\n{}\nEOF\n", ps_output).unwrap();
            drop(f);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&ps, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap_or_default());
            let out = Command::new("sh")
                .arg("-c")
                .arg(scan_script(&TOOLS))
                .arg("sh")
                .arg(session)
                .env("PATH", path)
                .output()
                .expect("sh runs");
            let _ = std::fs::remove_dir_all(&dir);
            String::from_utf8_lossy(&out.stdout).into_owned()
        }

        const PROCS: &str = concat!(
            "45985 claude ZELLIJ=0 ZELLIJ_PANE_ID=2 ZELLIJ_SESSION_NAME=mob\n",
            "47845 claude ZELLIJ=0 ZELLIJ_PANE_ID=3 ZELLIJ_SESSION_NAME=mob\n",
            "43820 codex ZELLIJ=0 ZELLIJ_PANE_ID=6 ZELLIJ_SESSION_NAME=mob\n",
            "67819 claude ZELLIJ=0 ZELLIJ_PANE_ID=11 ZELLIJ_SESSION_NAME=other\n",
            "1234 nvim ZELLIJ=0 ZELLIJ_PANE_ID=9 ZELLIJ_SESSION_NAME=mob\n",
            "5678 claude SOME=thing\n",
        );

        #[test]
        fn finds_agents_only_in_the_named_session() {
            assert_eq!(run("named", PROCS, "mob"), "2 claude\n3 claude\n6 codex\n");
        }

        #[test]
        fn other_sessions_get_their_own_agents() {
            assert_eq!(run("other", PROCS, "other"), "11 claude\n");
        }

        #[test]
        fn a_session_with_no_agents_prints_nothing() {
            assert_eq!(run("empty", PROCS, "empty"), "");
        }

        /// An agent typed into an existing shell is a child of that shell. It is
        /// still its own process, so the executable-name match finds it where a
        /// command-line pattern anchored at the start would not.
        #[test]
        fn finds_a_shell_launched_agent() {
            let procs = "999 /opt/homebrew/bin/claude ZELLIJ_PANE_ID=4 ZELLIJ_SESSION_NAME=mob\n";
            assert_eq!(
                run("shell", procs, "mob"),
                "4 claude\n",
                "absolute paths must match on basename"
            );
        }

        /// A process with no Zellij environment at all must not be attributed to
        /// whatever session is being scanned.
        #[test]
        fn a_process_outside_zellij_is_skipped() {
            let procs = "5678 claude SOME=thing\n";
            assert_eq!(run("nozellij", procs, "mob"), "");
        }

        /// Session names are compared whole: `mob` must not match `mob-2`.
        #[test]
        fn session_match_is_exact_not_a_prefix() {
            let procs = "1 claude ZELLIJ_PANE_ID=2 ZELLIJ_SESSION_NAME=mob-2\n";
            assert_eq!(run("prefix", procs, "mob"), "");
        }
    }
}
