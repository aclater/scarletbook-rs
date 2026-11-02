Fixed
- The `core-lib` Miri slice fits its 25-minute limit again: under Miri, the #228 read-buffer unit test checks one raw format and reads only the first frame (#228, #235).
