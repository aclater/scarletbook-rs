# SPDX-License-Identifier: MIT OR Apache-2.0
"""Cold seeks through the C interface (issue #94), measured from Python's ctypes.

Usage: clib-seekcost.py PACKAGE_DIR IMAGE [--slot 0|1] [--frames N] [--repeat R]
                        [--rtt-ms MS] [--mbps MB] [--min-gap SECTORS]

Loads the package's shared library and opens IMAGE through sb_disc_open_callbacks, with
read and seek callbacks that count the read calls and bytes, as a caller reading over a
network would. For ranges of N frames (default 14) starting at 0 %, 16 %, 42 %, 72 % and
99 % of the area's play time it opens a fresh disc (no seek index: a cold seek), opens the
range with sb_range_audio_open and decodes it, once with the default exact policy and once
with SB_SEEK_POLICY_SPECULATIVE (sb_disc_seek_policy_set; the minimum gap is
SB_SEEK_DEFAULT_MIN_GAP unless --min-gap says otherwise), and
prints, for each, the read calls and bytes from the range's opening to its last frame, the
best time of R runs (default 3, callbacks included), a network model (calls x MS + bytes /
MB per second, default 0.5 ms and 110 MB/s) and how the range started. The provisional
frames must equal the exact ones; the script fails otherwise.

The Rust equivalent is scarletbook-seekcost (crates/scarletbook-conformance), which also
measures seek indexes.
"""

import argparse
import ctypes as C
import pathlib
import re
import sys
import time

parser = argparse.ArgumentParser()
parser.add_argument("package")
parser.add_argument("image")
parser.add_argument("--slot", type=int, default=0, choices=(0, 1))
parser.add_argument("--frames", type=int, default=14)
parser.add_argument("--repeat", type=int, default=3)
parser.add_argument("--rtt-ms", type=float, default=0.5)
parser.add_argument("--mbps", type=float, default=110.0)
parser.add_argument("--min-gap", type=int, default=None)
args = parser.parse_args()

pkg = pathlib.Path(args.package).resolve()
header = (pkg / "include" / "scarletbook.h").read_text(encoding="utf-8")
const = {n: int(v) for n, v in re.findall(r"^#define (SB_\w+) (\d+)$", header, flags=re.M)}
min_gap = const["SB_SEEK_DEFAULT_MIN_GAP"] if args.min_gap is None else args.min_gap
lib_path = next(p for n in ("scarletbook_ffi.dll", "libscarletbook_ffi.dylib", "libscarletbook_ffi.so")
                for p in [pkg / "lib" / n, pkg / "bin" / n] if p.exists())
lib = C.CDLL(str(lib_path))
OK, END = 0, 1

ReadFn = C.CFUNCTYPE(C.c_int64, C.c_void_p, C.POINTER(C.c_uint8), C.c_size_t)
SeekFn = C.CFUNCTYPE(C.c_int64, C.c_void_p, C.c_int64, C.c_int32)


class AreaInfo(C.Structure):
    _fields_ = [(name, C.c_uint32) for name in (
        "slot", "kind_channels", "toc_lsn", "toc_sectors", "channel_count", "area_config",
        "sample_frequency_code", "frame_format", "max_byte_rate", "playtime_frames",
        "audio_start_lsn", "audio_end_lsn", "track_count")]


class FrameInfo(C.Structure):
    _fields_ = [("number", C.c_uint32), ("status", C.c_uint32), ("dst_rule", C.c_uint32), ("size", C.c_uint32)]


class Speculation(C.Structure):
    _fields_ = [("claim", C.c_uint64), ("lsn", C.c_uint64), ("slot", C.c_uint32), ("state", C.c_uint32),
                ("first", C.c_uint32), ("last_returned", C.c_uint32), ("exact_from", C.c_uint32),
                ("reserved", C.c_uint32)]


H, S, U32, P = C.c_void_p, C.c_int32, C.c_uint32, C.POINTER
for name, restype, *argtypes in (
        ("sb_last_error_message", C.c_char_p),
        ("sb_disc_open_callbacks", S, ReadFn, SeekFn, C.c_void_p, P(H)),
        ("sb_disc_free", None, H),
        ("sb_disc_area", S, H, U32, P(AreaInfo)),
        ("sb_disc_seek_policy_set", S, H, U32, U32, C.c_uint64),
        ("sb_range_audio_open", S, H, U32, U32, U32, P(H)),
        ("sb_track_audio_channels", U32, H),
        ("sb_track_audio_read", S, H, H, C.c_char_p, C.c_size_t, P(FrameInfo)),
        ("sb_track_audio_speculation", S, H, P(Speculation)),
        ("sb_track_audio_free", None, H)):
    f = getattr(lib, name)
    f.restype, f.argtypes = restype, argtypes


def call(fn, *a):
    status = getattr(lib, fn)(*a)
    if status not in (OK, END):
        sys.exit(f"{fn}: status {status}: {lib.sb_last_error_message().decode()}")
    return status


class Source:
    """The image file behind the callbacks, counting read calls and bytes."""

    def __init__(self, path):
        self.file = open(path, "rb")
        self.calls = self.bytes = 0
        self.read = ReadFn(self._read)
        self.seek = SeekFn(self._seek)

    def _read(self, _user, buf, n):
        try:
            data = self.file.read(n)
        except OSError:
            return -1
        C.memmove(buf, data, len(data))
        self.calls += 1
        self.bytes += len(data)
        return len(data)

    def _seek(self, _user, offset, whence):
        try:
            return self.file.seek(offset, whence)
        except (OSError, ValueError):
            return -1


def run(first, end, policy):
    """A cold seek: a fresh disc, the range opened and decoded. Returns (calls, bytes,
    seconds, start, frames)."""
    src = Source(args.image)
    disc = H()
    call("sb_disc_open_callbacks", src.read, src.seek, None, C.byref(disc))
    if policy is not None:
        call("sb_disc_seek_policy_set", disc, args.slot, policy, min_gap)
    src.calls = src.bytes = 0
    t = time.perf_counter()
    audio = H()
    call("sb_range_audio_open", disc, args.slot, first, end, C.byref(audio))
    buf = C.create_string_buffer(lib.sb_track_audio_channels(audio) * 4704)
    fi, frames = FrameInfo(), []
    while call("sb_track_audio_read", audio, disc, buf, len(buf), C.byref(fi)) == OK:
        frames.append((fi.number, fi.status, buf.raw[:fi.size]))
    elapsed = time.perf_counter() - t
    s = Speculation()
    call("sb_track_audio_speculation", audio, C.byref(s))
    start = "exact" if s.state == const["SB_SPECULATION_EXACT"] else f"provisional at LSN {s.lsn}"
    lib.sb_track_audio_free(audio)
    lib.sb_disc_free(disc)
    src.file.close()
    return src.calls, src.bytes, elapsed, start, frames


def best(first, end, policy):
    runs = [run(first, end, policy) for _ in range(max(1, args.repeat))]
    return min(runs, key=lambda r: r[2])


src = Source(args.image)
disc = H()
call("sb_disc_open_callbacks", src.read, src.seek, None, C.byref(disc))
area = AreaInfo()
for index in (0, 1):
    if lib.sb_disc_area(disc, index, C.byref(area)) == OK and area.slot == args.slot:
        break
else:
    sys.exit(f"no area in slot {args.slot}")
lib.sb_disc_free(disc)
p = area.playtime_frames
print(f"{args.image}: slot {args.slot}, {area.audio_end_lsn - area.audio_start_lsn + 1} audio sectors, "
      f"play time {p} frames, {args.frames} frames per range, minimum gap {min_gap} sectors, "
      f"through {lib_path.name} (ctypes callbacks)")
print()
print(f"| Range starts at | Policy | Read calls | Bytes read | Time | Network model "
      f"({args.rtt_ms} ms, {args.mbps:g} MB/s) | Start | Equal to exact |")
print("|---|---|---|---|---|---|---|---|")


def model(calls, nbytes):
    return calls * args.rtt_ms + nbytes / (args.mbps * 1e6) * 1e3


failed = False
for pct in (0, 16, 42, 72, 99):
    first = min(p * pct // 100, max(p - args.frames, 0))
    end = min(first + args.frames, p)
    exact = best(first, end, None)
    spec = best(first, end, const["SB_SEEK_POLICY_SPECULATIVE"])
    equal = spec[4] == exact[4]
    failed |= not equal
    for name, (calls, nbytes, secs, start, _), eq in (("exact", exact, ""), ("speculative", spec, str(equal).lower())):
        print(f"| {first} ({pct} %) | {name} | {calls} | {nbytes} | {secs * 1e3:.1f} ms | "
              f"{model(calls, nbytes):.1f} ms | {start} | {eq} |")
if failed:
    sys.exit("provisional frames differ from the exact ones")
