Changed
- A seek index's damage record replaced by a read that met other marks keeps its points before the first sector where the two sets of marks differ, which fit both: a range opened between the old and the new marks still starts near its first frame instead of scanning the area from its first sector. What ranges return is unchanged (#443, #449).
