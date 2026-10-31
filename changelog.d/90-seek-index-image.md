Changed
- A seek index is of one image (#90): within a process, an `AreaIndex` (and an
  `SbSeekIndex`) is of the image of the first cursor it takes checkpoints from or the first
  `extend`, and refuses cursors, reads and indexes of any other image of the same size and
  layout with `Error::InvalidArgument` (`SB_STATUS_INVALID_ARGUMENT`; a refused cursor stops
  recording), so that two images can no longer mix their state in one index. The identity
  is not saved: a loaded index takes the image of its first absorb or extend.
  `AreaIndex::absorb` takes the cursor by `&mut` and drops what it took from it;
  `sb_track_audio_absorb` and `sb_raw_frames_absorb` do likewise. Docs: keep one index per
  image; loading is safe, not exact.
