// SPDX-License-Identifier: MIT OR Apache-2.0

//! The diagnostic log: the trail of corpus images, frame problems with their position,
//! and nothing formatted without a sink.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use scarletbook::log::{Level, Log};
use scarletbook::{AudioFrame, Disc, FrameStatus, Slot};

fn image(id: &str) -> Vec<u8> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images").join(format!("{id}.iso"));
    std::fs::read(path).unwrap()
}

type Lines = Arc<Mutex<Vec<(Level, String)>>>;

/// A log that keeps every line at `level` and above.
fn recorder(level: Level) -> (Log, Lines) {
    let lines: Lines = Arc::default();
    let sink = Arc::clone(&lines);
    (Log::new(level, move |l, m| sink.lock().unwrap().push((l, m.to_string()))), lines)
}

/// Every decoded frame of every track of every area, in order, through one cursor per
/// area.
fn decode_all(disc: &mut Disc<Cursor<Vec<u8>>>) -> Vec<(Slot, AudioFrame)> {
    let mut out = Vec::new();
    for slot in Slot::ALL {
        let Some(count) = disc.toc().area(slot).map(|a| a.tracks.len() as u32) else { continue };
        let mut cursor = None;
        for n in 1..=count {
            if disc.check_track(slot, n).is_err() {
                continue;
            }
            let next = match cursor.take() {
                Some(prev) => disc.continue_track_audio(prev, slot, n).unwrap(),
                None => disc.track_audio_cursor(slot, n).unwrap(),
            };
            let mut audio = disc.read_track_audio(next);
            for frame in audio.by_ref() {
                match frame {
                    Ok(f) => out.push((slot, f)),
                    Err(_) => break,
                }
            }
            cursor = Some(audio.into_cursor());
        }
    }
    out
}

type Trail = (Vec<(Level, String)>, Vec<(Slot, AudioFrame)>, Disc<Cursor<Vec<u8>>>);

/// The log lines and frames of decoding every track of an image, and the disc.
fn trail(id: &str, level: Level) -> Trail {
    let (log, lines) = recorder(level);
    let mut disc = Disc::open_with_log(Cursor::new(image(id)), log).unwrap();
    let frames = decode_all(&mut disc);
    let lines = lines.lock().unwrap().clone();
    (lines, frames, disc)
}

fn has(lines: &[(Level, String)], level: Level, text: &str) -> bool {
    lines.iter().any(|(l, m)| *l == level && m == text)
}

fn starting(lines: &[(Level, String)], level: Level, prefix: &str) -> Vec<String> {
    lines.iter().filter(|(l, m)| *l == level && m.starts_with(prefix)).map(|(_, m)| m.clone()).collect()
}

/// The `(lsn, count, start, end)` of every read line.
fn reads(lines: &[(Level, String)]) -> Vec<(u64, u64, u64, u64)> {
    starting(lines, Level::Debug, "read ")
        .iter()
        .filter_map(|m| {
            let n: Vec<u64> = m
                .split(|c: char| !c.is_ascii_digit())
                .filter(|s| !s.is_empty())
                .map(|s| s.parse().unwrap())
                .collect();
            (n.len() == 4).then(|| (n[1], n[0], n[2], n[3]))
        })
        .collect()
}

fn position(lines: &[(Level, String)], text: &str) -> usize {
    lines.iter().position(|(_, m)| m == text).unwrap_or_else(|| panic!("no line {text:?}"))
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn trail_of_a_corpus_image() {
    let (lines, frames, disc) = trail("dst-features-2ch-6ch", Level::Debug);
    assert!(lines.iter().all(|(l, _)| *l <= Level::Debug), "nothing above the level");
    assert!(!frames.is_empty());
    let detected = "sector format Plain detected (signature at byte 1044480): 2048 bytes per sector, data at offset 0, 874 sectors";
    let master = "master TOC at LSN 510 used, version 1.20";
    assert!(has(&lines, Level::Info, detected));
    assert!(has(&lines, Level::Debug, "read 10 sectors at LSN 510: bytes 1044480..1064960"));
    assert!(has(&lines, Level::Info, master));
    assert!(has(&lines, Level::Debug, "master TOC copy at LSN 520: same as LSN 510"));
    assert!(has(&lines, Level::Debug, "master TOC copy at LSN 530: same as LSN 510"));
    assert!(has(&lines, Level::Info, "TOC read: 2 areas, 0 warnings"));
    assert!(position(&lines, detected) < position(&lines, master));
    for area in &disc.toc().areas {
        let slot = area.slot;
        let used = format!("area {slot}: TOC-1 at LSN {} used (TOC-2 identical)", area.toc_lsn);
        let (start, end) = (area.audio_start_lsn, u64::from(area.audio_end_lsn) + 1);
        let scan = format!("area {slot}: scanning audio sectors LSN {start}..{end}");
        let ended = format!("area {slot}: scan ended at LSN {end}, 0 malformed sectors");
        assert!(has(&lines, Level::Info, &used), "{used}");
        assert!(position(&lines, master) < position(&lines, &used));
        assert!(position(&lines, &used) < position(&lines, &scan));
        assert!(position(&lines, &scan) < position(&lines, &ended));
        assert!(has(
            &lines,
            Level::Info,
            &format!(
                "area {slot} track 1: frames 0..{} ({} channels), decoded audio, new scan of the area",
                area.tracks[0].end_frame(),
                area.channel_count
            )
        ));
        assert!(has(
            &lines,
            Level::Info,
            &format!(
                "area {slot} track 2: frames {}..{} ({} channels), decoded audio, continuing the earlier scan",
                area.tracks[1].start_frame,
                area.tracks[1].end_frame(),
                area.channel_count
            )
        ));
        // The audio sectors are read once, in order, in whole.
        let audio: Vec<_> =
            reads(&lines).into_iter().filter(|r| (u64::from(start)..end).contains(&r.0)).collect();
        assert_eq!(audio.first().map(|r| r.0), Some(u64::from(start)));
        assert!(audio.windows(2).all(|w| w[0].0 + w[0].1 == w[1].0));
        assert_eq!(audio.last().map(|r| r.0 + r.1), Some(end));
    }
    // Every read gives its LSN, sector count and byte range.
    let all = reads(&lines);
    assert!(all.len() >= 9, "{all:?}");
    assert!(all.iter().all(|&(lsn, count, a, b)| a == lsn * 2048 && b == a + count * 2048));
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn sector_formats_and_toc_copies() {
    let (lines, _, _) = trail("layout-2064", Level::Debug);
    assert!(has(&lines, Level::Debug, "format Plain: no signature at byte 1044480"));
    assert!(has(
        &lines,
        Level::Info,
        "sector format Raw2064 detected (signature at byte 1052652): 2064 bytes per sector, data at offset 12, 703 \
         sectors"
    ));
    assert!(reads(&lines).iter().all(|&(lsn, count, a, b)| a == lsn * 2064 && b == a + count * 2064));

    // A format chosen by the caller: no probes, the same TOC trail.
    let (log, chosen) = recorder(Level::Info);
    Disc::open_with_format_and_log(
        Cursor::new(image("layout-2064")),
        scarletbook::SectorFormat::Raw2064,
        log,
    )
    .unwrap();
    let chosen = chosen.lock().unwrap();
    assert_eq!(
        chosen.first().map(|(_, m)| m.as_str()),
        Some(
            "sector format Raw2064 chosen by the caller: 2064 bytes per sector, data at offset 12, 703 sectors \
             (1450992 bytes)"
        )
    );
    let toc = |lines: &[(Level, String)]| -> Vec<String> {
        lines
            .iter()
            .filter(|(l, m)| *l == Level::Info && !m.starts_with("sector format"))
            .map(|(_, m)| m.clone())
            .collect()
    };
    let (detected, _, _) = trail("layout-2064", Level::Info);
    let detected: Vec<_> = detected.into_iter().filter(|(_, m)| !m.contains(" track ")).collect();
    assert_eq!(toc(&chosen), toc(&detected));
    drop(chosen);

    let (lines, _, _) = trail("toc1-past-end-toc2-used", Level::Debug);
    assert!(has(&lines, Level::Debug, "read of 6 sectors at LSN 662: past the last sector (562)"));
    assert!(has(&lines, Level::Warn, "warning: area-toc1-unreadable (area 2ch)"));
    assert!(has(&lines, Level::Info, "area 2ch: TOC-2 at LSN 556 used (TOC-1 unreadable)"));
    assert!(has(&lines, Level::Info, "area multichannel: slot unused"));

    let (lines, _, disc) = trail("area-toc1-toc2-differ", Level::Info);
    let toc1 = disc.toc().area(Slot::TwoChannel).unwrap().toc_lsn;
    assert!(has(&lines, Level::Warn, "warning: area-toc-copies-differ (area 2ch)"));
    assert!(has(&lines, Level::Info, &format!("area 2ch: TOC-1 at LSN {toc1} used (TOC-2 differs)")));

    // Every TOC warning is logged where it is met, in the order of the list.
    let (lines, _, disc) = trail("truncated-in-audio", Level::Warn);
    let logged = starting(&lines, Level::Warn, "warning: ");
    let listed: Vec<_> = disc.warnings().iter().map(|w| format!("warning: {w}")).collect();
    assert_eq!(logged, listed);

    // A rejected image says why.
    let (log, lines) = recorder(Level::Debug);
    assert!(Disc::open_with_log(Cursor::new(vec![0u8; 600 * 2048]), log).is_err());
    let lines = lines.lock().unwrap();
    assert!(has(&lines, Level::Debug, "format Raw2054: no signature at byte 1047546"));
    assert!(has(&lines, Level::Info, "no sector format matches: not a Scarletbook image"));
}

/// The lines about returned frames: `area S frame N: ...` (not the reassembler's).
fn frame_lines(lines: &[(Level, String)], level: Level) -> Vec<String> {
    lines
        .iter()
        .filter(|(l, m)| {
            *l == level
                && m.starts_with("area ")
                && m.split_once(": ").is_some_and(|(a, _)| a.contains(" frame ") && !a.contains("(from"))
        })
        .map(|(_, m)| m.clone())
        .collect()
}

fn expected_frame_line(slot: Slot, f: &AudioFrame) -> Option<String> {
    match f.status {
        FrameStatus::Missing => {
            Some(format!("area {slot} frame {}: missing, filled with 0x69 (4.6)", f.number))
        }
        FrameStatus::Invalid(e) => Some(format!(
            "area {slot} frame {}: invalid DST frame ({e}, rule {} of 5.6), filled with 0x55 (4.6)",
            f.number,
            e.rule()
        )),
        FrameStatus::Dsd | FrameStatus::Decoded => None,
    }
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn frame_and_sector_problems_with_their_position() {
    for id in ["dst-invalid-frames-2ch", "dst-invalid-frames-6ch", "audio-tc-backwards", "truncated-in-audio"]
    {
        let (lines, frames, _) = trail(id, Level::Warn);
        let expected: Vec<_> = frames.iter().filter_map(|(slot, f)| expected_frame_line(*slot, f)).collect();
        assert!(!expected.is_empty(), "{id}");
        assert_eq!(frame_lines(&lines, Level::Warn), expected, "{id}");
    }

    // Malformed sectors: one line per sector, with the LSN of each warning.
    let (lines, _, mut disc) = trail("audio-packets-overrun", Level::Warn);
    let mut listed = Vec::new();
    for slot in Slot::ALL {
        let Some(count) = disc.toc().area(slot).map(|a| a.tracks.len() as u32) else { continue };
        let cursor = disc.track_audio_cursor(slot, count).unwrap();
        let mut audio = disc.read_track_audio(cursor);
        audio.by_ref().for_each(drop);
        listed.extend(audio.warnings().iter().map(|w| (slot, w.lsn.unwrap())));
    }
    let logged: Vec<_> = lines
        .iter()
        .filter(|(l, m)| {
            *l == Level::Warn && m.contains(" audio sector LSN ") && m.contains(": audio-sector-malformed: ")
        })
        .map(|(_, m)| {
            let slot = if m.starts_with("area 2ch ") { Slot::TwoChannel } else { Slot::Multichannel };
            let lsn = m.split("LSN ").nth(1).unwrap().split(':').next().unwrap().parse::<u32>().unwrap();
            (slot, lsn)
        })
        .collect();
    assert!(!listed.is_empty());
    assert_eq!(logged, listed);

    // A frame the reassembler drops, with the sector it started in.
    let (lines, _, _) = trail("truncated-in-audio", Level::Warn);
    assert!(
        has(
            &lines,
            Level::Warn,
            "area 2ch frame 1 (from LSN 550): discarded, incomplete (Dst, 4896 bytes, sector count does not match; 4.4.2)"
        ),
        "{lines:?}"
    );
}

#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn no_sink_or_a_quiet_level_changes_nothing() {
    let id = "dst-invalid-frames-2ch";
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let quiet = Log::new(Level::Error, move |_, _| {
        counter.fetch_add(1, Ordering::Relaxed);
    });
    let mut logged = Disc::open_with_log(Cursor::new(image(id)), quiet).unwrap();
    let mut plain = Disc::open(Cursor::new(image(id))).unwrap();
    assert_eq!(decode_all(&mut logged), decode_all(&mut plain));
    assert_eq!(calls.load(Ordering::Relaxed), 0, "no error on this image: no call at all");
    assert_eq!(plain.log().level(), None);

    // set_log takes effect from the next call on.
    let (log, lines) = recorder(Level::Warn);
    plain.set_log(log);
    let cursor = plain.track_audio_cursor(Slot::TwoChannel, 1).unwrap();
    let first = plain
        .read_track_audio(cursor)
        .filter_map(Result::ok)
        .find(|f| matches!(f.status, FrameStatus::Invalid(_)));
    let first = first.unwrap();
    assert_eq!(
        frame_lines(&lines.lock().unwrap(), Level::Warn),
        [expected_frame_line(Slot::TwoChannel, &first).unwrap()]
    );
}

/// Decoding on a pool: the frame lines, logged on the calling thread as frames are
/// returned, are those of a serial run in the same order; the groups are reported by the
/// pool's threads. Which of them decodes a group is the scheduler's choice: the caller is
/// one of the pool's threads and may decode every group itself while it waits (a small
/// image on a runner with few cores), so only "on this pool" is checked.
#[cfg(feature = "parallel")]
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn parallel_decoding_logs_from_the_pool_in_order() {
    for id in ["dst-features-2ch-6ch", "dst-invalid-frames-2ch"] {
        let serial = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let (serial_lines, serial_frames, _) = serial.install(|| trail(id, Level::Trace));
        let lines: Arc<Mutex<Vec<(String, Level, String)>>> = Arc::default();
        let sink = Arc::clone(&lines);
        let log = Log::new(Level::Trace, move |l, m| {
            let name = std::thread::current().name().unwrap_or("").to_owned();
            sink.lock().unwrap().push((name, l, m.to_string()));
        });
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .thread_name(|i| format!("logpool-{i}"))
            .build()
            .unwrap();
        let (caller, frames) = pool.install(|| {
            let mut disc = Disc::open_with_log(Cursor::new(image(id)), log).unwrap();
            (std::thread::current().name().unwrap_or("").to_owned(), decode_all(&mut disc))
        });
        assert_eq!(frames, serial_frames, "{id}");
        let lines = lines.lock().unwrap();
        let plain: Vec<_> = lines.iter().map(|(_, l, m)| (*l, m.clone())).collect();
        for level in [Level::Warn, Level::Trace] {
            assert_eq!(frame_lines(&plain, level), frame_lines(&serial_lines, level), "{id}");
        }
        assert!(
            lines
                .iter()
                .filter(|(_, _, m)| !m.starts_with("decoding DST frames"))
                .all(|(t, _, _)| *t == caller)
        );
        let groups: Vec<&String> = lines
            .iter()
            .filter(|(_, _, m)| m.starts_with("decoding DST frames"))
            .map(|(t, _, _)| t)
            .collect();
        assert!(!groups.is_empty(), "{id}: no group decoded");
        assert!(
            groups.iter().all(|t| t.starts_with("logpool-")),
            "{id}: a group decoded off the pool: {groups:?}"
        );
    }
}

/// A cursor given its own pool decodes there, from a thread outside any pool and from a
/// thread of another pool alike, with the frames and frame lines of the calling thread
/// (the demultiplexing lines run further ahead), and its continuations keep the pool.
#[cfg(feature = "parallel")]
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn a_cursor_decodes_on_the_pool_it_is_given() {
    use scarletbook::DecodeOn;

    /// Each line with the name of the thread that logged it.
    type Named = Vec<(String, Level, String)>;

    fn name() -> String {
        std::thread::current().name().unwrap_or("").to_owned()
    }

    /// The frames and lines of every track of the 2-channel area, decoded `on`.
    fn decode(id: &str, on: &DecodeOn) -> (Vec<AudioFrame>, Named) {
        let lines: Arc<Mutex<Named>> = Arc::default();
        let sink = Arc::clone(&lines);
        let log = Log::new(Level::Trace, move |l, m| sink.lock().unwrap().push((name(), l, m.to_string())));
        let mut disc = Disc::open_with_log(Cursor::new(image(id)), log).unwrap();
        let count = disc.toc().area(Slot::TwoChannel).unwrap().tracks.len() as u32;
        let (mut frames, mut cursor) = (Vec::new(), None);
        for n in 1..=count {
            let next = match cursor.take() {
                Some(prev) => disc.continue_track_audio(prev, Slot::TwoChannel, n).unwrap(),
                None => {
                    let mut c = disc.track_audio_cursor(Slot::TwoChannel, n).unwrap();
                    c.set_decode_on(on.clone());
                    c
                }
            };
            assert_eq!(format!("{:?}", next.decode_on()), format!("{on:?}"));
            let mut audio = disc.read_track_audio(next);
            frames.extend(audio.by_ref().map(Result::unwrap));
            cursor = Some(audio.into_cursor());
        }
        let lines = lines.lock().unwrap().clone();
        (frames, lines)
    }

    let named = |prefix: &'static str, threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(move |i| format!("{prefix}-{i}"))
            .build()
            .unwrap()
    };
    let plain =
        |lines: &Named| -> Vec<(Level, String)> { lines.iter().map(|(_, l, m)| (*l, m.clone())).collect() };
    let id = "dst-features-2ch-6ch";
    let (serial, serial_lines) = decode(id, &DecodeOn::CallingThread);
    assert!(serial_lines.iter().all(|(t, _, _)| *t == name()));
    let other = named("other", 3);
    for threads in [1, 2, 4] {
        let pool = Arc::new(named("chosen", threads));
        // From this thread, outside any pool, then from a thread of another pool.
        let runs = [
            (name(), decode(id, &DecodeOn::Pool(Arc::clone(&pool)))),
            other.install(|| (name(), decode(id, &DecodeOn::Pool(Arc::clone(&pool))))),
        ];
        for (caller, (frames, lines)) in runs {
            assert_eq!(frames, serial, "{threads} threads from {caller:?}");
            for level in [Level::Warn, Level::Trace] {
                assert_eq!(frame_lines(&plain(&lines), level), frame_lines(&plain(&serial_lines), level));
            }
            // The groups are decoded on the chosen pool's threads only (a pool of one
            // thread decodes on the calling thread); every other line is the caller's.
            let groups: Vec<&String> = lines
                .iter()
                .filter(|(_, _, m)| m.starts_with("decoding DST frames"))
                .map(|(t, _, _)| t)
                .collect();
            assert!(!groups.is_empty());
            if threads == 1 {
                assert!(groups.iter().all(|t| **t == caller), "{groups:?}");
            } else {
                assert!(groups.iter().all(|t| t.starts_with("chosen-")), "{threads} threads: {groups:?}");
            }
            assert!(
                lines
                    .iter()
                    .filter(|(_, _, m)| !m.starts_with("decoding DST frames"))
                    .all(|(t, _, _)| *t == caller)
            );
        }
    }
}

/// Every corpus image: the logged TOC warnings are the disc's warnings, the frame lines
/// are the returned frames' problems in order, and a rejected image logs no error-level
/// line for a well-behaved reader.
#[test]
#[cfg_attr(miri, ignore = "reads whole corpus images")]
fn every_corpus_image_logs_what_it_returns() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/images");
    let mut ids: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().to_str()?.strip_suffix(".iso").map(str::to_owned))
        .collect();
    ids.sort();
    assert!(ids.len() >= 136, "{}", ids.len());
    for id in &ids {
        let (log, lines) = recorder(Level::Trace);
        let Ok(mut disc) = Disc::open_with_log(Cursor::new(image(id)), log) else {
            let lines = lines.lock().unwrap();
            assert!(lines.iter().all(|(l, _)| *l != Level::Error), "{id}");
            assert!(
                lines.iter().any(|(l, m)| *l == Level::Info
                    && (m.starts_with("rejected") || m.starts_with("no sector format"))),
                "{id}"
            );
            continue;
        };
        let frames = decode_all(&mut disc);
        let lines = lines.lock().unwrap().clone();
        let listed: Vec<_> = disc.warnings().iter().map(|w| format!("warning: {w}")).collect();
        assert_eq!(starting(&lines, Level::Warn, "warning: "), listed, "{id}");
        let expected: Vec<_> = frames.iter().filter_map(|(slot, f)| expected_frame_line(*slot, f)).collect();
        assert_eq!(frame_lines(&lines, Level::Warn), expected, "{id}");
        assert_eq!(frame_lines(&lines, Level::Trace).len(), frames.len() - expected.len(), "{id}");
        assert!(lines.iter().all(|(l, _)| *l != Level::Error), "{id}");
    }
}
