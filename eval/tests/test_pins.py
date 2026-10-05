"""Run from eval/: `python -m unittest discover -s tests`. Needs no audio, models or network."""

import tempfile
import unittest
from pathlib import Path

from tyst_eval.pins import APP_MANIFEST, app_pins, check_archive, check_snapshot

ABC = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"


class PinTests(unittest.TestCase):
    def test_app_manifest_pins_pianissimo(self):
        pins = app_pins("KlangAI/pianissimo-sv-onnx", "63730c6021234f26b9bbae9a07a04fec39e7a52e")
        self.assertIn("encoder-model.int8.onnx", pins)
        self.assertEqual(app_pins("KlangAI/pianissimo-sv-onnx", "other-revision"), {})
        self.assertTrue(APP_MANIFEST.is_file())

    def test_snapshot_must_match_every_pin(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            (d / "a.onnx").write_bytes(b"abc")
            self.assertEqual(check_snapshot(d, {"a.onnx": ABC}), [])
            self.assertTrue(check_snapshot(d, {}))
            (d / "a.onnx").write_bytes(b"abd")
            self.assertEqual(check_snapshot(d, {"a.onnx": ABC}), ["a.onnx: checksum mismatch"])
            (d / "b.onnx").write_bytes(b"x")
            self.assertIn("b.onnx: not pinned", check_snapshot(d, {"a.onnx": ABC}))
            self.assertIn("c.txt: missing", check_snapshot(d, {"c.txt": ABC}))

    def test_archive_without_a_pin_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            f = Path(d) / "x.tar.bz2"
            f.write_bytes(b"abc")
            self.assertIsNone(check_archive(f, ABC))
            self.assertIsNotNone(check_archive(f, ""))
            self.assertIsNotNone(check_archive(f, None))
            self.assertIn("mismatch", check_archive(f, "00"))


if __name__ == "__main__":
    unittest.main()
