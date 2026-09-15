#!/usr/bin/env python3
"""Tests for ``tools/picocalc-ptt``.

The helper has no ``.py`` extension (it is installed as a command), so it is
loaded here through ``SourceFileLoader``. Run with::

    python3 tools/test_picocalc_ptt.py

The transcription and tmux commands are faked on ``PATH``, so these tests
exercise the real end-to-end path - framing, WAV writing, command
substitution, transcript lookup, and typing - without needing whisper or tmux
installed.
"""

import contextlib
import importlib.machinery
import importlib.util
import io
import os
import stat
import sys
import tempfile
import unittest
import wave
from pathlib import Path

TOOL = Path(__file__).resolve().parent / "picocalc-ptt"
_loader = importlib.machinery.SourceFileLoader("picocalc_ptt", str(TOOL))
_spec = importlib.util.spec_from_loader("picocalc_ptt", _loader)
ptt = importlib.util.module_from_spec(_spec)
_loader.exec_module(ptt)


def frame(payload: bytes) -> bytes:
    """One wire frame, as `terminal_model::ptt_frame` encodes it."""
    return len(payload).to_bytes(4, "little") + payload


END = frame(b"")


def samples(values) -> bytes:
    """A little-endian 16-bit PCM payload."""
    out = bytearray()
    for value in values:
        out += (value & 0xFFFF).to_bytes(2, "little")
    return bytes(out)


class ReadUtterances(unittest.TestCase):
    def collect(self, data, limit=1024):
        logs = []
        return list(ptt.read_utterances(io.BytesIO(data), limit, logs.append)), logs

    def test_decodes_the_layout_the_firmware_encoder_pins(self):
        # The same bytes `terminal_model::ptt_frame`'s
        # `encoding_is_little_endian_length_then_samples` test asserts, so the
        # two ends of the wire cannot drift apart without a test failing on one
        # side or the other: byte count 6, then 1, -2, 32767 little-endian.
        wire = bytes([6, 0, 0, 0, 0x01, 0x00, 0xFE, 0xFF, 0xFF, 0x7F]) + END
        utterances, _ = self.collect(wire)
        self.assertEqual(utterances, [samples([1, -2, 32767])])

    def test_splits_on_end_markers(self):
        data = frame(samples([1, 2])) + END + frame(samples([3])) + END
        utterances, _ = self.collect(data)
        self.assertEqual(utterances, [samples([1, 2]), samples([3])])

    def test_flushes_an_unterminated_utterance_at_eof(self):
        # The device closes the session channel mid-utterance: what arrived
        # should still be transcribed rather than thrown away.
        utterances, _ = self.collect(frame(samples([1, 2])))
        self.assertEqual(utterances, [samples([1, 2])])

    def test_keeps_a_truncated_final_frame(self):
        utterances, _ = self.collect(frame(samples([1, 2, 3]))[:-2])
        self.assertEqual(utterances, [samples([1, 2])])

    def test_empty_utterances_are_not_reported(self):
        utterances, _ = self.collect(END + END)
        self.assertEqual(utterances, [])

    def test_an_oversized_frame_is_skipped_and_the_stream_stays_aligned(self):
        # A length no capture could produce must not be buffered, and must not
        # desynchronise the frames that follow it.
        huge = (4096).to_bytes(4, "little") + b"x" * 4096
        data = huge + END + frame(samples([9])) + END
        utterances, logs = self.collect(data, limit=64)
        self.assertEqual(utterances, [samples([9])])
        self.assertEqual(len(logs), 1)

    def test_an_oversized_utterance_is_dropped_with_one_log_line(self):
        data = b"".join(frame(samples([1] * 40)) for _ in range(6)) + END
        utterances, logs = self.collect(data, limit=64)
        self.assertEqual(utterances, [])
        self.assertEqual(len(logs), 1)


class Wav(unittest.TestCase):
    def test_writes_mono_16_bit_at_the_configured_rate(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "audio.wav"
            ptt.write_wav(path, samples([1, -2, 3]), 16000, lambda _: None)
            with wave.open(str(path), "rb") as wav:
                self.assertEqual(wav.getnchannels(), 1)
                self.assertEqual(wav.getsampwidth(), 2)
                self.assertEqual(wav.getframerate(), 16000)
                self.assertEqual(wav.readframes(3), samples([1, -2, 3]))

    def test_drops_an_odd_trailing_byte(self):
        with tempfile.TemporaryDirectory() as tmp:
            logs = []
            path = Path(tmp) / "audio.wav"
            ptt.write_wav(path, samples([1]) + b"\x07", 8000, logs.append)
            with wave.open(str(path), "rb") as wav:
                self.assertEqual(wav.getnframes(), 1)
            self.assertEqual(len(logs), 1)


class Transcript(unittest.TestCase):
    def test_collapses_whitespace_to_one_line(self):
        self.assertEqual(ptt.normalise_transcript("  hello\n\nworld  "), "hello world")
        self.assertEqual(ptt.normalise_transcript("[00:00:00.000] hi there"), "[00:00:00.000] hi there")
        self.assertEqual(ptt.normalise_transcript("a\nb"), "a b")


class FakeCommands:
    """A temp directory holding fake ``whisper``/``tmux`` commands on PATH."""

    def __init__(self, test_case):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)
        self.bin = self.dir / "bin"
        self.bin.mkdir()
        self.log = self.dir / "calls.log"
        self.test_case = test_case

    def add(self, name, body):
        path = self.bin / name
        path.write_text("#!/bin/sh\n" + body)
        path.chmod(path.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
        return path

    def calls(self):
        if not self.log.exists():
            return []
        return self.log.read_text().splitlines()

    def __enter__(self):
        self.old_path = os.environ.get("PATH", "")
        os.environ["PATH"] = f"{self.bin}{os.pathsep}{self.old_path}"
        return self

    def __exit__(self, *exc):
        os.environ["PATH"] = self.old_path
        self.tmp.cleanup()
        return False


def run_main(args, stdin, fake, env=None):
    """Run the helper with `stdin` as its audio stream, returning (rc, stderr)."""
    old_env = {}
    for key, value in (env or {}).items():
        old_env[key] = os.environ.get(key)
        os.environ[key] = value
    stderr = io.StringIO()
    try:
        with contextlib.redirect_stderr(stderr):
            rc = ptt.main(args, stdin=io.BytesIO(stdin))
    finally:
        for key, value in old_env.items():
            if value is None:
                del os.environ[key]
            else:
                os.environ[key] = value
    return rc, stderr.getvalue()


class EndToEnd(unittest.TestCase):
    def test_transcribes_each_utterance_and_types_it_into_tmux(self):
        with FakeCommands(self) as fake:
            fake.add("whisper-fake", 'echo "hello world"\n')
            fake.add("tmux", f'echo "$@" >> {fake.log}\n')
            audio = frame(samples([1, 2, 3, 4])) + END + frame(samples([5, 6])) + END
            rc, stderr = run_main(
                [
                    "--whisper",
                    "whisper-fake -f %wav",
                    "--tmux-target",
                    "work",
                ],
                audio,
                fake,
            )
            self.assertEqual(rc, 0)
            self.assertEqual(
                fake.calls(),
                [
                    "send-keys -t work -l -- hello world ",
                    "send-keys -t work -l -- hello world ",
                ],
            )
            self.assertIn("typed 11 characters", stderr)

    def test_enter_flag_presses_enter_after_typing(self):
        with FakeCommands(self) as fake:
            fake.add("whisper-fake", 'echo "ok"\n')
            fake.add("tmux", f'echo "$@" >> {fake.log}\n')
            rc, _ = run_main(
                ["--whisper", "whisper-fake", "--enter", "--rate", "8000"],
                frame(samples([1, 2])) + END,
                fake,
            )
            self.assertEqual(rc, 0)
            self.assertEqual(fake.calls(), ["send-keys -l -- ok ", "send-keys Enter"])

    def test_transcript_is_read_from_the_whisper_output_file(self):
        # whisper.cpp with `-of %dir/picocalc-ptt` writes picocalc-ptt.txt and
        # says nothing on stdout, which is where the helper looks next.
        with FakeCommands(self) as fake:
            fake.add("whisper-fake", 'printf "from a file" > "$2"\n')
            fake.add("tmux", f'echo "$@" >> {fake.log}\n')
            rc, _ = run_main(
                ["--whisper", "whisper-fake --out %dir/picocalc-ptt.txt"],
                frame(samples([1, 2])) + END,
                fake,
            )
            self.assertEqual(rc, 0)
            self.assertEqual(fake.calls(), ["send-keys -l -- from a file "])

    def test_the_wav_and_output_directory_are_substituted(self):
        with FakeCommands(self) as fake:
            fake.add("whisper-fake", 'printf "%s %s" "$1" "$2" > /dev/null; echo seen\n')
            fake.add("tmux", f'echo "$@" >> {fake.log}\n')
            rc, _ = run_main(["--whisper", "whisper-fake %wav %dir"], frame(samples([1])) + END, fake)
            self.assertEqual(rc, 0)
            self.assertEqual(fake.calls(), ["send-keys -l -- seen "])

    def test_a_failing_whisper_types_nothing(self):
        with FakeCommands(self) as fake:
            fake.add("whisper-fake", 'echo "boom" >&2\nexit 1\n')
            fake.add("tmux", f'echo "$@" >> {fake.log}\n')
            rc, stderr = run_main(["--whisper", "whisper-fake"], frame(samples([1])) + END, fake)
            self.assertEqual(rc, 0)
            self.assertEqual(fake.calls(), [])
            self.assertIn("whisper exited 1: boom", stderr)

    def test_a_whisper_timeout_types_nothing(self):
        with FakeCommands(self) as fake:
            fake.add("whisper-fake", "sleep 5\n")
            fake.add("tmux", f'echo "$@" >> {fake.log}\n')
            rc, stderr = run_main(
                ["--whisper", "whisper-fake", "--timeout", "0.2"], frame(samples([1])) + END, fake
            )
            self.assertEqual(rc, 0)
            self.assertEqual(fake.calls(), [])
            self.assertIn("timed out", stderr)

    def test_an_empty_transcript_types_nothing(self):
        with FakeCommands(self) as fake:
            fake.add("whisper-fake", "true\n")
            fake.add("tmux", f'echo "$@" >> {fake.log}\n')
            rc, stderr = run_main(["--whisper", "whisper-fake"], frame(samples([1])) + END, fake)
            self.assertEqual(rc, 0)
            self.assertEqual(fake.calls(), [])
            self.assertIn("no transcript", stderr)

    def test_the_log_file_records_diagnostics(self):
        with FakeCommands(self) as fake:
            fake.add("whisper-fake", 'echo "logged words"\n')
            fake.add("tmux", "true\n")
            log = fake.dir / "ptt.log"
            rc, _ = run_main(
                ["--whisper", "whisper-fake", "--log", str(log)],
                frame(samples([1])) + END,
                fake,
            )
            self.assertEqual(rc, 0)
            text = log.read_text()
            self.assertIn("listening at 16000 Hz", text)
            self.assertIn("typed 12 characters", text)

    def test_no_whisper_command_is_a_usage_error(self):
        with FakeCommands(self) as fake:
            fake.add("tmux", "true\n")
            rc, stderr = run_main(["--config", str(fake.dir / "missing.conf")], b"", fake)
            self.assertEqual(rc, 2)
            self.assertIn("no whisper command configured", stderr)

    def test_aix_style_target_is_passed_through(self):
        with FakeCommands(self) as fake:
            fake.add("whisper-fake", 'echo "hi"\n')
            fake.add("tmux", f'echo "$@" >> {fake.log}\n')
            rc, _ = run_main(
                ["--whisper", "whisper-fake", "--tmux-target", "work:1.2"],
                frame(samples([1])) + END,
                fake,
            )
            self.assertEqual(rc, 0)
            self.assertEqual(fake.calls(), ["send-keys -t work:1.2 -l -- hi "])

    def test_missing_tmux_is_reported_without_crashing(self):
        with FakeCommands(self) as fake:
            fake.add("whisper-fake", 'echo "hi"\n')
            # A PATH with the fake whisper but no tmux at all.
            old_path = os.environ["PATH"]
            os.environ["PATH"] = str(fake.bin)
            try:
                stderr = io.StringIO()
                with contextlib.redirect_stderr(stderr):
                    rc = ptt.main(
                        ["--whisper", "whisper-fake"], stdin=io.BytesIO(frame(samples([1])) + END)
                    )
            finally:
                os.environ["PATH"] = old_path
            self.assertEqual(rc, 0)
            self.assertIn("tmux is not installed", stderr.getvalue())


class ConfigPrecedence(unittest.TestCase):
    def test_config_file_then_env_then_flag(self):
        with tempfile.TemporaryDirectory() as tmp:
            config = Path(tmp) / "ptt.conf"
            config.write_text(
                "# a comment\n"
                "whisper = from-file\n"
                "tmux_target = file-target\n"
                "enter = yes\n"
                "rate = 8000\n"
                "bogus = ignored\n"
            )
            args = ptt.build_parser().parse_args(
                ["--config", str(config), "--tmux-target", "flag-target"]
            )
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                resolved = ptt.resolve_config(args)
            self.assertEqual(resolved.whisper, "from-file")
            self.assertEqual(resolved.tmux_target, "flag-target")
            self.assertTrue(resolved.enter)
            self.assertEqual(resolved.rate, 8000)
            self.assertEqual(resolved.timeout, ptt.DEFAULT_TIMEOUT)
            # An unknown key is reported, not applied.
            self.assertIn("bogus", stderr.getvalue())

            os.environ["PICOCALC_PTT_WHISPER"] = "from-env"
            try:
                args = ptt.build_parser().parse_args(["--config", str(config)])
                with contextlib.redirect_stderr(io.StringIO()):
                    resolved = ptt.resolve_config(args)
                self.assertEqual(resolved.whisper, "from-env")
                self.assertEqual(resolved.tmux_target, "file-target")
            finally:
                del os.environ["PICOCALC_PTT_WHISPER"]


if __name__ == "__main__":
    sys.exit(unittest.main(verbosity=2))
