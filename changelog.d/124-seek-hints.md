Added
- Seek hints (specification 4, 4.2 item 8 locator samples, sacd-spec `cb9530b`): a seek
  index records, from every cursor made from it, sectors with the frame number of their first
  frame record (`IndexOptions::hint_spacing`, default every 8 frames, and `max_hints`,
  default 2 048, `with_hints`; `AreaIndex::hints`; `seek::DEFAULT_HINT_SPACING`,
  `DEFAULT_MAX_HINTS`, `MAX_HINTS`, `HINT_REACH`). They are saved with the index (format
  version 2; version 1 still loads and is still written when there is nothing to add), so
  that after a remount a provisional reading near a range read before begins at a hint with
  one read call through the next hint, without a search. Its positions stay provisional
  (`Speculation::Pending`). A hint whose sector does not hold its frame record is dropped with
  a warning, and the cursor searches instead. An index kept by a disc carries a fingerprint
  of the disc's TOC (master and area TOC fields, sector count and format), saved with it:
  `Disc::set_seek_index` refuses an index of a disc with another TOC. `scarletbook-seekcost`
  measures seeks after a remount (#124, #133).
