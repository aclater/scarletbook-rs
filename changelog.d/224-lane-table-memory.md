Fixed
- Documentation of `MAX_PARALLEL_BATCH`: a DST decoder lane's tables are about 0.9 MB in use, but keep the capacity they grew to, and up to 1.33 MB per lane was measured in the memory soak (#246); the note said only "about 0.9 MB". Documentation only; no behaviour changes (#224, #356).
