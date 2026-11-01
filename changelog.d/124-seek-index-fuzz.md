Added
- The `seek_index` fuzz target also reads speculatively from a loaded index's seek hints,
  and has a synthetic disc keep the index (binding its TOC fingerprint, so that saved
  confirmations are used) and read and verify through it; readings from seek hints alone
  must equal reads without an index from their first exact position on. New seeds hold that
  disc's hints and confirmations (#124).
