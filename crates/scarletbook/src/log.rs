// SPDX-License-Identifier: MIT OR Apache-2.0

//! An optional diagnostic trail: what the library read, detected and met on the way.
//!
//! A [`Log`] belongs to one image (and so to one [`crate::Disc`]); there is no global
//! logger. Without one, or above its level, nothing is formatted: each potential message
//! costs one test of a field. Warnings ([`crate::Warning`]) are returned as before; the
//! log repeats them, at [`Level::Warn`], where they occur.
//!
//! The sink is called on the thread that makes the call that leads to the message, in
//! program order, with one exception: with the `parallel` feature, jobs that decode DST
//! frames on rayon's pool report at [`Level::Trace`] and [`Level::Error`] from the pool's
//! threads, possibly after the call that started them has returned. Their lines can
//! interleave with each other and with the calling thread's. Frame problems are reported
//! on the calling thread, in timeline order, when the frame is returned.
//!
//! A job of the pool may also run on the calling thread, or on a pool thread inside a
//! sink call, whenever that thread waits for a frame (rayon runs pending jobs while it
//! waits): the sink can then be entered again on a thread that is already inside it, for
//! example when it decodes on the same pool. A sink that takes a lock which it, or the
//! caller around its calls into this crate, already holds can deadlock.
//!
//! Problem lines are capped: each cursor writes at most
//! [`MAX_PROBLEM_LINES`](crate::audio::MAX_PROBLEM_LINES) lines per kind of problem
//! (malformed sectors and discarded frames per area scan, missing and invalid frames per
//! track), then one line saying the rest are suppressed. The counts are kept in full.
//!
//! A sink should not panic. If it does on the calling thread, the panic propagates out of
//! the call that logged; the cursor or disc in use is then in an unspecified (but safe)
//! state, and may skip or repeat frames if used further. A returned frame stays in the
//! cursor while its line is logged, so that one is not lost. On a pool thread, the panic
//! is caught with the decoding job's and comes back on the caller as a panic while
//! decoding, from the call that takes the frames of that job.
//!
//! ```
//! use scarletbook::log::{Level, Log};
//! let log = Log::new(Level::Debug, |level, message| eprintln!("{level}: {message}"));
//! assert!(log.enabled(Level::Info) && !log.enabled(Level::Trace));
//! ```

use std::fmt;
use std::sync::Arc;

/// The severity of a message, most severe first. The values are those of the C
/// interface's `SB_LOG_*` constants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// The reader failed, or decoding stopped on an internal error.
    #[default]
    Error = 1,
    /// A problem in the image: a warning of specification 7, a malformed sector, a
    /// missing or invalid frame, a frame the reassembler discarded.
    Warn = 2,
    /// What was found: the sector format, the TOC copies used, the areas, the tracks.
    Info = 3,
    /// What was done: every read (LSN, sector count, byte range), format probes, TOC
    /// copy comparisons, area scans.
    Debug = 4,
    /// Every timeline position returned, and each group of DST frames decoded.
    Trace = 5,
}

impl Level {
    /// Every level, most severe first.
    pub const ALL: [Level; 5] = [Level::Error, Level::Warn, Level::Info, Level::Debug, Level::Trace];

    /// The level's name, in lower case.
    pub fn name(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
            Level::Trace => "trace",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The function a [`Log`] calls.
type Sink = dyn Fn(Level, fmt::Arguments<'_>) + Send + Sync;

/// A log sink and the most verbose level it takes, or none (the default).
///
/// Cloning shares the sink. The sink must be `Send + Sync`: with the `parallel` feature it
/// is called from rayon's threads too (see the [module documentation](self), which also
/// says what follows a sink that panics).
#[derive(Clone, Default)]
pub struct Log {
    sink: Option<Arc<Sink>>,
    level: Level,
}

impl Log {
    /// A log that passes the messages at `level` and more severe ones to `sink`.
    pub fn new(level: Level, sink: impl Fn(Level, fmt::Arguments<'_>) + Send + Sync + 'static) -> Self {
        Log { sink: Some(Arc::new(sink)), level }
    }

    /// No log: nothing is formatted.
    pub fn none() -> Self {
        Log::default()
    }

    /// The most verbose level passed on, `None` without a sink.
    pub fn level(&self) -> Option<Level> {
        self.sink.as_ref().map(|_| self.level)
    }

    /// Whether a message at `level` would reach the sink.
    #[inline]
    pub fn enabled(&self, level: Level) -> bool {
        level <= self.level && self.sink.is_some()
    }

    /// Passes a message to the sink if `level` is enabled.
    #[inline]
    pub fn write(&self, level: Level, message: fmt::Arguments<'_>) {
        if let Some(sink) = &self.sink {
            if level <= self.level {
                sink(level, message);
            }
        }
    }
}

impl fmt::Debug for Log {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Log").field("level", &self.level()).finish_non_exhaustive()
    }
}

/// Logs a message at a level, `sb_log!(log, Debug, "format", args...)`: the arguments are
/// evaluated and formatted only if the level is enabled.
macro_rules! sb_log {
    ($log:expr, $level:ident, $($arg:tt)+) => {{
        let log: &$crate::log::Log = $log;
        if log.enabled($crate::log::Level::$level) {
            log.write($crate::log::Level::$level, format_args!($($arg)+));
        }
    }};
}
pub(crate) use sb_log;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts how many times it is formatted.
    struct Counted<'a>(&'a AtomicUsize);

    impl fmt::Display for Counted<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.fetch_add(1, Ordering::Relaxed);
            f.write_str("counted")
        }
    }

    #[test]
    fn nothing_is_formatted_without_a_sink_or_above_its_level() {
        let formatted = AtomicUsize::new(0);
        let evaluated = AtomicUsize::new(0);
        let arg = || {
            evaluated.fetch_add(1, Ordering::Relaxed);
            Counted(&formatted)
        };
        sb_log!(&Log::none(), Error, "{}", arg());
        assert_eq!(Log::none().level(), None);
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let log = Log::new(Level::Info, move |level, m| sink.lock().unwrap().push(format!("{level}: {m}")));
        sb_log!(&log, Debug, "{}", arg());
        sb_log!(&log, Trace, "{}", arg());
        assert_eq!((formatted.load(Ordering::Relaxed), evaluated.load(Ordering::Relaxed)), (0, 0));
        sb_log!(&log, Info, "{}", arg());
        sb_log!(&log.clone(), Warn, "{} {}", arg(), 2);
        assert_eq!((formatted.load(Ordering::Relaxed), evaluated.load(Ordering::Relaxed)), (2, 2));
        assert_eq!(*lines.lock().unwrap(), ["info: counted", "warn: counted 2"]);
        assert_eq!(log.level(), Some(Level::Info));
    }

    #[test]
    fn levels_are_ordered_and_numbered() {
        assert!(Level::ALL.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(Level::ALL.map(|l| l as i32), [1, 2, 3, 4, 5]);
    }
}
