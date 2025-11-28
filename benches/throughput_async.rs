use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use gil::channel;
use std::hint::black_box;
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

async fn run_channel_batch(capacity: usize, counts: u32) {
    let (mut tx, mut rx) = channel::<u32>(capacity);
    const BATCH_SIZE: usize = 128;

    tokio::spawn(async move {
        let iter = (0..counts).map(black_box);
        tx.batch_send_all_async(iter).await;
    });

    let mut rx_vec = Vec::with_capacity(BATCH_SIZE);
    let mut received = 0;
    while received < counts {
        let n = rx.batch_recv_async(&mut rx_vec, BATCH_SIZE).await;
        received += n as u32;
        rx_vec.clear();
    }
}

pub fn throughput_bench(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    const COUNTS: u32 = 1_000_000;

    let mut group = c.benchmark_group("async_throughput");
    group.throughput(Throughput::Bytes(COUNTS as u64 * 4));
    group.sample_size(50);
    group.measurement_time(std::time::Duration::from_secs(10));

    for capacity in [64, 1024, 4096] {
        group.bench_function(format!("capacity_{}", capacity), |b| {
            b.to_async(&runtime).iter(|| run_channel(capacity, COUNTS))
        });
        group.bench_function(format!("batch_capacity_{}", capacity), |b| {
            b.to_async(&runtime)
                .iter(|| run_channel_batch(capacity, COUNTS))
        });
    }
    group.finish();
}

criterion_group!(benches, throughput_bench);
criterion_main!(benches);
