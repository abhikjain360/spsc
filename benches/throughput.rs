use std::{hint::black_box, ptr, thread};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use gil::{QueueValue, channel};

fn run_channel(capacity: usize, counts: QueueValue) {
    let (tx, rx) = channel(capacity);

    thread::spawn(move || {
        for i in 0..counts {
            tx.send(black_box(i));
        }
    });

    for _ in 0..counts {
        black_box(rx.recv());
    }
}

fn run_channel_batch(capacity: usize, counts: QueueValue, batch_size: usize) {
    let (tx, rx) = channel(capacity);

    thread::spawn(move || {
        let iter = (0..counts).map(black_box);
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

fn run_channel_zerocopy(capacity: usize, counts: QueueValue, batch_size: usize) {
    let (tx, rx) = channel(capacity);

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

pub fn throughput_bench(c: &mut Criterion) {
    const COUNTS: QueueValue = 1_000_000;
    const BATCH_SIZE: usize = 128;

    let mut group = c.benchmark_group("throughput");
    group.throughput(Throughput::ElementsAndBytes {
        elements: COUNTS as u64,
        bytes: size_of::<QueueValue>() as u64 * COUNTS as u64,
    });
    group.sample_size(50); // Reduce sample size to keep total time down while increasing per-iter work
    group.measurement_time(std::time::Duration::from_secs(10));

    for capacity in [64, 1024, 4096] {
        group.bench_function(format!("simple_capacity_{}", capacity), |b| {
            b.iter(|| run_channel(capacity, COUNTS))
        });
        group.bench_function(format!("batch_capacity_{}", capacity), |b| {
            b.iter(|| run_channel_batch(capacity, COUNTS, BATCH_SIZE))
        });
        group.bench_function(format!("zerocopy_capacity_{}", capacity), |b| {
            b.iter(|| run_channel_zerocopy(capacity, COUNTS, BATCH_SIZE))
        });
    }
    group.finish();
}

criterion_group!(benches, throughput_bench);
criterion_main!(benches);
