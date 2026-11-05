//! Push-to-talk audio wire framing and the delivery of the server-side
//! helper, both mirrored by `tools/picocalc-ptt`.
//!
//! The framing contract is deliberately tiny: the device sends a sequence of
//! frames, each a 4-byte little-endian byte count followed by that many bytes
//! of payload - signed 16-bit little-endian mono PCM at `ptt_rate` (16 kHz
//! unless configured otherwise, which is not signalled on the wire, so a
//! receiver has to be told out of band).
//!
//! The session's channel stays open for the whole session, so an utterance
//! needs an explicit end-of-utterance marker in band: a frame with a zero byte
//! count ([`END_OF_UTTERANCE`]), which is unambiguous because a non-empty
//! capture never produces an empty chunk. A receiver must treat the channel's
//! EOF as "the session ended" and flush whatever audio it has buffered.
//!
//! The same channel also carries the helper script itself at session start,
//! before any frames: [`helper_exec_command`] is the command that copies it off
//! the channel and runs it, and the byte count in that command is the boundary
//! between the script and the first frame.
//!
//! Keeping this here (free of embassy and `crate` dependencies) lets host tests
//! pin the byte layout and the delivery command, per AGENTS.md's
//! "host-testable logic lives in terminal-model" guidance.

use alloc::format;
use alloc::string::String;

/// Bytes in the little-endian byte count that starts every frame.
pub const LEN_PREFIX_BYTES: usize = 4;

/// The end-of-utterance marker for the SSH transport: a frame whose byte count
/// is zero. See the module docs for why that is unambiguous.
pub const END_OF_UTTERANCE: [u8; LEN_PREFIX_BYTES] = [0; LEN_PREFIX_BYTES];

/// Byte length of one frame carrying `samples` 16-bit samples: the byte count
/// followed by the samples.
pub const fn frame_bytes(samples: usize) -> usize {
    LEN_PREFIX_BYTES + samples * 2
}

/// The little-endian byte count that prefixes a payload of `payload_bytes`
/// bytes, or `None` when it cannot be represented (never in practice: a
/// capture chunk is a few hundred samples).
pub fn len_bytes(payload_bytes: usize) -> Option<[u8; LEN_PREFIX_BYTES]> {
    let byte_len = u32::try_from(payload_bytes).ok()?;
    Some(byte_len.to_le_bytes())
}

/// Writes `samples` as little-endian 16-bit values at the start of `out`,
/// returning how many bytes were written, or `None` if `out` is too small.
/// This is the payload half of a frame, for a sender that writes the byte
/// count separately.
pub fn encode_samples(out: &mut [u8], samples: &[i16]) -> Option<usize> {
    let total = samples.len() * 2;
    if out.len() < total {
        return None;
    }
    for (i, sample) in samples.iter().enumerate() {
        out[i * 2..i * 2 + 2].copy_from_slice(&sample.to_le_bytes());
    }
    Some(total)
}

/// Encodes one whole wire frame - the 4-byte little-endian payload byte count,
/// then each sample little-endian - into the start of `out`, returning how
/// many bytes were written, or `None` if `out` is too small for the whole
/// frame (so a caller can never emit a truncated frame with a length prefix
/// that promises more than it wrote).
pub fn encode_frame(out: &mut [u8], samples: &[i16]) -> Option<usize> {
    let total = frame_bytes(samples.len());
    if out.len() < total {
        return None;
    }
    out[..LEN_PREFIX_BYTES].copy_from_slice(&len_bytes(samples.len() * 2)?);
    encode_samples(&mut out[LEN_PREFIX_BYTES..], samples)?;
    Some(total)
}

/// The shell command that receives the helper script over the session's audio
/// channel, for a script of `script_bytes` bytes.
///
/// The firmware streams its embedded copy of `tools/picocalc-ptt` down the
/// channel right after this command's `exec` request, so the script is written
/// to a private file and then run by `python3` with the channel still on its
/// stdin - the audio frames start exactly where the script ends. `dd bs=1
/// count=N` is what makes that boundary exact: it reads one byte per read, so
/// it cannot consume any of the audio that follows the script (a block-sized
/// read could).
///
/// The command runs through the login shell, so the single-quoted `sh -c`
/// argument keeps the device's own `$$`/`$TMPDIR` expansions off the remote
/// shell and hands them to the inner one.
///
/// `python3 -I` (isolated mode) is load-bearing, not cosmetic: the helper is
/// written to a shared temp directory under a PID-derived name
/// (`$TMPDIR/picocalc-ptt.$$`), and Python's default behavior prepends a
/// script's own directory to `sys.path` - so any unrelated file already
/// sitting in that temp directory under a name that shadows a stdlib module
/// (observed on real hardware: a stray `gettext.py` in `/tmp` broke the
/// helper's `import argparse` before it ever reached config parsing or
/// whisper) silently wins the import instead of the real stdlib module.
/// `-I` implies `-P` (never prepend the script's directory, added in 3.11,
/// too new to use alone given this project's Python 3.8+ floor), `-E`
/// (ignore `PYTHON*` env vars, which the helper never sets or relies on -
/// its own config knobs are plain `PICOCALC_PTT_*`/`XDG_*`/`HOME`/`TMUX*`
/// names untouched by `-E`), and `-s` (skip the user site-packages
/// directory, harmless here since the helper only shells out to
/// `whisper-cli`/`whisper`/`tmux` via `PATH` and never imports a
/// non-stdlib package itself). Together these make the run immune to any
/// file that happens to exist in the script's temp directory, not just
/// today's specific `gettext.py` collision.
pub fn helper_exec_command(script_bytes: usize) -> String {
    format!(
        "sh -c 'f=\"${{TMPDIR:-/tmp}}/picocalc-ptt.$$\"; \
         dd bs=1 count={script_bytes} of=\"$f\" 2>/dev/null && exec python3 -I \"$f\"'"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_command_writes_and_runs_exactly_the_sent_script() {
        // The whole delivery contract in one string: run through `sh -c`, copy
        // exactly the script's byte count with byte-at-a-time `dd` (so no
        // audio frame is ever consumed), then exec python3 -I on that file,
        // which leaves the channel on python's stdin for the frames that
        // follow.
        assert_eq!(
            helper_exec_command(21633),
            "sh -c 'f=\"${TMPDIR:-/tmp}/picocalc-ptt.$$\"; dd bs=1 count=21633 of=\"$f\" 2>/dev/null && exec python3 -I \"$f\"'"
        );
    }

    #[test]
    fn helper_command_count_tracks_the_script_length() {
        // A count that drifted from the script's real length would either feed
        // python a truncated script or swallow the first audio frames as code.
        for len in [1usize, 4096, 999_999] {
            let command = helper_exec_command(len);
            assert!(command.contains(&format!("count={len} ")), "{command}");
            assert_eq!(command.matches("count=").count(), 1, "{command}");
            assert!(command.ends_with("exec python3 -I \"$f\"'"), "{command}");
        }
    }

    /// Regression test for a live device log: a stray `/tmp/gettext.py` on
    /// the server shadowed the stdlib `gettext` module because Python by
    /// default prepends the script's own directory to `sys.path`, crashing
    /// the helper's `import argparse` before it ever reached config parsing
    /// or whisper.
    ///
    /// This runs the real [`helper_exec_command`] through a shell, exactly as
    /// the server does, with the helper script *and* a colliding `gettext.py`
    /// in the same `$TMPDIR`, and asserts the helper still gets past its
    /// imports to its own startup. The second half re-runs the same setup with
    /// the isolation flag stripped (`python3 "$f"`, the command before the
    /// fix) and asserts the collision *does* reproduce there, so the passing
    /// half can't be passing because the fixture never actually shadowed
    /// anything.
    #[cfg(unix)]
    #[test]
    fn helper_command_survives_a_stdlib_shadow_in_its_own_temp_dir() {
        use std::io::Write as _;
        use std::process::{Command, Stdio};

        struct TempDir(std::path::PathBuf);
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        if Command::new("python3")
            .args(["-E", "-c", "pass"])
            .status()
            .is_err()
        {
            eprintln!("skipping: no python3 on PATH");
            return;
        }

        // The exact bytes the firmware embeds and streams down the channel.
        let script = include_bytes!("../../tools/picocalc-ptt");
        let dir = TempDir(
            std::env::temp_dir().join(format!("picocalc-ptt-shadow-{}", std::process::id())),
        );
        std::fs::create_dir_all(&dir.0).expect("create temp dir");
        // The real collision: the helper's `import argparse` does
        // `from gettext import gettext as _, ngettext`, and a file named
        // `gettext.py` sitting in the script's own directory wins that import
        // over the stdlib module and then dies on `sys.argv`.
        std::fs::write(
            dir.0.join("gettext.py"),
            "import sys\n\
             print('SHADOWED_GETTEXT_IMPORTED', file=sys.stderr, flush=True)\n\
             host = sys.argv[1]; name = sys.argv[2]\n",
        )
        .expect("write shadow module");

        let run = |command: &str| {
            let mut child = Command::new("sh")
                .arg("-c")
                .arg(command)
                .env("TMPDIR", &dir.0)
                // A config path that does not exist, so the helper's own
                // startup is machine-independent: it reports what is missing
                // or starts listening, but it must get that far.
                .env("PICOCALC_PTT_CONFIG", dir.0.join("missing.conf"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn the helper command");
            let mut stdin = child.stdin.take().expect("stdin");
            stdin.write_all(script).expect("stream the helper script");
            drop(stdin);
            let output = child.wait_with_output().expect("wait for the helper");
            String::from_utf8_lossy(&output.stderr).into_owned()
        };

        let isolated = run(&helper_exec_command(script.len()));
        assert!(
            !isolated.contains("SHADOWED_GETTEXT_IMPORTED") && !isolated.contains("Traceback"),
            "the temp dir's gettext.py still shadowed a stdlib import: {isolated}"
        );

        // Control: the same fixture without `-I` is the command this fix
        // replaced, and must still reproduce the reported crash.
        let unisolated = helper_exec_command(script.len()).replace("python3 -I", "python3");
        assert!(
            run(&unisolated).contains("SHADOWED_GETTEXT_IMPORTED"),
            "the shadowing fixture no longer reproduces the collision it exists for"
        );
    }

    #[test]
    fn empty_frame_is_the_end_of_utterance_marker() {
        let mut buf = [0xAAu8; 8];
        assert_eq!(encode_frame(&mut buf, &[]), Some(4));
        assert_eq!(&buf[..4], &END_OF_UTTERANCE);
        // Nothing past the marker is touched, so it can be written into the
        // same buffer a data frame was built in.
        assert_eq!(buf[4], 0xAA);
    }

    #[test]
    fn frame_bytes_counts_prefix_and_samples() {
        assert_eq!(frame_bytes(0), 4);
        assert_eq!(frame_bytes(1), 6);
        assert_eq!(frame_bytes(400), 804);
    }

    #[test]
    fn encoding_is_little_endian_length_then_samples() {
        let mut buf = [0u8; frame_bytes(3)];
        assert_eq!(encode_frame(&mut buf, &[0x0001, -2, 0x7FFF]), Some(10));
        assert_eq!(
            buf,
            [
                6, 0, 0, 0, // byte count: 3 samples * 2
                0x01, 0x00, // 1
                0xFE, 0xFF, // -2
                0xFF, 0x7F, // 32767
            ]
        );
    }

    #[test]
    fn short_buffer_is_refused_rather_than_truncated() {
        let mut buf = [0xAAu8; 9];
        assert_eq!(encode_frame(&mut buf, &[1, 2, 3]), None);
        assert_eq!(buf, [0xAA; 9]);
    }

    /// The helper in `tools/picocalc-ptt` decodes by reading the byte count
    /// and then exactly that many bytes, so the encoder must agree with that
    /// reader for a payload that does not fit a single read.
    #[test]
    fn a_decoder_reading_prefix_then_payload_recovers_the_samples() {
        let samples: [i16; 5] = [0, -1, 1, i16::MIN, i16::MAX];
        let mut buf = [0u8; 64];
        let n = encode_frame(&mut buf, &samples).expect("buffer is large enough");

        let byte_len = u32::from_le_bytes(buf[..LEN_PREFIX_BYTES].try_into().unwrap()) as usize;
        assert_eq!(byte_len, samples.len() * 2);
        let payload = &buf[LEN_PREFIX_BYTES..LEN_PREFIX_BYTES + byte_len];
        let decoded: Vec<i16> = payload
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes(pair.try_into().unwrap()))
            .collect();
        assert_eq!(decoded, samples);
        assert_eq!(LEN_PREFIX_BYTES + byte_len, n);
    }

    /// A sender that writes the byte count and then the payload in batches
    /// must produce the same bytes as the whole-frame encoder, so the two
    /// halves can never drift.
    #[test]
    fn split_prefix_and_batched_payload_match_the_whole_frame_encoder() {
        let samples: [i16; 5] = [7, -8, 9, -10, 11];
        let mut whole = [0u8; 32];
        let whole_len = encode_frame(&mut whole, &samples).unwrap();

        let mut split = [0u8; 32];
        split[..LEN_PREFIX_BYTES].copy_from_slice(&len_bytes(samples.len() * 2).unwrap());
        let mut written = LEN_PREFIX_BYTES;
        for batch in samples.chunks(2) {
            let n = encode_samples(&mut split[written..], batch).unwrap();
            written += n;
        }
        assert_eq!(written, whole_len);
        assert_eq!(split[..written], whole[..whole_len]);
    }

    #[test]
    fn sample_encoder_refuses_a_short_buffer() {
        let mut buf = [0xAAu8; 3];
        assert_eq!(encode_samples(&mut buf, &[1, 2]), None);
        assert_eq!(encode_samples(&mut buf, &[1]), Some(2));
        assert_eq!(buf[..2], [1, 0]);
    }

    #[test]
    fn length_prefix_is_little_endian() {
        assert_eq!(len_bytes(0).unwrap(), END_OF_UTTERANCE);
        assert_eq!(len_bytes(258).unwrap(), [2, 1, 0, 0]);
    }
}
