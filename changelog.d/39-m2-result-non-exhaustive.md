Changed
- **Breaking:** the result structs `Warning`, `audio::AreaFrame`, `audio::TrackFrame` and `audio::AudioFrame` are `#[non_exhaustive]`, so that they can gain fields later. Field access is unchanged. Outside the crate, struct patterns need `..`, and struct literals no longer compile: a `TrackFrame` for `audio::decode_frame` now comes from the new `TrackFrame::new(number, content)`; the others are produced only by the library (#39).

  Before:

  ```rust
  let frame = TrackFrame { number, content: FrameContent::Dst(saved) };
  let AudioFrame { number, status, data } = decode_frame(&mut decoder, frame, channels)?;
  ```

  After:

  ```rust
  let frame = TrackFrame::new(number, FrameContent::Dst(saved));
  let AudioFrame { number, status, data, .. } = decode_frame(&mut decoder, frame, channels)?;
  ```
