// Comparison benchmark for different channel implementations
//
// This benchmark compares:
// - gil (this library)
// - bounded-spsc-queue (SPSC only, requires nightly)
// - crossbeam-channel (MPMC)
// - flume (MPMC)
//
// Benchmarks include:
// 1. Latency tests with queue sizes: 512, 4096
//    - Measures round-trip latency using ping-pong pattern
//
// 2. Throughput tests with queue sizes: 512, 4096
//    - Simple: Single send/recv per operation (1M elements)
//    - Batched: Batch send/recv operations (128 element batches)
//    - Zero-copy: Direct memory access (gil only)
//
// Note: bounded-spsc-queue requires nightly Rust due to allocator_api feature
// Run with: `cargo +nightly bench --bench comparison`

use std::hint::black_box;
use std::ptr;
use std::thread;
use std::time::Instant;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use gil::{QueueValue, channel};

// ============================================================================
// Latency Benchmarks
// ============================================================================

fn latency_gil(capacity: usize, iters: u64) {
    let (mut tx1, mut rx1) = channel(capacity);
    let (mut tx2, mut rx2) = channel(capacity);

    let t = thread::spawn(move || {
        for _ in 0..iters {
            let val = rx1.recv();
            tx2.send(val);
        }
    });

    for i in 0..iters {
        tx1.send(black_box(i as QueueValue));
        let val = rx2.recv();
        black_box(val);
    }

    t.join().unwrap();
}

fn latency_bounded_spsc_queue(capacity: usize, iters: u64) {
    let (p1, c1) = bounded_spsc_queue::make(capacity);
    let (p2, c2) = bounded_spsc_queue::make(capacity);

    let t = thread::spawn(move || {
        for _ in 0..iters {
            let val = c1.pop();
            p2.push(val);
        }
    });

    for i in 0..iters {
        p1.push(black_box(i as QueueValue));
        let val = c2.pop();
        black_box(val);
    }

    t.join().unwrap();
}

fn latency_crossbeam(capacity: usize, iters: u64) {
    let (s1, r1) = crossbeam_channel::bounded(capacity);
    let (s2, r2) = crossbeam_channel::bounded(capacity);

    let t = thread::spawn(move || {
        for _ in 0..iters {
            let val = r1.recv().unwrap();
            s2.send(val).unwrap();
        }
    });

    for i in 0..iters {
        s1.send(black_box(i as QueueValue)).unwrap();
        let val = r2.recv().unwrap();
        black_box(val);
    }

    t.join().unwrap();
}

fn latency_flume(capacity: usize, iters: u64) {
    let (s1, r1) = flume::bounded(capacity);
    let (s2, r2) = flume::bounded(capacity);

    let t = thread::spawn(move || {
        for _ in 0..iters {
            let val = r1.recv().unwrap();
            s2.send(val).unwrap();
        }
    });

    for i in 0..iters {
        s1.send(black_box(i as QueueValue)).unwrap();
        let val = r2.recv().unwrap();
        black_box(val);
    }

    t.join().unwrap();
}

fn latency_bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("latency");

    for capacity in [512, 4096] {
        group.bench_function(format!("gil_{}", capacity), |b| {
            b.iter_custom(|iters| {
                let start = Instant::now();
                latency_gil(capacity, iters);
                start.elapsed()
            })
        });

        group.bench_function(format!("bounded_spsc_queue_{}", capacity), |b| {
            b.iter_custom(|iters| {
                let start = Instant::now();
                latency_bounded_spsc_queue(capacity, iters);
                start.elapsed()
            })
        });

        group.bench_function(format!("crossbeam_{}", capacity), |b| {
            b.iter_custom(|iters| {
                let start = Instant::now();
                latency_crossbeam(capacity, iters);
                start.elapsed()
            })
        });

        group.bench_function(format!("flume_{}", capacity), |b| {
            b.iter_custom(|iters| {
                let start = Instant::now();
                latency_flume(capacity, iters);
                start.elapsed()
            })
        });
    }

    group.finish();
}

// ============================================================================
// Throughput Benchmarks - Simple (no batching)
// ============================================================================

fn throughput_gil_simple(capacity: usize, counts: QueueValue) {
    let (mut tx, mut rx) = channel(capacity);

    thread::spawn(move || {
        for i in 0..counts {
            tx.send2(black_box(i));
        }
    });

    for _ in 0..counts {
        black_box(rx.recv2());
    }
}

fn throughput_bounded_spsc_queue_simple(capacity: usize, counts: u64) {
    let (p, c) = bounded_spsc_queue::make(capacity);

    thread::spawn(move || {
        for i in 0..counts {
            p.push(black_box(i as QueueValue));
        }
    });

    for _ in 0..counts {
        black_box(c.pop());
    }
}

fn throughput_crossbeam_simple(capacity: usize, counts: u64) {
    let (s, r) = crossbeam_channel::bounded(capacity);

    thread::spawn(move || {
        for i in 0..counts {
            s.send(black_box(i as QueueValue)).unwrap();
        }
    });

    for _ in 0..counts {
        black_box(r.recv().unwrap());
    }
}

fn throughput_flume_simple(capacity: usize, counts: u64) {
    let (s, r) = flume::bounded(capacity);

    thread::spawn(move || {
        for i in 0..counts {
            s.send(black_box(i as QueueValue)).unwrap();
        }
    });

    for _ in 0..counts {
        black_box(r.recv().unwrap());
    }
}

// ============================================================================
// Throughput Benchmarks - Batched
// ============================================================================

fn throughput_gil_batch(capacity: usize, counts: QueueValue, batch_size: usize) {
    let (mut tx, mut rx) = channel(capacity);

    thread::spawn(move || {
        let iter = (0..counts).map(|i| black_box(i));
        tx.batch_send_all(iter);
    });

    let mut rx_vec = Vec::with_capacity(batch_size);
    let mut received = 0;
    while received < counts {
        let n = rx.batch_recv(&mut rx_vec, batch_size);
        received += n as QueueValue;
        rx_vec.clear();
    }
}

fn throughput_crossbeam_batch(capacity: usize, counts: u64, batch_size: usize) {
    let (s, r) = crossbeam_channel::bounded(capacity);

    thread::spawn(move || {
        // Crossbeam doesn't have batch_send, so we send individually
        // but we can at least prepare the data in batches
        let mut batch = Vec::with_capacity(batch_size);
        for i in 0..counts {
            batch.push(black_box(i as QueueValue));
            if batch.len() == batch_size {
                for val in batch.drain(..) {
                    s.send(val).unwrap();
                }
            }
        }
        // Send remaining
        for val in batch {
            s.send(val).unwrap();
        }
    });

    let mut received = 0;
    let mut batch = Vec::with_capacity(batch_size);
    while received < counts {
        // Use try_iter to receive multiple messages
        let mut got_any = false;
        for val in r.try_iter().take(batch_size) {
            batch.push(val);
            received += 1;
            got_any = true;
            if received >= counts {
                break;
            }
        }

        // If we didn't get any messages and still need more, do a blocking recv
        if !got_any && received < counts {
            match r.recv() {
                Ok(val) => {
                    batch.push(val);
                    received += 1;
                }
                Err(_) => break, // Channel disconnected
            }
        }

        black_box(&batch);
        batch.clear();
    }
}

fn throughput_flume_batch(capacity: usize, counts: u64, batch_size: usize) {
    let (s, r) = flume::bounded(capacity);

    thread::spawn(move || {
        // Flume doesn't have batch_send, but we prepare data in batches
        let mut batch = Vec::with_capacity(batch_size);
        for i in 0..counts {
            batch.push(black_box(i as QueueValue));
            if batch.len() == batch_size {
                for val in batch.drain(..) {
                    s.send(val).unwrap();
                }
            }
        }
        // Send remaining
        for val in batch {
            s.send(val).unwrap();
        }
    });

    let mut received = 0;
    let mut batch = Vec::with_capacity(batch_size);
    while received < counts {
        // Try to drain available messages
        let mut got_any = false;
        for val in r.drain().take(batch_size.min((counts - received) as usize)) {
            batch.push(val);
            received += 1;
            got_any = true;
        }

        // If we didn't get any messages from drain and still need more, do a blocking recv
        if !got_any && received < counts {
            match r.recv() {
                Ok(val) => {
                    batch.push(val);
                    received += 1;
                }
                Err(_) => break, // Channel disconnected
            }
        }

        black_box(&batch);
        batch.clear();
    }
}

// ============================================================================
// Throughput Benchmarks - Zero-copy (gil only)
// ============================================================================

fn throughput_gil_zerocopy(capacity: usize, counts: QueueValue, batch_size: usize) {
    let (mut tx, mut rx) = channel(capacity);

    thread::spawn(move || {
        let mut remaining = counts;
        let dummy_data = [123u128; 128]; // Source data

        while remaining > 0 {
            let slice = tx.get_write_slice();
            if slice.is_empty() {
                continue;
            }

            let batch = remaining.min(slice.len() as QueueValue);
            let batch = batch.min(batch_size as QueueValue) as usize;

            // MEMCPY! This is what gets you high GB/s
            unsafe {
                ptr::copy_nonoverlapping(dummy_data.as_ptr(), slice.as_mut_ptr(), batch);
            }

            tx.commit(batch);
            remaining -= batch as QueueValue;
        }
    });

    let mut received = 0;
    while received < counts {
        let len = {
            let slice = rx.get_read_slice();
            if slice.is_empty() {
                continue;
            }

            // Just touch the memory to ensure we read it
            let _ = black_box(slice[0]);
            slice.len()
        };

        rx.advance(len);
        received += len as QueueValue;
    }
}

fn throughput_bench(c: &mut Criterion) {
    const COUNTS: QueueValue = 1_000_000;
    const BATCH_SIZE: usize = 128;

    let mut group = c.benchmark_group("throughput");
    group.throughput(Throughput::ElementsAndBytes {
        elements: COUNTS as u64,
        bytes: size_of::<QueueValue>() as u64 * COUNTS as u64,
    });
    group.sample_size(50);
    group.measurement_time(std::time::Duration::from_secs(10));

    for capacity in [512, 4096] {
        // Simple throughput tests
        group.bench_function(format!("gil_simple_{}", capacity), |b| {
            b.iter(|| throughput_gil_simple(capacity, COUNTS))
        });

        group.bench_function(format!("bounded_spsc_queue_simple_{}", capacity), |b| {
            b.iter(|| throughput_bounded_spsc_queue_simple(capacity, COUNTS as u64))
        });

        group.bench_function(format!("crossbeam_simple_{}", capacity), |b| {
            b.iter(|| throughput_crossbeam_simple(capacity, COUNTS as u64))
        });

        group.bench_function(format!("flume_simple_{}", capacity), |b| {
            b.iter(|| throughput_flume_simple(capacity, COUNTS as u64))
        });

        // Batched throughput tests
        group.bench_function(format!("gil_batch_{}", capacity), |b| {
            b.iter(|| throughput_gil_batch(capacity, COUNTS, BATCH_SIZE))
        });

        group.bench_function(format!("crossbeam_batch_{}", capacity), |b| {
            b.iter(|| throughput_crossbeam_batch(capacity, COUNTS as u64, BATCH_SIZE))
        });

        group.bench_function(format!("flume_batch_{}", capacity), |b| {
            b.iter(|| throughput_flume_batch(capacity, COUNTS as u64, BATCH_SIZE))
        });

        // Zero-copy test (gil only)
        group.bench_function(format!("gil_zerocopy_{}", capacity), |b| {
            b.iter(|| throughput_gil_zerocopy(capacity, COUNTS, BATCH_SIZE))
        });
    }

    group.finish();
}

criterion_group!(benches, latency_bench, throughput_bench);
criterion_main!(benches);
