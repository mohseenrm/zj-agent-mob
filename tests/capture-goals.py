"""Capture the actual Zellij terminal viewer, not an HTML mockup."""

import argparse
from pathlib import Path
import subprocess
import json

from playwright.sync_api import sync_playwright


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--inline-session", required=True)
    parser.add_argument("--inspector-session", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    destination = Path(args.output)
    destination.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True, executable_path="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
        for layout, session, port in (("inline", args.inline_session, 17861), ("inspector", args.inspector_session, 17862)):
            page = browser.new_page(viewport={"width": 1320, "height": 760}, device_scale_factor=1)
            page.goto("http://127.0.0.1:" + str(port))
            page.wait_for_load_state("networkidle")
            page.locator(".xterm-screen").wait_for(state="visible")
            page.wait_for_timeout(1200)
            def prepare():
                def action(*command):
                    return subprocess.check_output(["zellij", "--session", session, "action", *command], text=True)
                panes = json.loads(action("list-panes", "--json"))
                panel = next(p for p in panes if p["is_plugin"] and p["title"] == "Agent Mob")
                action("launch-or-focus-plugin", "--floating", "--configuration",
                       "goal_display=" + layout + ",icons=unicode,check_updates=false,notify=", panel["plugin_url"])
                page.wait_for_timeout(250)
                action("focus-pane-id", "plugin_" + str(panel["id"]))
                action("send-keys", *(["Backspace"] * 65))
                page.wait_for_timeout(250)
                action("change-floating-pane-coordinates", "--pane-id", "plugin_" + str(panel["id"]),
                       "--x", "0", "--y", "0", "--width", "100%", "--height", "100%")
                action("write-chars", "/goal-qa-" + layout)
                page.wait_for_timeout(700)
            prepare()
            # Inspect the live backing pane before each screenshot.
            screen = subprocess.check_output(["zellij", "--session", session, "action", "dump-screen"], text=True)
            if "goal: active" not in screen or "done" not in screen or "/goal-qa-" not in screen:
                raise AssertionError(screen)
            page.screenshot(path=str(destination / ("goal-build-" + layout + "-wide.png")))
            page.set_viewport_size({"width": 650, "height": 640})
            page.wait_for_timeout(800)
            prepare()
            screen = subprocess.check_output(["zellij", "--session", session, "action", "dump-screen"], text=True)
            if "goal: active" not in screen or "/goal-qa-" not in screen:
                raise AssertionError(screen)
            page.screenshot(path=str(destination / ("goal-build-" + layout + "-narrow.png")))
            page.close()
        browser.close()


if __name__ == "__main__":
    main()
