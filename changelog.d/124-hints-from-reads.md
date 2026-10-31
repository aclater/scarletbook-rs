Changed
- Seek hints are recorded only from the ranges callers read (cursors made from a seek index,
  `TrackCursor::for_range_indexed` included), no longer by `AreaIndex::extend`,
  `Disc::extend_seek_index` and `Disc::verify_speculation`: a pass over a whole area recorded
  a hint every 8 frames, and thinning to `IndexOptions::max_hints` then dropped the hints of
  the positions actually read, which are the ones a later seek needs (#124, #144).
