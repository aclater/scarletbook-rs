Added
- TOC timecodes as stored: `Disc::track_timecodes_stored` gives a track's start and
  duration timecodes as stored in `SACDTRL2` (specification 3, 3.7.2), before the clamping
  of 3.9.2 and whatever their range (`Area::playtime` is already the stored play time). In
  the C interface, `SbTimecode` (3 bytes, no padding), `sb_area_playtime_stored` and
  `sb_track_timecodes_stored`. Frame counts are unchanged. Additive on ABI 2, no change to
  public Rust structs. README ctypes bindings (#127, #128).
