Changed
- PROTOTYPE, not for merging: `output::Layout::fill` and `Layout::read_at` run under one run-time AVX2/BMI dispatch point, so that the DSF channel separation beneath them is built with those features in generic builds (#202, #220).
