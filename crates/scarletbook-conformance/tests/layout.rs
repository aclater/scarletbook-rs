// SPDX-License-Identifier: MIT OR Apache-2.0

//! The loudspeaker layout of every corpus area (specification 3, 3.4.1): the defined
//! combinations give their layouts, every other one is unspecified.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;

use scarletbook::{Disc, LoudspeakerLayout, Slot, Speaker};
use scarletbook_conformance::{corpus_dir, load_cases};

#[test]
fn corpus_area_layouts() {
    use Speaker::*;
    let dir = corpus_dir();
    let cases = load_cases(&dir).unwrap_or_else(|e| panic!("manifest: {e}"));
    let mut seen: BTreeMap<(u8, u8), usize> = BTreeMap::new();
    for case in &cases {
        let id = case["id"].as_str().unwrap_or("?");
        let Some(want) = case["expected"]["toc"]["areas"].as_array() else { continue };
        let file = File::open(dir.join("images").join(format!("{id}.iso"))).unwrap();
        let disc = Disc::open(BufReader::new(file)).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(disc.toc().areas.len(), want.len(), "{id}");
        for w in want {
            let slot = Slot::ALL.into_iter().find(|s| s.name() == w["slot"]).unwrap();
            let area = disc.toc().area(slot).unwrap();
            let channels = u8::try_from(w["channel_count"].as_u64().unwrap()).unwrap();
            let config = u8::try_from(w["area_config"].as_u64().unwrap()).unwrap();
            assert_eq!((area.channel_count, area.area_config), (channels, config), "{id} {slot}");
            *seen.entry((channels, config)).or_default() += 1;
            let (layout, speakers): (_, Option<&[Speaker]>) = match (channels, config) {
                (2, 0) => (LoudspeakerLayout::Stereo, Some(&[FrontLeft, FrontRight])),
                (5, 3) => (
                    LoudspeakerLayout::ThreeTwo,
                    Some(&[FrontLeft, FrontRight, Centre, SurroundLeft, SurroundRight]),
                ),
                (6, 4) => (
                    LoudspeakerLayout::FivePointOne,
                    Some(&[FrontLeft, FrontRight, Centre, LowFrequencyEffects, SurroundLeft, SurroundRight]),
                ),
                _ => (LoudspeakerLayout::Unspecified { channels, config }, None),
            };
            assert_eq!(area.loudspeaker_layout(), layout, "{id} {slot}");
            assert_eq!(area.channel_speakers(), speakers, "{id} {slot}");
        }
    }
    // The corpus's combinations: the three defined ones, 1, 3 and 4 channels, and
    // channel counts or configurations outside the table (0, 255, 6 with 0, 2 and 5 with 4).
    let want: BTreeMap<(u8, u8), usize> = [
        ((0, 0), 1),
        ((1, 0), 2),
        ((2, 0), 104),
        ((2, 4), 1),
        ((3, 0), 3),
        ((4, 0), 2),
        ((5, 3), 5),
        ((5, 4), 1),
        ((6, 0), 1),
        ((6, 4), 84),
        ((255, 0), 1),
    ]
    .into();
    assert_eq!(seen, want);
}
