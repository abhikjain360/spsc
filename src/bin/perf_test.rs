use std::thread;

use spsc::channel;

fn main() {
    const COUNT: u32 = 100_000_000;

    let (mut tx, mut rx) = channel::<u32>(COUNT as usize);

    let handle = thread::spawn(move || {
        for i in 0..COUNT {
            tx.send(i);
        }
    });

    let _ = handle.join();

    for i in 0..COUNT {
        let r = rx.recv();
        assert_eq!(r, i);
    }
}
