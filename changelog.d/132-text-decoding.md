Changed
- Text strings of the TOC are decoded in one pass: control characters are replaced only
  when a string holds some, ASCII Latin-1 text is copied as it is, and fixed text fields
  are trimmed before they are decoded. The text is unchanged (#132, #142).
