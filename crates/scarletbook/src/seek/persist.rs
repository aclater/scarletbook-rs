// SPDX-License-Identifier: MIT OR Apache-2.0

//! Saving and loading an [`AreaIndex`], and merging two.
//!
//! The format is little-endian. Version 1:
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 8 | `SBSEEKIX` |
//! | 8 | 4 | version, 1 |
//! | 12 | 1 | slot: 0 for 2-channel, 1 for multichannel |
//! | 13 | 1 | channel count |
//! | 14 | 1 | flags: bit 0 set when complete, others 0 |
//! | 15 | 1 | 0 |
//! | 16 | 4 | the area's first audio LSN (area TOC) |
//! | 20 | 4 | the area's last audio LSN (area TOC) |
//! | 24 | 8 | the image's sector count |
//! | 32 | 4 | interval `K` |
//! | 36 | 4 | checkpoints `n` |
//! | 40 | 4 | stored frame buffers `f` |
//! | 44 | 4 | checkpoints with a stored frame under assembly `h` |
//! | 48 | 4 | malformed LSNs `m` |
//! | 52 | 4 | 0 |
//! | 56 | 8 | inline budget in bytes |
//! | 64 | 4 | when complete, `a` at the end; else 0 |
//! | 68 | 4 | 0 |
//! | 72 | 8 | when complete, the area's malformed sectors; else 0 |
//! | 80 | 24 `n` | checkpoints: `a` (4), `c - r` (4), malformed before (8), digest (8) |
//! | | | `f` buffers, by increasing start: start LSN (4), length `l` (4), `l` bytes |
//! | | 24 `h` | by increasing checkpoint: checkpoint number (4), buffer (4), length (4), frame number (4), sector count left (4, signed), kind (1: 0 DST, 1 plain), 0 (3) |
//! | | 4 `m` | malformed LSNs, increasing |
//! | | 8 | FNV-1a (64-bit) of every byte before |
//!
//! A buffer holds the longest prefix of one frame under assembly that any checkpoint
//! holds; the checkpoints within that frame refer to it with their own lengths.
//!
//! Version 2 (issue #124) is version 1 with the version field 2, flags bit 1 set when the
//! TOC fingerprint is known, and, before the checksum:
//!
//! | Size | Field |
//! |---|---|
//! | 8 | TOC fingerprint (`toc_fingerprint`), 0 when not known |
//! | 4 | seek hint spacing in frames |
//! | 4 | most seek hints kept, at most `MAX_HINTS` |
//! | 4 | seek hints `s`, at most that |
//! | 4 | 0 |
//! | 8 `s` | seek hints by increasing LSN: LSN (4), frame number of its first record (4) |
//!
//! Version 3 (specification 4, 4.2 item 9) is version 2 with the version field 3, the
//! fingerprint known (flags bit 1), and, after the seek hints:
//!
//! | Size | Field |
//! |---|---|
//! | 4 | confirmed provisional starts `c`, 1 to `MAX_HINTS` |
//! | 4 | 0 |
//! | 16 `c` | by increasing LSN: `L` (4), `u_L` (4), digest of sector `L` (8) |
//!
//! An index is saved as version 1 when it has no fingerprint, no hints and the default
//! hint options, so that version 1 bytes load and save unchanged; version 2 bytes that add
//! nothing to version 1 are refused, so that each index has one saved form.

use super::{AreaIndex, Checkpoint, EndState, Frame, Held, MAX_HINTS, MAX_INTERVAL, Stored};
use crate::audio::{AreaKey, FrameKind, MAX_FRAME_BYTES, MAX_MALFORMED_WARNINGS};
use crate::error::{Error, Slot};
use crate::toc::Area;

const MAGIC: &[u8; 8] = b"SBSEEKIX";
const VERSION: u32 = 1;
const VERSION_HINTS: u32 = 2;
const VERSION_CONFIRMED: u32 = 3;
const CONFIRMED_HEADER: usize = 8;
const CONFIRMED: usize = 16;
const HINTS_HEADER: usize = 24;
const HINT: usize = 8;
const HEADER: usize = 80;
const CHECKPOINT: usize = 24;
const FRAME_HEADER: usize = 8;
const HELD: usize = 24;

/// The largest frame number plus one: no `a` is larger (specification 4, 4.5).
const MAX_NEXT_ACCEPTABLE: u32 = (255 * 60 + 255) * 75 + 255 + 1;

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

fn bad(what: &'static str) -> Error {
    Error::InvalidArgument(what)
}

/// A little-endian reader over the bytes, failing past their end.
struct Input<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Input<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.bytes.len());
        let out = end.and_then(|e| self.bytes.get(self.at..e)).ok_or(bad("seek index truncated"))?;
        self.at += n;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        let lo = u64::from(self.u32()?);
        let hi = u64::from(self.u32()?);
        Ok(lo | hi << 32)
    }

    fn left(&self) -> usize {
        self.bytes.len() - self.at
    }
}

impl AreaIndex {
    /// The index as bytes, to keep it beyond this process (see [`AreaIndex::from_bytes`]):
    /// about 24 bytes per checkpoint plus the stored frames under assembly.
    pub fn to_bytes(&self) -> Vec<u8> {
        let st = &self.stored;
        let frames: usize = st.frames.iter().map(|f| FRAME_HEADER + f.bytes.len()).sum();
        let mut out = Vec::with_capacity(
            HEADER
                + CHECKPOINT * self.checkpoints.len()
                + frames
                + HELD * st.held.len()
                + 4 * self.malformed.len()
                + 8,
        );
        let u32s = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
        let u64s = |out: &mut Vec<u8>, v: u64| out.extend_from_slice(&v.to_le_bytes());
        let defaults = super::IndexOptions::new();
        let v2 = self.fingerprint.is_some()
            || !self.samples.is_empty()
            || (self.hint_spacing, self.max_hints) != (defaults.hint_spacing, defaults.max_hints);
        let v3 = !self.confirmed.is_empty() && self.fingerprint.is_some();
        let v2 = v2 || v3;
        out.extend_from_slice(MAGIC);
        u32s(
            &mut out,
            if v3 {
                VERSION_CONFIRMED
            } else if v2 {
                VERSION_HINTS
            } else {
                VERSION
            },
        );
        out.push(match self.key.slot {
            Slot::TwoChannel => 0,
            Slot::Multichannel => 1,
        });
        out.push(self.key.channels);
        out.push(u8::from(self.end_state.is_some()) | u8::from(v2 && self.fingerprint.is_some()) << 1);
        out.push(0);
        u32s(&mut out, self.key.start);
        u32s(&mut out, self.key.end);
        u64s(&mut out, self.key.image_sectors);
        u32s(&mut out, self.interval);
        // Each count is at most the area's sectors, which come from u32 fields.
        for n in [self.checkpoints.len(), st.frames.len(), st.held.len(), self.malformed.len()] {
            u32s(&mut out, u32::try_from(n).unwrap_or(u32::MAX));
        }
        u32s(&mut out, 0);
        // At most 4 GiB is saved, so that every target loads it as it was saved.
        u64s(&mut out, (self.inline_budget as u64).min(u64::from(u32::MAX)));
        let end = self.end_state.unwrap_or(EndState { next_acceptable: 0, malformed: 0 });
        u32s(&mut out, end.next_acceptable);
        u32s(&mut out, 0);
        u64s(&mut out, end.malformed);
        for c in &self.checkpoints {
            u32s(&mut out, c.next_acceptable);
            u32s(&mut out, c.back);
            u64s(&mut out, c.malformed_before);
            u64s(&mut out, c.digest);
        }
        for f in &st.frames {
            // Audio LSNs come from u32 fields; frames are at most 65 536 bytes.
            u32s(&mut out, u32::try_from(f.start).unwrap_or(u32::MAX));
            u32s(&mut out, f.bytes.len() as u32);
            out.extend_from_slice(&f.bytes);
        }
        for h in &st.held {
            u32s(&mut out, h.at);
            u32s(&mut out, h.frame);
            u32s(&mut out, h.len);
            u32s(&mut out, h.number);
            out.extend_from_slice(&h.remaining.to_le_bytes());
            out.extend_from_slice(&[u8::from(h.kind == FrameKind::Dsd), 0, 0, 0]);
        }
        for &m in &self.malformed {
            u32s(&mut out, m);
        }
        if v2 {
            u64s(&mut out, self.fingerprint.unwrap_or(0));
            u32s(&mut out, self.hint_spacing);
            // At most MAX_HINTS.
            u32s(&mut out, self.max_hints as u32);
            u32s(&mut out, self.samples.len() as u32);
            u32s(&mut out, 0);
            for &(lsn, t) in &self.samples {
                u32s(&mut out, lsn);
                u32s(&mut out, t);
            }
        }
        if v3 {
            // At most MAX_HINTS.
            u32s(&mut out, self.confirmed.len() as u32);
            u32s(&mut out, 0);
            for &(lsn, u, d) in &self.confirmed {
                u32s(&mut out, lsn);
                u32s(&mut out, u);
                u64s(&mut out, d);
            }
        }
        let sum = fnv(&out);
        u64s(&mut out, sum);
        out
    }

    /// An index of `area` in an image of `image_sectors` sectors, from bytes of
    /// [`AreaIndex::to_bytes`], which may be anything: everything is checked before it is
    /// used, and nothing larger than the bytes given is allocated.
    ///
    /// The checks: the format and version; the key (the area's slot, audio sector range
    /// and channel count, and the image's sector count); every count against the area's
    /// size; each checkpoint against the ones before (`a` and the malformed count never
    /// decrease, `a` at most 1 166 881, no frame under assembly starting before the area);
    /// each stored frame under assembly (buffers of at most 65 536 bytes in all within the
    /// inline budget, by increasing start sector inside the area; each referred to by
    /// checkpoints that have a frame under assembly starting there, with lengths that
    /// never decrease and the same frame number and kind); the malformed LSNs (increasing, below the frontier,
    /// as many as the count says up to 4096); and the trailing checksum. Whether the index
    /// is of this very image is checked as it is used (see the [module
    /// documentation](super#trust)).
    ///
    /// The loaded index's inline budget is the smaller of the saved one and
    /// `max_inline_budget`, the caller's bound on the memory of stored frames under
    /// assembly (specification 7, 7.1 item 2): stored frames beyond it are dropped, and
    /// their checkpoints are replayed instead (exact, only slower). The seek hints (format
    /// version 2: at most [`MAX_HINTS`] and the saved maximum, inside the area, by
    /// increasing LSN) have an allowance of the same size, separate from the stored frames
    /// (so the two together take at most twice `max_inline_budget`): the loaded index keeps
    /// at most `max_inline_budget / 8` hints, now and as it grows
    /// ([`super::IndexOptions::max_hints`] becomes the smaller of the saved one and that), dropping
    /// those closest to the one before them; saved confirmations (format version 3, 16 bytes
    /// each) are kept up to as many, and to `max_inline_budget / 16` when loaded, the lowest
    /// sectors going first (specification 7, 7.1 item 2, sacd-spec `cb9530b`). Pass
    /// [`crate::seek::DEFAULT_INLINE_BUDGET`] or the [`IndexOptions`](super::IndexOptions)
    /// the caller builds with.
    ///
    /// These checks make loading **safe**, not **exact**: bytes forged to pass them (and
    /// the digests checked as the index is used) can make cursors return other frames than
    /// the image holds. If exact output matters, rebuild an index read from untrusted
    /// storage ([`AreaIndex::extend`]) rather than load it.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if any check fails, including a saved inline budget above
    /// 4 GiB (`u32::MAX` bytes).
    pub fn from_bytes(
        area: &Area,
        image_sectors: u64,
        bytes: &[u8],
        max_inline_budget: usize,
    ) -> Result<Self, Error> {
        let Some((body, sum)) = bytes.split_last_chunk::<8>() else {
            return Err(bad("seek index truncated"));
        };
        if body.len() < HEADER || body[..8] != MAGIC[..] {
            return Err(bad("not a seek index"));
        }
        if fnv(body) != u64::from_le_bytes(*sum) {
            return Err(bad("seek index checksum does not match"));
        }
        let mut input = Input { bytes: body, at: 8 };
        let version = input.u32()?;
        if !(VERSION..=VERSION_CONFIRMED).contains(&version) {
            return Err(bad("seek index version not supported"));
        }
        let slot = match input.u8()? {
            0 => Slot::TwoChannel,
            1 => Slot::Multichannel,
            _ => return Err(bad("seek index slot invalid")),
        };
        let channels = input.u8()?;
        let flags = input.u8()?;
        let known = if version >= VERSION_HINTS { 3 } else { 1 };
        if flags & !known != 0 || input.u8()? != 0 {
            return Err(bad("seek index flags invalid"));
        }
        let key =
            AreaKey { slot, start: input.u32()?, end: input.u32()?, channels, image_sectors: input.u64()? };
        if key != AreaKey::new(area, image_sectors) {
            return Err(bad("seek index of another area or image"));
        }
        let interval = input.u32()?;
        if !(1..=MAX_INTERVAL).contains(&interval) {
            return Err(bad("seek index interval invalid"));
        }
        let (n, f, h, m) =
            (input.u32()? as usize, input.u32()? as usize, input.u32()? as usize, input.u32()? as usize);
        if input.u32()? != 0 {
            return Err(bad("seek index reserved field not 0"));
        }
        let saved_budget = input.u64()?;
        if saved_budget > u64::from(u32::MAX) {
            return Err(bad("seek index inline budget above 4 GiB"));
        }
        // At most u32::MAX: fits a usize on every supported target.
        let inline_budget = usize::try_from(saved_budget).unwrap_or(usize::MAX);
        let end_next = input.u32()?;
        if input.u32()? != 0 {
            return Err(bad("seek index reserved field not 0"));
        }
        let end_malformed = input.u64()?;
        let options = super::IndexOptions { interval, inline_budget, ..super::IndexOptions::new() };
        let mut index = AreaIndex::new(area, image_sectors, options)?;
        let boundaries = index.boundaries();
        let complete = flags & 1 != 0;
        // Sizes before any allocation: the fixed parts must fit in what is left.
        if n > boundaries
            || (complete && n != boundaries)
            || h > n
            || f > h
            || m > MAX_MALFORMED_WARNINGS
            || n.saturating_mul(CHECKPOINT)
                .saturating_add(f.saturating_mul(FRAME_HEADER))
                .saturating_add(h.saturating_mul(HELD))
                .saturating_add(4 * m)
                .saturating_add(if version >= VERSION_HINTS { HINTS_HEADER } else { 0 })
                .saturating_add(if version >= VERSION_CONFIRMED { CONFIRMED_HEADER } else { 0 })
                > input.left()
        {
            return Err(bad("seek index counts invalid"));
        }
        let k = u64::from(interval);
        let mut checkpoints = Vec::with_capacity(n);
        for j in 0..n as u64 {
            let c = Checkpoint {
                next_acceptable: input.u32()?,
                back: input.u32()?,
                malformed_before: input.u64()?,
                digest: input.u64()?,
            };
            let (a, ma) =
                checkpoints.last().map_or((0, 0), |p: &Checkpoint| (p.next_acceptable, p.malformed_before));
            if c.next_acceptable < a
                || c.next_acceptable > MAX_NEXT_ACCEPTABLE
                || c.malformed_before < ma
                || c.malformed_before > j * k
                || u64::from(c.back) > j * k
            {
                return Err(bad("seek index checkpoint invalid"));
            }
            checkpoints.push(c);
        }
        let mut stored = Stored::default();
        stored.frames.reserve_exact(f);
        for _ in 0..f {
            let start = u64::from(input.u32()?);
            let len = input.u32()? as usize;
            stored.used = stored.used.saturating_add(len);
            let after = stored.frames.last().is_none_or(|p| start > p.start);
            if !after
                || start < index.start
                || start >= index.end
                || len > MAX_FRAME_BYTES
                || stored.used > inline_budget
            {
                return Err(bad("seek index frame under assembly invalid"));
            }
            stored.frames.push(Frame { start, bytes: input.take(len)?.to_vec() });
        }
        stored.held.reserve_exact(h);
        let mut referred = 0usize;
        for _ in 0..h {
            let (at, frame, len, number) = (input.u32()?, input.u32()?, input.u32()?, input.u32()?);
            let remaining = i32::from_le_bytes(input.u32()?.to_le_bytes());
            let kind = match input.take(4)? {
                [0, 0, 0, 0] => FrameKind::Dst,
                [1, 0, 0, 0] => FrameKind::Dsd,
                _ => return Err(bad("seek index frame kind invalid")),
            };
            let held = Held { at, frame, len, number, kind, remaining };
            // The checkpoint's frame under assembly starts where the buffer does.
            let starts = checkpoints.get(at as usize).is_some_and(|c| {
                c.back > 0
                    && stored.frames.get(frame as usize).is_some_and(|b| {
                        index.checkpoint_lsn(at as usize) - u64::from(c.back) == b.start
                            && len as usize <= b.bytes.len()
                    })
            });
            let follows = stored.held.last().is_none_or(|p| {
                at > p.at
                    && (frame == p.frame && len >= p.len && number == p.number && kind == p.kind
                        || frame == p.frame + 1)
            });
            if !starts || !follows || (stored.held.is_empty() && frame != 0) || number >= MAX_NEXT_ACCEPTABLE
            {
                return Err(bad("seek index frame under assembly invalid"));
            }
            referred = frame as usize + 1;
            stored.held.push(held);
        }
        if referred != f {
            return Err(bad("seek index frame under assembly invalid"));
        }
        let end_state = complete.then_some(EndState { next_acceptable: end_next, malformed: end_malformed });
        let last = checkpoints.last().map_or((0, 0), |c| (c.next_acceptable, c.malformed_before));
        let end_invalid = |e: EndState| {
            e.next_acceptable < last.0
                || e.next_acceptable > MAX_NEXT_ACCEPTABLE
                || e.malformed < last.1
                || e.malformed > index.end - index.start
        };
        if end_state.is_some_and(end_invalid) {
            return Err(bad("seek index end state invalid"));
        }
        if !complete && (end_next != 0 || end_malformed != 0) {
            return Err(bad("seek index reserved field not 0"));
        }
        index.checkpoints = checkpoints;
        index.end_state = end_state;
        let frontier = index.frontier();
        let counted = end_state.map_or(last.1, |e| e.malformed);
        if m as u64 != counted.min(MAX_MALFORMED_WARNINGS as u64) {
            return Err(bad("seek index malformed LSNs invalid"));
        }
        let mut malformed = Vec::with_capacity(m);
        for _ in 0..m {
            let lsn = input.u32()?;
            if u64::from(lsn) < index.start
                || u64::from(lsn) >= frontier
                || malformed.last().is_some_and(|&p| lsn <= p)
            {
                return Err(bad("seek index malformed LSNs invalid"));
            }
            malformed.push(lsn);
        }
        if version >= VERSION_HINTS {
            let fingerprint = input.u64()?;
            let (spacing, most, s) = (input.u32()?, input.u32()? as usize, input.u32()? as usize);
            if input.u32()? != 0 {
                return Err(bad("seek index reserved field not 0"));
            }
            let defaults = super::IndexOptions::new();
            // Version 2 only when it adds something: one way to save each index.
            let adds = flags & 2 != 0
                || s > 0
                || (spacing, most) != (defaults.hint_spacing, defaults.max_hints)
                || version == VERSION_CONFIRMED;
            if !adds
                || (flags & 2 == 0 && fingerprint != 0)
                || most > MAX_HINTS
                || s > most
                || s.saturating_mul(HINT) > input.left()
            {
                return Err(bad("seek index hints invalid"));
            }
            // At most the bytes given: checked above.
            let mut samples: Vec<(u32, u32)> = Vec::with_capacity(s);
            for _ in 0..s {
                let (lsn, t) = (input.u32()?, input.u32()?);
                if !(index.start..index.end).contains(&u64::from(lsn))
                    || t >= MAX_NEXT_ACCEPTABLE
                    || samples.last().is_some_and(|&(p, _)| lsn <= p)
                {
                    return Err(bad("seek index hints invalid"));
                }
                samples.push((lsn, t));
            }
            index.fingerprint = (flags & 2 != 0).then_some(fingerprint);
            (index.hint_spacing, index.max_hints, index.samples) = (spacing, most, samples);
        }
        if version == VERSION_CONFIRMED {
            let c = input.u32()? as usize;
            if input.u32()? != 0
                || flags & 2 == 0
                || c == 0
                || c > MAX_HINTS
                || c.saturating_mul(CONFIRMED) > input.left()
            {
                return Err(bad("seek index confirmations invalid"));
            }
            // At most the bytes given: checked above.
            let mut confirmed: Vec<(u32, u32, u64)> = Vec::with_capacity(c);
            for _ in 0..c {
                let (lsn, u, d) = (input.u32()?, input.u32()?, input.u64()?);
                if !(index.start..index.end).contains(&u64::from(lsn))
                    || u >= MAX_NEXT_ACCEPTABLE
                    || confirmed.last().is_some_and(|&(p, _, _)| lsn <= p)
                {
                    return Err(bad("seek index confirmations invalid"));
                }
                confirmed.push((lsn, u, d));
            }
            index.confirmed = confirmed;
        }
        if input.left() != 0 {
            return Err(bad("seek index has trailing bytes"));
        }
        // Within the caller's bound: the last frames go first, with the checkpoints that
        // refer to them.
        index.inline_budget = inline_budget.min(max_inline_budget);
        while stored.used > index.inline_budget {
            let Some(last) = stored.frames.pop() else { break };
            stored.used -= last.bytes.len();
            let gone = stored.frames.len() as u32;
            stored.held.retain(|h| h.frame != gone);
        }
        index.stored = stored;
        index.malformed = malformed;
        // The caller's bound also bounds the hints, now and as the index grows: at most
        // `max_inline_budget / 8` of them, in addition to the stored frames; and as many saved
        // confirmations at most, within the same bound in bytes when loaded.
        index.max_hints = index.max_hints.min(max_inline_budget / HINT);
        index.thin_hints(index.max_hints);
        let keep = index.max_hints.min(max_inline_budget / CONFIRMED);
        if index.confirmed.len() > keep {
            // The lowest sectors go first, as when kept.
            index.confirmed.drain(..index.confirmed.len() - keep);
        }
        Ok(index)
    }

    /// Takes into this index the checkpoints of `other`, an index of the same area, image
    /// size and interval (for example one built on another thread): checkpoints both hold
    /// must agree, and the index then reaches the farther of the two frontiers. Frames
    /// under assembly beyond this index's inline budget are replayed instead.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`], with both indexes unchanged, if `other` is of another
    /// area, image size or interval, was built from another image than this one in this
    /// process (see [One index per image](AreaIndex#one-index-per-image)), or a checkpoint
    /// both hold differs (indexes of two images).
    pub fn merge(&mut self, other: &AreaIndex) -> Result<(), Error> {
        if other.key != self.key || other.interval != self.interval {
            return Err(bad("seek index of another area, image size or interval"));
        }
        if matches!((self.image_id, other.image_id), (Some(a), Some(b)) if a != b)
            || matches!((self.fingerprint, other.fingerprint), (Some(a), Some(b)) if a != b)
        {
            return Err(bad("seek indexes of two images"));
        }
        let both = self.checkpoints.len().min(other.checkpoints.len());
        if self.checkpoints[..both] != other.checkpoints[..both] {
            return Err(bad("seek indexes differ"));
        }
        if matches!((self.end_state, other.end_state), (Some(a), Some(b)) if a != b) {
            return Err(bad("seek indexes differ"));
        }
        if self.image_id.is_none() {
            self.image_id = other.image_id;
        }
        self.fingerprint = self.fingerprint.or(other.fingerprint);
        self.add_hints(other.samples.clone());
        for &(l, u, d) in &other.confirmed {
            self.keep_confirmation(u64::from(l), u, d);
        }
        // Provisional readings and checkpoints: the union (they are of the same image).
        for c in &other.claims {
            if !self.claims.iter().any(|m| m.id == c.id) {
                self.claims.push(std::sync::Arc::clone(c));
            }
        }
        self.claims.retain(|c| c.verdict().is_none());
        self.cap_claims();
        for (&lsn, &p) in &other.provisional {
            self.provisional.entry(lsn).or_insert(p);
        }
        self.prune_provisional();
        // An area without existing audio sectors has its frontier at its end from the
        // start: completeness is then all there is to take.
        if other.frontier() <= self.frontier() && (self.is_complete() || !other.is_complete()) {
            return Ok(());
        }
        for j in self.checkpoints.len()..other.checkpoints.len() {
            let c = other.checkpoints[j];
            let start = self.checkpoint_lsn(j) - u64::from(c.back);
            // At most one checkpoint per audio sector: below 2^32.
            self.stored.take(&other.stored, j as u32, start, self.inline_budget);
            self.checkpoints.push(c);
        }
        self.end_state = other.end_state;
        self.malformed.clone_from(&other.malformed);
        self.prune_provisional();
        Ok(())
    }
}
