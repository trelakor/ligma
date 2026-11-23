//! Threads kept to encode units on, as starting them for each tile took
//! half a millisecond of it

use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

type Task = Box<dyn FnOnce() + Send + 'static>;

/// The most threads `run` takes at once
pub const MOST: usize = 8;

static POOL: OnceLock<Mutex<Sender<Task>>> = OnceLock::new();

fn pool() -> &'static Mutex<Sender<Task>> {
    POOL.get_or_init(|| {
        let (send, tasks) = channel::<Task>();
        let tasks: Arc<Mutex<Receiver<Task>>> = Arc::new(Mutex::new(tasks));
        for _ in 0..MOST {
            let tasks = tasks.clone();
            let _ = std::thread::Builder::new().name("ligma encoder".into()).spawn(move || loop {
                let task = tasks.lock().unwrap().recv();
                match task {
                    Ok(task) => task(),
                    Err(_) => return,
                }
            });
        }
        Mutex::new(send)
    })
}

/// What `work` returns run on `threads` of the pool's threads at once,
/// waiting for all of them. One that panics returns nothing
pub fn run<R: Send>(threads: usize, work: &(dyn Fn() -> R + Sync)) -> Vec<R> {
    let results: Mutex<Vec<R>> = Mutex::new(Vec::with_capacity(threads));
    // Each task's own, as it's still told the count once this has seen it
    // reach none and gone on
    let left = Arc::new((Mutex::new(threads), Condvar::new()));
    let results_at = &results;
    let send = pool().lock().unwrap().clone();
    for _ in 0..threads {
        let left_at = left.clone();
        let task: Box<dyn FnOnce() + Send + '_> = Box::new(move || {
            if let Ok(out) = std::panic::catch_unwind(AssertUnwindSafe(work)) {
                results_at.lock().unwrap().push(out);
            }
            *left_at.0.lock().unwrap() -= 1;
            left_at.1.notify_all();
        });
        // Safety: this waits below till every task sent has run, so what
        // they borrow outlives them
        let task: Task = unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + '_>, Task>(task) };
        if let Err(unsent) = send.send(task) {
            drop(unsent);
            *left.0.lock().unwrap() -= 1;
        }
    }
    let mut waiting = left.0.lock().unwrap();
    while *waiting > 0 {
        waiting = left.1.wait(waiting).unwrap();
    }
    drop(waiting);
    results.into_inner().unwrap()
}
