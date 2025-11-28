use std::{hint::black_box, thread};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use gil::{QueueValue, channel};

fn run_channel(capacity: usize, counts: QueueValue) {
    let (mut tx, mut rx) = channel(capacity);

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

pub fn throughput_bench(c: &mut Criterion) {
    const COUNTS: QueueValue = 1_000_000;
    const BATCH_SIZE: usize = 128;

    let mut group = c.benchmark_group("throughput");
    group.throughput(Throughput::Bytes(
        COUNTS as u64 * size_of::<QueueValue>() as u64,
    ));
    group.sample_size(50); // Reduce sample size to keep total time down while increasing per-iter work
    group.measurement_time(std::time::Duration::from_secs(10));

    for capacity in [64, 1024, 4096] {
        group.bench_function(format!("capacity_{}", capacity), |b| {
            b.iter(|| run_channel(capacity, COUNTS))
        });
        group.bench_function(format!("batch_capacity_{}", capacity), |b| {
            b.iter(|| run_channel_batch(capacity, COUNTS, BATCH_SIZE))
        });
    }
    group.finish();
}

criterion_group!(benches, throughput_bench);
criterion_main!(benches);
