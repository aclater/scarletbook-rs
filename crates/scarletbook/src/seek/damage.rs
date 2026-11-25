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

use super::{AreaIndex, Checkpoint, digest};
use crate::audio::{AreaKey, MAX_MALFORMED_WARNINGS};
use crate::image::SECTOR_SIZE;

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

    /// Where a cursor of positions `first` or more may start: the last checkpoint with
    /// `a <= first` (`a` grows with the LSN), with the runs before it and the malformed
    /// sectors below it. Usable only if the marks before it are those runs
    /// ([`DamageStart::fits`]).
    pub(crate) fn start(&self, first: u32) -> Option<DamageStart> {
        let j = self.checkpoints.partition_point(|c| c.checkpoint.next_acceptable <= first).checked_sub(1)?;
        let at = self.checkpoints[j];
        Some(DamageStart {
            at,
            runs: self.runs.get(..at.runs as usize)?.to_vec(),
            malformed: self.malformed.iter().copied().take_while(|&m| u64::from(m) < at.lsn).collect(),
            read_to: 0,
        })
    }
}

/// A damage checkpoint a cursor may start from, chosen when it first reads.
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

impl DamageStart {
    /// Whether the marks before the checkpoint and at it, `marked` (the runs of the set
    /// within `[S0, lsn + 1)`), are the runs its scan processed: only then is its state that
    /// of a scan from the first sector with these marks.
    pub(crate) fn fits(&self, marked: &[(u64, u64)]) -> bool {
        marked == self.runs.as_slice()
    }
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
    /// and saves ([`AreaIndex::to_bytes`]) in the form readers before 1.3.0 load.
    pub fn without_damage(&self) -> AreaIndex {
        AreaIndex { damage: Damage::default(), ..self.clone() }
    }

    /// Where a cursor of positions `first` or more may start from the damage record.
    pub(crate) fn damage_start(&self, first: u32) -> Option<DamageStart> {
        self.damage.start(first)
    }

    /// Takes the damage checkpoints `r` recorded, with the malformed sectors `warnings` of
    /// its scan (from the area's first sector). They extend the record when the runs of
    /// both agree up to the lower of their last checkpoints, its sector included; otherwise they are of other
    /// marks and replace it (the newer marks are the likelier to be read with). Either way
    /// each checkpoint stays keyed by the runs before it, so the record is never wrong,
    /// only of use to fewer marks.
    pub(crate) fn take_damage(&mut self, r: &DamageRecorder, warnings: impl Iterator<Item = u32>) {
        if r.key != self.key || r.interval != self.interval || r.start != self.start {
            return;
        }
        let Some(last) = r.checkpoints.last().map(|c| c.lsn) else { return };
        let d = &mut self.damage;
        let held = d.checkpoints.last().map_or(0, |c| c.lsn);
        // Through the sector at the lower checkpoint: it must be readable to both.
        let upto = held.min(last) + 1;
        let below = |runs: &[(u64, u64)]| -> Vec<(u64, u64)> {
            runs.iter().take_while(|&&(s, _)| s < upto).map(|&(s, e)| (s, e.min(upto))).collect()
        };
        let agree = !d.is_empty() && below(&d.runs) == below(&r.runs);
        if agree && last <= held {
            return;
        }
        if !agree {
            d.checkpoints.clear();
        }
        d.checkpoints.extend(r.checkpoints.iter().filter(|c| c.lsn > held || !agree));
        d.runs.clone_from(&r.runs);
        d.malformed = warnings.take_while(|&m| u64::from(m) < last).take(MAX_MALFORMED_WARNINGS).collect();
    }
}
