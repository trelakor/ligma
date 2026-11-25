//! Threads kept to encode units on, as starting them for each tile took
//! half a millisecond of it

use std::cell::Cell;
use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{GetCurrentThread, GetCurrentThreadId, GetThreadPriority, OpenThread, SetThreadPriority, THREAD_PRIORITY, THREAD_PRIORITY_LOWEST, THREAD_SET_LIMITED_INFORMATION};

type Task = Box<dyn FnOnce() + Send + 'static>;

/// The most threads `run` takes at once, the caller's among them
pub const MOST: usize = 8;

static POOL: OnceLock<Mutex<Sender<Task>>> = OnceLock::new();

thread_local! {
    /// A pool thread's handle to itself, which the thread it works for
    /// raises it by
    static ME: Cell<isize> = const { Cell::new(0) };
}

fn pool() -> &'static Mutex<Sender<Task>> {
    POOL.get_or_init(|| {
        let (send, tasks) = channel::<Task>();
        let tasks: Arc<Mutex<Receiver<Task>>> = Arc::new(Mutex::new(tasks));
        for _ in 1..MOST {
            let tasks = tasks.clone();
            let _ = std::thread::Builder::new().name("ligma encoder".into()).spawn(move || {
                if let Ok(me) = unsafe { OpenThread(THREAD_SET_LIMITED_INFORMATION, false, GetCurrentThreadId()) } {
                    ME.with(|cell| cell.set(me.0 as isize));
                }
                loop {
                    let task = tasks.lock().unwrap().recv();
                    match task {
                        Ok(task) => task(),
                        Err(_) => return,
                    }
                }
            });
        }
        Mutex::new(send)
    })
}

enum Status {
    Waiting,
    /// On the pool thread with this handle
    Running(isize),
    Done,
    LetGo,
}

struct Shared {
    status: Mutex<Vec<Status>>,
    done: Condvar,
}

/// What `work` returns run on `threads` threads at once: the caller's and
/// the pool's. `work` takes what's left of work shared between them, so
/// once the caller's run out of it, pool threads yet to begin are let go.
/// One that panics returns nothing.
///
/// Pool threads run behind every other thread, the browser's above all,
/// whose frames Figma's wait on: at the same priority they halved Figma's
/// frame rate as new things came into view. Those the caller waits on run at
/// its priority till done, as with the browser keeping every core busy as a
/// file opened, they had none for a second at a time and held the caller up
/// all that while
pub fn run<R: Send>(threads: usize, work: &(dyn Fn() -> R + Sync)) -> Vec<R> {
    if threads == 0 {
        return Vec::new();
    }
    let results: Mutex<Vec<R>> = Mutex::new(Vec::with_capacity(threads));
    let helpers = threads.saturating_sub(1);
    // Each task's own, as a task let go runs after this has gone on
    let shared = Arc::new(Shared { status: Mutex::new((0..helpers).map(|_| Status::Waiting).collect()), done: Condvar::new() });
    let results_at = &results;
    if helpers > 0 {
        let send = pool().lock().unwrap().clone();
        for index in 0..helpers {
            let shared_at = shared.clone();
            let task: Box<dyn FnOnce() + Send + '_> = Box::new(move || {
                // Lowered before it's taken, so the caller raising it after
                // holds
                unsafe {
                    let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST);
                }
                {
                    let mut status = shared_at.status.lock().unwrap();
                    if !matches!(status[index], Status::Waiting) {
                        return;
                    }
                    status[index] = Status::Running(ME.with(Cell::get));
                }
                if let Ok(out) = std::panic::catch_unwind(AssertUnwindSafe(work)) {
                    results_at.lock().unwrap().push(out);
                }
                shared_at.status.lock().unwrap()[index] = Status::Done;
                shared_at.done.notify_all();
            });
            // Safety: this waits below till every task that's begun is done,
            // and those let go touch nothing they borrow, so what they
            // borrow outlives their use of it
            let task: Task = unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + '_>, Task>(task) };
            if send.send(task).is_err() {
                shared.status.lock().unwrap()[index] = Status::LetGo;
            }
        }
    }
    if let Ok(out) = std::panic::catch_unwind(AssertUnwindSafe(work)) {
        results_at.lock().unwrap().push(out);
    }
    let mut status = shared.status.lock().unwrap();
    let priority = THREAD_PRIORITY(unsafe { GetThreadPriority(GetCurrentThread()) });
    for task in status.iter_mut() {
        match *task {
            Status::Waiting => *task = Status::LetGo,
            Status::Running(thread) if thread != 0 => unsafe {
                let _ = SetThreadPriority(HANDLE(thread as *mut _), priority);
            },
            _ => {}
        }
    }
    while status.iter().any(|task| matches!(task, Status::Running(_))) {
        status = shared.done.wait(status).unwrap();
    }
    drop(status);
    results.into_inner().unwrap()
}
