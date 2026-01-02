# 🪤 lockfree-ringbuffer
"One Ring to buffer them all..."
*My preciousss... a lock-free ring buffer, yesss!*

A high-performance, lock-free ring buffer for Rust with **MPMC** (Multiple Producer, Multiple Consumer) support. We wants it, we needs it for our real-time applications, precious!

## ✨ Features, Precious Features!

- **Lock-free pushes** - Producer never blocks, no nasty mutexes, precious!
- **Auto-eviction** - When full, oldest items gets evicted. Gone like the hobbitses!
- **MPMC support** - Multiple consumers can compete for items with CAS-based claiming!
- **SPSC fast path** - Use `pop_exclusive()` for maximum single-consumer performance
- **Zero-copy iteration** - Pop returns an iterator, no copying until you wants it
- **Miri-verified** - No undefined behavior, we checked, we did! Safe from tricksy memory bugs

## 🚀 Performance, Precious Numbers!

We benchmarked against other ringses, precious. Look how fast we goes!

### SPSC Throughput (with `pop_exclusive()`)

| Buffer Size | lockfree-ringbuffer | rtrb | ringbuf | crossbeam |
|-------------|---------------------|------|---------|-----------| 
| **64** | 75 Melem/s | 172 Melem/s | 108 Melem/s | 23 Melem/s |
| **1024** | 80 Melem/s | 194 Melem/s | 117 Melem/s | 69 Melem/s |
| **16384** | 110 Melem/s | 258 Melem/s | 117 Melem/s | 71 Melem/s |

### MPMC Throughput (2 consumers competing)

| Implementation | Throughput |
|----------------|------------|
| **lockfree-ringbuffer** | **187 Melem/s** |
| crossbeam-channel | 187 Melem/s |

*We matches crossbeam-channel for MPMC, precious! And we has AUTO-EVICTION, something crossbeam doesn't have, yesss!*

*What's that, rtrb is faster for SPSC? Bah! Tricksy library doesn't have MPMC or auto-eviction, does it precious? It just BLOCKS when full! We never blocks, we is always ready!*

## 📦 Installation

Add to your `Cargo.toml`, precious:

```toml
[dependencies]
lockfree-ringbuffer = "0.2"
```

## 🎯 Usage Examples

### Basic Push and Pop, Simple and Nice!

```rust
use lockfree_ringbuffer::threadsafe::LockFreeRingBuffer;

// Creates a precious ring with capacity for 8 items
let ring = LockFreeRingBuffer::new(8);

// Push some items into it, yesss
ring.push(1);
ring.push(2);
ring.push(3);

// Pop them out! Returns an iterator, precious
for item in ring.pop() {
    println!("Got: {}", item);
}
```

### SPSC - Single Producer, Single Consumer (Fast Path!)

```rust
use lockfree_ringbuffer::threadsafe::LockFreeRingBuffer;
use std::sync::Arc;
use std::thread;

let ring = Arc::new(LockFreeRingBuffer::new(64));

// Producer thread - pushes precious data
let producer_ring = Arc::clone(&ring);
let producer = thread::spawn(move || {
    for i in 0..1000 {
        producer_ring.push(i);
    }
});

// Consumer thread - uses EXCLUSIVE iterator for max speed!
let consumer_ring = Arc::clone(&ring);
let consumer = thread::spawn(move || {
    let mut received = Vec::new();
    loop {
        // pop_exclusive() is fastest when you're the only consumer!
        for item in consumer_ring.pop_exclusive() {
            received.push(item);
        }
        if received.len() >= 100 {
            break;  // Got enough, precious!
        }
        thread::yield_now();
    }
    received
});

producer.join().unwrap();
let items = consumer.join().unwrap();
println!("Received {} items, yesss!", items.len());
```

### MPMC - Multiple Consumers Competing!

```rust
use lockfree_ringbuffer::threadsafe::LockFreeRingBuffer;
use std::sync::Arc;
use std::thread;

let ring = Arc::new(LockFreeRingBuffer::new(128));

// Producer
let producer_ring = Arc::clone(&ring);
thread::spawn(move || {
    for i in 0..10000 {
        producer_ring.push(i);
    }
});

// Multiple consumers! They compete for items, precious!
let mut handles = vec![];
for id in 0..4 {
    let consumer_ring = Arc::clone(&ring);
    handles.push(thread::spawn(move || {
        let mut count = 0;
        loop {
            // try_pop() is safe for multiple concurrent consumers!
            if let Some(item) = consumer_ring.try_pop() {
                count += 1;
            } else {
                // Buffer empty, maybe check if producer is done
                break;
            }
        }
        println!("Consumer {} got {} items", id, count);
        count
    }));
}

for h in handles {
    h.join().unwrap();
}
// Items distributed among consumers - no duplicates!
```

### Peek Without Stealing, Just Looking!

```rust
use lockfree_ringbuffer::threadsafe::LockFreeRingBuffer;

let ring = LockFreeRingBuffer::new(4);
ring.push("first");
ring.push("second");
ring.push("third");

// Peek at items without removing them, sneaky!
assert_eq!(ring.front_cloned(), Some("first"));   // Oldest
assert_eq!(ring.back_cloned(), Some("third"));    // Newest  
assert_eq!(ring.get_cloned(1), Some("second"));   // By index

// Still there, precious! Not stolen!
assert_eq!(ring.len(), 3);
```

### Auto-Eviction, Goodbye Old Ones!

```rust
use lockfree_ringbuffer::threadsafe::LockFreeRingBuffer;

let ring = LockFreeRingBuffer::new(3);  // Only fits 3!

ring.push(1);
ring.push(2);
ring.push(3);
// Ring is full: [1, 2, 3]

ring.push(4);  // Evicts 1! Gone like Isildur!
// Ring now: [2, 3, 4]

ring.push(5);  // Evicts 2! 
// Ring now: [3, 4, 5]

assert_eq!(ring.front_cloned(), Some(3));  // Oldest survivor
```

## 🔧 API Reference

| Method | Use Case | Performance |
|--------|----------|-------------|
| `push(item)` | Add item to buffer | Lock-free, always succeeds |
| `try_pop()` | Pop single item (MPMC-safe) | CAS-based, ~187 Melem/s |
| `pop()` | Iterator using try_pop (MPMC-safe) | CAS-based |
| `pop_exclusive()` | Fast iterator (SPSC only) | Lock-based, ~80-110 Melem/s |
| `front_cloned()` | Peek oldest without removing | Clones the item |
| `back_cloned()` | Peek newest without removing | Clones the item |
| `len()` | Current number of items | Atomic load |

## 🧪 Testing

```bash
# Run tests, make sure it works, precious!
cargo test

# Run with Miri for memory safety, very importantses!
cargo +nightly miri test

# Benchmarks, see how fast we goes!
cargo bench
```

## ⚠️ Limitations

- **Items may be lost** - If buffer is full and no consumer, items get evicted. Gone!
- **No blocking** - Producer never waits, just evicts. Consumer spins if needed.
- **`pop_exclusive()` panics with multiple consumers** - Use `try_pop()` for MPMC!

## 📜 License

MIT License - Free like the wind on the mountains, precious!

---

*"One Ring to buffer them all, One Ring to find them,  
One Ring to bring them all, and in the lockfree bind them."*

🪤✨
