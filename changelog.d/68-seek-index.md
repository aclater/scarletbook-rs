Added
- Seeking within an area (#68, #73): `AreaIndex` keeps the reading state of an area every
  `K` sectors (default 64), and `TrackCursor::for_range_indexed` /
  `TrackAudioCursor::for_range_indexed` start a range at its last checkpoint whose next
  acceptable frame number is at most the range's first frame, instead of at the area's
  first sector (sacd-spec `da6b900`: specification 4, 4.2 item 7, 4.4.5 items 2 and 4, 4.5
  item 6 and 4.7.1 item 6, and 7.1 items 2 and 3). Results, missing counts, malformed
  counts and warnings equal those of a scan from the first sector; only reads and log lines
  differ. Once the index covers a range, opening it reads at most `W + K` sectors (`W` = 32)
  before the range's own frames, wherever it lies. Indexes are built lazily by the cursors
  made from them (`AreaIndex::absorb`) or eagerly in steps (`AreaIndex::extend`). Frames
  under assembly spread over more than `W` sectors are stored up to an inline budget
  (default 1 MiB). Opt-in; no change to existing cursors.
