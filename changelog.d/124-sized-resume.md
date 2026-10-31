Changed
- An exact read resumed from a seek index makes its first read sized to the range (the
  replay, the interval and up to 64 sectors), one read call instead of two or three.
  The changed cancellation and progress bound of #123 (a read of up to 512 sectors in
  progress) applies to it too (#124, #133).
