Fixed
- A caller-kept seek index could confirm a provisional reading against another image of the
  same size and layout (#102, in 0.4.0): it took the reading without binding to the image the
  reading had read, so a later `AreaIndex::extend` or `AreaIndex::verify` with the other image
  checked it against that image's state (specification 4, 4.2 item 8). Taking a provisional
  reading now binds an index that has no image yet to the reading's image, and an index of
  another image refuses it; a reading also remembers its image and is never checked against
  another (it becomes `Speculation::Unverified`, for example after `Disc::set_seek_index`
  moves an index to a disc of another image).
