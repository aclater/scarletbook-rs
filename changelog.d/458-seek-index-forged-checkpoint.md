Fixed
- `AreaIndex::absorb`, `extend` and `extend_past_marks` refuse a cursor's checkpoints that do not continue the index's last one (`a` or the malformed count lower), leaving the index unchanged. Before, a loaded index with a forged checkpoint ahead of a cursor's scan took that scan's checkpoints and then saved to bytes `AreaIndex::from_bytes` refuses; found by fuzzing (#458, #459).
