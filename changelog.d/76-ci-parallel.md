Changed
- CI: fuzz targets run as parallel jobs (30 s each on pull requests, 60 s on main), Miri runs as parallel slices with the long core check split by frame count, and a newer push to main cancels an older run instead of queuing behind it (#76).
