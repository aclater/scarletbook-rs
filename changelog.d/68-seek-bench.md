Added
- Seek index measurements and corpus checks (#68, part 5): `scarletbook-seekcost` prints
  the cost of opening a range against its position with and without an index (flat with
  one: 32 to 96 sectors and 2.3 to 2.4 ms on a 50-minute DST area, against up to 470 272
  sectors and 146 ms without). Conformance tests read every corpus area's tracks, tracks
  with pause and random ranges through indexes (K = 1 and 64), fresh and continued, and on
  two threads at once through a clone and a share of one disc keeping one index, against
  plain reads.
