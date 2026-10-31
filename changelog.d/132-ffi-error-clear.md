Changed
- C interface: a call no longer allocates to clear the thread's error message
  (`sb_last_error_message`) when it is already empty, as it is after every successful
  call: about 10 ns less per call (#132).
