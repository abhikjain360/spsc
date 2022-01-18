use std::thread;

use spsc::channel;

fn main() {
    const COUNT: u32 = 100_000_000;

    let (mut tx, mut rx) = channel::<u32>(COUNT as usize);

    let handle = thread::spawn(move || {
        for i in 0..COUNT {
            let _ = tx.try_send(i);
        }
    });

    let _ = handle.join();

    let mut sum = 0;
    for i in 0..COUNT {
        let r = unsafe { rx.try_recv().unwrap_unchecked() };
        sum += r & 1000007;
        assert_eq!(r, i);
    }

    println!("{}", sum);
}
