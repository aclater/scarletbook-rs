Changed
- **Breaking:** twelve field-carrying enum variants are `#[non_exhaustive]`, so that they can gain fields later: `seek::Speculation::{Pending, Confirmed, Refuted, Unverified}`, `seek::ResumedFrom::{Checkpoint, Discarded, Speculative}` and `Error::{MasterTocVersionUnsupported, ChannelCountUnsupported, TrackExceedsCapacity, NoSuchTrack, FrameRangeInvalid}`. Outside the crate, a pattern on one of them needs `..` even when it names every field, and the variants can no longer be built: a program that made its own `Error::NoSuchTrack` needs its own error type or variant for that case (#39, #302).

  Before:

  ```rust
  match cursor.speculation() {
      Speculation::Refuted { lsn, first, last_returned, exact_from } => redo(first, exact_from),
      Speculation::Confirmed { lsn, first } => keep(lsn, first),
      _ => {}
  }
  if let Err(Error::NoSuchTrack { slot, track }) = result { report(slot, track) }
  ```

  After:

  ```rust
  match cursor.speculation() {
      Speculation::Refuted { first, exact_from, .. } => redo(first, exact_from),
      Speculation::Confirmed { lsn, first, .. } => keep(lsn, first),
      _ => {}
  }
  if let Err(Error::NoSuchTrack { slot, track, .. }) = result { report(slot, track) }
  ```
