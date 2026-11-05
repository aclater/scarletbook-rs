Changed
- `benchmarks/run.py` records free memory by block size at the start of each round (`free_below_2m_gib`, `free_from_2m_gib`), and `benchmarks/README.md` reports what moves plain-DSD seconds between byte-identical image copies: the free memory a copy is written into predicts them. Why, and whether that was the cause of the gap between published tables, are not established (#259, #PR).
