use std::{
    hint::{black_box, spin_loop},
    num::NonZeroUsize,
    ptr,
    thread::spawn,
    time::{Duration, SystemTime},
};

use criterion::{
    BenchmarkGroup, Criterion, Throughput, criterion_group, criterion_main, measurement::WallTime,
};
use gil::channel;

fn make_group<'a>(c: &'a mut Criterion, name: &str) -> BenchmarkGroup<'a, WallTime> {
    let mut group = c.benchmark_group(name);
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(50);
    group.warm_up_time(Duration::from_secs(1));

    group
}

fn benchmark(c: &mut Criterion) {
    const SIZES: [NonZeroUsize; 3] = [
        NonZeroUsize::new(512).unwrap(),
        NonZeroUsize::new(4096).unwrap(),
        NonZeroUsize::new(65_536).unwrap(),
    ];

    let mut group = make_group(c, "roundtrip_latency");

    for size in SIZES {
        group.bench_function(format!("size_{size}"), |b| {
            b.iter_custom(move |iter| {
                let (mut tx1, mut rx1) = channel(size);
                let (mut tx2, mut rx2) = channel(size);

                let iter = iter as usize;

                spawn(move || {
                    for i in 0..iter {
                        let x = rx1.recv();
                        black_box(x);
                        tx2.send(black_box(i));
                    }
                });

                let start = SystemTime::now();

                for i in 0..iter {
                    tx1.send(black_box(i));
                    let x = rx2.recv();
                    black_box(x);
                }

                start.elapsed().unwrap()
            });
        });
    }
    drop(group);

    let mut group = make_group(c, "push_latency");

    for size in SIZES {
        group.bench_function(format!("size_{size}"), |b| {
            b.iter_custom(|iter| {
                let (mut tx, mut rx) = channel(size);
                spawn(move || {
                    for _ in 0..iter {
                        black_box(rx.recv());
                    }
                });
                let start = SystemTime::now();

                for _ in 0..iter {
                    tx.send(black_box(0));
                }

                start.elapsed().unwrap()
            });
        });
    }
    drop(group);

    let mut group = make_group(c, "throughput");
    group.throughput(Throughput::ElementsAndBytes {
        elements: ELEMENTS as u64,
        bytes: (ELEMENTS * size_of::<usize>()) as u64,
    });

    const ELEMENTS: usize = 1_000_000;

    for size in SIZES {
        group.bench_function(format!("size_{size}/direct"), |b| {
            b.iter(|| {
                let (mut tx, mut rx) = channel(size);

                spawn(move || {
                    for i in 0..ELEMENTS {
                        let x = black_box(rx.recv());
                        assert_eq!(i, x);
                    }
                });

                for i in 0..ELEMENTS {
                    tx.send(black_box(i));
                }
            });
        });
    }

    for size in SIZES {
        group.bench_function(format!("size_{size}/batched"), |b| {
            b.iter(|| {
                let (mut tx, mut rx) = channel(size);

                spawn(move || {
                    let mut received = 0;
                    while received < ELEMENTS {
                        let buf = rx.read_buffer();
                        let len = buf.len();
                        if len == 0 {
                            spin_loop();
                            continue;
                        }

                        black_box(buf[0]);

                        unsafe { rx.advance(len) };
                        received += len;
                    }
                });

                let mut sent = 0;
                let src_data = vec![1usize; size.get()];

                while sent < ELEMENTS {
                    let buf = tx.write_buffer();
                    let len = buf.len().min(ELEMENTS - sent);
                    if len == 0 {
                        spin_loop();
                        continue;
                    }

                    unsafe {
                        ptr::copy_nonoverlapping(src_data.as_ptr(), buf.as_mut_ptr(), len);
                    }

                    unsafe { tx.commit(len) };
                    sent += len;
                }
            });
        });
    }
}

#[cfg(feature = "async")]
fn benchmark_async(c: &mut Criterion) {
    use criterion::async_executor::FuturesExecutor;

    const SIZES: [NonZeroUsize; 3] = [
        NonZeroUsize::new(512).unwrap(),
        NonZeroUsize::new(4096).unwrap(),
        NonZeroUsize::new(65_536).unwrap(),
    ];

    let mut group = make_group(c, "async_roundtrip_latency");

    for size in SIZES {
        group.bench_function(format!("size_{size}"), |b| {
            b.to_async(FuturesExecutor)
                .iter_custom(move |iter| async move {
                    let (mut tx1, mut rx1) = channel(size);
                    let (mut tx2, mut rx2) = channel(size);

                    let iter = iter as usize;

                    let handle = spawn(move || {
                        futures::executor::block_on(async move {
                            for i in 0..iter {
                                let x = rx1.recv_async().await;
                                black_box(x);
                                tx2.send_async(black_box(i)).await;
                            }
                        })
                    });

                    let start = SystemTime::now();

                    for i in 0..iter {
                        tx1.send_async(black_box(i)).await;
                        let x = rx2.recv_async().await;
                        black_box(x);
                    }

                    handle.join().unwrap();

                    start.elapsed().unwrap()
                });
        });
    }
    drop(group);

    let mut group = make_group(c, "async_push_latency");

    for size in SIZES {
        group.bench_function(format!("size_{size}"), |b| {
            b.to_async(FuturesExecutor)
                .iter_custom(move |iter| async move {
                    let (mut tx, mut rx) = channel(size);

                    let handle = spawn(move || {
                        futures::executor::block_on(async move {
                            for _ in 0..iter {
                                black_box(rx.recv_async().await);
                            }
                        })
                    });

                    let start = SystemTime::now();

                    for _ in 0..iter {
                        tx.send_async(black_box(0)).await;
                    }

                    handle.join().unwrap();

                    start.elapsed().unwrap()
                });
        });
    }
    drop(group);

    let mut group = make_group(c, "async_throughput");
    const ELEMENTS: usize = 1_000_000;

    group.throughput(Throughput::ElementsAndBytes {
        elements: ELEMENTS as u64,
        bytes: (ELEMENTS * size_of::<usize>()) as u64,
    });

    for size in SIZES {
        group.bench_function(format!("size_{size}"), |b| {
            b.to_async(FuturesExecutor).iter(move || async move {
                let (mut tx, mut rx) = channel(size);

                let handle = spawn(move || {
                    futures::executor::block_on(async move {
                        for i in 0..ELEMENTS {
                            let x = black_box(rx.recv_async().await);
                            assert_eq!(i, x);
                        }
                    })
                });

                for i in 0..ELEMENTS {
                    tx.send_async(black_box(i)).await;
                }

                handle.join().unwrap();
            });
        });
    }
}

#[cfg(feature = "async")]
criterion_group! {benches, benchmark, benchmark_async}

#[cfg(not(feature = "async"))]
criterion_group! {benches, benchmark}

criterion_main! {benches}
