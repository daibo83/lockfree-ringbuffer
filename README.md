# 🪤 lockfree-ringbuffer
"One Ring to buffer them all..."
*My preciousss... a lock-free ring buffer, yesss!*

A high-performance, lock-free SPSC (Single Producer, Single Consumer) ring buffer for Rust. We wants it, we needs it for our real-time applications, precious!

## ✨ Features, Precious Features!

- **Lock-free pushes** - Producer never blocks, no nasty mutexes, precious!
- **Auto-eviction** - When full, oldest items gets evicted. Gone like the hobbitses!
- **Single consumer** - One reader at a time, keeps it simple and safeses
- **Zero-copy iteration** - Pop returns an iterator, no copying until you wants it
- **Miri-verified** - No undefined behavior, we checked, we did! Safe from tricksy memory bugs

## 🚀 Performance, Precious Numbers!

We benchmarked against other ringses, precious. Look how fast we goes!

### SPSC Throughput (10,000 items)

| Buffer Size | lockfree-ringbuffer | rtrb | ringbuf | crossbeam |
|-------------|---------------------|------|---------|-----------|
| **64** | 108 Melem/s | 158 Melem/s | 100 Melem/s | 22 Melem/s |
| **1024** | 114 Melem/s | 175 Melem/s | 108 Melem/s | 61 Melem/s |
| **16384** | 143 Melem/s | 234 Melem/s | 109 Melem/s | 67 Melem/s |

*Faster than ringbuf, yesss! And crossbeam? Preciousss, we crushes it!*

*What's that, rtrb is faster? Bah! Tricksy library doesn't have auto-eviction, does it precious? It just BLOCKS when full! We never blocks, we is always ready! Besides, rtrb has no soul, no character... just cold, empty speed. We has FEATURES, precious!*

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

### Producer-Consumer, Like Gollum and Sméagol!

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

// Consumer thread - takes the preciousss
let consumer_ring = Arc::clone(&ring);
let consumer = thread::spawn(move || {
    let mut received = Vec::new();
    loop {
        for item in consumer_ring.pop() {
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

- **Single consumer only** - Multiple consumers will panic! One precious at a time!
- **Items may be lost** - If buffer is full and no consumer, items get evicted. Gone!
- **No blocking** - Producer never waits, just evicts. Consumer spins if needed.

## 📜 License

MIT License - Free like the wind on the mountains, precious!

---

*"One Ring to buffer them all, One Ring to find them,  
One Ring to bring them all, and in the lockfree bind them."*

🪤✨
