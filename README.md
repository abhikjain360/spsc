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

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
gil = "0.3"
```

## Usage

The producer and consumer can run on different threads, but there can only be one producer and only one consumer. The producer (or consumer) can be moved between threads, but cannot be shared between threads. The queue has a fixed capacity that must be specified when creating the channel.

Consumer blocks until there is a value on the queue, or use `Receiver::try_recv` for non-blocking version. Similarly, producer blocks until there is a free slot on the queue, or use `Sender::try_send` for non-blocking version.

### Basic Example (Synchronous)

```rust
use std::thread;
use std::num::NonZeroUsize;
use gil::channel;

const COUNT: usize = 100_000;

let (mut tx, mut rx) = channel(NonZeroUsize::new(COUNT).unwrap());

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

To use async features, enable the `async` feature in your `Cargo.toml`.

```toml
[dependencies]
gil = { version = "0.3", features = ["async"] }
```

```rust
use gil::channel;
use std::num::NonZeroUsize;

# #[cfg(feature = "async")]
# async fn example() {
const COUNT: usize = 100_000;

let (mut tx, mut rx) = channel(NonZeroUsize::new(COUNT).unwrap());

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
# }
```

### Non-blocking Operations

```rust
use gil::channel;
use std::num::NonZeroUsize;

let (mut tx, mut rx) = channel(NonZeroUsize::new(10).unwrap());

// Try to send without blocking
match tx.try_send(42) {
    true => println!("Sent successfully"),
    false => println!("Queue full"),
}

// Try to receive without blocking
match rx.try_recv() {
    Some(val) => println!("Received: {}", val),
    None => println!("Queue empty"),
}
```

### Batch Operations (Zero-copy)

For maximum performance, you can directly access the internal buffer. This allows you to write or read multiple items at once, bypassing the per-item synchronization overhead.

```rust
use gil::channel;
use std::ptr;
use std::num::NonZeroUsize;

let (mut tx, mut rx) = channel(NonZeroUsize::new(128).unwrap());

// Zero-copy write
let data = [1usize, 2, 3, 4, 5];
let slice = tx.write_buffer();
let count = data.len().min(slice.len());

unsafe {
    ptr::copy_nonoverlapping(
        data.as_ptr(),
        slice.as_mut_ptr(),
        count
    );
    // Commit the written items to make them visible to the consumer
    tx.commit(count);
}

// Zero-copy read
let len = {
    let slice = rx.read_buffer();
    for &value in slice {
        println!("Value: {}", value);
    }
    slice.len()
};
// Advance the consumer head to mark items as processed
unsafe { rx.advance(len); }
```

## Performance

The queue achieves high throughput through several optimizations:

- **Cache-line alignment**: Head and tail pointers are on separate cache lines to prevent false sharing
- **Local caching**: Each side caches the other side's position to reduce atomic operations
- **Batch operations**: Amortize atomic operation costs across multiple items
- **Zero-copy API**: Direct buffer access eliminates memory copies

## Type Constraints

The current implementation is optimized for `usize`.

## Safety

The code has been verified using:
- [loom](https://github.com/tokio-rs/loom) - Concurrency testing
- [miri](https://github.com/rust-lang/miri) - Undefined behavior detection

## License

MIT License - see [LICENSE](https://github.com/abhikjain360/spsc/blob/main/LICENSE) file for details.
