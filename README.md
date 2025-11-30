# gil

**Get In Line** - A fast single-producer single-consumer (SPSC) queue with sync and async support.

Inspired by [Facebook's folly's ProducerConsumerQueue](https://github.com/facebook/folly/blob/main/folly/ProducerConsumerQueue.h).

> ⚠️ WIP: things **WILL** change a lot without warnings even in minor updates until v1, use at your own risk.

## Features

- **Lock-free**: Uses atomic operations for synchronization
- **Single-producer, single-consumer**: Optimized for this specific use case
- **Thread-safe**: Producer and consumer can run on different threads
- **Blocking and non-blocking operations**: Both sync and async APIs
- **Batch operations**: Send and receive multiple items efficiently
- **Zero-copy operations**: Direct buffer access for maximum performance
- **High performance** (probably): Competitive with Facebook's folly implementation

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
gil = "0.2"
```

## Usage

The producer and consumer can run on different threads, but there can only be one producer and only one consumer. The producer (or consumer) can be moved between threads, but cannot be shared between threads. The queue has a fixed capacity that must be specified when creating the channel.

Consumer blocks until there is a value on the queue, or use `Receiver::try_recv` for non-blocking version. Similarly, producer blocks until there is a free slot on the queue, or use `Sender::try_send` for non-blocking version.

### Basic Example (Synchronous)

```rust
use std::thread;
use gil::channel;

const COUNT: u128 = 100_000;

let (mut tx, mut rx) = channel(COUNT as usize);

let handle = thread::spawn(move || {
    for i in 0..COUNT {
        // Block until send completes
        tx.send(i);
    }
});

for i in 0..COUNT {
    // Block until recv completes
    let value = rx.recv();
    assert_eq!(value, i);
}

handle.join().unwrap();
```

### Async Example

```rust
use gil::channel;
const COUNT: u128 = 100_000;

let (mut tx, mut rx) = channel(COUNT as usize);

let handle = tokio::spawn(async move {
    for i in 0..COUNT {
        // Await until send completes
        tx.send_async(i).await;
    }
});

for i in 0..COUNT {
    // Await until recv completes
    let value = rx.recv_async().await;
    assert_eq!(value, i);
}

handle.await.unwrap();
```

### Non-blocking Operations

```rust
use gil::channel;

let (mut tx, mut rx) = channel(10);

// Try to send without blocking
match tx.try_send(42) {
    Ok(()) => println!("Sent successfully"),
    Err(val) => println!("Queue full, got value back: {}", val),
}

// Try to receive without blocking
match rx.try_recv() {
    Some(val) => println!("Received: {}", val),
    None => println!("Queue empty"),
}
```

### Batch Operations

Batch operations are more efficient than individual sends/receives because they amortize the cost of atomic operations:

```rust
use std::thread;
use std::collections::VecDeque;
use gil::channel;

let (mut tx, mut rx) = channel(256);

thread::spawn(move || {
    let values: Vec<u128> = (0..1000).collect();
    // Send all values, blocking as necessary
    tx.batch_send_all(values.into_iter());
});

let mut buffer = VecDeque::new();
let mut received = 0;

while received < 1000 {
    // Receive up to 128 values at once
    let count = rx.batch_recv(&mut buffer, 128);
    received += count;
    
    for value in buffer.drain(..) {
        println!("Received: {}", value);
    }
}
```

### Zero-Copy Operations

For maximum performance, you can directly access the internal buffer:

```rust
use gil::channel;
use std::ptr;

let (mut tx, mut rx) = channel(128);

// Zero-copy write
let data = [1u128, 2, 3, 4, 5];
let slice = tx.get_write_slice();
let count = data.len().min(slice.len());

unsafe {
    ptr::copy_nonoverlapping(
        data.as_ptr(),
        slice.as_mut_ptr(),
        count
    );
}
tx.commit(count);

// Zero-copy read
let len = {
    let slice = rx.get_read_slice();
    for &value in slice {
        println!("Value: {}", value);
    }
    slice.len()
};
rx.advance(len);
```

## Performance

The queue achieves high throughput through several optimizations:

- **Cache-line alignment**: Head and tail pointers are on separate cache lines to prevent false sharing
- **Local caching**: Each side caches the other side's position to reduce atomic operations
- **Batch operations**: Amortize atomic operation costs across multiple items
- **Zero-copy API**: Direct buffer access eliminates memory copies

On Apple M3, the queue can achieve ~50GB/s throughput and with batching and zero-copy operations. Latency is around 80ns, but depends on which cores the producer and consumer are running on.

## Type Constraints

The queue works with:
- `u128` on aarch64 (ARM64) architectures
- `u64` on x86_64 architectures

This allows the queue to store values that fit within these sizes directly. For larger types, consider using indices or pointers with an external storage mechanism.

## Safety

The code has been verified using:
- [loom](https://github.com/tokio-rs/loom) - Concurrency testing
- [miri](https://github.com/rust-lang/miri) - Undefined behavior detection

## Future Improvements

- [ ] More comprehensive benchmarks
- [ ] Support for generic types (not just `u64`/`u128`) using custom arena allocators
- [ ] Optimize for x86
- [ ] Try benching with Intel x86's `cldemote` instruction
- [ ] Run and benchmark on NVIDIA Grace (or any NVLink-C2C), just for fun and to see how fast this can really go. In theory NVIDIA Grace should go upto 900GB/s.

## Contributing

Contributions are welcome! Please feel free to submit issues or pull requests.

## License

MIT License - see [LICENSE](https://github.com/abhikjain360/spsc/blob/main/LICENSE) file for details.
