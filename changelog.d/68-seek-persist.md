Added
- Saved seek indexes (#68, #83): `AreaIndex::to_bytes` and `AreaIndex::from_bytes` (a
  versioned little-endian format with the area's key and a checksum; every count, bound and
  invariant is checked before anything is allocated, so untrusted bytes give an error; the
  loaded inline budget is at most the caller's, stored frames beyond it are replayed instead),
  and `AreaIndex::merge` for indexes built apart. A cursor compares each checkpoint's sector
  digest with the sector it reads there, and on a mismatch (a stale index, or one of another
  image of the same size) discards the index, logs a warning and scans from the area's first
  sector (`ResumedFrom::Discarded`); `extend` on such an index fails. A `seek_index` fuzz
  target loads untrusted index bytes and reads through them.
- Seek index refinements (#68, review of #73): the checkpoints within one frame under
  assembly share one stored buffer (one frame's bytes, however many checkpoints it spans);
  a read error during a replay no longer counts replayed malformed sectors twice;
  `AreaIndex::absorb` takes any `SeekCursor` (a sealed trait of `AreaCursor`,
  `TrackCursor` and `TrackAudioCursor`) instead of `AsRef<AreaCursor>`; the documentation
  says what the sector digest catches and that a cursor continued backwards without its
  index scans from the area's first sector.
