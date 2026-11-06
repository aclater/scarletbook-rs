Added
- `TrackAudioCursor::with_dst_lanes` sets how many DST frames decode together on each thread, for measuring on your own CPU, and `bench decode --lanes K` exposes it in the benchmark kit. The default and the decoded output are unchanged; an out-of-range count is the new `Error::DstLanesInvalid` (#203, #318)
