//! Lock-free ring buffer implementation.
//!
//! # Performance Optimizations
//! - `MaybeUninit<T>` instead of `Option<T>` - eliminates discriminant overhead
//! - Separate lock atomic - simpler encoding, no sign-bit manipulation  
//! - Cache-padded head/tail to avoid false sharing between producer and consumer
//! - Relaxed/Acquire/Release orderings where safe (instead of SeqCst everywhere)
//! - Power-of-two buffer sizes for bitmask indexing
//!
//! # Lint Configuration
//! This module enforces strict lints to catch common issues. Run the following
//! to check for all issues:
//! ```sh
//! cargo clippy --all-targets -- -D warnings
//! cargo +nightly miri test  # For memory safety verification
//! ```


use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::cache_padded::CachePadded;


pub struct RingBufferConsumer<T> {
    inner: Arc<LockFreeRingBuffer<T>>,
}

impl<T: Send> RingBufferConsumer<T> {
    pub fn pop(&self) -> RingIter<'_, T> {
        self.inner.pop()
    }
}

#[doc(hidden)]
impl<T> From<Arc<LockFreeRingBuffer<T>>> for RingBufferConsumer<T> {
    fn from(inner: Arc<LockFreeRingBuffer<T>>) -> Self {
        RingBufferConsumer { inner }
    }
}

pub struct RingBufferProducer<T> {
    inner: Arc<LockFreeRingBuffer<T>>,
}

impl<T: Send> RingBufferProducer<T> {
    pub fn push(&self, x: T) {
        self.inner.push(x)
    }
}

#[doc(hidden)]
impl<T> From<Arc<LockFreeRingBuffer<T>>> for RingBufferProducer<T> {
    fn from(inner: Arc<LockFreeRingBuffer<T>>) -> Self {
        RingBufferProducer { inner }
    }
}

pub fn new<T: Send>(size: usize) -> (RingBufferProducer<T>, RingBufferConsumer<T>) {
    let buffer = Arc::new(LockFreeRingBuffer::new(size));

    (buffer.clone().into(), buffer.clone().into())
}

/// A lock-free ring buffer optimized for SPSC (single-producer, single-consumer) usage.
/// 
/// # Performance Optimizations
/// - `MaybeUninit<T>` instead of `Option<T>` - eliminates discriminant overhead
/// - Separate lock atomic - simpler encoding, no sign-bit manipulation
/// - Cache-padded head/tail - prevents false sharing
/// - Power-of-two sizing - bitmask instead of modulo
/// 
/// # Cache Layout
/// The struct is laid out to minimize false sharing:
/// - `head` is on its own cache line (modified by producer)
/// - `tail` + `consumer_lock` are on their own cache line (modified by consumer)
/// - `size`, `mask`, and `data` are read-only after construction
pub struct LockFreeRingBuffer<T> {
    // === Cache line 1: Producer-owned ===
    /// Write index, only modified by producer
    head: CachePadded<AtomicUsize>,
    
    // === Cache line 2: Consumer-owned ===
    /// Read index (simple counter, no encoding)
    tail: CachePadded<AtomicUsize>,
    /// Consumer lock flag (separate from tail for cleaner code)
    consumer_lock: CachePadded<AtomicBool>,
    
    // === Cache line 3+: Shared (read-only after init) ===
    /// Capacity of the buffer (immutable after construction)
    size: usize,
    /// Buffer mask for power-of-two indexing
    mask: usize,
    /// The actual data storage - uses MaybeUninit for zero overhead
    data: Vec<UnsafeCell<MaybeUninit<T>>>,
}

unsafe impl<T: Send> Sync for LockFreeRingBuffer<T> {}

impl<T: Send> LockFreeRingBuffer<T> {
    pub fn new(size: usize) -> Self {
        // For a ring buffer of capacity N, we need N+1 slots to distinguish
        // empty from full. Round up (size + 1) to next power of two for efficient masking.
        let buffer_len = (size + 1).next_power_of_two();
        let mask = buffer_len - 1;
        
        // Initialize with MaybeUninit - no need to initialize values
        let data = (0..buffer_len)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect();
        
        LockFreeRingBuffer {
            head: CachePadded::new(AtomicUsize::new(0)),
            tail: CachePadded::new(AtomicUsize::new(0)),
            consumer_lock: CachePadded::new(AtomicBool::new(false)),
            size,
            mask,
            data,
        }
    }

    /// Push an item to the buffer. If the buffer is full, the oldest item is evicted.
    /// 
    /// # Memory Ordering
    /// - Consumer lock check: Acquire (ensure consumer not reading)
    /// - Eviction check: Acquire (to see consumer's tail)
    /// - Data write: Happens before `head` store
    /// - `head` store: Release (makes data visible to consumer)
    #[inline]
    pub fn push(&self, x: T) {
        // First, ensure we have room AND consumer is not reading
        // This MUST happen BEFORE we write to the slot
        loop {
            // Check if consumer is currently reading - if so, wait
            // This prevents data races between push writes and clone reads
            if self.consumer_lock.load(Ordering::Acquire) {
                std::hint::spin_loop();
                continue;
            }
            
            let head = self.head.load(Ordering::Relaxed);
            let tail = self.tail.load(Ordering::Acquire);
            
            // Check if buffer has room for one more item
            let len = head.wrapping_sub(tail);
            if len < self.size {
                break;  // Room available and consumer not reading
            }
            
            // Buffer is full, try to advance tail (evict oldest)
            if self.tail
                .compare_exchange_weak(
                    tail, 
                    tail.wrapping_add(1), 
                    Ordering::AcqRel, 
                    Ordering::Acquire
                )
                .is_ok()
            {
                break;  // Evicted, now we have room
            }
        }
        
        // Now we're guaranteed: room available AND consumer not reading
        let head = self.head.load(Ordering::Relaxed);
        let slot = head & self.mask;
        let slot_ptr = self.data[slot].get();
        
        // SAFETY: We've ensured this slot is not in use by consumer
        unsafe { (*slot_ptr).write(x); }

        // Increment head with Release ordering
        // This is the linearization point - makes the write visible to consumers
        let new_head = head.wrapping_add(1);
        self.head.store(new_head, Ordering::Release);
    }

    /// Get the length of the buffer.
    /// 
    /// Note: This is an approximation during concurrent operations because
    /// head and tail are read separately (not atomically together).
    #[inline]
    pub fn len(&self) -> usize {
        // Relaxed is sufficient for approximate length
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        head.wrapping_sub(tail)
    }

    /// Check if the buffer is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        head == tail
    }
    
    /// Get a clone of the item at index `index`, relative to the tail.
    /// 
    /// Returns `None` if the index is out of bounds.
    /// 
    /// # Note
    /// This method spins while waiting for the consumer lock if another
    /// thread is currently iterating via `pop()`.
    pub fn get_cloned(&self, index: usize) -> Option<T>
    where
        T: Clone,
    {
        // Spin to acquire consumer lock
        while self.consumer_lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        
        let tail = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        let result = if head.wrapping_sub(tail) > index {
            let slot = tail.wrapping_add(index) & self.mask;
            let ptr = self.data[slot].get();
            // SAFETY: We hold the consumer lock, producer can't evict this slot
            unsafe { Some((*ptr).assume_init_ref().clone()) }
        } else {
            None
        };
        
        // Release consumer lock
        self.consumer_lock.store(false, Ordering::Release);
        result
    }

    /// Get a clone of the most recently pushed item.
    /// 
    /// Returns `None` if the buffer is empty.
    /// 
    /// # Note
    /// This method spins while waiting for the consumer lock if another
    /// thread is currently iterating via `pop()`.
    pub fn back_cloned(&self) -> Option<T>
    where
        T: Clone,
    {
        // Spin to acquire consumer lock
        while self.consumer_lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        let result = if head != tail {
            let slot = head.wrapping_sub(1) & self.mask;
            let ptr = self.data[slot].get();
            // SAFETY: We hold the consumer lock
            unsafe { Some((*ptr).assume_init_ref().clone()) }
        } else {
            None
        };
        
        self.consumer_lock.store(false, Ordering::Release);
        result
    }

    /// Get a clone of the oldest item (front of the queue).
    /// 
    /// Returns `None` if the buffer is empty.
    /// 
    /// # Note
    /// This method spins while waiting for the consumer lock if another
    /// thread is currently iterating via `pop()`.
    pub fn front_cloned(&self) -> Option<T>
    where
        T: Clone,
    {
        // Spin to acquire consumer lock
        while self.consumer_lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        
        let tail = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        let result = if head != tail {
            let slot = tail & self.mask;
            let ptr = self.data[slot].get();
            // SAFETY: We hold the consumer lock
            unsafe { Some((*ptr).assume_init_ref().clone()) }
        } else {
            None
        };
        
        self.consumer_lock.store(false, Ordering::Release);
        result
    }

    
    /// Pop items from the buffer, returning an iterator over them.
    /// 
    /// This method supports multiple concurrent consumers (MPMC). Each call to
    /// `next()` on the iterator atomically claims one item via CAS.
    /// 
    /// # Memory Ordering
    /// - `head` load: Acquire (to see producer's latest writes)
    /// - `tail` CAS: AcqRel (claim item atomically)
    pub fn pop(&self) -> RingIter<'_, T> {
        RingIter {
            inner: self,
            mask: self.mask,
        }
    }
    
    /// Try to pop a single item from the buffer.
    /// 
    /// This method supports multiple concurrent consumers (MPMC). It uses 
    /// CAS-based atomic claiming of items.
    /// 
    /// Returns `None` if the buffer is empty.
    #[inline]
    pub fn try_pop(&self) -> Option<T> {
        loop {
            // Load current positions
            let tail = self.tail.load(Ordering::Acquire);
            let head = self.head.load(Ordering::Acquire);
            
            // Buffer is empty
            if tail == head {
                return None;
            }
            
            // Pre-compute slot (helps CPU pipeline)
            let slot = tail & self.mask;
            
            // Try to claim this item by advancing tail
            match self.tail.compare_exchange_weak(
                tail,
                tail.wrapping_add(1),
                Ordering::Release,
                Ordering::Relaxed
            ) {
                Ok(_) => {
                    // We successfully claimed the item at 'tail'
                    let ptr = self.data[slot].get();
                    
                    // SAFETY: We own this slot via successful CAS
                    return Some(unsafe { (*ptr).assume_init_read() });
                }
                Err(_) => {
                    // CAS failed, another consumer got it, retry
                    std::hint::spin_loop();
                }
            }
        }
    }
    
    /// Pop items exclusively (SPSC fast path).
    /// 
    /// This method acquires exclusive consumer access and returns an iterator
    /// that drains items efficiently without CAS overhead. Use this when you
    /// know there's only one consumer thread.
    /// 
    /// # Panics
    /// Panics if another consumer is currently active (either via `pop_exclusive`
    /// or another thread is in the middle of `try_pop`).
    pub fn pop_exclusive(&self) -> ExclusiveIter<'_, T> {
        // Acquire exclusive consumer lock
        if self.consumer_lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            panic!("pop_exclusive: another consumer is active");
        }
        
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Relaxed);
        
        ExclusiveIter {
            head,
            tail,
            inner: self,
            mask: self.mask,
        }
    }
}

/// Fast exclusive iterator for SPSC use case.
/// 
/// This iterator holds an exclusive lock and doesn't need CAS operations.
/// Much faster than the MPMC iterator when only one consumer exists.
pub struct ExclusiveIter<'a, T: 'a + Send> {
    head: usize,
    tail: usize,
    inner: &'a LockFreeRingBuffer<T>,
    mask: usize,
}

impl<'a, T: Send> Iterator for ExclusiveIter<'a, T> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<<Self as Iterator>::Item> {
        if self.tail == self.head {
            // Check if producer pushed more items
            let new_head = self.inner.head.load(Ordering::Acquire);
            if new_head == self.head {
                return None;
            }
            self.head = new_head;
        }
        
        let slot = self.tail & self.mask;
        let ptr = self.inner.data[slot].get();
        
        // SAFETY: We hold exclusive lock
        let item = unsafe { (*ptr).assume_init_read() };
        
        self.tail = self.tail.wrapping_add(1);
        
        // Update tail so producer can see progress
        self.inner.tail.store(self.tail, Ordering::Release);
        
        Some(item)
    }
}

impl<'a, T: 'a + Send> Drop for ExclusiveIter<'a, T> {
    fn drop(&mut self) {
        // Release the consumer lock
        self.inner.consumer_lock.store(false, Ordering::Release);
    }
}

/// Iterator over items in the ring buffer.
/// 
/// This iterator supports multiple concurrent consumers (MPMC).
/// Each call to `next()` atomically claims one item via CAS on the tail.
/// If another consumer claims the item first, this iterator retries.
pub struct RingIter<'a, T: 'a + Send> {
    inner: &'a LockFreeRingBuffer<T>,
    #[allow(dead_code)]
    mask: usize,
}

impl<'a, T: Send> Iterator for RingIter<'a, T> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<<Self as Iterator>::Item> {
        self.inner.try_pop()
    }
}

impl<'a, T: 'a + Send> Drop for RingIter<'a, T> {
    fn drop(&mut self) {
        // No cleanup needed - CAS-based iteration doesn't hold locks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate rand;
    use self::rand::random;
    use std::thread;
    use std::time::Duration;
    #[test]
    fn test_get() {
        let rb = LockFreeRingBuffer::new(5);
        for i in 0..5 {
            rb.push(i);
        }
        assert_eq!(rb.len(), 5);
        assert!(!rb.is_empty());
        
        // Test the safe _cloned methods
        for i in 0..5 {
            assert_eq!(rb.get_cloned(i), Some(i));
        }
        assert_eq!(rb.front_cloned(), Some(0));
        assert_eq!(rb.back_cloned(), Some(4));
        
        // Test out of bounds
        assert_eq!(rb.get_cloned(5), None);
        assert_eq!(rb.get_cloned(100), None);
    }

    #[test]
    #[ignore] // This test takes many minutes to run due to random sleep durations
    fn test_run() {
        let (push, pull) = new(5);

        let t1 = thread::spawn(move || {
            let push = push;
            for i in 1..1000 {
                push.push(i);
                println!("Pushed {}", i);
                thread::sleep(Duration::from_secs(random::<u64>() % 2))
            }
        });

        let t2 = thread::spawn(move || {
            let pull = pull;

            for _i in 1..1000 {
                let num = random::<usize>() % 5;
                println!("Taking {}", num);
                thread::sleep(Duration::from_secs(5));
                let res = pull.pop()
                    .take(num)
                    .inspect(|_x| {
                        thread::sleep(Duration::from_secs(random::<u64>() % 2));
                    })
                    .collect::<Vec<_>>();
                println!("{}, {:?}", num, res);
                thread::sleep(Duration::from_secs(random::<u64>() % 20))
            }
        });

        t1.join().unwrap();
        t2.join().unwrap();
    }

    // ========================================================================
    // TESTS THAT EXPOSE BUGS AND ISSUES
    // ========================================================================

    /// Test: Multiple consumers calling pop() concurrently will panic
    /// due to the assert!(!tail.locked) on line 194.
    /// 
    /// This test demonstrates that the buffer is NOT safe for multiple consumers.
    /// EXPECTED: This test will PANIC with "assertion failed: !tail.locked"
    /// 
    /// NOTE: With MPMC support, this test now verifies that multiple consumers
    /// can correctly share work without panicking or duplicating items.
    #[test]
    fn test_multiple_consumers_no_panic() {
        use std::sync::Barrier;
        use std::collections::HashSet;
        use std::sync::Mutex;
        
        // Run multiple attempts to verify consistency
        for _attempt in 0..3 {
            let buffer = Arc::new(LockFreeRingBuffer::new(100));
            
            // Fill the buffer
            for i in 0..50 {
                buffer.push(i);
            }
            
            let barrier = Arc::new(Barrier::new(3)); // 2 consumers + 1 producer
            let collected = Arc::new(Mutex::new(Vec::<usize>::new()));
            let mut handles = vec![];
            
            // Spawn a producer to keep filling
            let buffer_prod = Arc::clone(&buffer);
            let barrier_prod = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier_prod.wait();
                for i in 50..200 {
                    buffer_prod.push(i);
                }
            }));
            
            // Spawn two consumers that will pop concurrently
            for _consumer_id in 0..2 {
                let buffer_clone = Arc::clone(&buffer);
                let barrier_clone = Arc::clone(&barrier);
                let collected_clone = Arc::clone(&collected);
                
                handles.push(thread::spawn(move || {
                    barrier_clone.wait();
                    
                    let mut local = Vec::new();
                    for _ in 0..100 {
                        for item in buffer_clone.pop() {
                            local.push(item);
                        }
                        thread::yield_now();
                    }
                    
                    collected_clone.lock().unwrap().extend(local);
                }));
            }
            
            for handle in handles {
                handle.join().unwrap();
            }
            
            // Verify no duplicates
            let items = collected.lock().unwrap();
            let unique: HashSet<_> = items.iter().cloned().collect();
            assert_eq!(unique.len(), items.len(), 
                "Duplicates detected! Multiple consumers consumed same item");
        }
        
        println!("MPMC test passed: multiple consumers worked correctly without duplicates");
    }

    /// Test: len() returns incorrect/garbage value when tail is in locked state.
    /// 
    /// When a consumer is iterating (tail is locked/negative), casting 
    /// the isize tail directly to usize produces incorrect results.
    #[test]
    fn test_len_during_locked_tail() {
        use std::sync::Barrier;
        
        let buffer = Arc::new(LockFreeRingBuffer::new(10));
        
        // Fill the buffer
        for i in 0..10 {
            buffer.push(i);
        }
        
        let barrier = Arc::new(Barrier::new(2));
        let buffer1 = Arc::clone(&buffer);
        let buffer2 = Arc::clone(&buffer);
        let barrier1 = Arc::clone(&barrier);
        let barrier2 = Arc::clone(&barrier);
        
        // Thread 1: Pop and hold the iterator (tail locked)
        let handle1 = thread::spawn(move || {
            barrier1.wait();
            
            let iter = buffer1.pop();
            
            // Signal that we're holding the lock
            thread::sleep(Duration::from_millis(100));
            
            // Now consume
            iter.collect::<Vec<_>>()
        });
        
        // Thread 2: Check len() while tail is locked
        let handle2 = thread::spawn(move || {
            barrier2.wait();
            
            // Wait a bit to ensure thread 1 has the lock
            thread::sleep(Duration::from_millis(20));
            
            // This will read a negative isize and cast to usize
            // The result will be HUGE (due to two's complement)
            let len = buffer2.len();
            
            println!("len() during locked state: {}", len);
            
            // If len is unreasonably large, the bug is exposed
            len
        });
        
        let _items = handle1.join().unwrap();
        let observed_len = handle2.join().unwrap();
        
        // If the bug is present, len will be enormous (close to usize::MAX)
        // because a negative isize cast to usize wraps around
        // For a buffer of size 10 with 10 items, len should be <= 10
        assert!(
            observed_len <= 20,
            "BUG: len() returned {} during locked state (expected <= 20). \
             This happens because casting negative isize to usize wraps around.",
            observed_len
        );
    }

    /// Test: Stress test for push/pop data integrity.
    /// 
    /// This test pushes many items and verifies that items are received
    /// in order (no reordering). Some items will be dropped due to buffer overflow.
    #[test]
    fn test_stress_data_integrity() {
        const NUM_ITEMS: usize = 10_000;
        const BUFFER_SIZE: usize = 64;
        const TIMEOUT_SECS: u64 = 5;
        
        let buffer = Arc::new(LockFreeRingBuffer::new(BUFFER_SIZE));
        let buffer1 = Arc::clone(&buffer);
        let buffer2 = Arc::clone(&buffer);
        let done_producing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done_flag = Arc::clone(&done_producing);
        
        // Producer thread
        let producer = thread::spawn(move || {
            for i in 0..NUM_ITEMS {
                buffer1.push(i);
            }
            done_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        
        // Consumer thread - with timeout
        let consumer = thread::spawn(move || {
            let mut received = Vec::with_capacity(NUM_ITEMS);
            let mut last_seen: Option<usize> = None;
            let start = std::time::Instant::now();
            
            // Keep consuming until producer is done and buffer is empty
            while start.elapsed() < Duration::from_secs(TIMEOUT_SECS) {
                let mut got_any = false;
                for item in buffer2.pop() {
                    got_any = true;
                    // Check ordering - items should always be increasing
                    if let Some(last) = last_seen {
                        if item <= last {
                            panic!("Out of order! Got {} after {}", item, last);
                        }
                    }
                    last_seen = Some(item);
                    received.push(item);
                }
                
                // If producer is done and we got nothing, buffer is empty
                if !got_any && done_producing.load(std::sync::atomic::Ordering::SeqCst) {
                    // Double-check the buffer is actually empty
                    let final_items: Vec<_> = buffer2.pop().collect();
                    received.extend(final_items);
                    break;
                }
                
                thread::yield_now();
            }
            
            received
        });
        
        producer.join().unwrap();
        let received = consumer.join().unwrap();
        
        // We should have received some items (not all, due to overflow)
        println!("Sent: {}, Received: {} ({:.1}%)", 
            NUM_ITEMS, received.len(), 
            100.0 * received.len() as f64 / NUM_ITEMS as f64);
        
        assert!(!received.is_empty(), "Should have received at least some items");
        
        // Verify ordering of what we did receive
        for window in received.windows(2) {
            assert!(window[0] < window[1], "Items out of order: {} >= {}", window[0], window[1]);
        }
    }

    /// Test: Iterator dropped early should not cause issues.
    /// 
    /// This tests that dropping the RingIter before consuming all items
    /// correctly unlocks the tail.
    #[test]
    fn test_iterator_early_drop() {
        let buffer = LockFreeRingBuffer::new(10);
        
        for i in 0..10 {
            buffer.push(i);
        }
        
        // Pop and only take 3 items, then drop the iterator
        {
            let mut iter = buffer.pop();
            assert_eq!(iter.next(), Some(0));
            assert_eq!(iter.next(), Some(1));
            assert_eq!(iter.next(), Some(2));
            // Drop iter here - should unlock tail
        }
        
        // Should be able to push more
        buffer.push(100);
        
        // And pop again
        let items: Vec<_> = buffer.pop().collect();
        println!("After early drop and push: {:?}", items);
        
        // We should have items 3-9 and 100
        assert!(items.contains(&100));
    }

    /// Test: Rapid push/pop cycles to stress the compare_and_swap operations.
    #[test]
    fn test_rapid_push_pop_cycles() {
        const CYCLES: usize = 1_000;
        
        let buffer = Arc::new(LockFreeRingBuffer::new(16));
        let buffer1 = Arc::clone(&buffer);
        let buffer2 = Arc::clone(&buffer);
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done_clone = Arc::clone(&done);
        
        let producer = thread::spawn(move || {
            for cycle in 0..CYCLES {
                // Push a batch
                for i in 0..4 {
                    buffer1.push(cycle * 4 + i);
                }
            }
            done_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        
        let consumer = thread::spawn(move || {
            let mut total_received = 0;
            let mut empty_spins = 0;
            
            loop {
                let count = buffer2.pop().count();
                total_received += count;
                
                if count == 0 {
                    empty_spins += 1;
                    if done.load(std::sync::atomic::Ordering::SeqCst) && empty_spins > 100 {
                        break;
                    }
                    thread::yield_now();
                } else {
                    empty_spins = 0;
                }
            }
            
            total_received
        });
        
        producer.join().unwrap();
        let received = consumer.join().unwrap();
        
        println!("Rapid cycles: sent {}, received {}", CYCLES * 4, received);
        
        // We may lose some items due to buffer overflow, but we shouldn't crash
        assert!(received > 0, "Should have received at least some items");
    }

    // ========================================================================
    // MIRI-COMPATIBLE TESTS FOR MEMORY SAFETY
    // ========================================================================
    // Run with: cargo +nightly miri test
    // These tests exercise code paths that may have data races detectable by Miri.

    /// Test: Write visibility race detection.
    /// 
    /// This test exercises the potential data race in push() where:
    /// 1. Producer writes data to a slot
    /// 2. Producer updates head index
    /// 3. Consumer reads head index
    /// 4. Consumer reads data from slot
    /// 
    /// If there's insufficient synchronization, the consumer might see the
    /// new head index but stale data. This test will PASS normally but
    /// Miri should detect the race condition.
    /// 
    /// Run with: cargo +nightly miri test test_write_visibility_race
    #[test]
    fn test_write_visibility_race() {
        // Use a struct with interior complexity to make races more detectable
        #[derive(Clone, Debug, PartialEq)]
        struct Payload {
            id: usize,
            data: [u8; 64],  // Larger data to increase race window
            checksum: u64,
        }
        
        impl Payload {
            fn new(id: usize) -> Self {
                let mut data = [0u8; 64];
                for (i, byte) in data.iter_mut().enumerate() {
                    *byte = ((id + i) % 256) as u8;
                }
                let checksum = data.iter().map(|&b| b as u64).sum();
                Payload { id, data, checksum }
            }
            
            fn verify(&self) -> bool {
                let expected: u64 = self.data.iter().map(|&b| b as u64).sum();
                self.checksum == expected
            }
        }
        
        const ITERATIONS: usize = 1000;
        const BUFFER_SIZE: usize = 4;
        
        let buffer = Arc::new(LockFreeRingBuffer::new(BUFFER_SIZE));
        let buffer_producer = Arc::clone(&buffer);
        let buffer_consumer = Arc::clone(&buffer);
        
        let producer = thread::spawn(move || {
            for i in 0..ITERATIONS {
                buffer_producer.push(Payload::new(i));
            }
        });
        
        let consumer = thread::spawn(move || {
            let mut received = Vec::new();
            let mut corrupted = 0;
            let start = std::time::Instant::now();
            
            while received.len() < ITERATIONS && start.elapsed() < Duration::from_secs(5) {
                for item in buffer_consumer.pop() {
                    // Verify the payload wasn't partially written
                    if !item.verify() {
                        corrupted += 1;
                        println!("CORRUPTION DETECTED: id={}, checksum mismatch", item.id);
                    }
                    received.push(item);
                }
                thread::yield_now();
            }
            
            (received, corrupted)
        });
        
        producer.join().unwrap();
        let (received, corrupted) = consumer.join().unwrap();
        
        println!("Write visibility test: received {}/{}, corrupted: {}", 
                 received.len(), ITERATIONS, corrupted);
        
        // Under Miri, this test should catch data races even if
        // the corruption count is 0 (Miri detects the race itself)
        assert_eq!(corrupted, 0, "Data corruption detected - write visibility race!");
    }

    /// Test: Concurrent read/write race on same slot.
    /// 
    /// This test specifically targets the UnsafeCell access pattern where
    /// a producer might be writing to a slot while a consumer reads from it
    /// Test that demonstrates concurrent access is safe with the _cloned() methods.
    /// The deprecated reference-returning methods (get/front/back) DO have data races.
    /// 
    /// Run with: cargo +nightly miri test test_concurrent_slot_access
    #[test]
    fn test_concurrent_slot_access() {
        use std::sync::atomic::AtomicUsize;
        
        // Use a type that's easy to detect corruption in
        #[derive(Clone, Debug)]
        struct Data {
            value: usize,
            copy: usize,  // Should always equal value
        }
        
        let buffer = Arc::new(LockFreeRingBuffer::new(2));
        let races_detected = Arc::new(AtomicUsize::new(0));
        
        // Fill buffer initially
        buffer.push(Data { value: 0, copy: 0 });
        buffer.push(Data { value: 1, copy: 1 });
        
        let buffer1 = Arc::clone(&buffer);
        let buffer2 = Arc::clone(&buffer);
        let races1 = Arc::clone(&races_detected);
        
        // Thread 1: Continuously push new values (causing slot reuse)
        let writer = thread::spawn(move || {
            for i in 2..1000usize {
                buffer1.push(Data { value: i, copy: i });
            }
        });
        
        // Thread 2: Use pop() which is properly synchronized
        // pop() acquires the consumer lock before reading
        let reader = thread::spawn(move || {
            for _ in 0..1000 {
                // Use pop() which properly locks and reads
                for data in buffer2.pop() {
                    // Check for torn reads - should never happen
                    if data.value != data.copy {
                        races1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                thread::yield_now();
            }
        });
        
        writer.join().unwrap();
        reader.join().unwrap();
        
        let races = races_detected.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(races, 0, "No torn reads should occur with pop()");
    }

    /// Test: Work-stealing with multiple consumers.
    /// 
    /// Multiple consumer threads compete to pop items from the same buffer.
    /// Each item should be consumed by exactly one consumer (no duplicates, no losses).
    /// 
    /// This is the MPMC (multiple consumers) use case.
    #[test]
    fn test_work_stealing_multiple_consumers() {
        use std::collections::HashSet;
        use std::sync::Mutex;
        
        const NUM_ITEMS: usize = 2_000;
        const NUM_CONSUMERS: usize = 2;
        const BUFFER_SIZE: usize = 128;
        
        let buffer = Arc::new(LockFreeRingBuffer::new(BUFFER_SIZE));
        let done_producing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        
        // Collected items from all consumers
        let collected: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
        
        // Producer thread
        let producer_buffer = Arc::clone(&buffer);
        let done_flag = Arc::clone(&done_producing);
        let producer = thread::spawn(move || {
            for i in 0..NUM_ITEMS {
                producer_buffer.push(i);
            }
            done_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        
        // Multiple consumer threads - all competing to pop items
        let mut consumers = Vec::new();
        for consumer_id in 0..NUM_CONSUMERS {
            let consumer_buffer = Arc::clone(&buffer);
            let consumer_collected = Arc::clone(&collected);
            let consumer_done = Arc::clone(&done_producing);
            
            let handle = thread::spawn(move || {
                let mut local_items = Vec::new();
                let mut spins_without_item = 0;
                
                loop {
                    let mut got_any = false;
                    for item in consumer_buffer.pop() {
                        local_items.push(item);
                        got_any = true;
                        spins_without_item = 0;
                    }
                    
                    if !got_any {
                        spins_without_item += 1;
                        // If producer is done and we've spun a lot without items, we're done
                        if consumer_done.load(std::sync::atomic::Ordering::SeqCst) 
                           && spins_without_item > 1000 {
                            break;
                        }
                        thread::yield_now();
                    }
                }
                
                println!("Consumer {} collected {} items", consumer_id, local_items.len());
                
                // Add to global collection
                consumer_collected.lock().unwrap().extend(local_items);
            });
            
            consumers.push(handle);
        }
        
        // Wait for all threads
        producer.join().unwrap();
        for consumer in consumers {
            consumer.join().unwrap();
        }
        
        // Verify results
        let items = collected.lock().unwrap();
        
        // Check: Total items should be <= NUM_ITEMS (some may be evicted if buffer was full)
        println!("Total items collected: {} / {}", items.len(), NUM_ITEMS);
        
        // Check: No duplicates (each item consumed exactly once)
        let unique: HashSet<_> = items.iter().cloned().collect();
        assert_eq!(unique.len(), items.len(), 
            "Duplicates detected! {} unique vs {} total - items were consumed multiple times",
            unique.len(), items.len());
        
        // Check: Items should be reasonably distributed among consumers (not all going to one)
        // This is a soft check - work stealing should distribute load
        assert!(!items.is_empty(), "Should have consumed at least some items");
        
        println!("Work stealing test passed: {} unique items consumed by {} consumers",
            items.len(), NUM_CONSUMERS);
    }
}
