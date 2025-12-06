use std::{hint::black_box, num::NonZeroUsize, thread::spawn, time::SystemTime};

fn main() {
    let (mut tx, mut rx) = gil::channel(NonZeroUsize::new(4096).unwrap());
    spawn(move || {
        let mut i = 0;
        while i < 100_000_000 {
            tx.send(black_box(i));
            i += 1;
        }
    });
    let start = SystemTime::now();
    let mut i = 0;
    while i < 100_000_000 {
        assert_eq!(i, rx.recv());
        i += 1;
    }
    println!("{:?}", start.elapsed().unwrap());
}
