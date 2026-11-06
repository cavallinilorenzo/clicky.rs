//! Wait-free single-producer / single-consumer ring buffer for `Copy` items.
//! This is the lock-free path from the key listener to the render callback:
//! pushing and popping never lock, allocate or block.
//!
//! Derived from Rustyvibes 2 (MIT, © 2021–2026 Kunal Bagaria,
//! https://github.com/KunalBagaria/rustyvibes-v2), `crates/rustyvibes/src/engine/ring.rs`.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Keeps the producer and consumer indices on separate cache lines.
#[repr(align(128))]
struct Padded(AtomicUsize);

struct Inner<T> {
    /// Next slot to read; written only by the consumer.
    head: Padded,
    /// Next slot to write; written only by the producer.
    tail: Padded,
    mask: usize,
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
}

// SAFETY: each slot is written by the single producer before it publishes `tail`
// (Release) and read by the single consumer after it observes `tail` (Acquire), and
// vice versa for `head`, so no slot is ever accessed concurrently.
unsafe impl<T: Send> Sync for Inner<T> {}

pub struct Producer<T> {
    inner: Arc<Inner<T>>,
    cached_head: usize,
}

pub struct Consumer<T> {
    inner: Arc<Inner<T>>,
    cached_tail: usize,
    /// Another ring this consumer drains after its own (see [`Consumer::with_producer`]).
    next: Option<Box<Consumer<T>>>,
}

/// Creates a ring holding at least `capacity` items (rounded up to a power of two).
pub fn ring<T: Copy + Send>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let capacity = capacity.next_power_of_two().max(2);
    let slots = (0..capacity).map(|_| UnsafeCell::new(MaybeUninit::uninit())).collect();
    let inner = Arc::new(Inner {
        head: Padded(AtomicUsize::new(0)),
        tail: Padded(AtomicUsize::new(0)),
        mask: capacity - 1,
        slots,
    });
    (
        Producer { inner: inner.clone(), cached_head: 0 },
        Consumer { inner, cached_tail: 0, next: None },
    )
}

impl<T: Copy + Send> Producer<T> {
    /// Appends `item`; returns `false` (dropping it) when the ring is full.
    pub fn push(&mut self, item: T) -> bool {
        let tail = self.inner.tail.0.load(Ordering::Relaxed);
        if tail.wrapping_sub(self.cached_head) > self.inner.mask {
            self.cached_head = self.inner.head.0.load(Ordering::Acquire);
            if tail.wrapping_sub(self.cached_head) > self.inner.mask {
                return false;
            }
        }
        // SAFETY: the slot is free (checked above) and invisible to the consumer until
        // `tail` is published below.
        unsafe { (*self.inner.slots[tail & self.inner.mask].get()).write(item) };
        self.inner.tail.0.store(tail.wrapping_add(1), Ordering::Release);
        true
    }
}

impl<T: Copy + Send> Consumer<T> {
    /// Gives this consumer a second producer with its own ring of `capacity`,
    /// so a second thread can feed it without sharing the first producer. Each
    /// ring stays single-producer; `pop` drains this one first, then the other.
    /// Allocates: call it before handing the consumer to its thread.
    pub fn with_producer(mut self, capacity: usize) -> (Consumer<T>, Producer<T>) {
        let (producer, consumer) = ring(capacity);
        let mut last = &mut self;
        while last.next.is_some() {
            last = last.next.as_mut().expect("checked above");
        }
        last.next = Some(Box::new(consumer));
        (self, producer)
    }

    /// Removes the oldest item of its own ring, else of the next one.
    pub fn pop(&mut self) -> Option<T> {
        self.pop_own().or_else(|| self.next.as_mut().and_then(|next| next.pop()))
    }

    fn pop_own(&mut self) -> Option<T> {
        let head = self.inner.head.0.load(Ordering::Relaxed);
        if head == self.cached_tail {
            self.cached_tail = self.inner.tail.0.load(Ordering::Acquire);
            if head == self.cached_tail {
                return None;
            }
        }
        // SAFETY: the producer initialised this slot before publishing `tail` (Release),
        // which we observed with Acquire.
        let item = unsafe { (*self.inner.slots[head & self.inner.mask].get()).assume_init_read() };
        self.inner.head.0.store(head.wrapping_add(1), Ordering::Release);
        Some(item)
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.inner.head.0.load(Ordering::Relaxed) == self.inner.tail.0.load(Ordering::Acquire)
            && self.next.as_ref().is_none_or(|next| next.is_empty())
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_order_and_capacity() {
        let (mut tx, mut rx) = ring::<u32>(4);
        for i in 0..4 {
            assert!(tx.push(i));
        }
        assert!(!tx.push(99), "full");
        for i in 0..4 {
            assert_eq!(rx.pop(), Some(i));
        }
        assert_eq!(rx.pop(), None);
        assert!(rx.is_empty());
    }

    #[test]
    fn capacity_rounds_up_to_a_power_of_two() {
        let (mut tx, _rx) = ring::<u8>(5);
        assert_eq!((0..100).filter(|_| tx.push(1)).count(), 8);
    }

    #[test]
    fn wraps_around_many_times() {
        let (mut tx, mut rx) = ring::<u64>(8);
        for i in 0..10_000u64 {
            assert!(tx.push(i));
            assert!(!rx.is_empty());
            assert_eq!(rx.pop(), Some(i));
        }
    }

    #[test]
    fn a_second_producer_feeds_the_same_consumer() {
        let (mut keys, rx) = ring::<u32>(4);
        let (mut rx, mut previews) = rx.with_producer(2);
        assert!(previews.push(100));
        assert!(keys.push(1));
        assert!(keys.push(2));
        assert!(previews.push(101));
        assert!(!previews.push(102), "each ring keeps its own capacity");
        let drained: Vec<u32> = std::iter::from_fn(|| rx.pop()).collect();
        assert_eq!(drained, [1, 2, 100, 101]);
        assert!(rx.is_empty());
        assert!(previews.push(103), "space is freed as the consumer pops");
        assert_eq!(rx.pop(), Some(103));
    }

    #[test]
    fn two_producer_threads_lose_nothing() {
        const N: u64 = 100_000;
        let (mut a, rx) = ring::<u64>(64);
        let (mut rx, mut b) = rx.with_producer(64);
        let spawn = |mut push: Box<dyn FnMut(u64) -> bool + Send>, offset: u64| {
            std::thread::spawn(move || {
                for i in 0..N {
                    while !push(offset + i) {
                        std::hint::spin_loop();
                    }
                }
            })
        };
        let ta = spawn(Box::new(move |x| a.push(x)), 0);
        let tb = spawn(Box::new(move |x| b.push(x)), N);
        let (mut next_a, mut next_b) = (0, N);
        while next_a < N || next_b < 2 * N {
            match rx.pop() {
                Some(v) if v < N => {
                    assert_eq!(v, next_a, "each producer's order is kept");
                    next_a += 1;
                }
                Some(v) => {
                    assert_eq!(v, next_b);
                    next_b += 1;
                }
                None => std::hint::spin_loop(),
            }
        }
        ta.join().unwrap();
        tb.join().unwrap();
    }

    #[test]
    fn concurrent_stress_preserves_order() {
        const N: u64 = 1_000_000;
        let (mut tx, mut rx) = ring::<u64>(64);
        let producer = std::thread::spawn(move || {
            for i in 0..N {
                while !tx.push(i) {
                    std::hint::spin_loop();
                }
            }
        });
        let mut expected = 0;
        while expected < N {
            if let Some(v) = rx.pop() {
                assert_eq!(v, expected);
                expected += 1;
            } else {
                std::hint::spin_loop();
            }
        }
        producer.join().unwrap();
    }
}
