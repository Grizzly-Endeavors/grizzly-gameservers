"""Tests for evals/harness/local_cluster.py: catalog env, docker command construction, state mapping."""

from __future__ import annotations

import fcntl
import json
import subprocess
import sys
import tempfile
import unittest
from contextlib import contextmanager
from importlib import import_module
from pathlib import Path
from unittest import mock

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
            image_prefix="localhost:5000/grizzly-gameservers-", use_bases=False,
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


class StatefulDocker:
    """A FakeRunner that remembers which volumes exist and which image the host holds."""

    BASE = "gary-eval-base-factorio"

    def __init__(self, image_id="sha256:one"):
        self.image_id = image_id
        self.volumes: set[str] = set()
        self.never_ready = False
        self.polls = 0
        self.posts: list[str] = []
        self.runner = FakeRunner([
            (["docker", "image", "inspect"], lambda argv: done(argv, 0, self.image_id + "\n")),
            (["docker", "volume", "inspect"], self._inspect_volume),
            (["docker", "volume", "create"], self._create_volume),
            (["docker", "volume", "rm"], self._remove_volume),
            (["docker", "run", "--rm"], (0, "")),
            (["docker", "run", "-d"], self._start_container),
            (["docker", "port"], (0, "127.0.0.1:49300\n")),
            (["docker", "inspect"], (0, "running\n")),
            (["docker", "rm"], (0, "")),
        ])

    def _start_container(self, argv):
        if "gev-box-abc123-base-factorio" in argv:
            self.posts.clear()
            self.polls = 0
        return done(argv, 0, "cid\n")

    def _inspect_volume(self, argv):
        name = argv[-1]
        return done(argv, 0, "[]\n") if name in self.volumes else done(argv, 1, "", f"Error: No such volume: {name}")

    def _create_volume(self, argv):
        self.volumes.add(argv[-1])
        return done(argv, 0, argv[-1] + "\n")

    def _remove_volume(self, argv):
        name = argv[-1]
        if name not in self.volumes:
            return done(argv, 1, "", f"Error: No such volume: {name}")
        self.volumes.discard(name)
        return done(argv, 0, "")

    def status(self, url):
        """Starting on the first poll, ready on the next, stopped once asked to stop."""
        if self.posts:
            return {"process": "stopped", "ready": False}
        if self.never_ready:
            return {"process": "starting", "ready": False}
        self.polls += 1
        if self.polls == 1:
            return {"process": "starting", "ready": False}
        return {"process": "running", "ready": True}

    def post(self, url):
        self.posts.append(url)

    def runs(self, flag):
        return [c for c in self.runner.calls if c[:2] == ["docker", "run"] and flag in c]

    def copies(self):
        return [c for c in self.runner.calls if "--entrypoint" in c]


class InstantCluster(lc.DockerCluster):
    """Never sleeps, and its clock moves one second per read, so a wait that never ends fails after its budget."""

    step = 1.0

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.clock = 0.0

    def now(self):
        self.clock += self.step
        return self.clock

    def sleep(self, seconds):
        return None


class ClockedCluster(InstantCluster):
    """Moves its clock 100 seconds per read, so a 50-second wait expires on the second poll."""

    step = 100.0


class BaseVolumeTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.state_dir = Path(temp.name)
        self.docker = StatefulDocker()

    def cluster(self, cls=InstantCluster, **kwargs):
        options = dict(
            runner=self.docker.runner, status=self.docker.status, post=self.docker.post,
            prefix="gev-box-abc123", image_prefix="localhost:5000/grizzly-gameservers-",
            state_dir=self.state_dir,
        )
        options.update(kwargs)
        return cls(None, **options)

    def sentinel(self):
        return self.state_dir / "base-factorio.image"

    def test_first_create_boots_and_stops_a_base_then_copies_it_into_the_case(self):
        cluster = self.cluster()
        cluster.tool("create_server", {"game": "factorio", "name": "ev-fa-1"})
        calls = self.docker.runner.calls
        builder = next(c for c in self.docker.runs("-d") if "gev-box-abc123-base-factorio" in c)
        self.assertIn(f"type=volume,src={StatefulDocker.BASE},dst=/factorio,volume-nocopy", builder)
        self.assertIn(["docker", "volume", "create", "--label", "grizzly-eval.base=factorio", StatefulDocker.BASE], calls)
        self.assertEqual(self.docker.posts, ["http://127.0.0.1:49300/stop"])
        self.assertEqual(self.sentinel().read_text(encoding="utf-8"), "sha256:one\n")
        self.assertIn(["docker", "rm", "-f", "-v", "gev-box-abc123-base-factorio"], calls)
        [copy] = self.docker.copies()
        self.assertEqual(copy[copy.index("--entrypoint") + 1], "cp")
        self.assertIn(f"type=volume,src={StatefulDocker.BASE},dst=/from,readonly", copy)
        self.assertIn("type=volume,src=gev-box-abc123-ev-fa-1-data,dst=/to", copy)
        self.assertEqual(copy[-4:], ["localhost:5000/grizzly-gameservers-factorio:dev", "-a", "/from/.", "/to/"])
        served = next(c for c in self.docker.runs("-d") if "gev-box-abc123-ev-fa-1" in c)
        self.assertIn("type=volume,src=gev-box-abc123-ev-fa-1-data,dst=/factorio,volume-nocopy", served)
        self.assertLess(calls.index(["docker", "rm", "-f", "-v", "gev-box-abc123-base-factorio"]), calls.index(copy))

    def test_later_creates_reuse_the_base_without_booting_it_again(self):
        cluster = self.cluster()
        cluster.tool("create_server", {"game": "factorio", "name": "ev-fa-1"})
        cluster.tool("create_server", {"game": "factorio", "name": "ev-fa-2"})
        self.assertEqual(len(self.docker.runs("-d")), 3)
        builders = [c for c in self.docker.runs("-d") if "gev-box-abc123-base-factorio" in c]
        self.assertEqual(len(builders), 1)
        self.assertEqual(len(self.docker.copies()), 2)
        self.assertEqual(self.docker.posts, ["http://127.0.0.1:49300/stop"])

    def test_a_new_image_id_rebuilds_the_base(self):
        self.cluster().tool("create_server", {"game": "factorio", "name": "ev-fa-1"})
        self.docker.image_id = "sha256:two"
        self.cluster().tool("create_server", {"game": "factorio", "name": "ev-fa-2"})
        builders = [c for c in self.docker.runs("-d") if "gev-box-abc123-base-factorio" in c]
        self.assertEqual(len(builders), 2)
        self.assertEqual(self.sentinel().read_text(encoding="utf-8"), "sha256:two\n")
        self.assertIn(["docker", "volume", "rm", "-f", StatefulDocker.BASE], self.docker.runner.calls)

    def test_a_base_that_never_reaches_ready_is_removed_and_not_recorded(self):
        self.docker.never_ready = True
        cluster = self.cluster(ClockedCluster, ready_timeout_s=50.0)
        with self.assertRaises(ClusterError) as raised:
            cluster.tool("create_server", {"game": "factorio", "name": "ev-fa-1"})
        self.assertIn("did not reach Ready", str(raised.exception))
        self.assertFalse(self.sentinel().exists())
        self.assertNotIn(StatefulDocker.BASE, self.docker.volumes)
        self.assertIn(["docker", "rm", "-f", "-v", "gev-box-abc123-base-factorio"], self.docker.runner.calls)
        self.assertEqual(self.docker.copies(), [])

    def test_a_sentinel_whose_volume_was_removed_triggers_a_rebuild(self):
        self.cluster().prepare_base("factorio")
        self.docker.volumes.discard(StatefulDocker.BASE)
        self.cluster().tool("create_server", {"game": "factorio", "name": "ev-fa-1"})
        builders = [c for c in self.docker.runs("-d") if "gev-box-abc123-base-factorio" in c]
        self.assertEqual(len(builders), 2)

    def test_prepare_base_builds_once_and_a_second_prepare_reuses_it(self):
        cluster = self.cluster()
        self.assertEqual(cluster.prepare_base("factorio"), StatefulDocker.BASE)
        cluster.prepare_base("factorio")
        self.assertEqual(len([c for c in self.docker.runs("-d") if "gev-box-abc123-base-factorio" in c]), 1)

    def test_the_base_build_and_copy_run_inside_the_game_lock(self):
        events = []
        recorded_calls = self.docker.runner.calls

        @contextmanager
        def recording_lock(state_dir, game):
            events.append(("enter", game, len(recorded_calls)))
            yield
            events.append(("exit", game, len(recorded_calls)))

        with mock.patch.object(lc, "base_lock", recording_lock):
            self.cluster().tool("create_server", {"game": "factorio", "name": "ev-fa-1"})
        self.assertEqual([e[0] for e in events], ["enter", "exit"])
        self.assertEqual(events[0][2], 0)
        copy_index = recorded_calls.index(self.docker.copies()[0])
        self.assertLess(copy_index, events[1][2])

    def test_the_game_lock_excludes_a_second_holder_of_the_same_game(self):
        with lc.base_lock(self.state_dir, "factorio"):
            other = (self.state_dir / "base-factorio.lock").open("a")
            self.addCleanup(other.close)
            with self.assertRaises(BlockingIOError):
                fcntl.flock(other.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            elsewhere = (self.state_dir / "base-terraria.lock").open("a")
            self.addCleanup(elsewhere.close)
            fcntl.flock(elsewhere.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.flock(elsewhere.fileno(), fcntl.LOCK_UN)

    def test_cold_games_and_no_bases_never_touch_a_base(self):
        self.cluster().tool("create_server", {"game": "minecraft", "name": "ev-mc-1"})
        self.cluster(use_bases=False).tool("create_server", {"game": "factorio", "name": "ev-fa-1"})
        self.assertFalse(any(c[:3] == ["docker", "image", "inspect"] for c in self.docker.runner.calls))
        self.assertNotIn(StatefulDocker.BASE, self.docker.volumes)
        self.assertEqual(self.docker.copies(), [])
        with self.assertRaises(ClusterError):
            self.cluster().prepare_base("minecraft")

    def test_base_games_do_not_depend_on_the_gameserver_name(self):
        for game in lc.BASE_GAMES:
            with self.subTest(game=game):
                template, service = lc.load_template(game)
                self.assertEqual(
                    lc.container_env(template, service, "ev-one", game),
                    lc.container_env(template, service, "ev-two", game),
                )


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
