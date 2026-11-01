Changed
- Unreleased APIs made extensible before their first release, so later additions stay
  minor (#39, #163). Non-exhaustive now: `scarletbook_pcm::FrameRun`, with
  `FrameRun::new`, `with_before` and `with_after` to build it; the variants
  `scarletbook_pcm::PcmError::{Unsupported, InputLength, OutputLength}`,
  `OutputRefusal::UnspecifiedLayout` and `EditedMasterCheck::TrackTooShort` (match them with
  `..`). `output::Id3Options` gains `new` and a `with_*` method per field, and
  `output::GenreSources` gains `new`, `with_track`, `with_album` and `with_disc`; both were
  already non-exhaustive. Nothing released in 0.5.1 changes.
