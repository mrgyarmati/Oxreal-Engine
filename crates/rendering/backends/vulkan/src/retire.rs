//! Deferred destruction (ADR-0016 §1).
//!
//! A destroyed object may still be used by the GPU, or by an encoder that is still recording and
//! hasn't been submitted yet. [`RetireQueue`] holds such objects until both are over:
//!
//! - Every submission made before the object was retired has finished (its timeline value).
//! - Every encoder that was open when it was retired has been submitted and has finished, or was
//!   dropped without submitting.
//!
//! This is pure bookkeeping with no Vulkan calls, so it's unit-tested without a GPU.

use std::collections::BTreeSet;

/// Identifies an open encoder in a [`RetireQueue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct EncoderId(u64);

#[derive(Debug)]
struct Entry<T> {
    object: T,
    /// The object can go once the timeline reaches this value.
    after: u64,
    /// Encoders that were open when the object was retired and haven't been submitted yet.
    blockers: BTreeSet<EncoderId>,
}

/// Objects waiting for the GPU and open encoders to finish with them.
#[derive(Debug)]
pub(crate) struct RetireQueue<T> {
    next_encoder: u64,
    open: BTreeSet<EncoderId>,
    entries: Vec<Entry<T>>,
}

impl<T> Default for RetireQueue<T> {
    fn default() -> Self {
        Self {
            next_encoder: 0,
            open: BTreeSet::new(),
            entries: Vec::new(),
        }
    }
}

impl<T> RetireQueue<T> {
    /// Registers a newly created encoder.
    pub(crate) fn open_encoder(&mut self) -> EncoderId {
        let id = EncoderId(self.next_encoder);
        self.next_encoder += 1;
        self.open.insert(id);
        id
    }

    /// The encoder was submitted with timeline value `value`. Objects it blocked now wait for
    /// that value too.
    pub(crate) fn encoder_submitted(&mut self, id: EncoderId, value: u64) {
        self.open.remove(&id);
        for entry in &mut self.entries {
            if entry.blockers.remove(&id) {
                entry.after = entry.after.max(value);
            }
        }
    }

    /// The encoder was dropped without being submitted, so it never reaches the GPU.
    pub(crate) fn encoder_abandoned(&mut self, id: EncoderId) {
        self.open.remove(&id);
        for entry in &mut self.entries {
            entry.blockers.remove(&id);
        }
    }

    /// Queues `object` until the timeline reaches `after` and every currently open encoder is
    /// done with it.
    pub(crate) fn retire(&mut self, object: T, after: u64) {
        self.entries.push(Entry {
            object,
            after,
            blockers: self.open.clone(),
        });
    }

    /// Queues `object` until the timeline reaches `after`, regardless of open encoders. For
    /// objects only the submission with that value uses, such as its command pool.
    pub(crate) fn retire_after_submission(&mut self, object: T, after: u64) {
        self.entries.push(Entry {
            object,
            after,
            blockers: BTreeSet::new(),
        });
    }

    /// Removes and returns the objects that are safe to destroy once the timeline has reached
    /// `completed`.
    pub(crate) fn take_ready(&mut self, completed: u64) -> Vec<T> {
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.entries)
            .into_iter()
            .partition(|e| e.blockers.is_empty() && e.after <= completed);
        self.entries = waiting;
        ready.into_iter().map(|e| e.object).collect()
    }

    /// Removes and returns every object, for teardown once the device is idle.
    pub(crate) fn drain(&mut self) -> Vec<T> {
        self.open.clear();
        std::mem::take(&mut self.entries)
            .into_iter()
            .map(|e| e.object)
            .collect()
    }

    /// Number of objects still waiting.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_the_timeline() {
        let mut queue = RetireQueue::default();
        queue.retire("a", 5);
        assert!(queue.take_ready(4).is_empty());
        assert_eq!(queue.take_ready(5), vec!["a"]);
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn waits_for_encoders_open_at_retirement() {
        let mut queue = RetireQueue::default();
        let recording = queue.open_encoder();
        queue.retire("pipeline", 3);
        let later = queue.open_encoder(); // Opened after: can't have used the object.
        assert!(
            queue.take_ready(100).is_empty(),
            "the open encoder may reference it"
        );
        queue.encoder_submitted(later, 200);
        assert!(queue.take_ready(100).is_empty(), "only `recording` matters");
        queue.encoder_submitted(recording, 7);
        assert!(
            queue.take_ready(6).is_empty(),
            "now waits for that submission"
        );
        assert_eq!(queue.take_ready(7), vec!["pipeline"]);
    }

    #[test]
    fn abandoned_encoders_stop_blocking() {
        let mut queue = RetireQueue::default();
        let encoder = queue.open_encoder();
        queue.retire("texture", 2);
        queue.encoder_abandoned(encoder);
        assert_eq!(queue.take_ready(2), vec!["texture"]);
    }

    #[test]
    fn submission_objects_ignore_open_encoders() {
        let mut queue = RetireQueue::default();
        let _open = queue.open_encoder();
        queue.retire_after_submission("pool", 4);
        assert_eq!(queue.take_ready(4), vec!["pool"]);
    }

    #[test]
    fn drain_returns_everything() {
        let mut queue = RetireQueue::default();
        let _open = queue.open_encoder();
        queue.retire(1, 10);
        queue.retire_after_submission(2, 20);
        let mut all = queue.drain();
        all.sort_unstable();
        assert_eq!(all, vec![1, 2]);
    }
}
