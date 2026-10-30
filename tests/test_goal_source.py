import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("goals", ROOT / "scripts/zj-agent-mob-goals.py")
goals = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(goals)
FIXTURES = json.loads((ROOT / "tests/fixtures/goals.json").read_text())


class FakeRpc:
    response = FIXTURES["active"]["result"]
    fail = False
    methods = []

    def __init__(self):
        self.messages = []

    def send(self, request_id, method, params):
        self.methods.append(method)
        if self.fail:
            raise OSError("Source unavailable")
        self.messages.append({"id": request_id, "result": {} if method == "initialize" else self.response})

    def read(self):
        return self.messages.pop(0)

    def close(self):
        pass


class GoalSourceTests(unittest.TestCase):
    def setUp(self):
        self.work = tempfile.TemporaryDirectory(prefix="zj-goal-test-")
        self.addCleanup(self.work.cleanup)
        self.env = mock.patch.dict(os.environ, {"ZJ_AGENT_SPOOL_DIR": self.work.name})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.rpc = mock.patch.object(goals, "Rpc", FakeRpc)
        self.rpc.start()
        self.addCleanup(self.rpc.stop)
        FakeRpc.response = FIXTURES["active"]["result"]
        FakeRpc.fail = False
        FakeRpc.methods = []
        started = subprocess.check_output(["ps", "-p", str(os.getpid()), "-o", "lstart="], env=dict(os.environ, LC_ALL="C"), text=True).strip()
        self.owner = {"server_pid": os.getpid(), "server_started": started,
                      "agent_pid": os.getpid()+1, "report_ts":time.time(),
                      "session": "qa", "pane_id": 3, "agent_session_id": "qa-thread"}
        self.table = {os.getpid():(1,started,"zellij"),os.getpid()+1:(os.getpid(),started,"codex")}
        process_patch = mock.patch.object(goals,"processes",return_value=self.table)
        process_patch.start()
        self.addCleanup(process_patch.stop)

    def refresh(self, owners=None):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            goals.refresh(owners or [self.owner])
        return [json.loads(line.removeprefix("GOAL ")) for line in output.getvalue().splitlines()]

    def test_reads_only_initialize_and_goal_get_and_publishes_private_atomic_json(self):
        result = self.refresh()[0]
        self.assertTrue(result["available"])
        self.assertEqual(result["goal"]["status"], "active")
        self.assertEqual(FakeRpc.methods, ["initialize", "initialized", "thread/goal/get"])
        files = list((Path(self.work.name) / "goals").glob("*.json"))
        self.assertEqual(len(files), 1)
        self.assertEqual(files[0].stat().st_mode & 0o777, 0o600)
        self.assertEqual(json.loads(files[0].read_text()), result)
        self.assertFalse(list((Path(self.work.name) / "goals").glob("*.tmp")))

    def test_same_second_updates_have_strict_observation_order(self):
        active = self.refresh()[0]
        FakeRpc.response = FIXTURES["paused"]["result"]
        paused = self.refresh()[0]
        self.assertEqual(active["goal"]["updatedAt"], paused["goal"]["updatedAt"])
        self.assertGreater(paused["observed_at"], active["observed_at"])

    def test_no_goal_is_explicit_and_distinct_from_failed_source(self):
        FakeRpc.response = FIXTURES["none"]["result"]
        self.assertTrue(self.refresh()[0]["available"])
        FakeRpc.fail = True
        result = self.refresh()
        self.assertEqual(len(result), 2)
        self.assertTrue(result[0]["available"])
        self.assertFalse(result[1]["available"])

    def test_outage_preserves_disk_truth_without_refreshing_its_observation(self):
        active = self.refresh()[0]
        FakeRpc.fail = True
        result = self.refresh()
        self.assertEqual(result[0]["observed_at"], active["observed_at"])
        self.assertEqual(result[0]["goal"], active["goal"])
        self.assertFalse(result[1]["available"])
        path = next((Path(self.work.name) / "goals").glob("*.json"))
        self.assertEqual(json.loads(path.read_text()), active)

    def test_malformed_cache_is_ignored_not_republished(self):
        self.refresh()
        path = next((Path(self.work.name) / "goals").glob("*.json"))
        FakeRpc.fail = True
        for value in ([], {"observed_at": "not-a-number"}, {"schema_version": 99}):
            path.write_text(json.dumps(value))
            result = self.refresh()
            self.assertEqual(len(result), 1)
            self.assertFalse(result[0]["available"])

    def test_new_agent_does_not_inherit_the_cached_goal(self):
        self.refresh()
        self.owner["agent_session_id"] = "new-thread"
        FakeRpc.fail = True
        result = self.refresh()
        self.assertEqual(len(result), 1)
        self.assertFalse(result[0]["available"])
        self.assertIsNone(result[0]["goal"])

    def test_reused_server_pid_does_not_query_or_publish_goal_truth(self):
        self.refresh()
        self.owner["server_started"] = "old server instance"
        result = self.refresh()[0]
        self.assertFalse(result["available"])
        self.assertFalse(result["instance_valid"])
        self.assertEqual(FakeRpc.methods, ["initialize", "initialized", "thread/goal/get"])

    def test_malformed_source_cannot_replace_a_valid_disk_snapshot(self):
        previous = self.refresh()[0]
        for key, value in (("tokensUsed", -1), ("tokenBudget", True), ("status", "unknown"),
                           ("objective", "x" * 4097), ("timeUsedSeconds", "12")):
            FakeRpc.response = {"goal": dict(FIXTURES["active"]["result"]["goal"], **{key: value})}
            result = self.refresh()
            self.assertEqual(result[0]["goal"], previous["goal"])
            self.assertFalse(result[1]["available"])

    def test_source_cannot_return_another_threads_goal(self):
        self.owner["agent_session_id"] = "another-thread"
        self.assertFalse(self.refresh()[0]["available"])

    def test_a_recycled_pane_cannot_adopt_the_previous_process_goal(self):
        self.refresh()
        self.table.pop(self.owner["agent_pid"])
        result = self.refresh()
        self.assertEqual(len(result),1)
        self.assertFalse(result[0]["available"])
        self.assertFalse(result[0]["instance_valid"])

    def test_reused_agent_pid_requires_a_report_after_the_process_started(self):
        self.owner["report_ts"] = 1
        self.assertFalse(self.refresh()[0]["instance_valid"])

    def test_an_agent_in_another_server_cannot_supply_goal_identity(self):
        pid = self.owner["agent_pid"]
        self.table[pid] = (999,self.table[pid][1],"codex")
        self.assertFalse(self.refresh()[0]["instance_valid"])

    def test_batch_uses_one_connection_and_no_mutation_methods(self):
        owners = [dict(self.owner, pane_id=n) for n in range(100)]
        self.assertEqual(len(self.refresh(owners)), 100)
        self.assertEqual(FakeRpc.methods.count("initialize"), 1)
        self.assertEqual(FakeRpc.methods.count("thread/goal/get"), 100)

    def test_lock_contention_does_not_spawn_or_publish_out_of_order(self):
        self.refresh()
        import fcntl
        with open(Path(self.work.name) / "goals/refresh.lock", "r") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            before = len(FakeRpc.methods)
            self.assertEqual(self.refresh(), [])
            self.assertEqual(len(FakeRpc.methods), before)

    def test_symlink_and_non_private_goal_directories_are_rejected(self):
        directory = Path(self.work.name) / "goals"
        directory.symlink_to(self.work.name, target_is_directory=True)
        with self.assertRaises(ValueError):
            self.refresh()
        directory.unlink()
        directory.mkdir(mode=0o755)
        with self.assertRaises(ValueError):
            self.refresh()


if __name__ == "__main__":
    unittest.main()
