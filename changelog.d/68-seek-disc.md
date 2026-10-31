Added
- Seek indexes kept by a `Disc` (#68), opt-in per area: `Disc::enable_seek_index`,
  `set_seek_index`, `take_seek_index`, `seek_index`, `extend_seek_index` and
  `absorb_seek_checkpoints`. With one enabled, every track and range cursor the disc makes
  starts from it, its borrowing iterators and continuations hand recorded checkpoints back
  as they read, and a continuation resumes from the index instead of the area's first
  sector when it cannot reuse the earlier cursor (or jumps ahead when the index is more than
  two checkpoints ahead of it). Results are those without an index; the opening is logged
  "from the seek index" at Info, the resume point at Debug. The indexes live in the state
  that discs of one image share (`Disc::share`, `clone`, #74): every disc of the image, on
  any thread, uses and grows the same index (a read lock to make a cursor, a write lock to
  take checkpoints). README section "Seeking within an area".
