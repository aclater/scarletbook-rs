Added
- Cold-seek measurements (#94, part 4; fixes #94): `scarletbook-seekcost` also measures
  provisional reading (no seek index, `SeekPolicy::Speculative`) against the range's position:
  read calls, sectors, time, a network model (`--rtt-ms`, default 0.5; `--mbps`, default 110)
  and whether the frames equal the exact ones. On a 50-minute 2-channel DST image: 4 to 7 read
  calls and 80 to 139 sectors wherever the range lies (3.5 to 5.6 ms modelled), against up
  to 470 272 sectors without an index. `.github/scripts/clib-seekcost.py` measures the same
  through the C interface (`sb_disc_open_callbacks` with counting callbacks, from ctypes):
  4 to 7 read calls against up to 14 696; the C package test runs it.
- C interface tests of provisional reading: on every well-formed corpus image a provisional
  range read equals an exact one (frames, statuses, missing and invalid counts) and is
  confirmed; a refutation reaches the C caller with `exact_from`, and an exact re-read below
  it completes a straight read; a clone checking readings on another thread settles the
  original handle's reading and queues the event on the original disc. The C package smoke
  test reads provisionally through the README's ctypes bindings and checks the readings with
  a disc-kept and a caller-kept seek index.
