Added
- C ABI: `sb_seek_index_without_damage` gives a copy of a seek index without the reading state it holds past unreadable sectors, to save for a library version that does not load that state (the C counterpart of `AreaIndex::without_damage`). An ABI 2 addition (#443, #448).
