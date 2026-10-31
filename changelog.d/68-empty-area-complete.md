Fixed
- `Disc::extend_seek_index` and `Disc::verify_speculation` never reported complete for an
  area without existing audio sectors (its first audio LSN at or past the image's end, as in
  the corpus image `area-audio-start-past-end`), so a loop until complete never ended: merging
  the extended copy back took only a farther frontier, and such an area's frontier does not
  move. `AreaIndex::merge` now also takes completeness. Affects 0.4.0 to 0.5.1 (#68)
