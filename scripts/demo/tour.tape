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
# A throwaway config dir, built from the real one so the personal theme and
# keybinds survive. It sets `default_layout "demo"` (no tab bar, no status bar)
# and turns the startup-tip modal off at the source.
#
# NOT `zellij -l demo.kdl`: combined with --session, Zellij documents -l as
# adding the layout to that session as a new tab rather than starting a session
# with it, and the session ends up never created under the name the tour drives.
Type "export ZELLIJ_CONFIG_DIR=/tmp/zj-demo-cfg" Enter
Type "sh scripts/demo/mkconfig.sh /tmp/zj-demo-cfg >/dev/null" Enter
Sleep 500ms
Type "zellij -s zjdemo" Enter
Sleep 3s
Type "clear" Enter
Sleep 500ms
# Compile the plugin BEFORE the tour starts, while nothing is being recorded.
#
# Zellij caches compiled wasm per session UUID, so every render is a cold cache
# and the first `pipe --plugin` blocks on a full compile - measured at 111s,
# which landed entirely between act 0 and act 1 and pushed the tour to 192s.
# Warming it here moves that cost into the hidden pre-roll, where it is free, and
# makes the act timings the trailing Sleep is sized against reproducible.
Type "sh scripts/demo/warmup.sh zjdemo" Enter
Type "clear" Enter
Type "ZJ_SESSION=zjdemo ZJ_DEMO_LOG=/tmp/zj-acts.log sh scripts/demo/stage-tour.sh >/tmp/zj-demo-stage.log 2>&1 &" Enter
# Spawning two extra sessions and four agent panes happens before anything is
# worth showing; start recording once the panel is up (act 0).
Sleep 20s
Show

# Measured from the +Ns stamps in /tmp/zj-acts.log: the tour reaches `done` at
# ~107s from script start, and `Show` fires 20s in, so ~87s of it is recordable.
# A few seconds of headroom on top, because the act clock drifts by 2-3s between
# runs and undershooting truncates the GIF while overshooting only costs a couple
# of frozen frames on the end.
#
# render.sh asserts this against the tour's own clock, so if the staging script
# gets slower this fails loudly instead of silently shipping a truncated GIF.
Sleep 92s
