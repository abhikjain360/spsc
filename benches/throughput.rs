use std::thread;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use spsc::channel;

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

pub fn throughput_bench(c: &mut Criterion) {
    const COUNTS: u32 = 1_000_000;

    c.bench_function(&format!("capacity 16 messages {}", COUNTS), |b| {
        b.iter(|| run_channel(16, COUNTS))
    });

    c.bench_function(&format!("capacity 32 messages {}", COUNTS), |b| {
        b.iter(|| run_channel(32, COUNTS))
    });

    c.bench_function(&format!("capacity 64 messages {}", COUNTS), |b| {
        b.iter(|| run_channel(64, COUNTS))
    });

    c.bench_function(&format!("capacity 128 messages {}", COUNTS), |b| {
        b.iter(|| run_channel(128, COUNTS))
    });

    c.bench_function(&format!("capacity 256 messages {}", COUNTS), |b| {
        b.iter(|| run_channel(256, COUNTS))
    });

    c.bench_function(&format!("capacity 512 messages {}", COUNTS), |b| {
        b.iter(|| run_channel(512, COUNTS))
    });

    c.bench_function(&format!("capacity 1024 messages {}", COUNTS), |b| {
        b.iter(|| run_channel(1024, COUNTS))
    });

    c.bench_function(&format!("capacity 2048 messages {}", COUNTS), |b| {
        b.iter(|| run_channel(2048, COUNTS))
    });

    c.bench_function(&format!("capacity 4096 messages {}", COUNTS), |b| {
        b.iter(|| run_channel(4096, COUNTS))
    });
}

criterion_group!(benches, throughput_bench);
criterion_main!(benches);
