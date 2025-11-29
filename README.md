# gil

**Get In Line** - A fast single-producer single-consumer queue with sync and async support.

Inspired by [Facebook's folly's ProducerConsumerQueue](https://github.com/facebook/folly/blob/main/folly/ProducerConsumerQueue.h).

> ⚠️ WIP: things **WILL** change a lot without warnings even in minor updates until v1, use at your own risk.

## Features

- **Lock-free**: Uses atomic operations for synchronization
- **Single-producer, single-consumer**: Optimized for this specific use case
- **Thread-safe**: Producer and consumer can run on different threads
- **Blocking and non-blocking operations**: Both sync and async APIs
- **Batch operations**: Send and receive multiple items efficiently
- **High performance** (probably): Competitive with Facebook's folly implementation

## Safety

The code was verified using [loom](https://github.com/tokio-rs/loom) and [miri](https://github.com/rust-lang/miri).

## Usage

The producer and consumer can run on different threads, but there can only be one producer and only one consumer. The producer (or consumer) can be moved between threads, but cannot be shared between threads. The queue has a fixed capacity that must be specified when creating the channel.

Consumer blocks until there is a value on the queue, or use `Receiver<T>::try_recv` for non-blocking version. Similarly, producer blocks until there is a free slot on the queue, or use `Sender<T>::try_send` for non-blocking version.

### Example (sync version)

```rust
use std::thread;
use gil::channel;

fn main() {
    const COUNT: u32 = 100_000_000;

    let (mut tx, mut rx) = channel::<u32>(COUNT as usize);

    let handle = thread::spawn(move || {
        for i in 0..COUNT {
            // block until send completes
            let _ = tx.send(i);
        }
    });

    let _ = handle.join();

    for i in 0..COUNT {
        // block until recv completes
        let r = rx.recv();
        assert_eq!(r, i);
    }
}
```

### Example (async version)

```rust
use gil::channel;

#[tokio::main]
async fn main() {
    const COUNT: u32 = 100_000_000;

    let (mut tx, mut rx) = channel::<u32>(COUNT as usize);

    let handle = tokio::spawn(async move {
        for i in 0..COUNT {
            // block until send completes
            let _ = tx.send_async(i).await;
        }
    });

    let _ = handle.await;

    for i in 0..COUNT {
        // block until recv completes
        let r = rx.recv_async().await;
        assert_eq!(r, i);
    }
}
```

## Performance

Probably good enough. On my testing on M3 Mac, I can get 10gb/s throughput when using 129 byte objects and batching sends and receives. Aim to improve more later.


## Possible improvements

- [ ] more docs
- [ ] use zero-copy when sending to push data directly on L3 cache. (`DC CVAC` on Apple Silicon, `CLDEMOTE` on intel x86).
- [ ] investigate having a queue only for uints + arena allocator for for both more throughput and lower latency at the same time.

## License

MIT License - see [LICENSE](LICENSE) file for details.
