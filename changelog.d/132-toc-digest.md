Changed
- Opening a disc is 2.5 to 4 times faster on typical discs: the digests of the TOC sectors
  that `Disc::share` (and `sb_disc_clone`) check a reader against are computed word by
  word in four lanes instead of byte by byte (FNV-1a). They are never stored, so nothing
  saved changes; seek index files keep their FNV-1a digests (#132).
