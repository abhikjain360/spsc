use gil::channel;
use std::time::Instant;

#[tokio::main]
async fn main() {
    const CAPACITY: usize = 1024;
    const ITERATIONS: usize = 50_000_000;

    let (mut tx, mut rx) = channel::<u64>(CAPACITY);

    let start = Instant::now();

    let p = tokio::spawn(async move {
        for i in 0..ITERATIONS {
            tx.send_async(i as u64).await;
        }
    });

    let c = tokio::spawn(async move {
        let mut sum: u64 = 0;
        for _ in 0..ITERATIONS {
            sum = sum.wrapping_add(rx.recv_async().await);
        }
        sum
    });

    p.await.unwrap();
    let sum = c.await.unwrap();

    let elapsed = start.elapsed();
    println!(
        "Async: Sum={}, Time={:?}, Ops/sec={:.2} M",
        sum,
        elapsed,
        (ITERATIONS as f64 / elapsed.as_secs_f64()) / 1_000_000.0
    );
}
