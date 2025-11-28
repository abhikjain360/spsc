use std::hint::black_box;
use std::thread;
use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use gil::{QueueValue, channel};

fn latency_bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("latency");

    for capacity in [64, 512, 1024, 4096, 65536] {
        group.bench_function(format!("capacity_{}", capacity), |b| {
            b.iter_custom(|iters| {
                let (mut tx1, mut rx1) = channel(capacity);
                let (mut tx2, mut rx2) = channel(capacity);

                let t = thread::spawn(move || {
                    for _ in 0..iters {
                        let val = rx1.recv();
                        tx2.send(val);
                    }
                });

                let start = Instant::now();
                for i in 0..iters {
                    tx1.send(black_box(i as QueueValue));
                    let val = rx2.recv();
                    black_box(val);
                }
                let elapsed = start.elapsed();

                t.join().unwrap();
                elapsed
            })
        });
    }
    group.finish();
}

criterion_group!(benches, latency_bench);
criterion_main!(benches);
