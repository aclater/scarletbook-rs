Fixed
- A provisional reading (`SeekPolicy::Speculative`) with a seek index kept could start a range
  at the provisional checkpoint where an earlier reading far before the range ended, because a
  later reading had passed the range's first frame, and then read every sector from there:
  in one measured case 42 880 sectors in 32-sector calls instead of a few. A provisional
  checkpoint is now used only with the later one of the same reading (consecutive boundaries);
  otherwise the cursor searches as without them. Affects 0.4.1 to 0.5.1 (#94, #134).
