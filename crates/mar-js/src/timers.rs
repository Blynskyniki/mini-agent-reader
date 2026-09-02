//! Timers on a virtual clock.
//!
//! A real browser sleeps for `setTimeout(fn, 3000)`. We do not: the clock is a
//! counter that jumps forward to the next due timer, so a page that staggers
//! its work over several seconds of wall time settles in microseconds. This is
//! the single biggest reason a page here costs milliseconds instead of seconds.
//!
//! The clock only jumps when nothing else is runnable. While a script is
//! producing microtasks or a network response is outstanding, time stands
//! still, which keeps the relative ordering scripts expect.

use rquickjs::{Function, Persistent};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerKind {
    Timeout,
    Interval,
    /// `requestAnimationFrame`. There are no frames, so these fire on the next
    /// tick at a nominal 60 Hz spacing.
    AnimationFrame,
    /// `requestIdleCallback`. Fires only once the queue is otherwise drained.
    Idle,
}

struct Timer {
    id: u32,
    due_ms: i64,
    /// Repeat interval, for `setInterval`.
    period_ms: Option<i64>,
    kind: TimerKind,
    callback: Persistent<Function<'static>>,
    /// Order of registration, to break ties at the same due time.
    seq: u64,
}

/// Heap ordering: earliest due time first, then registration order.
impl PartialEq for Timer {
    fn eq(&self, other: &Self) -> bool {
        self.due_ms == other.due_ms && self.seq == other.seq
    }
}
impl Eq for Timer {}
impl PartialOrd for Timer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Timer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.due_ms
            .cmp(&other.due_ms)
            .then(self.seq.cmp(&other.seq))
    }
}

pub struct TimerQueue {
    /// `Reverse` turns the max-heap into a min-heap on due time.
    heap: BinaryHeap<Reverse<Timer>>,
    cancelled: std::collections::HashSet<u32>,
    next_id: u32,
    next_seq: u64,
    /// Virtual time since navigation started, in milliseconds.
    now_ms: i64,
    /// Timers scheduled beyond this point are dropped rather than run. Pages
    /// commonly schedule polling far into the future; running those would keep
    /// the page alive forever without changing the content.
    horizon_ms: i64,
    /// Total callbacks run, so a runaway `setInterval` cannot spin forever.
    fired: u64,
    max_fired: u64,
}

impl TimerQueue {
    pub fn new(horizon_ms: i64, max_fired: u64) -> Self {
        TimerQueue {
            heap: BinaryHeap::new(),
            cancelled: std::collections::HashSet::new(),
            next_id: 1,
            next_seq: 0,
            now_ms: 0,
            horizon_ms,
            fired: 0,
            max_fired,
        }
    }

    #[inline]
    pub fn now_ms(&self) -> i64 {
        self.now_ms
    }

    #[inline]
    pub fn fired(&self) -> u64 {
        self.fired
    }

    pub fn schedule(
        &mut self,
        callback: Persistent<Function<'static>>,
        delay_ms: i64,
        kind: TimerKind,
        repeat: bool,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        // Browsers clamp negative and sub-millisecond delays to zero, and nested
        // timeouts to 4ms. Clamping to zero is enough for our purposes.
        let delay = delay_ms.max(0);
        let due = self.now_ms.saturating_add(delay);
        if due > self.horizon_ms {
            // Past the horizon: accept the id so `clearTimeout` still works,
            // but never run it.
            return id;
        }
        self.heap.push(Reverse(Timer {
            id,
            due_ms: due,
            period_ms: repeat.then_some(delay.max(1)),
            kind,
            callback,
            seq: self.next_seq,
        }));
        self.next_seq += 1;
        id
    }

    pub fn cancel(&mut self, id: u32) {
        self.cancelled.insert(id);
    }

    /// Due time of the earliest live timer, ignoring idle callbacks.
    pub fn next_due(&mut self) -> Option<i64> {
        self.drop_cancelled();
        self.heap
            .iter()
            .filter(|Reverse(t)| t.kind != TimerKind::Idle)
            .map(|Reverse(t)| t.due_ms)
            .min()
    }

    pub fn has_pending(&mut self) -> bool {
        self.drop_cancelled();
        !self.heap.is_empty()
    }

    fn drop_cancelled(&mut self) {
        if self.cancelled.is_empty() {
            return;
        }
        while let Some(Reverse(t)) = self.heap.peek() {
            if self.cancelled.contains(&t.id) {
                let Reverse(t) = self.heap.pop().expect("peeked");
                self.cancelled.remove(&t.id);
            } else {
                break;
            }
        }
    }

    /// Advance the clock to the next due timer and return it, or `None` when
    /// the queue is empty or the callback budget is spent.
    ///
    /// `allow_idle` gates `requestIdleCallback`: pass false while other work is
    /// still queued, matching the browser rule that idle work waits its turn.
    pub fn pop_due(&mut self, allow_idle: bool) -> Option<(Persistent<Function<'static>>, u32)> {
        if self.fired >= self.max_fired {
            return None;
        }
        loop {
            let Reverse(peeked) = self.heap.peek()?;
            if peeked.kind == TimerKind::Idle && !allow_idle {
                // The earliest entry is idle work and other work remains. There
                // is nothing else in the heap ahead of it, so stop here.
                return None;
            }
            let Reverse(timer) = self.heap.pop()?;
            if self.cancelled.remove(&timer.id) {
                continue;
            }
            // Jump the clock forward; never backwards.
            self.now_ms = self.now_ms.max(timer.due_ms);
            self.fired += 1;

            if let Some(period) = timer.period_ms {
                let next_due = self.now_ms.saturating_add(period);
                if next_due <= self.horizon_ms {
                    self.heap.push(Reverse(Timer {
                        id: timer.id,
                        due_ms: next_due,
                        period_ms: Some(period),
                        kind: timer.kind,
                        callback: timer.callback.clone(),
                        seq: self.next_seq,
                    }));
                    self.next_seq += 1;
                }
            }
            return Some((timer.callback, timer.id));
        }
    }

    /// Drop every pending timer. Called when the page is done settling.
    pub fn clear(&mut self) {
        self.heap.clear();
        self.cancelled.clear();
    }
}
