// SPDX-License-Identifier: MIT OR Apache-2.0

//! Small images from the synthetic image generator open cleanly and decode fully.

use std::io::Cursor;

use scarletbook::{Disc, FrameKind, FrameStatus, SectorFormat, Slot};
use scarletbook_conformance::corpus_dir;
use scarletbook_conformance::synth::{Config, DstSource, Kind, Layout, write_image};

fn generate(kind: Kind, format: SectorFormat, mc_channels: u8, src: &DstSource) -> (Vec<u8>, Layout) {
    let cfg = Config {
        // 2 s per area, a track every 0.5 s.
        frames: 150,
        two_channel: true,
        multichannel: true,
        mc_channels,
        kind,
        format,
        track_frames: 38,
    };
    let mut out = Cursor::new(Vec::new());
    let layout = write_image(&mut out, &cfg, src).unwrap();
    (out.into_inner(), layout)
}

fn check(kind: Kind, format: SectorFormat, mc_channels: u8, src: &DstSource) {
    let (bytes, layout) = generate(kind, format, mc_channels, src);
    assert_eq!(bytes.len() as u64, layout.sectors * format.stored_size());
    let mut disc = Disc::open(Cursor::new(&bytes)).unwrap();
    assert_eq!(disc.image_mut().format(), format);
    assert!(disc.warnings().is_empty(), "{:?}", disc.warnings());

    // The three master TOC copies are identical (specification 1, 1.5.1).
    let mut copies = [vec![0u8; 10 * 2048], vec![0u8; 10 * 2048], vec![0u8; 10 * 2048]];
    for (lsn, buf) in [510, 520, 530].into_iter().zip(&mut copies) {
        assert!(disc.image_mut().read_sectors(lsn, buf).unwrap());
    }
    assert!(copies[0] == copies[1] && copies[0] == copies[2]);

    let toc = disc.toc().clone();
    assert_eq!(toc.areas.len(), 2);
    for (area, want) in toc.areas.iter().zip(&layout.areas) {
        assert_eq!((area.slot, area.channel_count), (want.slot, want.channels));
        assert_eq!((area.audio_start_lsn, area.audio_end_lsn), want.audio);
        assert_eq!(area.playtime.frame_number(), 150);
        assert_eq!(area.tracks.len(), 4);
        let got: Vec<_> = area
            .tracks
            .iter()
            .map(|t| (t.start_lsn, t.length_sectors, t.start_frame, t.duration_frames))
            .collect();
        assert_eq!(got, want.tracks);

        // The area's frames, in order, with every timecode present.
        let frames: Vec<_> = disc.area_frames(area.slot).unwrap().map(Result::unwrap).collect();
        assert_eq!(frames.len(), 150);
        let c = usize::from(area.channel_count);
        for (n, f) in frames.iter().enumerate() {
            assert_eq!(f.number as usize, n);
            match kind {
                Kind::Dsd => assert_eq!((f.kind, f.data.len()), (FrameKind::Dsd, c * 4704)),
                Kind::Dst => {
                    let pool = match c {
                        2 => &src.two,
                        5 => &src.five,
                        _ => &src.six,
                    };
                    assert_eq!(f.kind, FrameKind::Dst);
                    assert_eq!(f.data, pool[n % pool.len()]);
                }
            }
        }

        for t in &area.tracks {
            let n = u32::from(t.number);
            let mut audio = disc.track_audio(area.slot, n).unwrap();
            let mut count = 0;
            for f in audio.by_ref() {
                let f = f.unwrap();
                let want = if kind == Kind::Dsd { FrameStatus::Dsd } else { FrameStatus::Decoded };
                assert_eq!(f.status, want, "{:?} track {n} frame {}", area.slot, f.number);
                assert_eq!(f.data.len(), c * 4704);
                count += 1;
            }
            assert_eq!(count, t.duration_frames);
            assert_eq!((audio.missing_frames(), audio.invalid_dst_frames()), (0, 0));
            assert_eq!(audio.malformed_sectors(), 0);
        }
    }
    assert!(disc.toc().area(Slot::TwoChannel).is_some());
}

#[test]
fn synthetic_images_decode() {
    let src = DstSource::from_corpus(&corpus_dir()).unwrap();
    let formats = [(SectorFormat::Plain, 5), (SectorFormat::Raw2064, 6), (SectorFormat::Raw2054, 5)];
    for kind in [Kind::Dsd, Kind::Dst] {
        for (format, mc) in formats {
            check(kind, format, mc, &src);
        }
    }
}
