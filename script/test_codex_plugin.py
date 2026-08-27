#!/usr/bin/env python3
"""Test the bundled Codex plugin without rebuilding the GUI or touching user config."""

import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import sys
import unittest


BUNDLE = Path(__file__).resolve().parents[1] / "app/assets/codex-plugins"
PLUGIN = BUNDLE / "plugins/smash"
HOOK = PLUGIN / "scripts/notify.sh"


class Hooks(unittest.TestCase):
    def run_hook(self, event, payload, variables=None):
        env = {key: value for key, value in os.environ.items()
               if not key.startswith(("SMASH_", "WARP_"))}
        env.update(variables if variables is not None else {
            "SMASH_CLI_AGENT_PROTOCOL_VERSION": "1", "SMASH_CLIENT_VERSION": "test"
        })
        with tempfile.TemporaryDirectory(prefix="smash-hook-test-") as temp:
            root = Path(temp)
            (root / "input").write_text(payload)
            command = (f"bash {shlex.quote(str(HOOK))} {shlex.quote(event)}"
                       f" < {shlex.quote(str(root / 'input'))}"
                       f" > {shlex.quote(str(root / 'stdout'))}"
                       f" 2> {shlex.quote(str(root / 'stderr'))}")
            args = (["script", "-q", "/dev/null", "bash", "-c", command]
                    if sys.platform == "darwin" else
                    ["script", "-qec", command, "/dev/null"])
            result = subprocess.run(args, env=env, stdin=subprocess.DEVNULL,
                                    capture_output=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / "stdout").read_bytes(), b"")
            self.assertEqual((root / "stderr").read_bytes(), b"")
            # BSD script echoes its synthetic EOF before the child starts.
            return result.stdout.removeprefix(b"^D\x08\x08") if sys.platform == "darwin" else result.stdout

    def decode(self, output, sentinel="smash://cli-agent"):
        prefix = f"\x1b]777;notify;{sentinel};".encode()
        self.assertTrue(output.startswith(prefix), output)
        self.assertTrue(output.endswith(b"\x07"), output)
        return json.loads(output[len(prefix):-1])

    def test_all_five_events(self):
        hooks = json.loads((PLUGIN / "hooks/hooks.json").read_text())["hooks"]
        self.assertEqual(len(hooks), 5)
        for config in hooks.values():
            command = config[0]["hooks"][0]["command"]
            event = command.rsplit(" ", 1)[1]
            with self.subTest(event=event):
                result = self.decode(self.run_hook(event, json.dumps({
                    "session_id": "session-123", "cwd": "/work/my repo",
                    "prompt": "Hello", "tool_name": "shell",
                    "tool_input": {"command": "ls"}, "last_assistant_message": "Done",
                })))
                self.assertEqual(result["event"], event)
                self.assertEqual(result["project"], "my repo")
                self.assertEqual(result["session_id"], "session-123")
                self.assertEqual(result["v"], 1)
                if event == "session_start":
                    self.assertEqual(result["plugin_version"], "1.0.0")
                if event == "permission_request":
                    self.assertEqual(result["summary"], "Wants to run shell: ls")

    def test_old_smash_build_compatibility(self):
        result = self.decode(self.run_hook("stop", "{}", {
            "WARP_CLI_AGENT_PROTOCOL_VERSION": "1", "WARP_CLIENT_VERSION": "local"
        }), "warp://cli-agent")
        self.assertEqual(result["event"], "stop")

    def test_inactive_outside_supported_terminal(self):
        self.assertEqual(self.run_hook("stop", "{}", {}), b"")

    def test_invalid_input_does_not_break_agent(self):
        for payload in ("not json", "null", "[]", '{"prompt": 42}'):
            with self.subTest(payload=payload):
                self.assertEqual(self.run_hook("prompt_submit", payload), b"")

    def test_unrecognized_event_is_ignored(self):
        self.assertEqual(self.run_hook("other", "{}"), b"")

    def test_invalid_protocol_is_ignored(self):
        for version in ("0", "garbage", "-1"):
            self.assertEqual(self.run_hook("stop", "{}", {
                "SMASH_CLI_AGENT_PROTOCOL_VERSION": version, "SMASH_CLIENT_VERSION": "test"
            }), b"")

    def test_long_and_control_character_text_is_safe(self):
        text = '\x1b]777;bad\x07\n"' + "x" * 300
        output = self.run_hook("prompt_submit", json.dumps({"prompt": text}))
        self.assertEqual(output.count(b"\x1b"), 1)
        self.assertEqual(output.count(b"\x07"), 1)
        self.assertEqual(len(self.decode(output)["query"]), 200)


@unittest.skipUnless(shutil.which("codex"), "Codex CLI is required")
class Installation(unittest.TestCase):
    def test_local_install_reinstall_and_remove(self):
        with tempfile.TemporaryDirectory(prefix="smash-plugin-test-") as temp:
            home = Path(temp) / "codex home 'quoted'"
            home.mkdir()
            root = home / "smash-integration"
            shutil.copytree(BUNDLE, root)
            env = dict(os.environ, CODEX_HOME=str(home))

            def codex(*args):
                result = subprocess.run(["codex", "plugin", *args], env=env,
                                        capture_output=True, text=True, timeout=30)
                self.assertEqual(result.returncode, 0, result.stderr)
                return result.stdout

            codex("marketplace", "add", str(root))
            codex("add", "smash@smash")
            codex("marketplace", "add", str(root))
            codex("add", "smash@smash")
            config = (home / "config.toml").read_text()
            self.assertEqual(config.count('[plugins."smash@smash"]'), 1)
            cached = home / "plugins/cache/smash/smash/1.0.0"
            manifest = json.loads((cached / ".codex-plugin/plugin.json").read_text())
            self.assertEqual(manifest["interface"]["displayName"], "Smash")
            self.assertTrue((cached / "hooks/hooks.json").is_file())
            codex("remove", "smash@smash")
            self.assertNotIn('[plugins."smash@smash"]', (home / "config.toml").read_text())


if __name__ == "__main__":
    unittest.main()
