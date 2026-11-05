Fixed
- `AreaIndex::from_bytes` refuses a saved index (format version 3) with more confirmations than its saved maximum of seek hints. It used to load it and drop them silently, so that saving the index gave other bytes. The library never writes such an index, so saved indexes keep loading (#253).
