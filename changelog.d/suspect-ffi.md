Added
- The suspect mark through the C interface (specification 4, 4.2 item 8, sacd-spec
  `eafaaba`): `sb_track_audio_suspect` and `sb_raw_frames_suspect` write an `SB_SUSPECT_*`
  reason (`SB_SUSPECT_NONE`, `SB_SUSPECT_AREA_LAYOUT`, `SB_SUSPECT_FRAME_NUMBERS_DECREASE`,
  `SB_SUSPECT_OTHER`), and `sb_disc_seek_policy_set` takes
  `SB_SEEK_POLICY_SPECULATIVE_EXACT_WHEN_SUSPECT`. Additive on ABI 2 (sacd-spec#37, #153)
