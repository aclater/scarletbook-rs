Added
- Provisional reading, cold seeks without a prior pass (#94, part 1; specification 4, 4.2
  item 8, 4.4.5 item 5 and 7.1 item 3, sacd-spec `82eac6c`): `SeekPolicy` (`Exact`, the
  default, or `Speculative(SpeculativeOptions)`), set per disc handle with
  `Disc::set_seek_policy` (clones and shares start with a copy), and the caller-kept forms
  `TrackCursor::for_range_speculative` and `TrackAudioCursor::for_range_speculative`. When
  an exact start would read more than `min_gap_sectors` before a range, a cursor allowed to
  searches the frame records' timecodes (interpolation with a bisection safeguard, a few
  small reads, at most a constant times `log2(n + 1)` sectors) and begins at a checkpoint
  boundary before the range's first frame `u`, with no frame under assembly and `u` as the
  next acceptable frame number. Its positions are marked provisional
  (`ResumedFrom::Speculative`, `Speculation::Pending`): they are exact when no frame numbered
  `u` or more was accepted before `L` and the frame under assembly at `L` is none or numbered
  below `u`, which holds on well-formed discs but cannot be established without reading the
  sectors before `L`. Its malformed count and warnings begin at `L` (`counts_from`).
  Verification and confirmation come in part 2.
