use std::{hint::black_box, num::NonZeroUsize, thread::spawn, time::SystemTime};

use futures::executor::block_on;

fn latency() {
    let (mut tx1, mut rx1) = gil::channel(NonZeroUsize::new(512).unwrap());
    let (mut tx2, mut rx2) = gil::channel(NonZeroUsize::new(512).unwrap());

    let iter = 1_000_000;

    spawn(move || {
        block_on(async move {
            for i in 0..iter {
                let x = rx1.recv_async().await;
                black_box(x);
                tx2.send_async(black_box(i)).await;
            }
        })
    });

    let start = SystemTime::now();

    block_on(async move {
        for i in 0..iter {
            tx1.send_async(black_box(i)).await;
            let x = rx2.recv_async().await;
            black_box(x);
        }
    });

    println!("{:?}", start.elapsed().unwrap());
}

fn throughpuut() {
    let (mut tx, mut rx) = gil::channel(NonZeroUsize::new(512).unwrap());

    let handle = spawn(move || {
        block_on(async move {
            let mut i = 0;
            while i < 100_000_000 {
                tx.send_async(black_box(i)).await;
                i += 1;
            }
        })
    });

    let start = SystemTime::now();
    block_on(async move {
        let mut i = 0;
        while i < 100_000_000 {
            let val = rx.recv_async().await;
            assert_eq!(i, val);
            i += 1;
        }
    });

    println!("{:?}", start.elapsed().unwrap());
    handle.join().unwrap();
}

fn main() {
    throughpuut();
    latency();
}
