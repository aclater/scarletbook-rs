// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reading state of a scan that met unreadable sectors (#441).
//!
//! The checkpoints of an [`AreaIndex`] hold the state of a scan that read every sector
//! (specification 4, 4.2 item 7). A scan that meets unreadable sectors
//! ([`crate::UnreadableSectors`], #435) has other state past them: frames dropped, another
//! next acceptable frame number, possibly another malformed count, and a lost stretch that
//! may still be open. None of that follows from the index's checkpoints, on a hostile image
//! in either direction, so a cursor never resumes from one at or after a marked sector.
//!
//! What does follow: the state of a scan from the area's first sector before sector `c`
//! depends only on the bytes of the sectors before `c` and on which of them it processed
//! as unreadable. So this module records the state of such a scan itself, at the index's
//! boundaries past the first unreadable sector, **keyed by the unreadable runs before each
//! boundary**: the *damage record*. A cursor resumes from a damage checkpoint only when the
//! sectors marked now before it, and at it, are exactly the runs that scan processed there;
//! everything past it the cursor processes against the marks as they are. The state it then
//! begins with is the one a scan from the first sector with today's marks has at that
//! sector, so it returns what that scan returns (positions, statuses, lost stretches,
//! counts and warnings), on any image. Marks added or removed before a checkpoint make the
//! runs differ, so the checkpoint is not used; marks past it change nothing it holds.
//!
//! Each checkpoint holds, besides the index checkpoint's fields (computed by the scan that
//! met the marks, not taken from the index), the start of the lost stretch open there:
//! where a stretch ends is then known to a resumed cursor, which tells lost positions from
//! missing ones as the scan does.
// TODO(sacd-spec#55): reading with unreadable sectors is not yet in the specification;
// resuming from such state is specification 4, 4.2 item 7 applied to a scan that processes
// unreadable sectors as proposed there.

use std::sync::Arc;

use std::io::{Read, Seek};

use super::{AreaIndex, Checkpoint, Plan, digest};
use crate::audio::{AreaCursor, AreaKey, MAX_MALFORMED_WARNINGS};
use crate::control::UnreadableSectors;
use crate::error::Error;
use crate::image::Image;
use crate::image::SECTOR_SIZE;
use crate::seek::ResumedFrom;

/// Most runs of unreadable sectors a damage record keeps: a scan that meets more records
/// nothing more (specification 7, 7.1 item 2: its memory is bounded whatever the marks).
pub(crate) const MAX_DAMAGE_RUNS: usize = 4096;

/// The state of a scan that met unreadable sectors, before sector `lsn`, a boundary of the
/// index's grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DamageCheckpoint {
    pub(crate) lsn: u64,
    /// The unreadable runs the scan processed before `lsn`: the first `runs` of the record's
    /// (at least one).
    pub(crate) runs: u32,
    /// `a`, the frame under assembly (never stored: replayed), the malformed count and the
    /// digest of sector `lsn`, as for an index checkpoint.
    pub(crate) checkpoint: Checkpoint,
    /// The start of the lost stretch open at `lsn`, if one is.
    pub(crate) lost_from: Option<u32>,
}

/// The damage record of an index: one scan's unreadable runs, and its checkpoints past the
/// first of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Damage {
    /// Half-open, increasing, apart (as the set keeps them), inside the area.
    pub(crate) runs: Vec<(u64, u64)>,
    /// By increasing LSN.
    pub(crate) checkpoints: Vec<DamageCheckpoint>,
    /// The first [`MAX_MALFORMED_WARNINGS`] malformed sectors of the scan below the last
    /// checkpoint.
    pub(crate) malformed: Vec<u32>,
}

impl Damage {
    pub(crate) fn is_empty(&self) -> bool {
        self.checkpoints.is_empty()
    }

    pub(crate) fn memory(&self) -> usize {
        self.runs.capacity() * std::mem::size_of::<(u64, u64)>()
            + self.checkpoints.capacity() * std::mem::size_of::<DamageCheckpoint>()
            + self.malformed.capacity() * std::mem::size_of::<u32>()
    }

    /// The checkpoints a cursor of positions `first` or more may start from: those with
    /// `a <= first` (`a` grows with the LSN), shared with the record; `None` if there is
    /// none. Which one is chosen on the first read ([`DamageCandidates::choose`]), against
    /// the marks of that moment.
    pub(crate) fn candidates(self: &Arc<Self>, first: u32) -> Option<DamageCandidates> {
        let last =
            self.checkpoints.partition_point(|c| c.checkpoint.next_acceptable <= first).checked_sub(1)?;
        Some(DamageCandidates { damage: Arc::clone(self), last, read_to: 0 })
    }

    /// Joins the record of another scan, `runs` and `checkpoints` with its malformed
    /// sectors `malformed` (from the area's first sector, increasing): it extends this one
    /// when the runs of both agree up to the lower of their last checkpoints, its sector
    /// included; otherwise it is of other marks and replaces this one if `replace` (the
    /// newer marks are the likelier to be read with) or this one is empty, keeping this
    /// one's checkpoints before the first sector where the runs differ, which fit both.
    /// Either way each checkpoint stays keyed by the runs before it, so the record is never
    /// wrong, only of use to fewer marks.
    fn join(
        &mut self,
        runs: &[(u64, u64)],
        checkpoints: &[DamageCheckpoint],
        malformed: impl Iterator<Item = u32>,
        replace: bool,
    ) {
        let Some((kept, from)) = self.joins(runs, checkpoints, replace) else { return };
        let last = checkpoints.last().map_or(0, |c| c.lsn);
        // Of other marks, this record's checkpoints before the first sector where the runs
        // differ fit both (the same scan up to there), so they stay, with the other's
        // (#443). They count the same runs before them.
        self.checkpoints.truncate(kept);
        self.checkpoints.extend_from_slice(&checkpoints[from..]);
        self.runs.clear();
        self.runs.extend_from_slice(runs);
        self.malformed.clear();
        self.malformed.extend(malformed.take_while(|&m| u64::from(m) < last).take(MAX_MALFORMED_WARNINGS));
    }

    /// What [`Damage::join`] keeps: this record's first checkpoints and `checkpoints` from
    /// an index on; `None` when it changes nothing.
    ///
    /// Nor does it join checkpoints that do not continue the last one kept (#458): `a` and
    /// the malformed count never decrease along a scan (specification 4, 4.5 item 1), so
    /// such a record is not of a scan of these bytes (a loaded record can be forged), and
    /// the joined record would save to bytes that loading refuses.
    fn joins(
        &self,
        runs: &[(u64, u64)],
        checkpoints: &[DamageCheckpoint],
        replace: bool,
    ) -> Option<(usize, usize)> {
        let last = checkpoints.last()?.lsn;
        let held = self.checkpoints.last().map_or(0, |c| c.lsn);
        // Through the sector at the lower checkpoint: it must be readable to both.
        let upto = held.min(last) + 1;
        let agree = !self.is_empty() && below(&self.runs, upto).eq(below(runs, upto));
        let changes = if agree { last > held } else { replace || self.is_empty() };
        if !changes {
            return None;
        }
        let kept = if agree {
            self.checkpoints.len()
        } else {
            let differ = first_difference(
                &below(&self.runs, upto).collect::<Vec<_>>(),
                &below(runs, upto).collect::<Vec<_>>(),
                upto,
            );
            self.checkpoints.partition_point(|c| c.lsn < differ)
        };
        let prev = kept.checked_sub(1).map(|k| self.checkpoints[k]);
        let from = checkpoints.partition_point(|c| c.lsn <= prev.map_or(0, |p| p.lsn));
        let continues = |p: DamageCheckpoint, c: &DamageCheckpoint| {
            c.checkpoint.next_acceptable >= p.checkpoint.next_acceptable
                && c.checkpoint.malformed_before >= p.checkpoint.malformed_before
        };
        if prev.zip(checkpoints.get(from)).is_some_and(|(p, c)| !continues(p, c)) {
            return None;
        }
        Some((kept, from))
    }
}

/// The runs of `runs` below `upto`, clipped to it.
fn below(runs: &[(u64, u64)], upto: u64) -> impl Iterator<Item = (u64, u64)> + '_ {
    runs.iter().take_while(move |&&(s, _)| s < upto).map(move |&(s, e)| (s, e.min(upto)))
}

/// The first sector below `to` that is in a run of one of `a` and `b` but not of the
/// other, or `to` if there is none. Both are half-open, increasing and apart (a sector
/// after a run is never the start of the next), and below `to`.
fn first_difference(a: &[(u64, u64)], b: &[(u64, u64)], to: u64) -> u64 {
    let mut i = 0;
    loop {
        let at = match (a.get(i), b.get(i)) {
            (None, None) => to,
            (Some(&x), Some(&y)) if x == y => {
                i += 1;
                continue;
            }
            // The earlier start is in one run only; with the same start, so is the
            // earlier end, which the longer run holds.
            (Some(&(s, e)), Some(&(t, f))) => {
                if s == t {
                    e.min(f)
                } else {
                    s.min(t)
                }
            }
            (Some(&(s, _)), None) | (None, Some(&(s, _))) => s,
        };
        return at.min(to);
    }
}

/// The damage checkpoints a cursor may start from (#441), chosen when it first reads: the
/// first `last + 1` of the record it shares (#443).
#[derive(Clone, Debug)]
pub(crate) struct DamageCandidates {
    damage: Arc<Damage>,
    /// The last checkpoint with `a <= first`.
    last: usize,
    /// The first read from the last candidate reaches up to this LSN (0: by chunks).
    pub(crate) read_to: u64,
}

impl DamageCandidates {
    /// The last candidate, the one a cursor starts from when the marks have not changed.
    pub(crate) fn last(&self) -> &DamageCheckpoint {
        &self.damage.checkpoints[self.last]
    }

    /// The latest candidate whose scan met exactly the marks there are now before it, and
    /// at it: `marked(to)` gives the runs marked now within `[S0, to)`. Its state is then
    /// that of a scan from the area's first sector with these marks.
    ///
    /// The candidates that fit form a prefix (#443): a checkpoint at `lsn` fits when the
    /// sectors `[S0, lsn]` are marked now as its scan found them, which holds below the
    /// first sector where the marks and the record's runs differ and nowhere from there on.
    /// The record's runs below a checkpoint are those its scan processed there (a
    /// checkpoint's own sector is readable, so its runs end before it).
    pub(crate) fn choose(&self, marked: impl FnOnce(u64) -> Vec<(u64, u64)>) -> Option<DamageStart> {
        let d = &self.damage;
        let to = self.last().lsn + 1;
        let differ = first_difference(&marked(to), &below(&d.runs, to).collect::<Vec<_>>(), to);
        let j = d.checkpoints[..=self.last].partition_point(|c| c.lsn < differ).checked_sub(1)?;
        let at = d.checkpoints[j];
        Some(DamageStart {
            at,
            runs: d.runs.get(..at.runs as usize)?.to_vec(),
            malformed: d.malformed.iter().copied().take_while(|&m| u64::from(m) < at.lsn).collect(),
            read_to: if j == self.last { self.read_to } else { 0 },
        })
    }
}

/// A damage checkpoint a cursor starts from, chosen when it first reads.
#[derive(Clone, Debug)]
pub(crate) struct DamageStart {
    pub(crate) at: DamageCheckpoint,
    /// The unreadable runs before it.
    pub(crate) runs: Vec<(u64, u64)>,
    /// The malformed sectors below it, the first [`MAX_MALFORMED_WARNINGS`].
    pub(crate) malformed: Vec<u32>,
    /// The first read reaches up to this LSN (0: by chunks).
    pub(crate) read_to: u64,
}

/// The damage checkpoints a cursor records, for [`AreaIndex::absorb`].
#[derive(Clone, Debug)]
pub(crate) struct DamageRecorder {
    key: AreaKey,
    start: u64,
    interval: u32,
    /// Made from the index a [`crate::Disc`] keeps (see [`super::Recorder::kept`]).
    pub(crate) kept: bool,
    /// Recording: the scan met an unreadable sector, and its state is that of a scan from
    /// the area's first sector.
    active: bool,
    /// More runs than [`MAX_DAMAGE_RUNS`]: records nothing more, and keeps the first
    /// [`MAX_DAMAGE_RUNS`] runs and the checkpoints before the next one.
    full: bool,
    /// Every unreadable run the scan processed, from the area's first sector.
    runs: Vec<(u64, u64)>,
    /// The boundary to record next.
    next: u64,
    pub(crate) checkpoints: Vec<DamageCheckpoint>,
}

impl DamageRecorder {
    /// Whether the scan met more runs than [`MAX_DAMAGE_RUNS`]: it records nothing more.
    pub(crate) fn is_full(&self) -> bool {
        self.full
    }

    /// Whether anything was recorded since the last absorb.
    pub(crate) fn has_news(&self) -> bool {
        !self.checkpoints.is_empty()
    }

    /// The first boundary after `lsn`.
    fn boundary_after(&self, lsn: u64) -> u64 {
        let k = u64::from(self.interval);
        self.start + ((lsn.saturating_sub(self.start)) / k + 1) * k
    }

    /// Starts afresh, for a scan from the area's first sector.
    pub(crate) fn restart(&mut self) {
        (self.active, self.full, self.next) = (false, false, u64::MAX);
        self.runs.clear();
        self.checkpoints.clear();
    }

    /// Records on from the damage checkpoint `from`, whose runs are `runs`.
    pub(crate) fn resume(&mut self, from: &DamageStart) {
        self.restart();
        self.active = true;
        self.runs.clone_from(&from.runs);
        self.next = self.boundary_after(from.at.lsn);
    }

    /// The scan processed the unreadable sector `lsn` (after every sector before it).
    pub(crate) fn unreadable(&mut self, lsn: u64) {
        if self.full {
            return;
        }
        let full = self.runs.len() == MAX_DAMAGE_RUNS;
        match self.runs.last_mut() {
            Some(run) if run.1 == lsn => run.1 += 1,
            _ if full => {
                // One run too many: nothing more is recorded. The runs kept are complete
                // (a readable sector ends the last), and the checkpoints recorded so far lie
                // before this one and count only them, so they stay a valid record.
                (self.active, self.full, self.next) = (false, true, u64::MAX);
                return;
            }
            _ => self.runs.push((lsn, lsn + 1)),
        }
        self.active = true;
        self.next = self.boundary_after(lsn);
    }

    /// Whether the state before sector `lsn` is to be recorded.
    #[inline]
    pub(crate) fn due(&self, lsn: u64) -> bool {
        lsn == self.next
    }

    /// Records the state before sector `lsn` (a due boundary, readable), whose bytes are
    /// `sector`: `back` as an index checkpoint's (0 for no frame under assembly).
    pub(crate) fn record(
        &mut self,
        lsn: u64,
        (next_acceptable, back, malformed_before): (u32, u32, u64),
        lost_from: Option<u32>,
        sector: &[u8; SECTOR_SIZE],
    ) {
        // At most MAX_DAMAGE_RUNS.
        let runs = self.runs.len() as u32;
        let checkpoint = Checkpoint { next_acceptable, back, malformed_before, digest: digest(sector) };
        self.checkpoints.push(DamageCheckpoint { lsn, runs, checkpoint, lost_from });
        self.next = self.boundary_after(lsn);
    }
}

impl AreaIndex {
    /// A recorder of damage checkpoints, for a cursor of this index whose state is that of
    /// a scan from the area's first sector.
    pub(crate) fn damage_recorder(&self) -> Box<DamageRecorder> {
        Box::new(DamageRecorder {
            key: self.key,
            start: self.start,
            interval: self.interval,
            kept: false,
            active: false,
            full: false,
            runs: Vec::new(),
            next: u64::MAX,
            checkpoints: Vec::new(),
        })
    }

    /// This index without the state it holds of scans that met unreadable sectors
    /// ([`crate::UnreadableSectors`]; see [`AreaIndex`]): what an index without marks holds,
    /// and saves ([`AreaIndex::to_bytes`]) in the form readers before 1.2.0 load.
    pub fn without_damage(&self) -> AreaIndex {
        AreaIndex { damage: Arc::default(), ..self.clone() }
    }

    /// Where a cursor of positions `first` or more may start from the damage record.
    pub(crate) fn damage_start(&self, first: u32) -> Option<DamageCandidates> {
        self.damage.candidates(first)
    }

    /// Builds the damage record ahead of the reads that use it (#443): reads the area on
    /// from where the record ends for the sectors of `image` marked unreadable now
    /// ([`crate::UnreadableSectors`]), about `max_sectors` sectors (at least to the next
    /// checkpoint it can record, so that each call makes progress), and records the state
    /// of a scan from the area's first sector that meets those sectors, as cursors that
    /// read past them do (see [Unreadable sectors](crate::seek#unreadable-sectors)):
    /// whether the record now reaches the area's end for these marks. With no sector of the
    /// area marked, there is nothing to record: `Ok(true)` without reading
    /// ([`AreaIndex::extend`] builds the index then).
    /// A record keeps at most 4096 runs of unreadable sectors: once the pass meets a run
    /// past those, it records nothing more and returns `Ok(true)`, and ranges past that
    /// run start as without the record.
    ///
    /// The pass starts at the latest damage checkpoint whose marks are those of now, else
    /// at the index's last checkpoint before the first mark (recording the index's own
    /// checkpoints up to it), else at the area's first sector. Marks added or removed
    /// during the pass, or sectors the reader reports unreadable, are processed as a
    /// cursor processes them: the record stays keyed by what the pass met. Building the
    /// whole record so costs one pass from the first mark, split into calls of any size,
    /// for example on another thread with an image of its own and a copy of the index,
    /// then joined with [`AreaIndex::merge`]. The index is of `image` from then on (see
    /// [One index per image](Self#one-index-per-image)).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if `image` has another sector count than the index's, is
    /// another image than the one the index was built from in this process, or does not
    /// match the index; [`Error::Io`] if a read fails ([`Error::Cancelled`] if cancelled).
    /// What was recorded before stays.
    pub fn extend_past_marks<R: Read + Seek>(
        &mut self,
        image: &mut Image<R>,
        max_sectors: u64,
    ) -> Result<bool, Error> {
        if image.sector_count() != self.key.image_sectors {
            return Err(Error::InvalidArgument("seek index of another image size"));
        }
        if self.image_id.is_some_and(|id| id != image.id()) {
            return Err(Error::InvalidArgument("seek index of another image"));
        }
        self.image_id = Some(image.id());
        let Some((mark, from)) = self.past_marks_from(image.unreadable()) else { return Ok(true) };
        let Some(need) = self.next_unmarked_boundary(from, image.unreadable()) else { return Ok(true) };
        let mut cursor = AreaCursor::past_marks(self, mark);
        // Past the next boundary to record, which is recorded when its sector is read.
        let stop = from.saturating_add(max_sectors).max(need.saturating_add(1));
        let scanned = cursor.scan_to(image, stop);
        if let ResumedFrom::Discarded { .. } = cursor.resumed_from() {
            return Err(Error::InvalidArgument("seek index does not match the image"));
        }
        self.take_from(&cursor)?;
        scanned?;
        // Past the most runs a record keeps, nothing more can be recorded: another call
        // would read the same sectors again.
        if cursor.damage_recorder().is_some_and(DamageRecorder::is_full) {
            return Ok(true);
        }
        Ok(self
            .past_marks_from(image.unreadable())
            .is_none_or(|(_, from)| self.next_unmarked_boundary(from, image.unreadable()).is_none()))
    }

    /// The first marked sector of the area, and where the damage record ends for the
    /// marks: its latest checkpoint whose marks are those of now, or the first mark.
    /// `None` if no sector of the area is marked.
    fn past_marks_from(&self, marks: &UnreadableSectors) -> Option<(u64, u64)> {
        let (start, end) = self.key.audio_sector_range();
        let mark = marks.first_in(start, end)?;
        let from = self.damage_start(u32::MAX).and_then(|d| d.choose(|to| marks.runs_in(start, to)));
        Some((mark, from.map_or(mark, |d| d.at.lsn)))
    }

    /// The first boundary of the index's grid after `lsn` that is not marked, if any: the
    /// next one the damage record can hold.
    fn next_unmarked_boundary(&self, lsn: u64, marks: &UnreadableSectors) -> Option<u64> {
        let k = u64::from(self.interval);
        let mut b = self.start + (lsn.saturating_sub(self.start) / k + 1) * k;
        while b < self.end {
            if !marks.contains(b) {
                return Some(b);
            }
            // The first boundary past the run that holds it.
            let run_end = marks.first_unmarked(b, self.end);
            b = self.start + (run_end - self.start).div_ceil(k) * k;
        }
        None
    }

    /// The checkpoint LSN a cursor of positions `first` or more would start at if it read
    /// now, with the unreadable sectors `marks` (as its first read chooses): from the
    /// index's checkpoint if no sector before it is marked, from the latest damage
    /// checkpoint whose marks are those of now if that is later or the index's cannot be
    /// used, or from the area's first sector. `None` for one that reads nothing.
    pub(crate) fn start_lsn_with(&self, first: u32, marks: &UnreadableSectors) -> Option<u64> {
        let (start, end) = self.key.audio_sector_range();
        let upto = match self.plan(first) {
            Plan::Start => None,
            Plan::End(_) => Some(end),
            Plan::At { lsn, .. } => Some(lsn.saturating_add(1)),
        };
        let marked = upto.and_then(|upto| marks.first_in(start, upto));
        let from = upto.map_or(start, |u| u - 1);
        let damage = self
            .damage_start(first)
            .and_then(|d| d.choose(|to| marks.runs_in(start, to)))
            .filter(|d| marked.is_some() || d.at.lsn > from);
        match (damage, marked) {
            (Some(d), _) => Some(d.at.lsn),
            (None, Some(_)) => Some(start),
            (None, None) => self.plan_lsn(first),
        }
    }

    /// Takes the damage checkpoints `r` recorded, with the malformed sectors `warnings` of
    /// its scan (from the area's first sector): see [`Damage::join`], the newer replacing
    /// the record when their marks differ.
    pub(crate) fn take_damage(&mut self, r: &DamageRecorder, warnings: impl Iterator<Item = u32>) {
        if r.key != self.key
            || r.interval != self.interval
            || r.start != self.start
            || r.checkpoints.is_empty()
        {
            return;
        }
        // Changed in place; cloned first only if a cursor still holds the record (#443).
        if self.damage.joins(&r.runs, &r.checkpoints, true).is_some() {
            Arc::make_mut(&mut self.damage).join(&r.runs, &r.checkpoints, warnings, true);
        }
    }

    /// Joins the damage record of `other`, an index of the same area and interval (see
    /// [`Damage::join`]): when their marks differ, `other`'s replaces this one's if
    /// `replace`, or if this index has none.
    pub(crate) fn join_damage(&mut self, other: &AreaIndex, replace: bool) {
        if Arc::ptr_eq(&self.damage, &other.damage) || other.damage.is_empty() {
            return;
        }
        let o = &other.damage;
        if self.damage.joins(&o.runs, &o.checkpoints, replace).is_some() {
            Arc::make_mut(&mut self.damage).join(
                &o.runs,
                &o.checkpoints,
                o.malformed.iter().copied(),
                replace,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Checkpoint, Damage, DamageCheckpoint, first_difference};

    /// A record of one scan that met the run `[10, 12)`, with checkpoints at 14, 16 and 18
    /// whose `a` is the LSN.
    fn record() -> Arc<Damage> {
        let at = |lsn: u64| DamageCheckpoint {
            lsn,
            runs: 1,
            checkpoint: Checkpoint { next_acceptable: lsn as u32, back: 0, malformed_before: 0, digest: 0 },
            lost_from: None,
        };
        Arc::new(Damage {
            runs: vec![(10, 12)],
            checkpoints: vec![at(14), at(16), at(18)],
            malformed: Vec::new(),
        })
    }

    /// The checkpoint a cursor of positions `first` or more starts from with the marked
    /// runs `marks` (#443).
    fn chosen(first: u32, marks: &[(u64, u64)]) -> Option<u64> {
        let clip = |to: u64| -> Vec<(u64, u64)> {
            marks.iter().filter(|r| r.0 < to).map(|&(s, e)| (s, e.min(to))).collect()
        };
        record().candidates(first)?.choose(clip).map(|d| d.at.lsn)
    }

    /// The exact fit boundary (#443 review): a checkpoint fits when the sectors before it
    /// and its own are marked as its scan found them, and only then.
    #[test]
    fn fit_boundaries() {
        let run = (10, 12);
        assert_eq!(chosen(u32::MAX, &[run]), Some(18));
        // A mark on a checkpoint's own sector: the one before it.
        assert_eq!(chosen(u32::MAX, &[run, (18, 19)]), Some(16));
        assert_eq!(chosen(u32::MAX, &[run, (16, 17)]), Some(14));
        assert_eq!(chosen(u32::MAX, &[run, (14, 15)]), None);
        // Just before one: the one before; just after the last: the last.
        assert_eq!(chosen(u32::MAX, &[run, (17, 18)]), Some(16));
        assert_eq!(chosen(u32::MAX, &[run, (15, 16)]), Some(14));
        assert_eq!(chosen(u32::MAX, &[run, (19, 20)]), Some(18));
        // The run met changed by one sector at either end, or gone: none fits.
        for marks in [&[(10, 13)][..], &[(9, 12)], &[(10, 11)], &[(11, 12)], &[]] {
            assert_eq!(chosen(u32::MAX, marks), None, "{marks:?}");
        }
        // Only the candidates with `a <= first`.
        assert_eq!(chosen(16, &[run]), Some(16));
        assert_eq!(chosen(17, &[run, (17, 18)]), Some(16));
        assert_eq!(chosen(13, &[run]), None);
    }

    #[test]
    fn first_differences() {
        let to = 100;
        assert_eq!(first_difference(&[], &[], to), to);
        assert_eq!(first_difference(&[(5, 7)], &[(5, 7)], to), to);
        // A run only one side has, a later or earlier start, a shorter or longer run.
        assert_eq!(first_difference(&[(5, 7)], &[], to), 5);
        assert_eq!(first_difference(&[(5, 7), (9, 10)], &[(5, 7)], to), 9);
        assert_eq!(first_difference(&[(5, 7)], &[(6, 7)], to), 5);
        assert_eq!(first_difference(&[(5, 7)], &[(5, 9)], to), 7);
        assert_eq!(first_difference(&[(1, 2), (5, 7)], &[(1, 2), (4, 7)], to), 4);
    }
}
