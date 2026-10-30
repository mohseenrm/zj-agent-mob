"""Opt-in real Codex App Server + real Zellij + release wasm acceptance test."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("goals", ROOT / "scripts/zj-agent-mob-goals.py")
goals = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(goals)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--keep", action="store_true", help="Keep disposable sessions for screenshot capture")
    args = parser.parse_args()
    for binary in ("codex", "zellij", "cc", "script"):
        if not shutil.which(binary):
            raise SystemExit("SKIP: real goal acceptance requires " + binary)
    work = Path(tempfile.mkdtemp(prefix="zj-goal-live-"))
    sessions = []
    threads = []
    clients = []
    env = dict(os.environ, ZJ_AGENT_SPOOL_DIR=str(work / "spool"))
    for name in ("ZELLIJ", "ZELLIJ_SESSION_NAME", "ZELLIJ_PANE_ID"):
        env.pop(name, None)
    rpc = goals.Rpc(["codex", "-c", 'model_provider="goal_qa"', "-c",
                     'model_providers.goal_qa={name="Local-only goal QA",base_url="http://127.0.0.1:9/v1",wire_api="responses",request_max_retries=0,stream_max_retries=0}',
                     "app-server", "--stdio"], env=env)
    request_id = 0

    def call(method, params):
        nonlocal request_id
        # The test has several five-second polling intervals, unlike a refresh.
        rpc.deadline = time.monotonic() + 10
        request_id += 1
        rpc.send(request_id, method, params)
        while True:
            response = rpc.read()
            if response.get("id") == request_id:
                if "error" in response:
                    raise AssertionError(response)
                return response["result"]

    def run(*command, **kwargs):
        return subprocess.run(command, capture_output=True, text=True, timeout=15, check=True, **kwargs).stdout.strip()

    def action(session, *command):
        return run("zellij", "--session", session, "action", *command, env=env)

    def screen(session):
        return action(session, "dump-screen")

    def wait_for(session, needle, absent=False):
        deadline = time.monotonic() + 20
        text = ""
        while time.monotonic() < deadline:
            text = screen(session)
            if (needle not in text) if absent else (needle in text):
                print("  ok  " + ("absent: " if absent else "") + needle, flush=True)
                return text
            time.sleep(0.2)
        raise AssertionError("Did not observe " + needle + ":\n" + text)

    try:
        call("initialize", {"clientInfo": {"name": "zj-agent-mob-live-qa", "version": "1"}, "capabilities": {"experimentalApi": True}})
        shutil.copy2(ROOT / "target/wasm32-wasip1/release/zj-agent-mob.wasm", work / "zj-agent-mob.wasm")
        run("cc", str(ROOT / "tests/fixtures/idle-agent.c"), "-o", str(work / "codex"))
        # Config is test-owned, not an edit to the user's Zellij configuration.
        config = work / "config.kdl"
        config.write_text('show_startup_tips false\npane_frames false\n')
        plugin = "file:" + str(work / "zj-agent-mob.wasm")
        for layout in ("inline", "inspector"):
            thread = call("thread/start", {"cwd": str(work), "ephemeral": False,
                "model":"goal-qa-no-inference", "sandbox":"read-only"})["thread"]["id"]
            threads.append(thread)
            call("thread/goal/set", {"threadId": thread, "objective": "Verify reliable agent navigation and goal state", "status": "active", "tokenBudget": 20000, "origin": "user"})
            session = "zjgoal-" + str(os.getpid()) + "-" + layout
            sessions.append(session)
            log = open(work / (layout + ".log"), "w")
            client = subprocess.Popen(["script", "-q", "/dev/null", "zellij", "--config-dir", str(work), "--session", session],
                                      stdin=subprocess.DEVNULL, stdout=log, stderr=log, env=env)
            clients.append(client)
            time.sleep(1)
            configuration = "goal_display=" + layout + ",icons=unicode,check_updates=false,notify="
            panel = action(session, "launch-or-focus-plugin", "--floating", "--configuration", configuration, plugin)
            time.sleep(1)
            action(session, "write-chars", "y")
            time.sleep(0.5)
            pane = run("zellij", "--session", session, "run", "--close-on-exit", "--", "sh", str(ROOT / "tests/fixtures/goal-agent.sh"), str(ROOT), str(work), thread, layout, env=env)
            time.sleep(1)
            action(session,"rename-pane","Verify agent navigation")
            action(session, "launch-or-focus-plugin", "--floating", "--configuration", configuration, plugin)
            action(session, "change-floating-pane-coordinates", "--pane-id", panel, "--x", "1", "--y", "1", "--width", "98%", "--height", "90%")
            action(session, "write-chars", "/goal-qa-" + layout)
            wait_for(session, "goal: active")
            text = wait_for(session, "done")
            if layout == "inspector":
                action(session, "write", "27")
                action(session, "write-chars", "/Verify reliable")
                wait_for(session, "Verify reliable")
                action(session, "write", "27")
                action(session, "write-chars", "/goal-qa-" + layout)
            for status, label in (("paused", "paused"), ("active", "active"), ("blocked", "blocked"),
                                  ("complete", "complete"), ("budgetLimited", "budget-limited"), ("usageLimited", "usage-limited")):
                call("thread/goal/set", {"threadId": thread, "status": status, "origin": "user"})
                wait_for(session, "goal: " + label)
            call("thread/goal/set", {"threadId": thread, "objective": "Replacement objective from an external control", "status": "active", "origin": "user"})
            wait_for(session, "goal: active")
            action(session, "write", "27")
            action(session, "write-chars", "/Replacement objective")
            wait_for(session, "done")
            action(session, "write", "27")
            action(session, "write-chars", "/goal-qa-" + layout)
            call("thread/goal/clear", {"threadId": thread, "origin": "user"})
            wait_for(session, "goal: active", absent=True)
            # Leave useful real source data visible for screenshots.
            call("thread/goal/set", {"threadId": thread, "objective": "Ship rename-safe agent navigation with verified goal state", "status": "active", "tokenBudget": 20000, "origin": "user"})
            wait_for(session, "goal: active")
            print(json.dumps({"session": session, "thread_id": thread, "panel": panel, "pane": pane, "layout": layout}), flush=True)
        print("LIVE_GOAL_WORK=" + str(work), flush=True)
        print("Real goal lifecycle acceptance passed for both layouts", flush=True)
    finally:
        if not args.keep:
            for thread in threads:
                try:
                    call("thread/goal/clear", {"threadId": thread, "origin": "user"})
                    call("thread/archive", {"threadId": thread})
                except (OSError, ValueError, AssertionError, TimeoutError):
                    pass
            for session in sessions:
                subprocess.run(["zellij", "delete-session", session, "--force"], capture_output=True, env=env)
            for client in clients:
                client.terminate()
            shutil.rmtree(work)
        rpc.close()


if __name__ == "__main__":
    main()
