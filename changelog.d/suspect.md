Added
- Suspect provisional readings (specification 4, 4.2 item 8, sacd-spec `eafaaba`): `suspect()`
  on `AreaCursor`, `TrackCursor`, `TrackAudioCursor`, `TrackFrames` and `TrackAudio` gives a
  `Suspect` reason for a provisional reading the library doubts: `AreaLayout` (the area's TOC
  places its audio past the image's end, or, for cursors a disc makes, over the other area's
  TOC or audio sectors) or `FrameNumbersDecrease` (the first frame records of the sectors read
  while choosing `L` decrease). The mark is informative and changes no verdict.
  `SpeculativeOptions::exact_when_suspect` (`with_exact_when_suspect`, default off) reads
  such areas and ranges exactly instead. The default policy is unchanged (sacd-spec#37, #151).
