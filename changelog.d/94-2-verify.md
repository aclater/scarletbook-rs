Added
- Checking provisional readings (#94, part 2; specification 4, 4.2 item 8, sacd-spec
  `82eac6c`). A seek index takes the provisional readings of cursors made from it (through a
  disc's iterators and continuations, `Disc::absorb_seek_checkpoints` or
  `AreaIndex::absorb`) and checks them once its exact checkpoints reach their `L`
  (`AreaIndex::verify`, which `AreaIndex::extend`, `Disc::extend_seek_index` and the new
  `Disc::verify_speculation` run): a reading is `Speculation::Confirmed` when (a) and (b)
  hold, and `Speculation::Refuted` ("possibly different") otherwise, with the frame from
  which its positions are exact in any case (at least the specification's `a*`). Verdicts
  reach the cursors at once (`speculation()`, `speculation_id()`) and an event queue
  (`Disc::take_seek_events`, shared by a disc's clones and shares; `AreaIndex::take_events`),
  and refutations are logged as warnings. At most `MAX_CLAIMS` (256) readings wait per
  index; older ones are dropped as `Speculation::Unverified`. A provisional reading records
  provisional checkpoints, kept apart from the exact ones (not saved, used only by
  provisional starts), from which later provisional starts nearby begin without a search.
