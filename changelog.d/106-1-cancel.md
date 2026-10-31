Added
- Cancelling and observing long reads (#106, part 1): `CancelToken`, one per image and
  shared by every disc made from it (`Disc::cancel_token`, `Image::cancel_token`). While
  cancellation is requested, reads of audio sectors return the new `Error::Cancelled`
  (`ErrorClass::Cancelled`) within 32 sectors, the scan before a range's first frame, seek
  index replays, provisional searches, the stored frame list and seek index extension
  included. The cursor stays where it was and continues exactly once the request is
  withdrawn. `Progress`, from `TrackAudioCursor::progress`, `TrackCursor::progress`,
  `AreaCursor::progress` and their iterators: the sectors read so far and the sectors
  expected, readable from any thread, updated with every read of at most 32 sectors.
