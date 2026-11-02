"""Tests for evals/harness/local_cluster.py: catalog env, docker command construction, state mapping."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from importlib import import_module
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
lc = import_module("local_cluster")
ClusterError = import_module("cluster_errors").ClusterError

REPO_ROOT = Path(__file__).resolve().parents[3]


def done(argv, rc=0, out="", err=""):
    return subprocess.CompletedProcess(argv, rc, out, err)


class FakeRunner:
    """Records each argv and answers by the first matching (prefix, reply) rule."""

    def __init__(self, rules=None):
        self.calls = []
        self.inputs = []
        self.rules = list(rules or [])

    def __call__(self, argv, **kwargs):
        self.calls.append(argv)
        self.inputs.append(kwargs.get("input"))
        for prefix, reply in self.rules:
            if argv[: len(prefix)] == prefix:
                return reply(argv) if callable(reply) else done(argv, *reply)
        return done(argv)


def env_of(args):
    return dict(args[i + 1].split("=", 1) for i, a in enumerate(args) if a == "-e")


class ContainerEnvTest(unittest.TestCase):
    def test_factorio_keeps_literals_drops_secrets_and_disables_auto_update(self):
        template, service = lc.load_template("factorio")
        env = lc.container_env(template, service, "ev-fa-1", "factorio")
        self.assertEqual(env["SUPERVISOR_CHILD_ENV_FILE"], ".grizzly/env")
        self.assertEqual(env["SUPERVISOR_CHILD_ENV_ALLOWLIST"], "LOAD_LATEST_SAVE,SAVE_NAME")
        self.assertEqual(env["SUPERVISOR_AUTO_UPDATE"], "false")
        self.assertNotIn("SUPERVISOR_CONTROL_TOKEN", env)
        self.assertNotIn("SUPERVISOR_AGENT_URL", env)

    def test_minecraft_gets_its_name_and_a_dead_agent_url(self):
        template, service = lc.load_template("minecraft")
        env = lc.container_env(template, service, "ev-mc-1", "minecraft")
        self.assertEqual(env["EULA"], "TRUE")
        self.assertEqual(env["VERSION"], "26.2")
        self.assertEqual(env["SUPERVISOR_GAMESERVER_NAME"], "ev-mc-1")
        self.assertEqual(env["SUPERVISOR_AGENT_URL"], lc.DUMMY_AGENT_URL)
        self.assertNotIn("SUPERVISOR_AGENT_TOKEN", env)
        self.assertNotIn("SUPERVISOR_CONTROL_TOKEN", env)

    def test_advertised_ports_take_their_container_port(self):
        template, service = lc.load_template("enshrouded")
        env = lc.container_env(template, service, "ev-en-1", "enshrouded")
        self.assertEqual(env["ENSHROUDED_QUERY_PORT"], "15637")
        self.assertEqual(env["SUPERVISOR_A2S_PORT"], "15637")

    def test_every_catalog_game_has_a_data_mount_and_memory_limit(self):
        for game_dir in sorted((REPO_ROOT / "games").iterdir()):
            if not (game_dir / "gameserver.yaml").exists() or game_dir.name.startswith("_"):
                continue
            with self.subTest(game=game_dir.name):
                template, _ = lc.load_template(game_dir.name)
                self.assertTrue(lc.data_mount(template).startswith("/"))
                self.assertGreater(lc.memory_limit(template) or 0, 0)

    def test_memory_quantities(self):
        self.assertEqual(lc.memory_bytes("6Gi"), 6 * 2**30)
        self.assertEqual(lc.memory_bytes("512Mi"), 512 * 2**20)
        self.assertEqual(lc.memory_bytes("2G"), 2 * 10**9)
        self.assertEqual(lc.memory_bytes("1000"), 1000)
        with self.assertRaises(ValueError):
            lc.memory_bytes("lots")


class DockerCommandTest(unittest.TestCase):
    def cluster(self, runner, status=None, gary=None):
        return lc.DockerCluster(
            gary, runner=runner, status=status or (lambda url: None), prefix="gev-box-abc123",
            image_prefix="localhost:5000/grizzly-gameservers-",
        )

    def test_create_runs_the_catalog_image_with_volume_port_memory_and_env(self):
        runner = FakeRunner([(["docker", "port"], (0, "127.0.0.1:49200\n[::1]:49200\n"))])
        cluster = self.cluster(runner)
        reply = cluster.tool("create_server", {"game": "factorio", "name": "ev-fa-1"})
        self.assertTrue(reply.startswith("created ev-fa-1"))
        volume, run_cmd, port = runner.calls
        self.assertEqual(volume[:3], ["docker", "volume", "create"])
        self.assertEqual(volume[-1], "gev-box-abc123-ev-fa-1-data")
        self.assertEqual(run_cmd[:3], ["docker", "run", "-d"])
        self.assertEqual(run_cmd[run_cmd.index("--name") + 1], "gev-box-abc123-ev-fa-1")
        self.assertIn("type=volume,src=gev-box-abc123-ev-fa-1-data,dst=/factorio,volume-nocopy", run_cmd)
        self.assertEqual(run_cmd[run_cmd.index("-p") + 1], "127.0.0.1::9359")
        self.assertEqual(run_cmd[run_cmd.index("--memory") + 1], str(6 * 2**30))
        self.assertIn("--cap-drop=ALL", run_cmd)
        self.assertIn("no-new-privileges", run_cmd)
        self.assertEqual(env_of(run_cmd)["SUPERVISOR_AUTO_UPDATE"], "false")
        self.assertEqual(run_cmd[-1], "localhost:5000/grizzly-gameservers-factorio:dev")
        self.assertEqual(port, ["docker", "port", "gev-box-abc123-ev-fa-1", "9359/tcp"])
        self.assertEqual(cluster._server("ev-fa-1").control_url, "http://127.0.0.1:49200")

    def test_failed_run_is_a_cluster_error(self):
        runner = FakeRunner([(["docker", "run"], (125, "", "Unable to find image"))])
        with self.assertRaises(ClusterError):
            self.cluster(runner).tool("create_server", {"game": "terraria", "name": "ev-tr-1"})

    def test_destroy_removes_container_and_volume_and_tolerates_absence(self):
        runner = FakeRunner([
            (["docker", "rm"], (1, "", "Error: No such container: gev-box-abc123-ev-tr-1")),
            (["docker", "volume", "rm"], (1, "", "Error: no such volume")),
        ])
        reply = self.cluster(runner).tool("destroy_server", {"name": "ev-tr-1"}, confirm=True)
        self.assertTrue(reply.startswith("deleted"))
        self.assertEqual(runner.calls[0], ["docker", "rm", "-f", "-v", "gev-box-abc123-ev-tr-1"])
        self.assertEqual(runner.calls[1], ["docker", "volume", "rm", "-f", "gev-box-abc123-ev-tr-1-data"])

    def test_exec_and_put_target_the_prefixed_container(self):
        runner = FakeRunner([(["docker", "exec"], (0, "pvp=false\n"))])
        cluster = self.cluster(runner)
        self.assertEqual(cluster.exec("ev-mc-1", ["cat", "/data/x"]), (0, "pvp=false\n"))
        cluster.put("ev-mc-1", Path("/tmp/world.tgz"), "/data/world.tgz")
        self.assertEqual(runner.calls[0], ["docker", "exec", "gev-box-abc123-ev-mc-1", "cat", "/data/x"])
        self.assertEqual(runner.calls[1], ["docker", "cp", "/tmp/world.tgz", "gev-box-abc123-ev-mc-1:/data/world.tgz"])

    def test_state_maps_container_and_supervisor_status(self):
        statuses = {}
        runner = FakeRunner([
            (["docker", "port"], (0, "127.0.0.1:49201\n")),
            (["docker", "inspect"], lambda argv: done(argv, 0, statuses["container"] + "\n")),
        ])
        cluster = self.cluster(runner, status=lambda url: statuses.get("body"))
        cluster.tool("create_server", {"game": "terraria", "name": "ev-tr-1"})
        cases = [
            ("running", None, "Scheduled"),
            ("running", {"process": "starting", "ready": False}, "Starting"),
            ("running", {"process": "running", "ready": True}, "Ready"),
            ("running", {"process": "crashed", "ready": True}, "Crashed"),
            ("running", {"process": "stopped", "ready": True}, "Stopped"),
            ("exited", None, "Unhealthy"),
        ]
        for container, body, want in cases:
            statuses.update(container=container, body=body)
            with self.subTest(container=container, body=body):
                self.assertEqual(cluster.state("ev-tr-1"), want)

    def test_state_of_a_missing_container_is_absent(self):
        runner = FakeRunner([(["docker", "inspect"], (1, "", "Error: No such object: x"))])
        self.assertEqual(self.cluster(runner).state("ev-tr-1"), "absent")

    def test_other_tools_go_through_gary_local_with_the_server_entry(self):
        docker = FakeRunner([(["docker", "port"], (0, "127.0.0.1:49202\n"))])
        binary = FakeRunner([([ "/bin/gary", "gary-local"], (0, json.dumps({"result": "pvp=true"})))])
        cluster = self.cluster(docker, gary=lc.GaryLocal(Path("/bin/gary"), Path("/x/.env"), runner=binary))
        cluster.tool("create_server", {"game": "minecraft", "name": "ev-mc-1"})
        self.assertEqual(cluster.tool("read_file", {"name": "ev-mc-1", "path": "server.properties"}), "pvp=true")
        body = json.loads(binary.inputs[0])
        self.assertEqual(body, {
            "servers": [{"name": "ev-mc-1", "game": "minecraft", "control_url": "http://127.0.0.1:49202"}],
            "tool": "read_file",
            "arguments": {"name": "ev-mc-1", "path": "server.properties"},
            "access": "admin",
        })

    def test_gary_turn_body_and_setup_failure(self):
        docker = FakeRunner([(["docker", "port"], (0, "127.0.0.1:49203\n"))])
        binary = FakeRunner([(["/bin/gary"], (0, json.dumps({"reply": "done"})))])
        cluster = self.cluster(docker, gary=lc.GaryLocal(Path("/bin/gary"), None, runner=binary))
        cluster.tool("create_server", {"game": "terraria", "name": "ev-tr-1"})
        profile = {"provider": "fireworks", "model": "m"}
        self.assertEqual(cluster.gary_turn("ev-tr-1", "hi", profile), {"reply": "done"})
        body = json.loads(binary.inputs[0])
        self.assertEqual(body["prompt"], "hi")
        self.assertEqual(body["model_profile"], profile)
        self.assertIs(body["memory"], False)
        self.assertEqual(body["access"], "admin")
        binary.rules.insert(0, (["/bin/gary"], (1, "", "FIREWORKS_API_KEY is not set")))
        with self.assertRaises(ClusterError):
            cluster.gary_turn("ev-tr-1", "hi", profile)
        self.assertEqual(cluster.failure_streak, 1)

    def test_binary_timeout_is_its_own_error(self):
        def slow(argv, **kwargs):
            raise subprocess.TimeoutExpired(argv, 1)

        gary = lc.GaryLocal(Path("/bin/gary"), None, runner=slow)
        with self.assertRaises(lc.GaryLocalTimeout):
            gary.call({}, 1)

    def test_run_prefix_is_a_docker_safe_per_process_label(self):
        first, second = lc.run_prefix("Bear's Box.local"), lc.run_prefix("Bear's Box.local")
        self.assertTrue(first.startswith("gev-bear-s-box-local-"))
        self.assertNotEqual(first, second)


class ResolveBinaryTest(unittest.TestCase):
    def test_env_then_release_then_debug(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            with self.assertRaises(ValueError):
                lc.resolve_binary({}, root)
            debug = root / "target" / "debug" / lc.BINARY_NAME
            debug.parent.mkdir(parents=True)
            debug.touch()
            self.assertEqual(lc.resolve_binary({}, root), debug)
            release = root / "target" / "release" / lc.BINARY_NAME
            release.parent.mkdir(parents=True)
            release.touch()
            self.assertEqual(lc.resolve_binary({}, root), release)
            self.assertEqual(lc.resolve_binary({lc.BINARY_KEY: str(debug)}, root), debug)
            with self.assertRaises(ValueError):
                lc.resolve_binary({lc.BINARY_KEY: str(root / "nope")}, root)


if __name__ == "__main__":
    unittest.main()
