Fixed
- `bench realtime --seeks` and `bench dsf --chunk` are limited (1,000,000 seeks, 256 MiB) and give a `usage-error` beyond it, instead of sizing an allocation from an unbounded number (#255, #316).
