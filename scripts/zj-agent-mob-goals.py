"""Bounded, read-only App Server snapshot adapter. Embedded in the wasm."""

import fcntl
import json
import math
import os
import select
import selectors
import stat
import subprocess
import sys
import tempfile
import time

MAX_AGENTS = 128
MAX_BYTES = 32768
TIMEOUT = 4


class Rpc:
    def __init__(self, command=None, env=None):
        self.deadline = time.monotonic() + TIMEOUT
        self.process = subprocess.Popen(
            command or ["codex", "app-server", "--stdio"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            env=env,
        )
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        os.set_blocking(self.process.stdin.fileno(), False)
        self.buffer = b""

    def send(self, request_id, method, params):
        request = {"method": method, "params": params}
        if request_id is not None:
            request["id"] = request_id
        payload = (json.dumps(request) + "\n").encode()
        while payload:
            remaining = self.deadline - time.monotonic()
            if remaining <= 0 or not select.select([], [self.process.stdin], [], remaining)[1]:
                raise TimeoutError("App Server write deadline")
            try:
                written = os.write(self.process.stdin.fileno(), payload)
                payload = payload[written:]
            except BlockingIOError:
                continue

    def read(self):
        while b"\n" not in self.buffer:
            remaining = self.deadline - time.monotonic()
            if remaining <= 0 or not self.selector.select(remaining):
                raise TimeoutError("App Server snapshot deadline")
            data = os.read(self.process.stdout.fileno(), 65536)
            if not data or len(self.buffer) + len(data) > 1048576:
                raise ValueError("Invalid App Server output")
            self.buffer += data
        line, self.buffer = self.buffer.split(b"\n", 1)
        message = json.loads(line)
        if not isinstance(message, dict):
            raise ValueError("Invalid App Server message")
        return message

    def close(self):
        self.selector.close()
        self.process.terminate()
        try:
            self.process.wait(timeout=0.3)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.process.stdin.close()
        self.process.stdout.close()


def private_directory(path):
    os.makedirs(path, mode=0o700, exist_ok=True)
    info = os.lstat(path)
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise ValueError("Goal spool must be a private owned directory")


def previous_snapshot(path, owner):
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(fd, "r", encoding="utf-8") as previous:
            info = os.fstat(previous.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
                return None
            value = json.loads(previous.read(MAX_BYTES + 1))
        if (isinstance(value, dict) and value.get("schema_version") == 1
                and value.get("source") == "codex-app-server-v1"
                and type(value.get("observed_at")) is int and value["observed_at"] > 0
                and type(value.get("available")) is bool
                and (value.get("goal") is None or valid_goal(value["goal"], owner["agent_session_id"]))
                and all(value.get(k) == owner.get(k) for k in ("server_pid", "server_started", "agent_pid", "pane_id", "agent_session_id"))):
            return value
    except (OSError, ValueError, KeyError, TypeError):
        pass
    return None


def snapshot(owner, result=None):
    value = dict(owner, schema_version=1, source="codex-app-server-v1",
                 observed_at=time.time_ns(), available=False, goal=None)
    if isinstance(result, dict) and "goal" in result:
        goal = result["goal"]
        if goal is None or valid_goal(goal, owner["agent_session_id"]):
            value.update(available=True, goal=goal)
    if len(json.dumps(value).encode()) > MAX_BYTES:
        value.update(available=False, goal=None)
    return value


def valid_goal(goal, thread):
    if not isinstance(goal, dict) or goal.get("threadId") != thread:
        return False
    objective = goal.get("objective")
    if not isinstance(objective, str) or not 0 < len(objective) <= 4096:
        return False
    if goal.get("status") not in ("active", "paused", "blocked", "complete", "usageLimited", "budgetLimited"):
        return False
    for key in ("tokensUsed", "timeUsedSeconds", "createdAt", "updatedAt"):
        if type(goal.get(key)) is not int or goal[key] < 0:
            return False
    budget = goal.get("tokenBudget")
    return (budget is None or (type(budget) is int and budget >= 0)) and goal["updatedAt"] >= goal["createdAt"]


def processes():
    output = subprocess.run(["ps", "ax", "-o", "pid=,ppid=,lstart=,comm="],
                            capture_output=True, text=True, timeout=1, env=dict(os.environ, LC_ALL="C"))
    if output.returncode != 0:
        raise OSError("Process identity unavailable")
    result = {}
    for line in output.stdout.splitlines():
        fields = line.split(None, 2)
        if len(fields) != 3 or len(fields[2]) < 25:
            continue
        pid, parent, rest = fields
        result[int(pid)] = (int(parent), rest[:24], os.path.basename(rest[24:].strip()))
    if not result:
        raise OSError("Process identity unavailable")
    return result


def valid_owner(owner, table):
    server = table.get(owner.get("server_pid"))
    agent = table.get(owner.get("agent_pid"))
    if not server or not agent or server[1] != owner.get("server_started") or server[2] != "zellij" or agent[2] != "codex":
        return False
    reported = owner.get("report_ts", 0)
    if type(reported) not in (int, float) or not math.isfinite(reported):
        return False
    started = time.mktime(time.strptime(agent[1], "%a %b %d %H:%M:%S %Y"))
    if reported < started:
        return False
    parent = owner["agent_pid"]
    for _ in range(64):
        if parent == owner["server_pid"]:
            return True
        parent = table.get(parent, (0, "", ""))[0]
        if not parent:
            break
    return False


def refresh(owners):
    spool = os.environ.get("ZJ_AGENT_SPOOL_DIR") or os.path.join(
        os.environ.get("TMPDIR", "/tmp"), "zj-agent-mob-" + str(os.getuid()), "status")
    private_directory(spool)
    directory = os.path.join(spool, "goals")
    private_directory(directory)
    descriptor = os.open(os.path.join(directory, "refresh.lock"), os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return
        # A shared lock serializes reads as well as publication across panels.
        # No tool-result arrival ordering or private Codex storage is involved.
        table = processes()
        valid = [o for o in owners if o.get("agent_session_id") and o.get("server_started")
                 and valid_owner(o, table)]
        results = {}
        rpc = None
        try:
            if valid:
                rpc = Rpc()
                rpc.send(0, "initialize", {"clientInfo": {"name": "zj-agent-mob", "version": "1"},
                                           "capabilities": {"experimentalApi": True}})
                while rpc.read().get("id") != 0:
                    pass
                rpc.send(None, "initialized", {})
                for index, owner in enumerate(valid, 1):
                    rpc.send(index, "thread/goal/get", {"threadId": owner["agent_session_id"]})
                pending = set(range(1, len(valid) + 1))
                while pending:
                    message = rpc.read()
                    index = message.get("id")
                    if index in pending:
                        pending.remove(index)
                        results[index] = message.get("result")
        except (OSError, ValueError, TimeoutError):
            pass
        finally:
            if rpc:
                rpc.close()
        for owner in owners:
            index = next((i for i, candidate in enumerate(valid, 1) if candidate == owner), None)
            value = snapshot(owner, results.get(index))
            value["instance_valid"] = index is not None
            # Fixed numeric address, never a path from a source response.
            filename = str(owner["server_pid"]) + "." + str(owner["pane_id"]) + ".json"
            path = os.path.join(directory, filename)
            previous = previous_snapshot(path, owner) if index is not None else None
            if previous:
                value["observed_at"] = max(value["observed_at"], previous["observed_at"] + 1)
                if not value["available"] and previous.get("available"):
                    previous["age_seconds"] = max(0, (time.time_ns() - previous["observed_at"]) // 1000000000)
                    print("GOAL " + json.dumps(previous, separators=(",", ":")))
                    print("GOAL " + json.dumps(value, separators=(",", ":")))
                    continue
            payload = json.dumps(value, ensure_ascii=True, separators=(",", ":"))
            fd, temporary = tempfile.mkstemp(dir=directory, suffix=".tmp")
            try:
                with os.fdopen(fd, "w", encoding="utf-8") as target:
                    target.write(payload)
                os.replace(temporary, path)
            finally:
                if os.path.exists(temporary):
                    os.unlink(temporary)
            print("GOAL " + payload)


def main():
    owners = json.loads(sys.argv[1])
    if not isinstance(owners, list) or len(owners) > MAX_AGENTS:
        return
    try:
        refresh(owners)
    except (OSError, ValueError, subprocess.TimeoutExpired):
        for owner in owners:
            print("GOAL " + json.dumps(snapshot(owner), separators=(",", ":")))


if __name__ == "__main__":
    main()
