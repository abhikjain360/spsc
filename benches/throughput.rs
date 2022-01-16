use std::thread;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use spsc::channel;

pub fn throughput_bench(c: &mut Criterion) {
    c.bench_function("capacity 16 messages 1_000_000", |b| b.iter(|| {
        let (mut tx, mut rx) = channel::<u32>(16);
        const COUNTS: u32 = 1_000_000;
        thread::spawn(move || {
            for i in 0..COUNTS {
                tx.send(black_box(i));
            }
        });
        for _ in 0..COUNTS {
            black_box(rx.recv());
        }
    }));
    c.bench_function("capacity 32 messages 1_000_000", |b| b.iter(|| {
        let (mut tx, mut rx) = channel::<u32>(32);
        const COUNTS: u32 = 1_000_000;
        thread::spawn(move || {
            for i in 0..COUNTS {
                tx.send(black_box(i));
            }
        });
        for _ in 0..COUNTS {
            black_box(rx.recv());
        }
    }));
    c.bench_function("capacity 64 messages 1_000_000", |b| b.iter(|| {
        let (mut tx, mut rx) = channel::<u32>(64);
        const COUNTS: u32 = 1_000_000;
        thread::spawn(move || {
            for i in 0..COUNTS {
                tx.send(black_box(i));
            }
        });
        for _ in 0..COUNTS {
            black_box(rx.recv());
        }
    }));
    c.bench_function("capacity 128 messages 1_000_000", |b| b.iter(|| {
        let (mut tx, mut rx) = channel::<u32>(128);
        const COUNTS: u32 = 1_000_000;
        thread::spawn(move || {
            for i in 0..COUNTS {
                tx.send(black_box(i));
            }
        });
        for _ in 0..COUNTS {
            black_box(rx.recv());
        }
    }));
    c.bench_function("capacity 256 messages 1_000_000", |b| b.iter(|| {
        let (mut tx, mut rx) = channel::<u32>(256);
        const COUNTS: u32 = 1_000_000;
        thread::spawn(move || {
            for i in 0..COUNTS {
                tx.send(black_box(i));
            }
        });
        for _ in 0..COUNTS {
            black_box(rx.recv());
        }
    }));
    c.bench_function("capacity 512 messages 1_000_000", |b| b.iter(|| {
        let (mut tx, mut rx) = channel::<u32>(512);
        const COUNTS: u32 = 1_000_000;
        thread::spawn(move || {
            for i in 0..COUNTS {
                tx.send(black_box(i));
            }
        });
        for _ in 0..COUNTS {
            black_box(rx.recv());
        }
    }));
    c.bench_function("capacity 1024 messages 1_000_000", |b| b.iter(|| {
        let (mut tx, mut rx) = channel::<u32>(1024);
        const COUNTS: u32 = 1_000_000;
        thread::spawn(move || {
            for i in 0..COUNTS {
                tx.send(black_box(i));
            }
        });
        for _ in 0..COUNTS {
            black_box(rx.recv());
        }
    }));
    c.bench_function("capacity 2048 messages 1_000_000", |b| b.iter(|| {
        let (mut tx, mut rx) = channel::<u32>(2048);
        const COUNTS: u32 = 1_000_000;
        thread::spawn(move || {
            for i in 0..COUNTS {
                tx.send(black_box(i));
            }
        });
        for _ in 0..COUNTS {
            black_box(rx.recv());
        }
    }));
    c.bench_function("capacity 4096 messages 1_000_000", |b| b.iter(|| {
        let (mut tx, mut rx) = channel::<u32>(4096);
        const COUNTS: u32 = 1_000_000;
        thread::spawn(move || {
            for i in 0..COUNTS {
                tx.send(black_box(i));
            }
        });
        for _ in 0..COUNTS {
            black_box(rx.recv());
        }
    }));
}

criterion_group!(benches, throughput_bench);
criterion_main!(benches);
