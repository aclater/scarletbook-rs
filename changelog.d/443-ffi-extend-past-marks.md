Added
- C ABI: `sb_disc_seek_index_extend_past_marks` and `sb_seek_index_extend_past_marks` build a seek index's reading state past unreadable sectors ahead of the reads (the C counterparts of `Disc::extend_seek_index_past_marks` and `AreaIndex::extend_past_marks`). An ABI 2 addition (#443, #451).
