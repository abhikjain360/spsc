use gil::{QueueValue, channel};
use std::thread;
use std::time::Instant;

fn main() {
    const CAPACITY: usize = 1024; // Power of 2
    const ITERATIONS: usize = 100_000_000;

    let (mut tx, mut rx) = channel(CAPACITY);

    let start = Instant::now();

    let p = thread::spawn(move || {
        for i in 0..ITERATIONS {
            tx.send(i as QueueValue);
        }
    });

    let c = thread::spawn(move || {
        let mut sum: QueueValue = 0;
        for _ in 0..ITERATIONS {
            sum = sum.wrapping_add(rx.recv());
        }
        sum
    });

    p.join().unwrap();
    let sum = c.join().unwrap();

    let elapsed = start.elapsed();
    println!(
        "Sync: Sum={}, Time={:?}, Ops/sec={:.2} M",
        sum,
        elapsed,
        (ITERATIONS as f64 / elapsed.as_secs_f64()) / 1_000_000.0
    );
}
