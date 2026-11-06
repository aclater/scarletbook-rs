Changed
- `benchmarks/run.py` records free memory by block size before and after each round's image copy (`free_kib_by_order_before`, `free_kib_by_order_after`) and the host's `page_bytes`. `benchmarks/README.md` reports that free-memory state changes plain-DSD seconds between byte-identical image copies; it is a candidate for the gap between tables, not an established cause (#259, #289).
- Correction to the 0.5.8 notes: "no row moves by more than 1 %" does not hold for plain-DSD rows, where differences under about 2 % are unresolved (#259, #289).
