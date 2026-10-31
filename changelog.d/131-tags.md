Added
- DSF tags (specification 8, 8.2.6 and 8.3): DSF layouts end with an ID3v2.4 tag built from the TOC (`output::TagSet::from_toc`: track text, track number, ISRC, genres, album or disc text, album set, date), from a caller's `TagSet`, or the caller's own tag bytes (`output::Tags`). Every ID3 writer's choice of 8.6 is a field of `output::Id3Options`: footer or padding, frame order, text encoding, terminators, control characters, splitting, and the frames and descriptions of the arranger, messages, album artist, copyright, album set and date (#131).

