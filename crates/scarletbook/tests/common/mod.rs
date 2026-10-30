// SPDX-License-Identifier: MIT OR Apache-2.0

//! A builder of small synthetic plain images for tests.

#![allow(dead_code, clippy::unwrap_used)]

pub const S: usize = 2048;

fn put(img: &mut [u8], at: usize, bytes: &[u8]) {
    img[at..at + bytes.len()].copy_from_slice(bytes);
}

fn put32(img: &mut [u8], at: usize, v: u32) {
    put(img, at, &v.to_be_bytes());
}

fn put16(img: &mut [u8], at: usize, v: u16) {
    put(img, at, &v.to_be_bytes());
}

fn tc(n: u32) -> [u8; 3] {
    [(n / 4500) as u8, (n / 75 % 60) as u8, (n % 75) as u8]
}

/// One track: start frame, duration in frames.
#[derive(Clone, Copy, Debug)]
pub struct TrackSpec {
    pub start: u32,
    pub duration: u32,
}

/// A synthetic disc with one 2-channel area whose TOC is at LSN 540 (6 sectors) and
/// whose audio sectors follow it.
#[derive(Clone, Debug)]
pub struct Spec {
    pub channels: u8,
    pub playtime: u32,
    pub tracks: Vec<TrackSpec>,
    /// Raw audio sectors (each exactly 2048 bytes) placed after the area TOC.
    pub audio: Vec<[u8; S]>,
}

pub const AREA_TOC_LSN: usize = 540;
pub const AREA_TOC_SECTORS: usize = 6;
pub const AUDIO_LSN: usize = AREA_TOC_LSN + AREA_TOC_SECTORS;

impl Spec {
    pub fn simple() -> Self {
        Spec {
            channels: 2,
            playtime: 10,
            tracks: vec![TrackSpec { start: 0, duration: 4 }, TrackSpec { start: 4, duration: 6 }],
            audio: vec![[0u8; S]; 2],
        }
    }

    pub fn build(&self) -> Vec<u8> {
        let audio_sectors = self.audio.len().max(1);
        let total = AUDIO_LSN + audio_sectors;
        let mut img = vec![0u8; total * S];
        // Master TOC.
        let m = 510 * S;
        put(&mut img, m, b"SACDMTOC");
        img[m + 8] = 1;
        img[m + 9] = 20;
        put16(&mut img, m + 16, 1);
        put16(&mut img, m + 18, 1);
        put(&mut img, m + 24, b"CAT-1");
        put32(&mut img, m + 64, AREA_TOC_LSN as u32);
        put16(&mut img, m + 84, AREA_TOC_SECTORS as u16);
        img[m + 128] = 1;
        put(&mut img, m + 136, b"en\x02");
        let t = m + S;
        put(&mut img, t, b"SACDText");
        put16(&mut img, t + 16, 48);
        put(&mut img, t + 48, b"Synthetic\0");
        put(&mut img, m + 9 * S, b"SACD_Man");
        // Area TOC.
        let a = AREA_TOC_LSN * S;
        put(&mut img, a, b"TWOCHTOC");
        img[a + 8] = 1;
        img[a + 9] = 20;
        img[a + 20] = 4;
        img[a + 32] = self.channels;
        put(&mut img, a + 64, &tc(self.playtime));
        img[a + 69] = self.tracks.len() as u8;
        put32(&mut img, a + 72, AUDIO_LSN as u32);
        put32(&mut img, a + 76, (AUDIO_LSN + audio_sectors - 1) as u32);
        img[a + 80] = 1;
        put(&mut img, a + 88, b"en\x02");
        let ttxt = a + S;
        put(&mut img, ttxt, b"SACDTTxt");
        let igl = a + 2 * S;
        put(&mut img, igl, b"SACD_IGL");
        let trl1 = a + 4 * S;
        put(&mut img, trl1, b"SACDTRL1");
        let trl2 = a + 5 * S;
        put(&mut img, trl2, b"SACDTRL2");
        let mut text_at = 600;
        for (i, tr) in self.tracks.iter().enumerate() {
            put32(&mut img, trl1 + 8 + 4 * i, AUDIO_LSN as u32);
            put32(&mut img, trl1 + 1028 + 4 * i, 1);
            put(&mut img, trl2 + 8 + 4 * i, &tc(tr.start));
            put(&mut img, trl2 + 1028 + 4 * i, &tc(tr.duration));
            put(&mut img, igl + 8 + 12 * i, b"XXAAA2600001");
            put(&mut img, igl + 3072 + 4 * i, &[1, 0, 0, 5]);
            if text_at + 16 < S {
                put16(&mut img, ttxt + 8 + 2 * i, text_at as u16);
                put(&mut img, ttxt + text_at, &[1, 0, 0, 0, 1, 0x20]);
                let title = format!("T{}\0", i + 1);
                put(&mut img, ttxt + text_at + 6, title.as_bytes());
                text_at += 16;
            }
        }
        for (i, s) in self.audio.iter().enumerate() {
            put(&mut img, (AUDIO_LSN + i) * S, s);
        }
        img
    }
}
