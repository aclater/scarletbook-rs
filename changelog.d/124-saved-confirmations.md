Added
- Saved confirmations (specification 4, 4.2 item 9, sacd-spec `cb9530b`): when verification
  confirms a provisional reading, a seek index bound to a disc keeps (`L`, `u_L`) and the digest
  of sector `L`, saves them (format version 3; versions 1 and 2 still load and are still
  written when there is nothing to add), and after a remount reports a reading from that hint
  for positions `u_L` or more `Speculation::Confirmed` at once, when sector `L` has that digest.
  `AreaIndex::confirmations`; `AreaIndex::without_checkpoints` keeps only the hints,
  confirmations and fingerprint, a few kilobytes to save per area (#124)
