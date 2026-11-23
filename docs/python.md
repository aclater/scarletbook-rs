<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Using from Python (ctypes)

The C library of the [release assets](releases.md#release-assets) can be used from Python
without a compiled extension. The C interface itself is described in the
[C ABI guide](c-abi.md).

The library loads with Python's `ctypes`; there are no variadic functions, and every
type has a fixed ctypes counterpart. Check the ABI version first: the bindings below are
written for `SB_ABI_VERSION` 2, and a library of another ABI version must not be used
with them (see [ABI compatibility](c-abi.md#abi-compatibility)). Give every function its `argtypes` and `restype`:
without them ctypes passes and returns C `int`, which truncates pointers and 64-bit values.

```python
import ctypes as C

# libscarletbook_ffi.2.dylib on macOS, scarletbook_ffi.dll on Windows; before 0.6.0,
# libscarletbook_ffi.so and libscarletbook_ffi.dylib
lib = C.CDLL("/path/to/libscarletbook_ffi.so.2")

SB_ABI_VERSION = 2  # the SB_ABI_VERSION of the header these bindings follow
lib.sb_abi_version.argtypes, lib.sb_abi_version.restype = [], C.c_uint32
if lib.sb_abi_version() != SB_ABI_VERSION:
    raise ImportError(f"scarletbook C ABI {lib.sb_abi_version()}, these bindings need {SB_ABI_VERSION}")

# The structs, field for field as in scarletbook.h (flags are uint8_t, 1 or 0).
class SbDiscInfo(C.Structure):
    _fields_ = [("sectors", C.c_uint64), ("sector_size", C.c_uint32), ("version_major", C.c_uint8),
                ("version_minor", C.c_uint8), ("album_set_size", C.c_uint16),
                ("album_sequence_number", C.c_uint16), ("hybrid", C.c_uint8),
                ("manufacturer_sector", C.c_uint8), ("year", C.c_uint16), ("month", C.c_uint8),
                ("day", C.c_uint8), ("area_count", C.c_uint32), ("warning_count", C.c_uint32)]

class SbAreaInfo(C.Structure):
    _fields_ = [(name, C.c_uint32) for name in (
        "slot", "kind_channels", "toc_lsn", "toc_sectors", "channel_count", "area_config",
        "sample_frequency_code", "frame_format", "max_byte_rate", "playtime_frames",
        "audio_start_lsn", "audio_end_lsn", "track_count")]

class SbTrackInfo(C.Structure):
    _fields_ = [("number", C.c_uint32), ("start_lsn", C.c_uint32), ("length_sectors", C.c_uint32),
                ("start_frame", C.c_uint32), ("duration_frames", C.c_uint32),
                ("start_flags", C.c_uint8), ("duration_flags", C.c_uint8), ("has_genre", C.c_uint8),
                ("genre_category", C.c_uint8), ("genre_index", C.c_uint8), ("reserved", C.c_uint8 * 3)]

class SbWarning(C.Structure):
    _fields_ = [("lsn", C.c_int64), ("slot", C.c_int32), ("track", C.c_uint32), ("code", C.c_char_p)]

class SbFrameInfo(C.Structure):
    _fields_ = [("number", C.c_uint32), ("status", C.c_uint32), ("dst_rule", C.c_uint32),
                ("size", C.c_uint32)]

class SbTimecode(C.Structure):
    _fields_ = [("minutes", C.c_uint8), ("seconds", C.c_uint8), ("frames", C.c_uint8)]

class SbRawFrameInfo(C.Structure):
    _fields_ = [("number", C.c_uint32), ("kind", C.c_uint32), ("size", C.c_uint32)]

class SbGenre(C.Structure):
    _fields_ = [("category", C.c_uint8), ("index", C.c_uint8)]

class SbTextChannel(C.Structure):  # language: the code's two bytes as stored
    _fields_ = [("language", C.c_uint8 * 2), ("charset", C.c_uint8), ("reserved", C.c_uint8)]

class SbAreaDetails(C.Structure):
    _fields_ = [(name, C.c_uint8) for name in (
        "version_major", "version_minor", "max_available_channels", "mute_flags",
        "track_attribute", "track_offset", "text_channel_count", "text_charset")]

class SbAreaCapacity(C.Structure):  # frames_per_sector: inf when audio_sectors is 0
    _fields_ = [("audio_sectors", C.c_uint64), ("capacity", C.c_uint64), ("plain_frame_bound", C.c_uint64),
                ("frames_per_sector", C.c_double), ("playtime_frames", C.c_uint32), ("max_track_end", C.c_uint32),
                ("has_plain_frame_bound", C.c_uint8), ("reserved", C.c_uint8 * 7)]

# The layout of sb_disc_toc (format 1.0); see "The whole TOC in one call" below.
class SbTocStr(C.Structure):
    _fields_ = [("offset", C.c_uint32), ("length", C.c_uint32)]

class SbTocHeader(C.Structure):
    _fields_ = [("magic", C.c_uint8 * 8), ("major", C.c_uint16), ("minor", C.c_uint16), ("size", C.c_uint32),
                ("disc_offset", C.c_uint32), ("disc_record_size", C.c_uint16),
                ("area_record_size", C.c_uint16), ("track_record_size", C.c_uint16),
                ("warning_record_size", C.c_uint16), ("areas_offset", C.c_uint32),
                ("area_count", C.c_uint32), ("warnings_offset", C.c_uint32),
                ("warning_count", C.c_uint32), ("strings_offset", C.c_uint32),
                ("strings_size", C.c_uint32), ("reserved", C.c_uint32)]

class SbTocDisc(C.Structure):
    _fields_ = [("info", SbDiscInfo), ("album_genres", SbGenre * 4), ("disc_genres", SbGenre * 4),
                ("text_channel_count", C.c_uint8), ("text_charset", C.c_uint8), ("disc_flags", C.c_uint8),
                ("reserved", C.c_uint8), ("text_channels", SbTextChannel * 8), ("reserved2", C.c_uint32),
                ("strings", SbTocStr * 18), ("catalog_numbers_stored", SbTocStr * 2),
                ("manufacturer", SbTocStr)]

class SbTocArea(C.Structure):
    _fields_ = [("info", SbAreaInfo), ("details", SbAreaDetails), ("loudspeaker_layout", C.c_uint32),
                ("channel_speakers", C.c_uint8 * 6), ("playtime_stored", SbTimecode),
                ("reserved", C.c_uint8), ("reserved2", C.c_uint16), ("tracks_offset", C.c_uint32),
                ("strings", SbTocStr * 4), ("text_channels", SbTextChannel * 10)]

class SbTocTrack(C.Structure):
    _fields_ = [("info", SbTrackInfo), ("start_stored", SbTimecode), ("duration_stored", SbTimecode),
                ("reserved", C.c_uint8 * 2), ("strings", SbTocStr * 16), ("reserved2", C.c_uint32)]

class SbTocWarning(C.Structure):
    _fields_ = [("lsn", C.c_int64), ("slot", C.c_int32), ("track", C.c_uint32), ("code", SbTocStr)]

class SbSpeculation(C.Structure):
    _fields_ = [("claim", C.c_uint64), ("lsn", C.c_uint64), ("slot", C.c_uint32), ("state", C.c_uint32),
                ("first", C.c_uint32), ("last_returned", C.c_uint32), ("exact_from", C.c_uint32),
                ("reserved", C.c_uint32)]

# DSF and DSDIFF output; see "Output files" in docs/c-abi.md.
class SbComment(C.Structure):
    _fields_ = [("text", C.c_char_p), ("year", C.c_uint32), ("kind", C.c_uint32), ("reference", C.c_uint32),
                ("month", C.c_uint8), ("day", C.c_uint8), ("hour", C.c_uint8), ("minutes", C.c_uint8)]

class SbMarker(C.Structure):
    _fields_ = [("text", C.c_char_p), ("frame", C.c_uint32), ("kind", C.c_uint32),
                ("track_flags", C.c_uint32), ("reserved", C.c_uint32)]

class SbOutputLayoutInfo(C.Structure):
    _fields_ = [("size", C.c_uint64), ("memory", C.c_uint64)] + [(name, C.c_uint32) for name in (
        "format", "slot", "first", "end", "channels", "source_track", "input", "regions")]

class SbOutputRegion(C.Structure):
    _fields_ = [("offset", C.c_uint64), ("len", C.c_uint64), ("kind", C.c_uint32), ("reserved", C.c_uint32)]

class SbPositionData(C.Structure):
    _fields_ = [("content", C.POINTER(C.c_uint8)), ("content_len", C.c_size_t),
                ("dst_equivalent", C.POINTER(C.c_uint8)), ("dst_equivalent_len", C.c_size_t),
                ("number", C.c_uint32), ("reserved", C.c_uint32)]

class SbDsfChainFile(C.Structure):
    _fields_ = [("byte_first", C.c_uint64), ("byte_end", C.c_uint64), ("first", C.c_uint32),
                ("end", C.c_uint32), ("chain_first", C.c_uint32), ("chain_end", C.c_uint32),
                ("status", C.c_int32), ("reserved", C.c_uint32)]

# Handles are opaque pointers; SbStatus is an int32_t (0 OK, 1 END, 2 ABSENT, negative:
# failure); size_t is only the length of a buffer of yours.
H, S, P, U32, SIZE = C.c_void_p, C.c_int32, C.POINTER, C.c_uint32, C.c_size_t

def bind(name, restype, *argtypes):
    f = getattr(lib, name)
    f.restype, f.argtypes = restype, list(argtypes)

bind("sb_version", C.c_char_p)
bind("sb_build_info", C.c_char_p)
bind("sb_last_error_message", C.c_char_p)
bind("sb_status_name", C.c_char_p, C.c_int32)
bind("sb_disc_open_file", S, C.c_char_p, P(H))
bind("sb_disc_open_memory", S, C.c_char_p, SIZE, P(H))
bind("sb_disc_free", None, H)
bind("sb_disc_info", S, H, P(SbDiscInfo))
bind("sb_disc_string", S, H, U32, C.c_char_p, SIZE, P(SIZE))
bind("sb_disc_area", S, H, U32, P(SbAreaInfo))
bind("sb_area_loudspeaker_layout", S, H, U32, P(U32))
bind("sb_area_channel_speaker", S, H, U32, U32, P(U32))
bind("sb_disc_warning", S, H, U32, P(SbWarning))
bind("sb_disc_read_sectors", S, H, C.c_uint64, U32, C.c_char_p, SIZE)
bind("sb_disc_genre", S, H, U32, U32, P(SbGenre))
bind("sb_disc_text_channel_count", S, H, P(U32))
bind("sb_disc_text_channel", S, H, U32, P(SbTextChannel))
bind("sb_disc_catalog_number_stored", S, H, U32, C.c_char_p, SIZE, P(SIZE))
bind("sb_disc_manufacturer", S, H, C.c_char_p, SIZE, P(SIZE))
bind("sb_area_details", S, H, U32, P(SbAreaDetails))
bind("sb_area_capacity", S, H, U32, P(SbAreaCapacity))
bind("sb_area_text_channel", S, H, U32, U32, P(SbTextChannel))
bind("sb_track_info", S, H, U32, U32, P(SbTrackInfo))
bind("sb_area_playtime_stored", S, H, U32, P(SbTimecode))
bind("sb_track_timecodes_stored", S, H, U32, U32, P(SbTimecode), P(SbTimecode))
bind("sb_track_string", S, H, U32, U32, U32, C.c_char_p, SIZE, P(SIZE))
bind("sb_disc_toc", S, H, U32, C.c_char_p, SIZE, P(SIZE))
bind("sb_track_check", S, H, U32, U32)
bind("sb_track_audio_open", S, H, U32, U32, P(H))
bind("sb_track_audio_open_next", S, H, H, U32, U32, P(H))
bind("sb_track_audio_channels", U32, H)
bind("sb_track_audio_read", S, H, H, C.c_char_p, SIZE, P(SbFrameInfo))
bind("sb_track_audio_counts", S, H, P(C.c_uint64), P(C.c_uint64))
bind("sb_track_audio_free", None, H)
bind("sb_pool_new", S, U32, P(H))
bind("sb_pool_free", None, H)
bind("sb_track_audio_set_pool", S, H, H)
bind("sb_raw_frames_open", S, H, U32, U32, U32, P(H))
bind("sb_raw_frames_read", S, H, H, C.c_char_p, SIZE, P(SbRawFrameInfo))
bind("sb_raw_frames_free", None, H)
bind("sb_dst_decode", S, C.c_char_p, SIZE, U32, C.c_char_p, SIZE, P(U32))
bind("sb_pcm_converter_new", S, U32, U32, U32, C.c_double, P(H))
bind("sb_pcm_converter_free", None, H)
bind("sb_pcm_samples_per_frame", S, H, P(U32))
bind("sb_pcm_convert_run", S, H, C.c_char_p, C.c_char_p, SIZE, C.c_char_p, U32, C.c_void_p, SIZE,
     P(C.c_uint64))
bind("sb_disc_seek_policy_set", S, H, U32, U32, C.c_uint64)
bind("sb_track_audio_speculation", S, H, P(SbSpeculation))
bind("sb_raw_frames_speculation", S, H, P(SbSpeculation))
bind("sb_track_audio_suspect", S, H, P(U32))
bind("sb_raw_frames_suspect", S, H, P(U32))
bind("sb_disc_seek_verify", S, H, U32, C.c_uint64, P(C.c_uint8))
bind("sb_disc_seek_event_next", S, H, U32, P(SbSpeculation))
bind("sb_seek_index_verify", S, H, H, P(U32))
bind("sb_seek_index_event_next", S, H, P(SbSpeculation))
bind("sb_disc_cancel_token", S, H, P(H))
bind("sb_cancel_request", S, H)
bind("sb_cancel_withdraw", S, H)
bind("sb_cancel_requested", S, H, P(C.c_uint8))
bind("sb_cancel_free", None, H)
bind("sb_track_audio_progress", S, H, P(H))
bind("sb_raw_frames_progress", S, H, P(H))
bind("sb_progress_get", S, H, P(C.c_uint64), P(C.c_uint64))
bind("sb_progress_free", None, H)
bind("sb_track_audio_keep_stored", S, H)
bind("sb_track_audio_read_with_stored", S, H, H, C.c_char_p, SIZE, P(SbFrameInfo),
     C.c_char_p, SIZE, P(SbRawFrameInfo), C.c_uint8)
bind("sb_output_options_new", S, P(H))
bind("sb_output_options_free", None, H)
bind("sb_output_options_set", S, H, U32, C.c_int64)
bind("sb_output_options_set_string", S, H, U32, C.c_char_p)
bind("sb_output_options_set_order", S, H, U32, P(U32), SIZE)
bind("sb_output_options_set_id3_frame_order", S, H, C.c_char_p, SIZE)
bind("sb_output_options_set_channel_numbers", S, H, P(C.c_uint16), SIZE)
bind("sb_output_options_add_comment", S, H, P(SbComment))
bind("sb_output_options_clear_comments", S, H)
bind("sb_output_options_add_marker", S, H, P(SbMarker))
bind("sb_output_options_clear_markers", S, H)
bind("sb_output_options_set_tags", S, H, H)
bind("sb_output_options_set_raw_tag", S, H, C.c_char_p, SIZE)
bind("sb_tags_new", S, P(H))
bind("sb_tags_from_toc", S, H, U32, U32, H, P(H))
bind("sb_tags_set", S, H, U32, C.c_char_p)
bind("sb_tags_push", S, H, U32, C.c_char_p)
bind("sb_tags_remove", S, H, U32)
bind("sb_tags_set_custom", S, H, C.c_char_p, C.c_char_p)
bind("sb_tags_count", S, H, U32, P(U32))
bind("sb_tags_get", S, H, U32, U32, C.c_char_p, SIZE, P(SIZE))
bind("sb_tags_free", None, H)
bind("sb_dst_lengths_scan", S, H, U32, U32, U32, U32, P(H))
bind("sb_dst_lengths_load", S, H, U32, U32, U32, U32, P(U32), SIZE, P(H))
bind("sb_dst_lengths_get", S, H, P(U32), SIZE, P(SIZE))
bind("sb_dst_lengths_free", None, H)
bind("sb_output_layout_new", S, H, U32, U32, U32, U32, H, P(H))
bind("sb_output_layout_new_dst", S, H, H, H, P(H))
bind("sb_output_layout_info", S, H, P(SbOutputLayoutInfo))
bind("sb_output_layout_region", S, H, U32, P(SbOutputRegion))
bind("sb_output_layout_positions", S, H, C.c_uint64, C.c_uint64, P(U32), P(U32))
bind("sb_output_layout_read", S, H, H, C.c_uint64, C.c_char_p, SIZE, P(SIZE))
bind("sb_output_layout_fill", S, H, C.c_uint64, C.c_char_p, SIZE, P(SbPositionData), SIZE, P(SIZE))
bind("sb_output_layout_free", None, H)
bind("sb_output_dsf_chain_plan", S, H, U32, P(U32), SIZE, P(SbDsfChainFile), SIZE, P(SIZE))
```

The other functions bind the same way from their declarations in `scarletbook.h`. Then:

```python
def check(status):
    """Raises on a failure, with the calling thread's error message."""
    if status < 0:
        raise RuntimeError(f"{lib.sb_status_name(status).decode()}: {lib.sb_last_error_message().decode()}")
    return status

def disc_string(disc, field):
    """A disc string (SbDiscString), or None when absent: size it, then copy it."""
    n = C.c_size_t()
    if lib.sb_disc_string(disc, field, None, 0, C.byref(n)) == 2:  # SB_STATUS_ABSENT
        return None
    buf = C.create_string_buffer(n.value + 1)
    check(lib.sb_disc_string(disc, field, buf, len(buf), C.byref(n)))
    return buf.raw[:n.value].decode()  # may contain U+0000: use the length, not the NUL

disc = H()
check(lib.sb_disc_open_file("disc.iso".encode(), C.byref(disc)))  # a UTF-8 path
try:
    info = SbDiscInfo()
    check(lib.sb_disc_info(disc, C.byref(info)))
    title = disc_string(disc, 0)  # SB_DISC_STRING_ALBUM_TITLE
    audio = H()
    check(lib.sb_track_audio_open(disc, 0, 1, C.byref(audio)))  # 2-channel area, track 1
    try:
        buf = C.create_string_buffer(lib.sb_track_audio_channels(audio) * 4704)
        frame = SbFrameInfo()
        while check(lib.sb_track_audio_read(audio, disc, buf, len(buf), C.byref(frame))) == 0:
            dsd = buf.raw[:frame.size]  # interleaved DSD, channels x 4704 bytes
    finally:
        lib.sb_track_audio_free(audio)
finally:
    lib.sb_disc_free(disc)
```

ctypes releases the GIL for each call, so conversions on several Python threads run in
parallel: give each thread its own disc (or a clone of one, `sb_disc_clone`) and handles
(a handle may move between threads but not be used by two at once; an `SbPool`,
`SbCancel` or `SbProgress` may be shared), and read `sb_last_error_message()`
on the thread whose call failed, before that thread's next call, as `check` does. Every
handle has its `_free` function; nothing else needs freeing (the strings the library
returns are static or copied into your buffers). Callbacks (`sb_disc_open_callbacks`,
`sb_disc_set_log`) are `ctypes.CFUNCTYPE` objects: keep a reference to them until the disc
is freed, and catch every exception inside them: ctypes only prints it, and the library then gets
an unspecified value (for a read callback, return -1 instead). A log callback can be
called from a pool's threads (see `sb_disc_set_log`).
`.github/scripts/clib-smoke.py` checks the first block above against the header and runs it.

## The whole TOC in one call

Reading a TOC field by field takes about 100 to 160 calls for a typical disc. From Python,
each call costs far more than the library spends on it. `sb_disc_toc` writes the whole TOC
into one buffer instead: what the disc, area, track, string, genre, text channel,
timecode and warning queries return, in a fixed, versioned layout (#132). It does no I/O,
since it uses the TOC parsed at open.

- **Layout.** `SbTocHeader` sits at offset 0. It gives the offset of the `SbTocDisc` record,
  of the `SbTocArea` records (in `sb_disc_area` order) and of the `SbTocWarning` records,
  with each kind's record size. Each area's `tracks_offset` points to its `SbTocTrack`
  records, in track number order. Strings are `SbTocStr` refs (`offset`, `length`) into
  the string area at the end. Each string is followed by a NUL that `length` does not
  count. `offset == SB_TOC_ABSENT` (0xFFFFFFFF) marks an absent string, where the
  per-field call would return `SB_STATUS_ABSENT`.
- **Byte order and alignment.** Every integer is little-endian on every host. Every
  field sits at a fixed offset that is a multiple of its size, and every record size is a
  multiple of 8. So a buffer aligned to 8 bytes can be read through the structs on a
  little-endian host, or parsed with `struct.unpack_from("<...")` anywhere.
- **Versions.** The header carries the magic `SBTOC\0\0\0`, `major` (`SB_TOC_MAJOR`, 1)
  and `minor` (`SB_TOC_MINOR`, 0). Refuse any other major version. A minor version only
  appends fields to the end of records, so step through records by the header's record
  sizes, not by `sizeof`. The format 1.0 sizes are 56, 256, 152, 168 and 24 bytes
  (header, disc, area, track, warning).
- **Size.** `flags` must be 0. `*len` receives the layout's size. If the buffer is NULL or
  smaller than that, the call writes nothing and returns `SB_STATUS_BUFFER_TOO_SMALL`. The
  recommended single-call pattern is to pass a 64 KiB buffer, which is enough for typical
  discs (a few KB), and to call again with `*len` bytes only when that is too small. A disc
  of 2 x 255 titled tracks needs about 97 KB.

```python
buf = C.create_string_buffer(64 * 1024)  # reuse it from disc to disc
n = C.c_size_t()
status = lib.sb_disc_toc(disc, 0, buf, len(buf), C.byref(n))
if status == -11:  # SB_STATUS_BUFFER_TOO_SMALL
    buf = C.create_string_buffer(n.value)
    status = lib.sb_disc_toc(disc, 0, buf, len(buf), C.byref(n))
check(status)
raw = memoryview(buf)[:n.value]

head = SbTocHeader.from_buffer(buf)  # create_string_buffer memory is malloc'd: 8-aligned
if bytes(head.magic) != b"SBTOC\0\0\0" or head.major != 1:
    raise RuntimeError("unknown sb_disc_toc layout")

def text(ref):
    return None if ref.offset == 0xFFFFFFFF else bytes(raw[ref.offset:ref.offset + ref.length]).decode()

d = SbTocDisc.from_buffer(buf, head.disc_offset)
album_title = text(d.strings[0])  # SB_DISC_STRING_ALBUM_TITLE
for i in range(head.area_count):
    a = SbTocArea.from_buffer(buf, head.areas_offset + i * head.area_record_size)
    for k in range(a.info.track_count):
        t = SbTocTrack.from_buffer(buf, a.tracks_offset + k * head.track_record_size)
        title = text(t.strings[0])  # SB_TRACK_STRING_TITLE
```

The overlay above assumes a little-endian host. On any host, `struct.unpack_from("<I", raw,
offset)` reads the same fields at the offsets documented in `scarletbook.h`.
