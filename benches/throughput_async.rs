use criterion::{black_box, criterion_group, criterion_main, Criterion};
use spsc::channel;
use tokio::runtime::Runtime;

async fn run_channel(capacity: usize, counts: u32) {
    let (mut tx, mut rx) = channel::<u32>(capacity);

    tokio::spawn(async move {
        for i in 0..counts {
            tx.send_async(black_box(i)).await;
        }
    });

    for _ in 0..counts {
        black_box(rx.recv_async().await);
    }
}

pub fn throughput_bench(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    const COUNTS: u32 = 1_000_000;

    c.bench_function(&format!("async capacity 16 messages {}", COUNTS), |b| {
        b.to_async(&runtime).iter(|| run_channel(16, COUNTS))
    });

    c.bench_function(&format!("async capacity 32 messages {}", COUNTS), |b| {
        b.to_async(&runtime).iter(|| run_channel(32, COUNTS))
    });

    c.bench_function(&format!("async capacity 64 messages {}", COUNTS), |b| {
        b.to_async(&runtime).iter(|| run_channel(64, COUNTS))
    });

    c.bench_function(&format!("async capacity 128 messages {}", COUNTS), |b| {
        b.to_async(&runtime).iter(|| run_channel(128, COUNTS))
    });

    c.bench_function(&format!("async capacity 256 messages {}", COUNTS), |b| {
        b.to_async(&runtime).iter(|| run_channel(256, COUNTS))
    });

    c.bench_function(&format!("async capacity 512 messages {}", COUNTS), |b| {
        b.to_async(&runtime).iter(|| run_channel(512, COUNTS))
    });

    c.bench_function(&format!("async capacity 1024 messages {}", COUNTS), |b| {
        b.to_async(&runtime).iter(|| run_channel(1024, COUNTS))
    });

    c.bench_function(&format!("async capacity 2048 messages {}", COUNTS), |b| {
        b.to_async(&runtime).iter(|| run_channel(2048, COUNTS))
    });

    c.bench_function(&format!("async capacity 4096 messages {}", COUNTS), |b| {
        b.to_async(&runtime).iter(|| run_channel(4096, COUNTS))
    });
}

criterion_group!(benches, throughput_bench);
criterion_main!(benches);
