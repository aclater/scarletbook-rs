Changed
- A saved seek index that holds reading state past unreadable sectors sets flags bit 2 and carries one more section before its checksum. Readers before 1.3.0 refuse it ("seek index flags invalid"), as any index they cannot use; save `AreaIndex::without_damage` for them. An index without that state saves exactly as before, and every earlier saved form loads unchanged (#441, #442).
