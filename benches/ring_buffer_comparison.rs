//! Benchmark comparing lockfree-ringbuffer against other concurrent ring buffer implementations.
//!
//! Run with: cargo bench
//!
//! Libraries compared:
//! - lockfree-ringbuffer (this crate)
//! - rtrb: Real-time ring buffer (wait-free SPSC)
//! - ringbuf: Popular ring buffer implementation  
//! - crossbeam-channel: High-performance bounded channel

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use ringbuf::traits::{Consumer, Producer, Split};
use std::sync::Arc;
use std::thread;

// Buffer sizes to test
const SMALL_BUFFER: usize = 64;
const MEDIUM_BUFFER: usize = 1024;
const LARGE_BUFFER: usize = 16384;

// Number of items to push/pop in throughput tests
const ITEMS_COUNT: usize = 10_000;

// ============================================================================
// Single-threaded benchmarks (push/pop latency)
// ============================================================================

fn bench_single_thread_push(c: &mut Criterion) {
    let mut group = c.benchmark_group("single_thread_push");
    
    for size in [SMALL_BUFFER, MEDIUM_BUFFER, LARGE_BUFFER] {
        group.throughput(Throughput::Elements(size as u64));
        
        // lockfree-ringbuffer
        group.bench_with_input(
            BenchmarkId::new("lockfree-ringbuffer", size),
            &size,
            |b, &size| {
                let buffer = lockfree_ringbuffer::threadsafe::LockFreeRingBuffer::new(size);
                b.iter(|| {
                    for i in 0..size {
                        buffer.push(black_box(i));
                    }
                    // Drain to reset
                    let _: Vec<_> = buffer.pop().collect();
                });
            },
        );
        
        // rtrb
        group.bench_with_input(
            BenchmarkId::new("rtrb", size),
            &size,
            |b, &size| {
                let (mut producer, mut consumer) = rtrb::RingBuffer::new(size);
                b.iter(|| {
                    for i in 0..size {
                        let _ = producer.push(black_box(i));
                    }
                    // Drain to reset
                    while consumer.pop().is_ok() {}
                });
            },
        );
        
        // ringbuf
        group.bench_with_input(
            BenchmarkId::new("ringbuf", size),
            &size,
            |b, &size| {
                let rb = ringbuf::HeapRb::<usize>::new(size);
                let (mut producer, mut consumer) = rb.split();
                b.iter(|| {
                    for i in 0..size {
                        let _ = producer.try_push(black_box(i));
                    }
                    // Drain to reset
                    while consumer.try_pop().is_some() {}
                });
            },
        );
        
        // crossbeam-channel
        group.bench_with_input(
            BenchmarkId::new("crossbeam-channel", size),
            &size,
            |b, &size| {
                let (sender, receiver) = crossbeam_channel::bounded(size);
                b.iter(|| {
                    for i in 0..size {
                        let _ = sender.try_send(black_box(i));
                    }
                    // Drain to reset
                    while receiver.try_recv().is_ok() {}
                });
            },
        );
    }
    
    group.finish();
}

// ============================================================================
// SPSC throughput benchmark (producer and consumer in separate threads)
// ============================================================================

fn bench_spsc_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("spsc_throughput");
    group.throughput(Throughput::Elements(ITEMS_COUNT as u64));
    
    for size in [SMALL_BUFFER, MEDIUM_BUFFER, LARGE_BUFFER] {
        // lockfree-ringbuffer
        // Note: This buffer auto-evicts oldest items when full, so we use
        // a done signal instead of waiting for all items
        group.bench_with_input(
            BenchmarkId::new("lockfree-ringbuffer", size),
            &size,
            |b, &size| {
                use std::sync::atomic::{AtomicBool, Ordering};
                
                b.iter(|| {
                    let buffer = Arc::new(
                        lockfree_ringbuffer::threadsafe::LockFreeRingBuffer::new(size)
                    );
                    let done = Arc::new(AtomicBool::new(false));
                    
                    let buffer_producer = Arc::clone(&buffer);
                    let done_signal = Arc::clone(&done);
                    
                    let buffer_consumer = Arc::clone(&buffer);
                    let done_check = Arc::clone(&done);
                    
                    let producer = thread::spawn(move || {
                        for i in 0..ITEMS_COUNT {
                            buffer_producer.push(black_box(i));
                        }
                        done_signal.store(true, Ordering::Release);
                    });
                    
                    let consumer = thread::spawn(move || {
                        let mut count = 0;
                        loop {
                            // Use exclusive iterator for SPSC performance
                            count += buffer_consumer.pop_exclusive().count();
                            if done_check.load(Ordering::Acquire) {
                                // Drain any remaining
                                count += buffer_consumer.pop_exclusive().count();
                                break;
                            }
                            std::hint::spin_loop();
                        }
                        count
                    });
                    
                    producer.join().unwrap();
                    let received = consumer.join().unwrap();
                    black_box(received);
                });
            },
        );
        
        // rtrb
        group.bench_with_input(
            BenchmarkId::new("rtrb", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let (mut producer, mut consumer) = rtrb::RingBuffer::new(size);
                    
                    let prod_handle = thread::spawn(move || {
                        for i in 0..ITEMS_COUNT {
                            loop {
                                match producer.push(black_box(i)) {
                                    Ok(()) => break,
                                    Err(_) => std::hint::spin_loop(),
                                }
                            }
                        }
                    });
                    
                    let cons_handle = thread::spawn(move || {
                        let mut count = 0;
                        while count < ITEMS_COUNT {
                            if consumer.pop().is_ok() {
                                count += 1;
                            } else {
                                std::hint::spin_loop();
                            }
                        }
                        count
                    });
                    
                    prod_handle.join().unwrap();
                    let received = cons_handle.join().unwrap();
                    black_box(received);
                });
            },
        );
        
        // ringbuf
        group.bench_with_input(
            BenchmarkId::new("ringbuf", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let rb = ringbuf::HeapRb::<usize>::new(size);
                    let (mut producer, mut consumer) = rb.split();
                    
                    let prod_handle = thread::spawn(move || {
                        for i in 0..ITEMS_COUNT {
                            loop {
                                match producer.try_push(black_box(i)) {
                                    Ok(()) => break,
                                    Err(_) => std::hint::spin_loop(),
                                }
                            }
                        }
                    });
                    
                    let cons_handle = thread::spawn(move || {
                        let mut count = 0;
                        while count < ITEMS_COUNT {
                            if consumer.try_pop().is_some() {
                                count += 1;
                            } else {
                                std::hint::spin_loop();
                            }
                        }
                        count
                    });
                    
                    prod_handle.join().unwrap();
                    let received = cons_handle.join().unwrap();
                    black_box(received);
                });
            },
        );
        
        // crossbeam-channel
        group.bench_with_input(
            BenchmarkId::new("crossbeam-channel", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let (sender, receiver) = crossbeam_channel::bounded(size);
                    
                    let prod_handle = thread::spawn(move || {
                        for i in 0..ITEMS_COUNT {
                            loop {
                                match sender.try_send(black_box(i)) {
                                    Ok(()) => break,
                                    Err(crossbeam_channel::TrySendError::Full(_)) => {
                                        std::hint::spin_loop();
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                    });
                    
                    let cons_handle = thread::spawn(move || {
                        let mut count = 0;
                        while count < ITEMS_COUNT {
                            if receiver.try_recv().is_ok() {
                                count += 1;
                            } else {
                                std::hint::spin_loop();
                            }
                        }
                        count
                    });
                    
                    prod_handle.join().unwrap();
                    let received = cons_handle.join().unwrap();
                    black_box(received);
                });
            },
        );
    }
    
    group.finish();
}

// ============================================================================
// Burst write benchmark (simulates real-time audio/video scenarios)
// ============================================================================

fn bench_burst_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("burst_write");
    
    const BURST_SIZE: usize = 256;
    const BURSTS: usize = 100;
    
    group.throughput(Throughput::Elements((BURST_SIZE * BURSTS) as u64));
    
    // lockfree-ringbuffer (with overflow - drops oldest)
    group.bench_function("lockfree-ringbuffer", |b| {
        let buffer = lockfree_ringbuffer::threadsafe::LockFreeRingBuffer::new(MEDIUM_BUFFER);
        b.iter(|| {
            for _burst in 0..BURSTS {
                for i in 0..BURST_SIZE {
                    buffer.push(black_box(i));
                }
            }
            // Drain
            let _: Vec<_> = buffer.pop().collect();
        });
    });
    
    // rtrb (blocks on full - different behavior)
    group.bench_function("rtrb", |b| {
        let (mut producer, mut consumer) = rtrb::RingBuffer::new(MEDIUM_BUFFER);
        b.iter(|| {
            for _burst in 0..BURSTS {
                for i in 0..BURST_SIZE {
                    // Skip if full (simulating drop behavior)
                    let _ = producer.push(black_box(i));
                }
                // Drain between bursts to prevent blocking
                while consumer.pop().is_ok() {}
            }
        });
    });
    
    // ringbuf
    group.bench_function("ringbuf", |b| {
        let rb = ringbuf::HeapRb::<usize>::new(MEDIUM_BUFFER);
        let (mut producer, mut consumer) = rb.split();
        b.iter(|| {
            for _burst in 0..BURSTS {
                for i in 0..BURST_SIZE {
                    let _ = producer.try_push(black_box(i));
                }
                while consumer.try_pop().is_some() {}
            }
        });
    });
    
    // crossbeam-channel
    group.bench_function("crossbeam-channel", |b| {
        let (sender, receiver) = crossbeam_channel::bounded(MEDIUM_BUFFER);
        b.iter(|| {
            for _burst in 0..BURSTS {
                for i in 0..BURST_SIZE {
                    let _ = sender.try_send(black_box(i));
                }
                while receiver.try_recv().is_ok() {}
            }
        });
    });
    
    group.finish();
}

// ============================================================================
// Memory access pattern benchmark (cache efficiency)
// ============================================================================

fn bench_memory_access(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_access_pattern");
    
    const OPS: usize = 10_000;
    group.throughput(Throughput::Elements(OPS as u64));
    
    // Test with larger data types to stress memory bandwidth
    #[derive(Clone, Default)]
    struct LargeData {
        #[allow(dead_code)]
        data: [u64; 8],  // 64 bytes
    }
    
    
    // lockfree-ringbuffer
    group.bench_function("lockfree-ringbuffer-64byte", |b| {
        let buffer = lockfree_ringbuffer::threadsafe::LockFreeRingBuffer::new(MEDIUM_BUFFER);
        b.iter(|| {
            for i in 0..OPS {
                buffer.push(LargeData { data: [i as u64; 8] });
            }
            let _: Vec<_> = buffer.pop().collect();
        });
    });
    
    // rtrb
    group.bench_function("rtrb-64byte", |b| {
        let (mut producer, mut consumer) = rtrb::RingBuffer::new(MEDIUM_BUFFER);
        b.iter(|| {
            for i in 0..OPS {
                let _ = producer.push(LargeData { data: [i as u64; 8] });
            }
            while consumer.pop().is_ok() {}
        });
    });
    
    // ringbuf
    group.bench_function("ringbuf-64byte", |b| {
        let rb = ringbuf::HeapRb::<LargeData>::new(MEDIUM_BUFFER);
        let (mut producer, mut consumer) = rb.split();
        b.iter(|| {
            for i in 0..OPS {
                let _ = producer.try_push(LargeData { data: [i as u64; 8] });
            }
            while consumer.try_pop().is_some() {}
        });
    });
    
    // crossbeam-channel
    group.bench_function("crossbeam-channel-64byte", |b| {
        let (sender, receiver) = crossbeam_channel::bounded(MEDIUM_BUFFER);
        b.iter(|| {
            for i in 0..OPS {
                let _ = sender.try_send(LargeData { data: [i as u64; 8] });
            }
            while receiver.try_recv().is_ok() {}
        });
    });
    
    group.finish();
}

// ============================================================================
// MPMC benchmarks (multiple consumers competing for items)  
// ============================================================================

/// Benchmark MPMC throughput: Tests the try_pop() method which supports
/// multiple concurrent consumers. Pre-fills buffer then times drain.
fn bench_mpmc_throughput(c: &mut Criterion) {
    use std::time::Instant;
    
    let mut group = c.benchmark_group("mpmc_throughput");
    group.sample_size(20);  // Fewer samples for threaded benchmarks
    
    const OPS: usize = 5_000;
    const BUFFER_SIZE: usize = 512;
    
    group.throughput(Throughput::Elements(OPS as u64));
    
    // lockfree-ringbuffer - 2 consumers
    group.bench_function("lockfree-ringbuffer/2-consumers", |b| {
        b.iter_custom(|iters| {
            let mut total_time = std::time::Duration::ZERO;
            
            for _ in 0..iters {
                let buffer = Arc::new(
                    lockfree_ringbuffer::threadsafe::LockFreeRingBuffer::new(BUFFER_SIZE)
                );
                
                // Pre-fill buffer
                for i in 0..OPS {
                    buffer.push(i);
                }
                
                let buffer1 = Arc::clone(&buffer);
                let buffer2 = Arc::clone(&buffer);
                
                let start = Instant::now();
                
                // Two consumers race to drain
                let h1 = thread::spawn(move || {
                    let mut count = 0;
                    while let Some(_) = buffer1.try_pop() {
                        count += 1;
                    }
                    count
                });
                
                let h2 = thread::spawn(move || {
                    let mut count = 0;
                    while let Some(_) = buffer2.try_pop() {
                        count += 1;
                    }
                    count
                });
                
                let c1 = h1.join().unwrap();
                let c2 = h2.join().unwrap();
                
                total_time += start.elapsed();
                
                black_box(c1 + c2);
            }
            
            total_time
        });
    });
    
    // crossbeam-channel - 2 consumers  
    group.bench_function("crossbeam-channel/2-consumers", |b| {
        b.iter_custom(|iters| {
            let mut total_time = std::time::Duration::ZERO;
            
            for _ in 0..iters {
                let (sender, receiver) = crossbeam_channel::bounded::<usize>(BUFFER_SIZE);
                
                // Pre-fill
                for i in 0..OPS {
                    let _ = sender.try_send(i);
                }
                drop(sender);  // Close sender so receivers know when done
                
                let r1 = receiver.clone();
                let r2 = receiver.clone();
                
                let start = Instant::now();
                
                let h1 = thread::spawn(move || {
                    let mut count = 0;
                    while r1.try_recv().is_ok() {
                        count += 1;
                    }
                    count
                });
                
                let h2 = thread::spawn(move || {
                    let mut count = 0;
                    while r2.try_recv().is_ok() {
                        count += 1;
                    }
                    count
                });
                
                let c1 = h1.join().unwrap();
                let c2 = h2.join().unwrap();
                
                total_time += start.elapsed();
                
                black_box(c1 + c2);
            }
            
            total_time
        });
    });
    
    group.finish();
}

criterion_group!(
    benches,
    bench_single_thread_push,
    bench_spsc_throughput,
    bench_burst_write,
    bench_memory_access,
    bench_mpmc_throughput,
);

criterion_main!(benches);
