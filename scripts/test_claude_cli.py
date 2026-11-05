"""python -m unittest scripts/test_claude_cli.py  (no network, no claude command needed)"""
import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import claude_cli as c


def delta(t):
    return json.dumps({"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": t}}})


class ClaudeCli(unittest.TestCase):
    def test_model_ids_map_to_aliases(self):
        self.assertEqual(c.alias("claude-cli:sonnet"), "sonnet")
        self.assertEqual(c.alias("claude-cli:opus"), "opus")
        self.assertEqual(c.alias("claude-cli:bogus"), "haiku")
        self.assertEqual([m["id"] for m in c.MODELS], ["claude-cli:haiku", "claude-cli:sonnet", "claude-cli:opus"])

    def test_prompt(self):
        self.assertEqual(c.prompt([{"role": "user", "content": "hi"}]), "hi")
        two = c.prompt([{"role": "user", "content": "a"}, {"role": "assistant", "content": "b"}, {"role": "user", "content": "c"}])
        self.assertEqual(two, "User: a\n\nAssistant: b\n\nUser: c\n\nAssistant:")

    def test_reads_stream_lines(self):
        self.assertEqual(c.read_line(delta("Hel")), ("delta", "Hel"))
        self.assertEqual(c.read_line(json.dumps({"type": "result", "is_error": False, "result": "Hello"})), ("done", "Hello"))
        err = c.read_line(json.dumps({"type": "result", "is_error": True, "result": "Not logged in"}))
        self.assertEqual(err, ("error", "Not logged in"))
        self.assertEqual(c.read_line(json.dumps({"type": "system", "subtype": "init"})), None)
        self.assertEqual(c.read_line("not json"), None)
        self.assertEqual(c.read_line(delta("")), None)

    def test_missing_cli_is_an_error_line(self):
        out = []
        old, c.find = c.find, lambda: ""
        try:
            c.chat("claude-cli:haiku", [{"role": "user", "content": "hi"}], "s", out.append)
        finally:
            c.find = old
        self.assertEqual(len(out), 1)
        self.assertIn("isn't installed", out[0]["error"])


class Mic(unittest.TestCase):
    def test_speech_threshold_follows_the_noise_floor(self):
        import buddy_listen as b
        self.assertAlmostEqual(b.threshold([0.0002, 0.0003, 0.0002]), 0.006, msg="dead mic: the minimum")
        self.assertAlmostEqual(b.threshold([0.004, 0.003, 0.005]), 0.009, msg="3x the quietest chunk")
        self.assertAlmostEqual(b.threshold([0.02, 0.03, 0.025]), 0.06)
        self.assertEqual(b.threshold([]), 0.015)

    def test_picks_a_microphone_by_name(self):
        import buddy_listen as b
        devs = [{"name": "Speakers (Realtek)", "max_input_channels": 0}, {"name": "Headset Microphone (2- Wireless", "max_input_channels": 1},
                {"name": "Microphone (MR720)", "max_input_channels": 2}]
        self.assertEqual(b.pick_mic(devs, "mr720"), 2)
        self.assertEqual(b.pick_mic(devs, "headset"), 1)
        self.assertIsNone(b.pick_mic(devs, "realtek"), "an output-only device is skipped")
        self.assertIsNone(b.pick_mic(devs, ""), "empty means the Windows default")
        self.assertIsNone(b.pick_mic(devs, "nothing like it"))


if __name__ == "__main__":
    unittest.main()
