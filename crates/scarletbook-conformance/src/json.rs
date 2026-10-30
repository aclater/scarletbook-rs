// SPDX-License-Identifier: MIT OR Apache-2.0

//! The TOC in the JSON shape of the sacd-spec conformance manifest.

use scarletbook::toc::{Genre, Locale};
use scarletbook::{Area, Toc, Track, Warning};
use serde_json::{Map, Value, json};

fn genre(g: Genre) -> Value {
    json!({ "category": g.category, "genre": g.index })
}

fn locales(l: &[Locale]) -> Value {
    l.iter().map(|l| json!({ "language": l.language, "charset": l.charset })).collect()
}

fn text_map<'a, I: Iterator<Item = (&'static str, &'a str)>>(items: I) -> Value {
    Value::Object(items.map(|(k, v)| (k.to_owned(), Value::from(v))).collect::<Map<_, _>>())
}

fn track(t: &Track) -> Value {
    json!({
        "number": t.number,
        "start_lsn": t.start_lsn,
        "length_sectors": t.length_sectors,
        "start_frame": t.start_frame,
        "duration_frames": t.duration_frames,
        "start_flags": t.start_flags,
        "duration_flags": t.duration_flags,
        "isrc": t.isrc,
        "genre": t.genre.map(genre),
        "text": text_map(t.text.iter().map(|(i, s)| (i.name(), s))),
    })
}

fn area(a: &Area) -> Value {
    json!({
        "slot": a.slot.name(),
        "signature": a.kind.signature(),
        "version": [a.version.major, a.version.minor],
        "toc_lsn": a.toc_lsn,
        "toc_sectors": a.toc_sectors,
        "max_byte_rate": a.max_byte_rate,
        "sample_frequency_code": a.sample_frequency_code,
        "frame_format": a.frame_format,
        "channel_count": a.channel_count,
        "area_config": a.area_config,
        "max_available_channels": a.max_available_channels,
        "area_mute_flags": a.area_mute_flags,
        "track_attribute": a.track_attribute,
        "playtime_frames": a.playtime_frames(),
        "track_offset": a.track_offset,
        "audio_start_lsn": a.audio_start_lsn,
        "audio_end_lsn": a.audio_end_lsn,
        "locales": locales(&a.locales),
        "text": text_map(a.text.iter().map(|(i, s)| (i.name(), s))),
        "tracks": a.tracks.iter().map(track).collect::<Vec<_>>(),
    })
}

/// The TOC as the manifest's `expected.toc` object.
pub fn toc(t: &Toc) -> Value {
    let m = &t.master;
    json!({
        "sector_size": t.sector_format.stored_size(),
        "sectors": t.sectors,
        "version": [m.version.major, m.version.minor],
        "album_set_size": m.album_set_size,
        "album_sequence_number": m.album_sequence_number,
        "album_catalog_number": m.album_catalog_number,
        "album_genres": m.album_genres.iter().copied().map(genre).collect::<Vec<_>>(),
        "disc_catalog_number": m.disc_catalog_number,
        "disc_genres": m.disc_genres.iter().copied().map(genre).collect::<Vec<_>>(),
        "hybrid": m.hybrid(),
        "date": { "year": m.date.year, "month": m.date.month, "day": m.date.day },
        "locales": locales(&m.locales),
        "text": text_map(m.text.iter().map(|(i, s)| (i.name(), s))),
        "manufacturer_sector": m.manufacturer.is_some(),
        "areas": t.areas.iter().map(area).collect::<Vec<_>>(),
    })
}

/// A warning as a manifest warning object.
pub fn warning(w: &Warning) -> Value {
    let mut o = Map::new();
    o.insert("code".into(), w.code.code().into());
    if let Some(s) = w.slot {
        o.insert("area".into(), s.name().into());
    }
    if let Some(t) = w.track {
        o.insert("track".into(), t.into());
    }
    if let Some(l) = w.lsn {
        o.insert("lsn".into(), l.into());
    }
    Value::Object(o)
}
