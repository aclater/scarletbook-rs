Fixed
- The `core-dispatch` Miri slice has room under its 25-minute limit again: under Miri, the output-path dispatch test compares one 2-channel area and three of its layouts (DSF, DSDIFF and a chain file starting inside a DSF row) instead of two areas and five layouts each (#202, #PR).
