use std::{hint::black_box, thread};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use gil::channel;

fn run_channel(capacity: usize, counts: u32) {
    let (mut tx, mut rx) = channel::<u32>(capacity);

    thread::spawn(move || {
        for i in 0..counts {
            tx.send(black_box(i));
        }
    });

    for _ in 0..counts {
        black_box(rx.recv());
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LargeStruct {
    data: [u64; 16], // 128 bytes
}

impl LargeStruct {
    fn new(i: usize) -> Self {
        let mut data = [0u64; 16];
        data[0] = i as u64;
        Self { data }
    }
}

fn run_channel_large(capacity: usize, counts: u32) {
    let (mut tx, mut rx) = channel::<LargeStruct>(capacity);

    thread::spawn(move || {
        for i in 0..counts {
            tx.send(black_box(LargeStruct::new(i as usize)));
        }
    });

    for _ in 0..counts {
        black_box(rx.recv());
    }
}

fn run_channel_batch(capacity: usize, counts: u32, batch_size: usize) {
    let (mut tx, mut rx) = channel::<u32>(capacity);

    thread::spawn(move || {
        let iter = (0..counts).map(black_box);
        tx.batch_send_all(iter);
    });

    let mut rx_vec = Vec::with_capacity(batch_size);
    let mut received = 0;
    while received < counts {
        let n = rx.batch_recv(&mut rx_vec, batch_size);
        received += n as u32;
        rx_vec.clear();
    }
}

pub fn throughput_bench(c: &mut Criterion) {
    const COUNTS: u32 = 1_000_000;
    const BATCH_SIZE: usize = 128;

    let mut group = c.benchmark_group("throughput_u32");
    group.throughput(Throughput::Bytes(COUNTS as u64 * 4));
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

    let mut group = c.benchmark_group("throughput_large_128b");
    group.throughput(Throughput::Bytes(COUNTS as u64 * 128));
    group.sample_size(50);
    group.measurement_time(std::time::Duration::from_secs(10));

    // Test large struct only on a medium capacity
    group.bench_function("capacity_1024", |b| {
        b.iter(|| run_channel_large(1024, COUNTS))
    });
    group.finish();
}

criterion_group!(benches, throughput_bench);
criterion_main!(benches);
