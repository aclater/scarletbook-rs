Fixed
- `benchmarks/run.py` skips a case for a kit whose `bench` has no such command, as it does for a missing program, instead of counting quick usage errors as failed runs, which could hide a real failure when comparing an older version. A case no kit can run fails the run (#290).
