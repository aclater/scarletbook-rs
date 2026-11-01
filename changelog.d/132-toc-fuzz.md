Added
- The `toc` fuzz target also reads every input that opens through the C interface's
  `sb_disc_toc` (a 64 KiB buffer, then one of exactly the size given, then one byte short),
  through `scarletbook-ffi`'s hidden `fuzz_disc_toc` behind a non-default `fuzzing`
  feature that the C library does not enable (#132, #150).
