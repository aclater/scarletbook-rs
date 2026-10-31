Added
- Seek indexes in the C interface (#68, part 4): the `SbSeekIndex` handle (`sb_seek_index_new`,
  `_extend`, `_frontier`, `_memory`, `_merge`, `_save`, `_load`, `_free`),
  `sb_range_audio_open_indexed`, `sb_range_audio_open_next_indexed`,
  `sb_raw_frames_open_indexed`, `sb_raw_frames_open_next_indexed`, `sb_track_audio_absorb`
  and `sb_raw_frames_absorb`, and `SB_SEEK_DEFAULT_INTERVAL`, `SB_SEEK_MAX_INTERVAL` and
  `SB_SEEK_DEFAULT_INLINE_BUDGET`; `sb_seek_index_load` takes the caller's inline budget.
  A disc can keep the index itself (`sb_disc_seek_index_enable`, `_set`, `_get`,
  `_disable`, `_extend`), shared with its clones (`sb_disc_clone`): handles opened through
  any of them start from it and their reads build it. Additive: the ABI version stays 1. The `_open_next`
  forms log while `prev` is still borrowed and run no callback once it is taken (#43).
