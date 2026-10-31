# SPDX-License-Identifier: MIT OR Apache-2.0
"""Smoke test of a packaged C library through Python's ctypes.

Usage: clib-smoke.py PACKAGE_DIR EXPORTS   (run from the repository root)

EXPORTS lists the sb_* symbols of the shared library's export table, one per line
(clib-test.sh writes it with nm or objdump). Loads the package's shared library, checks
that the functions the header declares are exactly the exported ones and can be looked up,
that sb_abi_version() equals the header's SB_ABI_VERSION and sb_version() the
package version, that the README's ctypes bindings run and match the header, then opens
corpus image dst-features-2ch-6ch, compares its TOC with the
conformance manifest and decodes track 1 of each area against the manifest's per-channel
SHA-256, and reads its master TOC sector (#105). Last, through the README's bindings,
reads ranges of it provisionally (#94) and checks the readings with a disc-kept seek index
and with a caller-kept one.
"""
import ctypes as C
import hashlib
import importlib.util
import json
import pathlib
import re
import sys

pkg = pathlib.Path(sys.argv[1]).resolve()
exports = set(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8").split())
header = (pkg / "include" / "scarletbook.h").read_text(encoding="utf-8")
lib_path = next(p for n in ("scarletbook_ffi.dll", "libscarletbook_ffi.dylib", "libscarletbook_ffi.so")
                for p in [pkg / "lib" / n] if p.exists())
lib = C.CDLL(str(lib_path))
print(f"loaded {lib_path}")

failures = []


def check(what, got, want):
    if got != want:
        failures.append(f"{what}: got {got!r}, want {want!r}")


# The declared functions are the exported ones (on Windows: the DLL's export table). The
# header is read through abi-check.py's normalisation: one declaration per line.
spec = importlib.util.spec_from_file_location("abi_check", pathlib.Path(__file__).with_name("abi-check.py"))
abi_check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(abi_check)
names = set(re.findall(r"^(?!typedef)[^(]*?\b(sb_\w+)\(", abi_check.normalise(header), flags=re.M))
check("functions declared but not in the export table", sorted(names - exports), [])
check("sb_* symbols exported but not declared", sorted(exports - names), [])
check("declared functions ctypes cannot find", sorted(n for n in names if not hasattr(lib, n)), [])
print(f"{len(names)} functions declared, {len(exports)} exported")

lib.sb_abi_version.restype = C.c_uint32
lib.sb_version.restype = C.c_char_p
lib.sb_last_error_message.restype = C.c_char_p
abi = int(re.search(r"^#define SB_ABI_VERSION (\d+)$", header, flags=re.M).group(1))
check("sb_abi_version()", lib.sb_abi_version(), abi)
# scarletbook-ffi-<version>-<target>; the version may carry a prerelease (0.3.0-rc.1).
version = re.fullmatch(r"scarletbook-ffi-(.+)-(x86_64|aarch64)-[a-z0-9_]+-[a-z0-9_]+(-[a-z0-9_]+)?", pkg.name).group(1)
check("sb_version()", lib.sb_version().decode(), version)
print(f"sb_version() {lib.sb_version().decode()}, sb_abi_version() {lib.sb_abi_version()}")


# The bindings of README.md ("Using from Python (ctypes)"), run against this library: they
# pass the version check, their structs are the header's field for field, and each bound
# function takes as many arguments as the header declares.
readme = pathlib.Path("README.md").read_text(encoding="utf-8")
block = re.search(r"^### Using from Python \(ctypes\)\n.*?^```python\n(.*?)^```", readme, flags=re.M | re.S).group(1)
bindings = {}
exec(block.replace('"/path/to/libscarletbook_ffi.so"', repr(str(lib_path))), bindings)
check("README SB_ABI_VERSION", bindings["SB_ABI_VERSION"], int(re.search(r"^#define SB_ABI_VERSION (\d+)$", header, flags=re.M).group(1)))
CTYPES = {"uint8_t": C.c_uint8, "uint16_t": C.c_uint16, "uint32_t": C.c_uint32, "uint64_t": C.c_uint64,
          "int32_t": C.c_int32, "int64_t": C.c_int64, "size_t": C.c_size_t, "const char *": C.c_char_p}
decls = abi_check.normalise(header)
structs = re.findall(r"^typedef struct (\w+) \{\n(.*?)^\} \1;", decls, flags=re.M | re.S)
check("structs", sorted(n for n, _ in structs), sorted(n for n, v in bindings.items()
                                                       if isinstance(v, type) and issubclass(v, C.Structure)))
for name, body in structs:
    # A field is "type name;" or, an array, "type name [ N ];" (as normalised).
    want = [(m.group(2), CTYPES.get(m.group(1).strip()) if m.group(3) is None
             else CTYPES.get(m.group(1).strip()) * int(m.group(3)))
            for m in re.finditer(r"^(.*?)\s*\b(\w+)(?: \[ (\d+) \])?;$", body, flags=re.M)]
    got = [(f, t) for f, t in bindings[name]._fields_] if name in bindings else []
    check(f"README struct {name}", got, want)
for m in re.finditer(r"^(?!typedef)[^(]*?\b(sb_\w+)\((.*)\);$", decls, flags=re.M):
    fn = getattr(bindings["lib"], m.group(1))
    if fn.argtypes is not None:
        params = 0 if m.group(2) == "void" else m.group(2).count(",") + 1
        check(f"README argtypes of {m.group(1)}", len(fn.argtypes), params)
print("README ctypes bindings match the header")


class DiscInfo(C.Structure):
    _fields_ = [("sectors", C.c_uint64), ("sector_size", C.c_uint32), ("version_major", C.c_uint8),
                ("version_minor", C.c_uint8), ("album_set_size", C.c_uint16),
                ("album_sequence_number", C.c_uint16), ("hybrid", C.c_uint8),
                ("manufacturer_sector", C.c_uint8), ("year", C.c_uint16), ("month", C.c_uint8),
                ("day", C.c_uint8), ("area_count", C.c_uint32), ("warning_count", C.c_uint32)]


class AreaInfo(C.Structure):
    _fields_ = [(n, C.c_uint32) for n in (
        "slot", "kind_channels", "toc_lsn", "toc_sectors", "channel_count", "area_config",
        "sample_frequency_code", "frame_format", "max_byte_rate", "playtime_frames",
        "audio_start_lsn", "audio_end_lsn", "track_count")]


class TrackInfo(C.Structure):
    _fields_ = [("number", C.c_uint32), ("start_lsn", C.c_uint32), ("length_sectors", C.c_uint32),
                ("start_frame", C.c_uint32), ("duration_frames", C.c_uint32), ("start_flags", C.c_uint8),
                ("duration_flags", C.c_uint8), ("has_genre", C.c_uint8), ("genre_category", C.c_uint8),
                ("genre_index", C.c_uint8), ("reserved", C.c_uint8 * 3)]


class FrameInfo(C.Structure):
    _fields_ = [("number", C.c_uint32), ("status", C.c_uint32), ("dst_rule", C.c_uint32),
                ("size", C.c_uint32)]


OK, END = 0, 1
for fn in ("sb_disc_open_file", "sb_disc_info", "sb_disc_string", "sb_disc_area", "sb_track_info",
           "sb_track_audio_open", "sb_track_audio_read"):
    getattr(lib, fn).restype = C.c_int32
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

# Logical sectors (#105), through the README's bindings: the master TOC sector holds its
# signature, the image's user data; a range past the last sector fails whole.
sectors = C.create_string_buffer(2 * 2048)
check("sb_disc_read_sectors", bindings["lib"].sb_disc_read_sectors(disc, 510, 1, sectors, 2048), OK)
check("master TOC sector", sectors.raw[:8], b"SACDMTOC")
check("sectors past the end", bindings["lib"].sb_disc_read_sectors(disc, info.sectors - 1, 2, sectors, len(sectors)),
      -18)  # SB_STATUS_SECTOR_RANGE_INVALID
lib.sb_disc_free(disc)

# Provisional reading (#94), through the README's bindings where they bind the call: ranges
# read provisionally equal exact reads, are reported PENDING, and are confirmed by a
# disc-kept seek index (sb_disc_seek_verify, sb_disc_seek_event_next) and by a caller-kept
# one (sb_seek_index_verify, sb_seek_index_event_next).
R, Spec = bindings["lib"], bindings["SbSpeculation"]
const = {n: int(v) for n, v in re.findall(r"^#define (SB_\w+) (\d+)$", header, flags=re.M)}
SPECULATIVE, BUDGET = const["SB_SEEK_POLICY_SPECULATIVE"], const["SB_SEEK_DEFAULT_INLINE_BUDGET"]
PENDING, CONFIRMED = const["SB_SPECULATION_PENDING"], const["SB_SPECULATION_CONFIRMED"]
for name, *args in (("sb_disc_seek_index_enable", C.c_void_p, C.c_uint32, C.c_uint32, C.c_uint64),
                    ("sb_range_audio_open", C.c_void_p, C.c_uint32, C.c_uint32, C.c_uint32, C.POINTER(C.c_void_p)),
                    ("sb_range_audio_open_indexed", C.c_void_p, C.c_uint32, C.c_uint32, C.c_uint32, C.c_void_p,
                     C.POINTER(C.c_void_p)),
                    ("sb_seek_index_new", C.c_void_p, C.c_uint32, C.c_uint32, C.c_uint64, C.POINTER(C.c_void_p)),
                    ("sb_seek_index_extend", C.c_void_p, C.c_void_p, C.c_uint64, C.POINTER(C.c_uint8)),
                    ("sb_track_audio_absorb", C.c_void_p, C.c_void_p)):
    if getattr(R, name).argtypes is None:
        getattr(R, name).restype, getattr(R, name).argtypes = C.c_int32, list(args)
if R.sb_seek_index_free.argtypes is None:
    R.sb_seek_index_free.restype, R.sb_seek_index_free.argtypes = None, [C.c_void_p]


def rcall(fn, *args):
    status = getattr(R, fn)(*args)
    if status not in (OK, END):
        sys.exit(f"{fn}: status {status}: {R.sb_last_error_message().decode()}")
    return status


def read_range(d, handle):
    out, buf, frame = [], C.create_string_buffer(6 * 4704), bindings["SbFrameInfo"]()
    while rcall("sb_track_audio_read", handle, d, buf, len(buf), C.byref(frame)) == OK:
        out.append((frame.number, frame.status, buf.raw[:frame.size]))
    return out


def speculation(handle):
    s = Spec()
    rcall("sb_track_audio_speculation", handle, C.byref(s))
    return s


def exact(first, end):
    d, h = C.c_void_p(), C.c_void_p()
    rcall("sb_disc_open_file", case["input"].encode(), C.byref(d))
    rcall("sb_range_audio_open", d, 0, first, end, C.byref(h))
    got = read_range(d, h)
    check("exact read is exact", speculation(h).state, const["SB_SPECULATION_EXACT"])
    R.sb_track_audio_free(h)
    R.sb_disc_free(d)
    return got


d = C.c_void_p()
rcall("sb_disc_open_file", case["input"].encode(), C.byref(d))
area = bindings["SbAreaInfo"]()
rcall("sb_disc_area", d, 0, C.byref(area))
p = area.playtime_frames
rcall("sb_disc_seek_policy_set", d, 0, SPECULATIVE, 0)
rcall("sb_disc_seek_index_enable", d, 0, 4, BUDGET)
ranges = [(p // 2, p // 2 + 3), (p * 2 // 3, p * 2 // 3 + 3)]
handles = []
for first, end in ranges:
    h = C.c_void_p()
    rcall("sb_range_audio_open", d, 0, first, end, C.byref(h))
    check(f"provisional read of {first}..{end}", read_range(d, h), exact(first, end))
    s = speculation(h)
    check(f"{first}..{end} pending", (s.state, s.first, s.claim > 0), (PENDING, first, True))
    handles.append((h, s.claim))
done = C.c_uint8(0)
while rcall("sb_disc_seek_verify", d, 0, 1 << 20, C.byref(done)) == OK and not done.value:
    pass
events, e = [], Spec()
while rcall("sb_disc_seek_event_next", d, 0, C.byref(e)) == OK:
    events.append((e.claim, e.state))
check("disc-kept index events", sorted(events), sorted((c, CONFIRMED) for _, c in handles))
for h, _ in handles:
    check("confirmed reading", speculation(h).state, CONFIRMED)
    R.sb_track_audio_free(h)
R.sb_disc_free(d)
# A disc without a seek index of its own (one would take the reading), with the caller's.
rcall("sb_disc_open_file", case["input"].encode(), C.byref(d))
rcall("sb_disc_seek_policy_set", d, 0, SPECULATIVE, 0)
index, h = C.c_void_p(), C.c_void_p()
rcall("sb_seek_index_new", d, 0, 4, BUDGET, C.byref(index))
rcall("sb_range_audio_open_indexed", d, 0, p // 2, p // 2 + 2, index, C.byref(h))
check("caller-kept provisional read", read_range(d, h), exact(p // 2, p // 2 + 2))
s = speculation(h)
check("caller-kept pending", s.state, PENDING)
rcall("sb_track_audio_absorb", h, index)
while rcall("sb_seek_index_extend", d, index, 1 << 20, C.byref(done)) == OK and not done.value:
    pass
settled = C.c_uint32(99)
rcall("sb_seek_index_verify", d, index, C.byref(settled))
check("caller-kept index: unsettled readings", settled.value, 0)
check("caller-kept index event", rcall("sb_seek_index_event_next", index, C.byref(e)), OK)
check("caller-kept index verdict", (e.claim, e.state), (s.claim, CONFIRMED))
check("caller-kept reading", speculation(h).state, CONFIRMED)
R.sb_track_audio_free(h)
R.sb_seek_index_free(index)
R.sb_disc_free(d)
print(f"provisional reading: {len(ranges) + 1} ranges read provisionally and confirmed")

if failures:
    sys.exit("smoke test failed:\n  " + "\n  ".join(failures))
print("smoke test passed")
