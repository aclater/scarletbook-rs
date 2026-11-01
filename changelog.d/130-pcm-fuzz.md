Added
- The `demux` fuzz target also converts one input in 4096 to PCM: the whole run must equal
  each frame converted with its neighbours and the run split in two, wrong lengths must be
  refused, and the C interface's `sb_pcm_converter_new` and `sb_pcm_convert_run` (through
  `scarletbook-ffi`'s hidden `fuzz_pcm_convert_run` behind the non-default `fuzzing`
  feature) must refuse the same arguments and give the same bytes and clip counts in both
  formats, writing nothing outside the output (#130, #PR).
