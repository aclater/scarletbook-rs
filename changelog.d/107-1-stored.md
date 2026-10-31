Added
- Stored and decoded frames of one range in one pass (#107, part 1):
  `TrackAudioCursor::keep_stored` (also on `TrackAudio`), then
  `TrackAudioCursor::next_frame_with_stored` / `TrackAudio::next_with_stored` give each
  position decoded, as `next_frame` does (on the cursor's pool, with the same positions held
  ahead), and its stored content, as `TrackCursor::next_frame` gives it, from one read of the
  area's sectors. The new `Stored` (a DST frame's bytes, `Plain` or `Missing`; a plain
  frame's bytes are the decoded data, not copied again) with `Stored::content` and
  `Stored::bytes`; `TrackAudioCursor::dst_equivalent`. Continuations keep the mode. The
  `pipeline` benchmark has `/stored` cases.
