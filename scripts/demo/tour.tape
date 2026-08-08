# Full-feature tour of zj-agent-mob.
#
# The tape deliberately types almost nothing: it starts Zellij, then a staging
# script drives the session from outside via `zellij action`. See docs/demo.md
# for why (Zellij's modal input makes keystroke-driven tapes non-deterministic).
#
#   vhs scripts/demo/tour.tape

Output demo/tour.gif

Set Shell "bash"
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 15
Set Width 1500
Set Height 900
Set Framerate 24
Set Padding 20
Set WindowBar Colorful
Set BorderRadius 8

Hide
# A bare prompt: the real profile prints a system-info banner into frame.
Type "PS1='$ '; clear" Enter
Type "zellij -s zjdemo" Enter
Sleep 6s
# Dismiss the first-run "About Zellij" tip, which covers the panel.
Escape
Sleep 500ms
Type "clear" Enter
Sleep 500ms
Type "ZJ_SESSION=zjdemo ZJ_DEMO_LOG=/tmp/zj-acts.log sh scripts/demo/stage-tour.sh >/tmp/zj-demo-stage.log 2>&1 &" Enter
# Spawning two extra sessions and four agent panes happens before anything is
# worth showing; start recording once the panel is up (act 0).
Sleep 20s
Show

# Staging runs ~85s from act 0 (measured); this covers it plus a beat to land on.
Sleep 96s
