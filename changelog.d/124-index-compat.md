Changed
- `Disc::set_seek_index` refuses an index saved by a disc whose TOC fingerprint differs: one of
  another disc of the same layout (which 0.5 accepted and rebound to this disc), and one saved
  from a rip in another sector format (2054 or 2064 bytes against 2048) of the same disc.
  Every index a disc keeps now has that fingerprint, so its saved form is format version 2,
  which 0.5.1 and earlier refuse ("version not supported"): a cache of saved indexes shared
  with an older release stops loading there. `AreaIndex::from_bytes` keeps at most
  `max_inline_budget / 8` seek hints, now and as the index grows, in addition to the stored
  frames (#124, #133).
