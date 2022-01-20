# spsc

A single-producer-single-consumer queue, inspired from [Facebook's folly's implementation](https://github.com/facebook/folly/blob/main/folly/ProducerConsumerQueue.h).

The producer and consumer can run on different threads, but there can only be one producer and only one consumer. The producer (or consumer) can be moved between threads, but can not be shared between threads. The queue doesn't grow as needed, the capacity needs to be passed when creating the queue.

Consumer blocks until there is some value on the queue, or use `Receiver<T>::try_recv` for non-blocking version. Similiarly, producer blocks until there is some slot free on the queue to push to, or use `Sender<T>::try_send` for non-blocking version.

Some primitive benchmarks show that it's performance in sync version is compared to Facebook's folly's implementation, and async version is usually faster.

### Example

sync version

```rust
use std::thread;

use spsc::channel;

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

async version

```rust
use std::thread;

use spsc::channel;

#[tokio::main]
fn main() {
    const COUNT: u32 = 100_000_000;

    let (mut tx, mut rx) = channel::<u32>(COUNT as usize);

    let handle = tokio::spawn(move || {
        for i in 0..COUNT {
			// block until send completes
            let _ = tx.send_async(i);
        }
    });

    let _ = handle.await();

    for i in 0..COUNT {
		// block until recv completes
        let r = rx.recv_async();
        assert_eq!(r, i);
    }
}
```

### TODO

- [X] make a unbounded version which grows capacity as needed
- [ ] add batch send/recv functions
