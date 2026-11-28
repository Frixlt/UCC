// SPDX-License-Identifier: GPL-2.0-only
//! Fixed-capacity `no_std` WMI event ring buffer for `uwcc-rs`.
//!
//! Designed for zero-allocation use in both kernel-space (`uwcc_rs.rs`)
//! and host unit tests (`cargo test` / `rustc --test`).

/// Fixed capacity of the WMI event ring buffer (matches `UWCC_WMI_RING_CAPACITY`).
pub(crate) const WMI_RING_CAPACITY: usize = 64;

/// Binary-compatible representation of `struct uwcc_wmi_event` from `uapi/uwcc.h`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WmiEvent {
    /// Monotonic kernel timestamp (`CLOCK_MONOTONIC`, nanoseconds).
    pub timestamp_ns: u64,
    /// Firmware WMI event code.
    pub code: u32,
    /// Auxiliary event payload.
    pub payload: u32,
}

const _: () = assert!(core::mem::size_of::<WmiEvent>() == 16);

/// Fixed-size FIFO ring buffer with oldest-drop overwrite semantics.
pub(crate) struct WmiRingBuffer {
    entries: [WmiEvent; WMI_RING_CAPACITY],
    head: usize,
    tail: usize,
    len: usize,
    dropped: u64,
}

impl WmiRingBuffer {
    /// Creates an empty ring buffer.
    pub(crate) const fn new() -> Self {
        Self {
            entries: [WmiEvent {
                timestamp_ns: 0,
                code: 0,
                payload: 0,
            }; WMI_RING_CAPACITY],
            head: 0,
            tail: 0,
            len: 0,
            dropped: 0,
        }
    }

    /// Appends an event to the ring buffer.
    ///
    /// Returns `true` if inserted without overwriting an unread event, or
    /// `false` if the buffer was full and the oldest entry was dropped.
    pub(crate) fn push(&mut self, event: WmiEvent) -> bool {
        let mut no_drop = true;
        if self.len == WMI_RING_CAPACITY {
            self.tail = (self.tail + 1) % WMI_RING_CAPACITY;
            self.len -= 1;
            self.dropped = self.dropped.saturating_add(1);
            no_drop = false;
        }

        self.entries[self.head] = event;
        self.head = (self.head + 1) % WMI_RING_CAPACITY;
        self.len += 1;
        no_drop
    }

    /// Removes and returns the oldest event, or `None` if empty.
    pub(crate) fn pop(&mut self) -> Option<WmiEvent> {
        if self.len == 0 {
            return None;
        }

        let event = self.entries[self.tail];
        self.tail = (self.tail + 1) % WMI_RING_CAPACITY;
        self.len -= 1;
        Some(event)
    }

    /// Returns `true` when no events are queued.
    #[inline]
    pub(crate) const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns `true` when the buffer is at capacity.
    #[cfg(test)]
    #[inline]
    pub(crate) const fn is_full(&self) -> bool {
        self.len == WMI_RING_CAPACITY
    }

    /// Returns the number of queued unread events.
    #[cfg(test)]
    #[inline]
    pub(crate) const fn len(&self) -> usize {
        self.len
    }

    /// Returns the total number of overwritten events since initialization.
    #[cfg(test)]
    #[inline]
    pub(crate) const fn dropped_count(&self) -> u64 {
        self.dropped
    }

    /// Empties all queued events while preserving the dropped counter.
    pub(crate) fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.len = 0;
    }
}

impl Default for WmiRingBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_order_and_empty_pop() {
        let mut rb = WmiRingBuffer::new();
        assert!(rb.is_empty());
        assert_eq!(rb.pop(), None);

        let e1 = WmiEvent {
            timestamp_ns: 100,
            code: 0x01,
            payload: 0xAA,
        };
        let e2 = WmiEvent {
            timestamp_ns: 200,
            code: 0x02,
            payload: 0xBB,
        };

        assert!(rb.push(e1));
        assert!(rb.push(e2));
        assert_eq!(rb.len(), 2);
        assert_eq!(rb.pop(), Some(e1));
        assert_eq!(rb.pop(), Some(e2));
        assert_eq!(rb.pop(), None);
        assert!(rb.is_empty());
        assert_eq!(rb.dropped_count(), 0);
    }

    #[test]
    fn overflow_drops_oldest_and_tracks_count() {
        let mut rb = WmiRingBuffer::new();
        for i in 0..WMI_RING_CAPACITY {
            assert!(rb.push(WmiEvent {
                timestamp_ns: i as u64,
                code: i as u32,
                payload: 0,
            }));
        }
        assert!(rb.is_full());

        // Push 5 extra events; oldest 5 (0..5) must be dropped.
        for i in WMI_RING_CAPACITY..(WMI_RING_CAPACITY + 5) {
            assert!(!rb.push(WmiEvent {
                timestamp_ns: i as u64,
                code: i as u32,
                payload: 1,
            }));
        }

        assert_eq!(rb.len(), WMI_RING_CAPACITY);
        assert_eq!(rb.dropped_count(), 5);

        let first = rb.pop().expect("ring non-empty");
        assert_eq!(first.code, 5);
        assert_eq!(rb.len(), WMI_RING_CAPACITY - 1);
    }

    #[test]
    fn wrap_around_and_clear() {
        let mut rb = WmiRingBuffer::new();
        for round in 0..4u32 {
            for i in 0..40u32 {
                let code = round * 100 + i;
                assert!(rb.push(WmiEvent {
                    timestamp_ns: code as u64,
                    code,
                    payload: round,
                }));
            }
            for i in 0..40u32 {
                let expected = round * 100 + i;
                assert_eq!(rb.pop().map(|e| e.code), Some(expected));
            }
            assert!(rb.is_empty());
        }

        assert!(rb.push(WmiEvent {
            timestamp_ns: 999,
            code: 42,
            payload: 0,
        }));
        rb.clear();
        assert!(rb.is_empty());
        assert_eq!(rb.pop(), None);
    }
}
