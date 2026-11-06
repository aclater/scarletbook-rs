Changed
- **Breaking:** the TOC model is `#[non_exhaustive]`, so that fields the specification defines can be exposed later without another break: `toc::Toc`, `MasterToc`, `Area`, `Track`, `Locale`, `DiscText`, `AreaText` and `TrackText`. Field access is unchanged. Outside the crate, struct patterns need `..`, and struct literals no longer compile: take an `Area` or `Track` from a parsed `Toc` (its fields stay public and assignable), and make a text struct from `Default` (#39).

  Before:

  ```rust
  let Track { number, start_frame, duration_frames, start_lsn, length_sectors,
              start_flags, duration_flags, isrc, genre, text } = track;
  let text = TrackText { title: Some(title), ..TrackText::default() };
  ```

  After:

  ```rust
  let Track { number, start_frame, duration_frames, .. } = track;
  let mut text = TrackText::default();
  text.title = Some(title);
  ```
