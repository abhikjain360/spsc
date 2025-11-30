use gil::{QueueValue, channel};
use std::ptr;
use std::thread;
use std::time::Instant;

const CAPACITY: usize = 1 << 16; // Power of 2
const ITERATIONS: usize = 100_000_000;

fn run() {
    let (mut tx, mut rx) = channel(CAPACITY);

    let start = Instant::now();

    let p = thread::spawn(move || {
        for i in 0..ITERATIONS {
            tx.send(i as QueueValue);
        }
    });

    let mut sum: QueueValue = 0;
    for _ in 0..ITERATIONS {
        sum = sum.wrapping_add(rx.recv());
    }

    p.join().unwrap();

    let elapsed = start.elapsed();
    println!(
        "Sync: Sum={}, Time={:?}, Ops/sec={:.2} M",
        sum,
        elapsed,
        (ITERATIONS as f64 / elapsed.as_secs_f64()) / 1_000_000.0
    );

    let (mut tx, mut rx) = channel(CAPACITY);

    let start = Instant::now();

    let p = thread::spawn(move || {
        let mut remaining = ITERATIONS;
        let dummy_data = [123u128; 128];

        while remaining > 0 {
            let slice = tx.get_write_slice();
            if slice.is_empty() {
                #[cfg(target_arch = "aarch64")]
                unsafe {
                    core::arch::asm!("wfe", options(nomem, nostack, preserves_flags));
                }
                continue;
            }

            let batch = remaining.min(slice.len() as usize);
            let batch = batch.min(128 as usize) as usize;

            unsafe {
                ptr::copy_nonoverlapping(dummy_data.as_ptr(), slice.as_mut_ptr(), batch);
            }

            tx.commit(batch);
            remaining -= batch;
        }
    });

    let mut received = 0;
    while received < ITERATIONS {
        let len = {
            let slice = rx.get_read_slice();
            if slice.is_empty() {
                #[cfg(target_arch = "aarch64")]
                unsafe {
                    core::arch::asm!("wfe", options(nomem, nostack, preserves_flags));
                }
                continue;
            }

            slice.len()
        };

        rx.advance(len);
        received += len;
    }

    p.join().unwrap();

    let elapsed = start.elapsed();
    println!(
        "Zero-copy: Sum={}, Time={:?}, Ops/sec={:.2} M",
        sum,
        elapsed,
        (ITERATIONS as f64 / elapsed.as_secs_f64()) / 1_000_000.0
    );
}

fn main() {
    run();
}
