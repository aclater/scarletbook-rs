Changed
- Cheaper provisional (cold) seeks (#123): a provisional reading (`SeekPolicy::Speculative`,
  specification 4, 4.2 item 8) now begins at the sector its search found, the last whose
  first frame record is numbered below the range's first frame, instead of a checkpoint
  boundary 32 to 95 sectors before it; the search interpolates between the area's track
  starts and then at the density of the frames it last read, uses every frame record of a
  probe, and stops once close; the cursor reads on from its last probe, and then up to the
  range's estimated end in reads sized to it, of at most the new `seek::MAX_READ_SECTORS`
  (512 sectors, 1 MiB) each, instead of 32. On a 50-minute 2-channel DST area of uneven
  bit rate, a 14-frame range costs 3.4 read calls and 81 KiB to its first frame (was 9.1 and
  266 KiB). `ResumedFrom::Speculative` and `Speculation` report that `L`, which is no longer
  a multiple of the index interval from the area's first sector. **Changed bound:** a
  cancellation request, and the progress counts, now wait for the read in progress, which
  is at most 32 sectors as before, or at most 512 for those reads sized to a provisional
  range (`CancelToken`, `Progress`, `sb_disc_cancel_token`, `sb_progress_get`).
  `scarletbook-seekcost` measures cold seeks on newly opened discs (`--positions`), and
  `scarletbook-synth --dst-frames uneven` writes an area of uneven bit rate.
