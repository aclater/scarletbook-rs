Added
- C interface: `sb_disc_toc` writes the whole TOC into one caller-sized buffer (the disc,
  areas, tracks, strings, genres, text channels, stored timecodes and warnings that the
  per-field queries return) in a fixed, versioned binary layout: little-endian, fixed
  offsets, described by `SbTocHeader`, `SbTocDisc`, `SbTocArea`, `SbTocTrack`,
  `SbTocWarning` and `SbTocStr`, format `SB_TOC_MAJOR`.`SB_TOC_MINOR` 1.0. One call
  replaces about 100 to 160 per disc. Additive on ABI 2. README ctypes bindings and an
  example (#132).
