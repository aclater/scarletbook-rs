# SPDX-License-Identifier: MIT OR Apache-2.0
"""Smoke test of a packaged C library through Python's ctypes.

Usage: clib-smoke.py PACKAGE_DIR   (run from the repository root)

Loads the package's shared library, checks that every function the header declares is
exported, that sb_abi_version() equals the header's SB_ABI_VERSION and sb_version() the
package version, then opens corpus image dst-features-2ch-6ch, compares its TOC with the
conformance manifest and decodes track 1 of each area against the manifest's per-channel
SHA-256.
"""
import ctypes as C
import hashlib
import json
import pathlib
import re
import sys

pkg = pathlib.Path(sys.argv[1]).resolve()
header = (pkg / "include" / "scarletbook.h").read_text(encoding="utf-8")
lib_path = next(p for n in ("scarletbook_ffi.dll", "libscarletbook_ffi.dylib", "libscarletbook_ffi.so")
                for p in [pkg / "lib" / n] if p.exists())
lib = C.CDLL(str(lib_path))
print(f"loaded {lib_path}")

failures = []


def check(what, got, want):
    if got != want:
        failures.append(f"{what}: got {got!r}, want {want!r}")


# Every declared function is exported (on Windows: by the DLL's export table).
names = re.findall(r"^[^#\s/*][^;{]*?\b(sb_\w+)\(", header, flags=re.M)
missing = [n for n in names if not hasattr(lib, n)]
check("functions declared in the header but not exported", missing, [])
print(f"{len(names)} functions exported")

lib.sb_abi_version.restype = C.c_uint32
lib.sb_version.restype = C.c_char_p
lib.sb_last_error_message.restype = C.c_char_p
abi = int(re.search(r"^#define SB_ABI_VERSION (\d+)$", header, flags=re.M).group(1))
check("sb_abi_version()", lib.sb_abi_version(), abi)
version = pkg.name.split("-")[2]
check("sb_version()", lib.sb_version().decode(), version)
print(f"sb_version() {lib.sb_version().decode()}, sb_abi_version() {lib.sb_abi_version()}")


class DiscInfo(C.Structure):
    _fields_ = [("sector_size", C.c_uint32), ("sectors", C.c_uint64), ("version_major", C.c_uint8),
                ("version_minor", C.c_uint8), ("album_set_size", C.c_uint16),
                ("album_sequence_number", C.c_uint16), ("hybrid", C.c_bool),
                ("manufacturer_sector", C.c_bool), ("year", C.c_uint16), ("month", C.c_uint8),
                ("day", C.c_uint8), ("area_count", C.c_uint32), ("warning_count", C.c_uint32)]


class AreaInfo(C.Structure):
    _fields_ = [(n, C.c_uint32) for n in (
        "slot", "kind_channels", "toc_lsn", "toc_sectors", "channel_count", "area_config",
        "sample_frequency_code", "frame_format", "max_byte_rate", "playtime_frames",
        "audio_start_lsn", "audio_end_lsn", "track_count")]


class TrackInfo(C.Structure):
    _fields_ = [("number", C.c_uint32), ("start_lsn", C.c_uint32), ("length_sectors", C.c_uint32),
                ("start_frame", C.c_uint32), ("duration_frames", C.c_uint32), ("start_flags", C.c_uint8),
                ("duration_flags", C.c_uint8), ("has_genre", C.c_bool), ("genre_category", C.c_uint8),
                ("genre_index", C.c_uint8)]


class FrameInfo(C.Structure):
    _fields_ = [("number", C.c_uint32), ("status", C.c_uint32), ("dst_rule", C.c_uint32),
                ("size", C.c_size_t)]


OK, END = 0, 1
for fn in ("sb_disc_open_file", "sb_disc_info", "sb_disc_string", "sb_disc_area", "sb_track_info",
           "sb_track_audio_open", "sb_track_audio_read"):
    getattr(lib, fn).restype = C.c_int
lib.sb_track_audio_channels.restype = C.c_uint32


def call(fn, *args):
    status = getattr(lib, fn)(*args)
    if status != OK:
        sys.exit(f"{fn}: status {status}: {lib.sb_last_error_message().decode()}")


case = next(c for c in json.load(open("corpus/manifest.json", encoding="utf-8"))["cases"]
            if c["id"] == "dst-features-2ch-6ch")
toc = case["expected"]["toc"]
disc = C.c_void_p()
call("sb_disc_open_file", case["input"].encode(), C.byref(disc))
info = DiscInfo()
call("sb_disc_info", disc, C.byref(info))
for field in ("sector_size", "sectors", "album_set_size", "album_sequence_number", "hybrid",
              "manufacturer_sector"):
    check(field, getattr(info, field), toc[field])
check("version", [info.version_major, info.version_minor], toc["version"])
check("date", [info.year, info.month, info.day], [toc["date"][k] for k in ("year", "month", "day")])
check("area_count", info.area_count, len(toc["areas"]))
check("warning_count", info.warning_count, len(case["expected"]["warnings"]))
title, length = C.create_string_buffer(256), C.c_size_t()
call("sb_disc_string", disc, C.c_uint32(0), title, C.c_size_t(256), C.byref(length))  # ALBUM_TITLE
check("album title", title.value.decode(), toc["text"]["album_title"])

slots = {"2ch": 0, "multichannel": 1}
for index, want in enumerate(toc["areas"]):
    area = AreaInfo()
    call("sb_disc_area", disc, C.c_uint32(index), C.byref(area))
    check(f"area {index} slot", area.slot, slots[want["slot"]])
    for field in ("toc_lsn", "toc_sectors", "channel_count", "max_byte_rate", "playtime_frames",
                  "audio_start_lsn", "audio_end_lsn"):
        check(f"area {index} {field}", getattr(area, field), want[field])
    check(f"area {index} track_count", area.track_count, len(want["tracks"]))
    for t in want["tracks"]:
        track = TrackInfo()
        call("sb_track_info", disc, C.c_uint32(area.slot), C.c_uint32(t["number"]), C.byref(track))
        for field in ("number", "start_lsn", "length_sectors", "start_frame", "duration_frames"):
            check(f"area {index} track {t['number']} {field}", getattr(track, field), t[field])
    print(f"area {want['slot']}: {area.channel_count} channels, {area.track_count} tracks")

    # Decode track 1 and compare each channel's SHA-256 with the manifest.
    audio = next(a for a in case["expected"]["audio"] if a["area"] == want["slot"])["tracks"][0]
    handle = C.c_void_p()
    call("sb_track_audio_open", disc, C.c_uint32(area.slot), C.c_uint32(1), C.byref(handle))
    channels = lib.sb_track_audio_channels(handle)
    buf = C.create_string_buffer(channels * 4704)
    frame = FrameInfo()
    hashes = [hashlib.sha256() for _ in range(channels)]
    while True:
        status = lib.sb_track_audio_read(handle, disc, buf, C.c_size_t(len(buf)), C.byref(frame))
        if status == END:
            break
        if status != OK:
            sys.exit(f"sb_track_audio_read: status {status}: {lib.sb_last_error_message().decode()}")
        data = buf.raw[:frame.size]
        for c in range(channels):
            hashes[c].update(data[c::channels])
    lib.sb_track_audio_free(handle)
    check(f"area {want['slot']} track 1 audio", [h.hexdigest() for h in hashes], audio["sha256"])
lib.sb_disc_free(disc)

if failures:
    sys.exit("smoke test failed:\n  " + "\n  ".join(failures))
print("smoke test passed")
