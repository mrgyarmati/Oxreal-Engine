//! Change detection (ADR-0014 §6): ticks, and mutable access that records writes.
//!
//! - The world keeps a **tick**, a 32-bit counter. Every component value (and resource) records
//!   the tick it was **added** at and the tick it was last **changed** at.
//! - Each system run gets the current tick as its *now*, and the world moves to the next tick
//!   after the run. So a system's own writes carry its *now*, and every write made after it ran
//!   (by later systems, commands, hooks, or code using the [`crate::World`] directly) carries a
//!   later tick.
//! - A system's [`crate::Changed`] and [`crate::Added`] filters compare a value's tick with the
//!   system's previous run. It sees what changed since then, never its own writes. On its first
//!   run, everything counts as added and changed.
//! - Writing through [`Mut`] ([`DerefMut`], [`Mut::into_inner`]) stamps the value as changed.
//!   Merely fetching it doesn't.
//!
//! Ticks wrap around. Comparisons are relative to *now*, so they stay right as long as no tick
//! is too old; the world clamps ages periodically ([`MAX_AGE`], [`CHECK_INTERVAL`]).
//!
//! Changed ticks are atomics, so a query can stamp a value (through `&mut T`) while its filter
//! reads the same column's ticks (`Changed<T>`), with no `unsafe` (the crate forbids it). They
//! use relaxed ordering: on common hardware that's an ordinary load or store, and the scheduler
//! orders systems that touch the same data.

use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU32, Ordering};

/// A point in the world's change history. `pub` only for the sealed query traits: this module
/// isn't exported, so it never reaches the public API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tick(u32);

/// The oldest a stored tick may be, in ticks before *now*. Older ones are clamped to this age.
pub(crate) const MAX_AGE: u32 = 3 << 30;

/// How many ticks pass between clamping passes. A tick clamped to [`MAX_AGE`] ages by at most
/// this much before the next pass, and the sum stays below 2³², so wrapping subtraction always
/// gives the true age.
pub(crate) const CHECK_INTERVAL: u32 = 1 << 28;

/// The oldest a system's previous run is taken to be. Stored ticks are at most
/// `MAX_AGE + CHECK_INTERVAL` old, so a clamped previous run is older than all of them: clamping
/// can report a value as changed when it wasn't, but never miss a change.
pub(crate) const MAX_SINCE_AGE: u32 = MAX_AGE + CHECK_INTERVAL + 1;

const _: () = assert!(
    (MAX_AGE as u64) + (CHECK_INTERVAL as u64) + 1 < (1_u64 << 32),
    "ticks could age past the point where wrapping comparisons hold"
);

impl Tick {
    /// The 32-bit tick of a 64-bit tick count. The world counts ticks in 64 bits, which never
    /// wrap in practice; values store the low 32 bits to save memory.
    #[must_use]
    pub(crate) const fn from_count(count: u64) -> Self {
        // Truncation is the point: the low 32 bits.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "ticks are the low 32 bits of the count"
        )]
        Self(count as u32)
    }

    /// The tick a system compares with: its previous run (`last_run`, a 64-bit count), seen
    /// from `now` (also a count), clamped to [`MAX_SINCE_AGE`]. Exact however long ago the
    /// previous run was, because the counts don't wrap.
    #[must_use]
    pub(crate) const fn since(last_run: u64, now: u64) -> Self {
        if now.saturating_sub(last_run) > MAX_SINCE_AGE as u64 {
            Self(Self::from_count(now).0.wrapping_sub(MAX_SINCE_AGE))
        } else {
            Self::from_count(last_run)
        }
    }

    /// How many ticks before `now` this one is.
    #[must_use]
    pub(crate) const fn age(self, now: Self) -> u32 {
        now.0.wrapping_sub(self.0)
    }

    /// This tick, clamped to at most `max_age` before `now`.
    #[must_use]
    pub(crate) const fn clamped(self, now: Self, max_age: u32) -> Self {
        if self.age(now) > max_age {
            Self(now.0.wrapping_sub(max_age))
        } else {
            self
        }
    }

    /// Whether this tick is later than `since`, both seen from `now`: it's younger.
    #[must_use]
    pub(crate) const fn is_after(self, since: Self, now: Self) -> bool {
        self.age(now) < since.age(now)
    }
}

/// A changed tick that can be stamped through a shared reference (see the module docs).
#[derive(Debug)]
pub(crate) struct ChangedTick(AtomicU32);

impl ChangedTick {
    pub(crate) const fn new(tick: Tick) -> Self {
        Self(AtomicU32::new(tick.0))
    }

    #[inline]
    pub(crate) fn get(&self) -> Tick {
        Tick(self.0.load(Ordering::Relaxed))
    }

    #[inline]
    pub(crate) fn set(&self, tick: Tick) {
        self.0.store(tick.0, Ordering::Relaxed);
    }
}

/// Which changes count: those after `since` (exclusive) up to `now`. `since` is `None` when
/// there's no previous run to compare with, and then everything counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangeWindow {
    pub(crate) since: Option<Tick>,
    pub(crate) now: Tick,
}

impl ChangeWindow {
    /// Everything counts, and writes are stamped `now`. For code outside systems.
    pub(crate) const fn everything(now: Tick) -> Self {
        Self { since: None, now }
    }

    /// Whether a value stamped `tick` changed within the window.
    #[inline]
    pub(crate) fn contains(&self, tick: Tick) -> bool {
        match self.since {
            Some(since) => tick.is_after(since, self.now),
            None => true,
        }
    }
}

/// The added and changed ticks of one value, for a resource or a component lookup.
#[derive(Debug)]
pub(crate) struct ValueTicks {
    pub(crate) added: Tick,
    pub(crate) changed: ChangedTick,
}

impl ValueTicks {
    pub(crate) const fn new(tick: Tick) -> Self {
        Self {
            added: tick,
            changed: ChangedTick::new(tick),
        }
    }
}

/// Mutable access to one component value, from [`crate::World::get_mut`] or a `&mut T` query.
///
/// Use it like `&mut T`: fields and methods are reached through it, and `*value = new` replaces
/// the value. Bind it with `mut` to write through it:
///
/// ```
/// # use oxreal_ecs::{Component, World};
/// struct Health(u32);
/// impl Component for Health {}
///
/// let mut world = World::new();
/// let e = world.spawn(Health(10));
/// if let Some(mut health) = world.get_mut::<Health>(e) {
///     health.0 -= 3;
/// }
/// assert_eq!(world.get::<Health>(e).map(|h| h.0), Some(7));
/// ```
///
/// Writing through it ([`DerefMut`], or taking the `&mut T` with [`Mut::into_inner`]) marks the
/// value changed, for [`crate::Changed`] filters (ADR-0014 §6). Reading through it doesn't, so
/// fetch mutably freely and write only when something really changes.
pub struct Mut<'w, T> {
    value: &'w mut T,
    changed: &'w ChangedTick,
    now: Tick,
    /// Whether this access has stamped the value already. Writing through it several times
    /// (one field after another) then stamps once, and after inlining the compiler can drop the
    /// repeated stores.
    stamped: bool,
}

impl<'w, T> Mut<'w, T> {
    pub(crate) fn new(value: &'w mut T, changed: &'w ChangedTick, now: Tick) -> Self {
        Self {
            value,
            changed,
            now,
            stamped: false,
        }
    }

    #[inline]
    fn stamp(&mut self) {
        if !self.stamped {
            self.changed.set(self.now);
            self.stamped = true;
        }
    }

    /// The plain `&mut T`, for passing to code that expects one. Taking it counts as a change.
    #[must_use]
    pub fn into_inner(mut self) -> &'w mut T {
        self.stamp();
        self.value
    }
}

impl<T> Deref for Mut<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.value
    }
}

impl<T> DerefMut for Mut<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        self.stamp();
        self.value
    }
}

impl<T> AsRef<T> for Mut<'_, T> {
    fn as_ref(&self) -> &T {
        self.value
    }
}

impl<T: fmt::Debug> fmt::Debug for Mut<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_ticks_are_after_earlier_ones_across_wraparound() {
        for start in [0, 100, u32::MAX - 5] {
            let since = Tick(start);
            let tick = Tick(start.wrapping_add(2));
            let now = Tick(start.wrapping_add(3));
            assert!(tick.is_after(since, now), "from {start}");
            assert!(!since.is_after(tick, now), "from {start}");
            assert!(!since.is_after(since, now), "a tick isn't after itself");
        }
    }

    #[test]
    fn a_long_unrun_system_compares_older_than_any_stored_tick() {
        let now: u64 = 50_000_000_000; // past several wraps of 32 bits
        let now_tick = Tick::from_count(now);
        // A previous run long ago, even a multiple of 2³² ago, is clamped, not wrapped.
        for last_run in [0, now - (1_u64 << 32), now - u64::from(MAX_SINCE_AGE) - 1] {
            let since = Tick::since(last_run, now);
            assert_eq!(since.age(now_tick), MAX_SINCE_AGE, "from {last_run}");
            // The oldest a stored tick can be still counts as changed.
            let oldest = Tick(now_tick.0.wrapping_sub(MAX_AGE + CHECK_INTERVAL));
            assert!(oldest.is_after(since, now_tick));
        }
        // A recent previous run is exact.
        assert_eq!(Tick::since(now - 3, now), Tick::from_count(now - 3));
    }

    #[test]
    fn clamping_limits_age_and_keeps_young_ticks() {
        let now = Tick(10);
        let ancient = Tick(now.0.wrapping_sub(MAX_AGE + 50));
        assert_eq!(ancient.clamped(now, MAX_AGE).age(now), MAX_AGE);
        let young = Tick(7);
        assert_eq!(young.clamped(now, MAX_AGE), young);
    }

    #[test]
    fn a_window_without_since_contains_everything() {
        let window = ChangeWindow::everything(Tick(5));
        assert!(window.contains(Tick(1)));
        assert!(window.contains(Tick(u32::MAX)));
        let since = ChangeWindow {
            since: Some(Tick(5)),
            now: Tick(9),
        };
        assert!(since.contains(Tick(6)) && since.contains(Tick(9)));
        assert!(!since.contains(Tick(5)) && !since.contains(Tick(4)));
    }

    #[test]
    fn writing_through_mut_stamps_and_reading_does_not() {
        let changed = ChangedTick::new(Tick(1));
        let mut value = 3;
        let mut access = Mut::new(&mut value, &changed, Tick(7));
        assert_eq!(*access, 3);
        assert_eq!(changed.get(), Tick(1), "reading isn't a change");
        *access = 4;
        assert_eq!(changed.get(), Tick(7));
    }
}
