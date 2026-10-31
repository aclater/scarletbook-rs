Added
- C interface: `sb_disc_read_sectors` copies the 2048-byte user data of a range of logical sectors, whatever the image's stored sector size, on every disc handle (clones and callback discs included), many sectors per source read; a range past the last sector fails whole with the new `SB_STATUS_SECTOR_RANGE_INVALID` (#105, #113).
