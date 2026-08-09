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
    // One invocation for both sources: a second dispatch would double the poll
    // cost for data that is always consumed together.
    format!(
        r#"ps axeww -o pid=,command= 2>/dev/null | awk '
{{
  cmd = $2; sub(/.*\//, "", cmd)
  if ({guard}) next
  pane = ""; sess = ""
  for (i = 3; i <= NF; i++) {{
    if ($i ~ /^ZELLIJ_PANE_ID=/)      pane = substr($i, 16)
    if ($i ~ /^ZELLIJ_SESSION_NAME=/) sess = substr($i, 21)
  }}
  if (pane != "" && sess != "") print "SCAN", sess, pane, cmd
}}' | sort -u
SPOOL_DIR="${{ZJ_AGENT_SPOOL_DIR:-${{TMPDIR:-/tmp}}/zj-agent-mob-$(id -u 2>/dev/null || echo 0)/status}}"
grep -s -H '' "$SPOOL_DIR"/* 2>/dev/null | sed 's/^/SPOOL /'
find "$SPOOL_DIR" -type f -mtime +1 -delete 2>/dev/null
printf 'SCANEND\n'"#
    )
}

pub(crate) fn dispatch() {
    let mut ctx = BTreeMap::new();
    ctx.insert(crate::install::CTX_KEY.to_string(), CTX_SCAN.to_string());
    host::run_command(&["sh", "-c", &scan_script(&TOOLS)], ctx);
}

/// A `session pane_id tool` triple from the scan.
pub(crate) struct Found {
    pub(crate) session: String,
    pub(crate) pane_id: u32,
    pub(crate) tool: String,
}

/// One agent's status record, read from the spool.
pub(crate) struct Spooled {
    pub(crate) session: String,
    pub(crate) pane_id: u32,
    pub(crate) ts: f64,
    pub(crate) args: std::collections::BTreeMap<String, String>,
}

#[derive(Default)]
pub(crate) struct Scan {
    pub(crate) found: Vec<Found>,
    pub(crate) spooled: Vec<Spooled>,
    /// The script ran to completion. Without this a truncated read looks like
    /// "no agents anywhere" and would cull every foreign row.
    pub(crate) complete: bool,
}

/// Splits `key=value,key=value` the way the pipe args arrive, so a spool record
/// and a pipe message parse into the same shape.
fn parse_args(rest: &str) -> std::collections::BTreeMap<String, String> {
    rest.split(',')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

/// Ignores anything malformed rather than failing the whole scan: a partial
/// list is more useful than none, and `ps` output is not a stable contract.
pub(crate) fn parse(stdout: &str) -> Scan {
    let mut scan = Scan::default();
    let mut seen_files: Vec<String> = Vec::new();
    for line in stdout.lines() {
        if line == "SCANEND" {
            scan.complete = true;
            continue;
        }
        let Some((tag, rest)) = line.split_once(' ') else {
            continue;
        };
        match tag {
            "SCAN" => {
                let mut parts = rest.split_whitespace();
                let Some(session) = parts.next() else { continue };
                let Some(Ok(pane_id)) = parts.next().map(str::parse::<u32>) else {
                    continue;
                };
                let Some(tool) = parts.next() else { continue };
                if TOOLS.contains(&tool) {
                    scan.found.push(Found {
                        session: crate::agent::sanitize_session(session),
                        pane_id,
                        tool: tool.to_string(),
                    });
                }
            }
            // `<path>:<record>`, one line per file from `grep -H`. The shell
            // only concatenates; every decision about the payload is made here.
            "SPOOL" => {
                let Some((path, record)) = rest.split_once(':') else {
                    continue;
                };
                let name = path.rsplit('/').next().unwrap_or(path);
                // A half-written record is still named `.tmp`; the rename into
                // place is what publishes it.
                if name.ends_with(".tmp") {
                    continue;
                }
                // First line wins: a file with more is malformed, and later
                // lines must not be read as separate agents.
                if seen_files.contains(&path.to_string()) {
                    continue;
                }
                seen_files.push(path.to_string());
                let args = parse_args(record);
                let (Some(ts), Some(pane_id)) = (
                    args.get("ts").and_then(|t| t.parse::<f64>().ok()),
                    args.get("pane_id").and_then(|p| p.parse::<u32>().ok()),
                ) else {
                    continue;
                };
                let Some(session) = args.get("session").filter(|s| !s.is_empty()).cloned() else {
                    continue;
                };
                // The filename is the authority on identity: a record claiming
                // a different pane than the file it lives in is malformed.
                if name != format!("{}.{}", session, pane_id) {
                    continue;
                }
                scan.spooled.push(Spooled {
                    session: crate::agent::sanitize_session(&session),
                    pane_id,
                    ts,
                    args,
                });
            }
            _ => {}
        }
    }
    scan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_of(lines: &str) -> Scan {
        parse(&format!("{}SCANEND\n", lines))
    }

    #[test]
    fn parses_session_pane_and_tool_triples() {
        let found = scan_of("SCAN mob 2 claude\nSCAN mob 3 codex\nSCAN other 11 claude\n").found;
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].session, "mob");
        assert_eq!(found[0].pane_id, 2);
        assert_eq!(found[0].tool, "claude");
        assert_eq!(found[1].tool, "codex");
        assert_eq!(found[2].session, "other");
    }

    #[test]
    fn ignores_malformed_and_unknown_lines() {
        let found =
            scan_of("SCAN mob 2 claude\nSCAN mob notanumber claude\nSCAN mob 4\n\nSCAN mob 5 nvim\nSCAN mob 6 codex\n")
                .found;
        let ids: Vec<u32> = found.iter().map(|f| f.pane_id).collect();
        assert_eq!(ids, vec![2, 6], "only well-formed known tools survive");
    }

    /// Same pane number in two sessions is normal and must stay distinct.
    #[test]
    fn identical_pane_ids_in_different_sessions_are_separate() {
        let found = scan_of("SCAN mob 3 claude\nSCAN other 3 claude\n").found;
        assert_eq!(found.len(), 2);
        assert_ne!(found[0].session, found[1].session);
    }

    #[test]
    fn empty_output_finds_nothing() {
        let scan = scan_of("");
        assert!(scan.found.is_empty() && scan.spooled.is_empty());
    }

    /// Without the sentinel the output may be truncated, and treating a partial
    /// read as "nothing is running" would cull every foreign row.
    #[test]
    fn output_without_the_sentinel_is_incomplete() {
        assert!(!parse("SCAN mob 2 claude\n").complete);
        assert!(scan_of("SCAN mob 2 claude\n").complete);
    }

    fn rec(name: &str, body: &str) -> String {
        format!("SPOOL /tmp/s/{}:{}\n", name, body)
    }

    #[test]
    fn parses_a_spool_record() {
        let s = scan_of(&rec(
            "other.3",
            "ts=100,pane_id=3,session=other,tool=claude,status=waiting,task=Fix it",
        ));
        assert_eq!(s.spooled.len(), 1);
        assert_eq!(s.spooled[0].session, "other");
        assert_eq!(s.spooled[0].pane_id, 3);
        assert_eq!(s.spooled[0].ts, 100.0);
        assert_eq!(s.spooled[0].args.get("task").unwrap(), "Fix it");
    }

    /// The rename into place is what publishes a record; a `.tmp` is by
    /// definition still being written.
    #[test]
    fn a_tmp_file_is_never_read() {
        let s = scan_of(&rec(
            "other.3.1234.tmp",
            "ts=100,pane_id=3,session=other,status=working",
        ));
        assert!(s.spooled.is_empty());
    }

    /// A torn or truncated record must be dropped, not half-applied.
    #[test]
    fn malformed_records_are_dropped() {
        for body in [
            "ts=notanumber,pane_id=3,session=other,status=working",
            "pane_id=3,session=other,status=working",
            "ts=100,session=other,status=working",
            "ts=100,pane_id=3,status=working",
            "ts=100,pane_id=3,session=,status=working",
            "garbage",
            "",
        ] {
            assert!(scan_of(&rec("other.3", body)).spooled.is_empty(), "body {:?}", body);
        }
    }

    /// The filename owns identity: a record claiming another agent's pane while
    /// living in this file is malformed and must not be attributed anywhere.
    #[test]
    fn a_record_disagreeing_with_its_filename_is_rejected() {
        let s = scan_of(&rec("other.3", "ts=100,pane_id=9,session=other,status=working"));
        assert!(s.spooled.is_empty(), "pane must match the filename");
        let s = scan_of(&rec("other.3", "ts=100,pane_id=3,session=elsewhere,status=working"));
        assert!(s.spooled.is_empty(), "session must match the filename");
    }

    /// A file with extra lines is malformed; later lines must not become rows.
    #[test]
    fn only_the_first_line_of_a_file_is_used() {
        let s = scan_of(&format!(
            "{}{}",
            rec("other.3", "ts=100,pane_id=3,session=other,status=waiting"),
            rec("other.3", "ts=200,pane_id=3,session=other,status=done"),
        ));
        assert_eq!(s.spooled.len(), 1);
        assert_eq!(s.spooled[0].ts, 100.0, "the first record wins");
    }

    #[test]
    fn scan_and_spool_are_parsed_from_one_stream() {
        let s = scan_of(&format!(
            "SCAN other 3 claude\n{}",
            rec("other.3", "ts=100,pane_id=3,session=other,status=working")
        ));
        assert_eq!(s.found.len(), 1);
        assert_eq!(s.spooled.len(), 1);
        assert!(s.complete);
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
        fn run(tag: &str, ps_output: &str) -> String {
            run_with_spool(tag, ps_output, &[])
        }

        /// `spool` is `(filename, contents)`, staged into a real directory the
        /// script reads, so the spool branch is executed rather than assumed.
        fn run_with_spool(tag: &str, ps_output: &str, spool: &[(&str, &str)]) -> String {
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
            let spool_dir = dir.join("spool");
            if !spool.is_empty() {
                std::fs::create_dir_all(&spool_dir).unwrap();
                for (name, body) in spool {
                    std::fs::write(spool_dir.join(name), body).unwrap();
                }
            }
            let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap_or_default());
            let out = Command::new("sh")
                .arg("-c")
                .arg(scan_script(&TOOLS))
                .env("PATH", path)
                .env("ZJ_AGENT_SPOOL_DIR", &spool_dir)
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

        /// Every session at once: the scan is no longer scoped to one.
        #[test]
        fn finds_agents_across_every_session() {
            assert_eq!(
                run("all", PROCS),
                "SCAN mob 2 claude\nSCAN mob 3 claude\nSCAN mob 6 codex\nSCAN other 11 claude\nSCANEND\n"
            );
        }

        /// `sort -u` keys on the whole line, so a pane number repeated in another
        /// session must survive rather than being deduplicated away.
        #[test]
        fn same_pane_id_in_two_sessions_both_survive() {
            let procs = concat!(
                "1 claude ZELLIJ_PANE_ID=3 ZELLIJ_SESSION_NAME=mob\n",
                "2 claude ZELLIJ_PANE_ID=3 ZELLIJ_SESSION_NAME=other\n",
            );
            assert_eq!(run("dup", procs), "SCAN mob 3 claude\nSCAN other 3 claude\nSCANEND\n");
        }

        /// An agent typed into an existing shell is a child of that shell. It is
        /// still its own process, so the executable-name match finds it where a
        /// command-line pattern anchored at the start would not.
        #[test]
        fn finds_a_shell_launched_agent() {
            let procs = "999 /opt/homebrew/bin/claude ZELLIJ_PANE_ID=4 ZELLIJ_SESSION_NAME=mob\n";
            assert_eq!(
                run("shell", procs),
                "SCAN mob 4 claude\nSCANEND\n",
                "absolute paths must match on basename"
            );
        }

        /// A process with no Zellij environment at all must not be attributed to
        /// whatever session is being scanned.
        #[test]
        fn a_process_outside_zellij_is_skipped() {
            let procs = "5678 claude SOME=thing\n";
            assert_eq!(run("nozellij", procs), "SCANEND\n");
        }

        /// A pane id with no session cannot be attributed to anything.
        #[test]
        fn a_pane_without_a_session_is_skipped() {
            let procs = "1 claude ZELLIJ_PANE_ID=2\n";
            assert_eq!(run("nosess", procs), "SCANEND\n");
        }

        /// The script must emit both sources in one invocation, so the poll cost
        /// stays at one command rather than two.
        #[test]
        fn one_invocation_returns_scan_and_spool() {
            let procs = "1 claude ZELLIJ_PANE_ID=3 ZELLIJ_SESSION_NAME=other\n";
            let out = run_with_spool(
                "both",
                procs,
                &[("other.3", "ts=100,pane_id=3,session=other,status=working\n")],
            );
            let scan = parse(&out);
            assert_eq!(scan.found.len(), 1, "{:?}", out);
            assert_eq!(scan.spooled.len(), 1, "{:?}", out);
            assert!(scan.complete);
        }

        /// A missing spool directory is the normal first-run state, not an error,
        /// and must not stop the scan half of the output.
        #[test]
        fn a_missing_spool_directory_is_silent() {
            let procs = "1 claude ZELLIJ_PANE_ID=3 ZELLIJ_SESSION_NAME=other\n";
            let out = run("nospool", procs);
            assert_eq!(out, "SCAN other 3 claude\nSCANEND\n", "no error output");
            assert!(parse(&out).complete);
        }

        /// An empty directory is the state right after a cleanup.
        #[test]
        fn an_empty_spool_directory_is_silent() {
            let out = run_with_spool("emptyspool", "", &[("ignored", "")]);
            let scan = parse(&out);
            assert!(scan.spooled.is_empty() && scan.complete, "{:?}", out);
        }

        /// A killed agent fires no SessionEnd, so without a sweep its record
        /// would sit in the directory forever. Anything this old is far past
        /// STALE_AFTER and can never render, so deleting it loses nothing.
        #[test]
        fn the_sweep_removes_orphans_but_keeps_current_records() {
            let dir = std::env::temp_dir().join(format!("zj-sweep-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let spool = dir.join("spool");
            std::fs::create_dir_all(&spool).unwrap();
            let fresh = spool.join("other.3");
            let orphan = spool.join("gone.9");
            std::fs::write(&fresh, "ts=100,pane_id=3,session=other,status=working\n").unwrap();
            std::fs::write(&orphan, "ts=1,pane_id=9,session=gone,status=working\n").unwrap();

            let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 60 * 48);
            let f = std::fs::File::options().write(true).open(&orphan).unwrap();
            f.set_modified(old).unwrap();
            drop(f);

            let out = Command::new("sh")
                .arg("-c")
                .arg(scan_script(&TOOLS))
                .env("ZJ_AGENT_SPOOL_DIR", &spool)
                .output()
                .expect("sh runs");
            assert!(parse(&String::from_utf8_lossy(&out.stdout)).complete);
            assert!(fresh.exists(), "a current record must survive the sweep");
            assert!(!orphan.exists(), "a day-old orphan must be swept");
            let _ = std::fs::remove_dir_all(&dir);
        }

        /// Multi-line and half-written files reach the parser as real bytes here,
        /// rather than as hand-written fixtures.
        #[test]
        fn real_files_are_filtered_by_the_parser() {
            let out = run_with_spool(
                "filtered",
                "",
                &[
                    ("other.3", "ts=100,pane_id=3,session=other,status=working\nextra\n"),
                    ("other.4.9.tmp", "ts=100,pane_id=4,session=other,status=working\n"),
                ],
            );
            let scan = parse(&out);
            assert_eq!(scan.spooled.len(), 1, "tmp skipped, extra line ignored: {:?}", out);
            assert_eq!(scan.spooled[0].pane_id, 3);
        }
    }
}
