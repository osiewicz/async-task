use std::cell::Cell;
use std::future::Future;
use std::panic::catch_unwind;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::thread;
use std::time::Duration;

use async_task::Task;
use atomic_waker::AtomicWaker;
use crossbeam::channel;
use easy_parallel::Parallel;
use futures_lite::future;

// Creates a future with event counters.
//
// Usage: `future!(f, waker, POLL, DROP)`
//
// The future `f` always sleeps for 200 ms, and panics the second time it is polled.
// When it gets polled, `POLL` is incremented.
// When it gets dropped, `DROP` is incremented.
//
// Every time the future is run, it stores the waker into a global variable.
// This waker can be extracted using the `waker` function.
macro_rules! future {
    ($name:pat, $waker:pat, $poll:ident, $drop:ident) => {
        static $poll: AtomicUsize = AtomicUsize::new(0);
        static $drop: AtomicUsize = AtomicUsize::new(0);
        static WAKER: AtomicWaker = AtomicWaker::new();

        let ($name, $waker) = {
            struct Fut(Cell<bool>, Box<i32>);

            impl Future for Fut {
                type Output = ();

                fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                    WAKER.register(cx.waker());
                    $poll.fetch_add(1, Ordering::SeqCst);
                    thread::sleep(ms(400));

                    if self.0.get() {
                        panic!()
                    } else {
                        self.0.set(true);
                        Poll::Pending
                    }
                }
            }

            impl Drop for Fut {
                fn drop(&mut self) {
                    $drop.fetch_add(1, Ordering::SeqCst);
                }
            }

            (Fut(Cell::new(false), Box::new(0)), || WAKER.take().unwrap())
        };
    };
}

// Creates a schedule function with event counters.
//
// Usage: `schedule!(s, chan, SCHED, DROP)`
//
// The schedule function `s` pushes the task into `chan`.
// When it gets invoked, `SCHED` is incremented.
// When it gets dropped, `DROP` is incremented.
//
// Receiver `chan` extracts the task when it is scheduled.
macro_rules! schedule {
    ($name:pat, $chan:pat, $sched:ident, $drop:ident) => {
        static $drop: AtomicUsize = AtomicUsize::new(0);
        static $sched: AtomicUsize = AtomicUsize::new(0);

        let ($name, $chan) = {
            let (s, r) = channel::unbounded();

            struct Guard(Box<i32>);

            impl Drop for Guard {
                fn drop(&mut self) {
                    $drop.fetch_add(1, Ordering::SeqCst);
                }
            }

            let guard = Guard(Box::new(0));
            let sched = move |task: Task| {
                &guard;
                $sched.fetch_add(1, Ordering::SeqCst);
                s.send(task).unwrap();
            };

            (sched, r)
        };
    };
}

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

fn try_await<T>(f: impl Future<Output = T>) -> Option<T> {
    future::block_on(future::poll_once(f))
}

#[test]
fn wake_during_run() {
    future!(f, waker, POLL, DROP_F);
    schedule!(s, chan, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    task.run();
    let w = waker();
    w.wake_by_ref();
    let task = chan.recv().unwrap();

    Parallel::new()
        .add(|| {
            assert!(catch_unwind(|| task.run()).is_err());
            drop(waker());
            assert_eq!(POLL.load(Ordering::SeqCst), 2);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
            assert_eq!(chan.len(), 0);
        })
        .add(|| {
            thread::sleep(ms(200));

            w.wake();
            handle.detach();
            assert_eq!(POLL.load(Ordering::SeqCst), 2);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
            assert_eq!(chan.len(), 0);

            thread::sleep(ms(400));

            assert_eq!(POLL.load(Ordering::SeqCst), 2);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
            assert_eq!(chan.len(), 0);
        })
        .run();
}

#[test]
fn cancel_during_run() {
    future!(f, waker, POLL, DROP_F);
    schedule!(s, chan, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    task.run();
    let w = waker();
    w.wake();
    let task = chan.recv().unwrap();

    Parallel::new()
        .add(|| {
            assert!(catch_unwind(|| task.run()).is_err());
            drop(waker());
            assert_eq!(POLL.load(Ordering::SeqCst), 2);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
            assert_eq!(chan.len(), 0);
        })
        .add(|| {
            thread::sleep(ms(200));

            drop(handle);
            assert_eq!(POLL.load(Ordering::SeqCst), 2);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
            assert_eq!(chan.len(), 0);

            thread::sleep(ms(400));

            assert_eq!(POLL.load(Ordering::SeqCst), 2);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
            assert_eq!(chan.len(), 0);
        })
        .run();
}

#[test]
fn wake_and_cancel_during_run() {
    future!(f, waker, POLL, DROP_F);
    schedule!(s, chan, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    task.run();
    let w = waker();
    w.wake_by_ref();
    let task = chan.recv().unwrap();

    Parallel::new()
        .add(|| {
            assert!(catch_unwind(|| task.run()).is_err());
            drop(waker());
            assert_eq!(POLL.load(Ordering::SeqCst), 2);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
            assert_eq!(chan.len(), 0);
        })
        .add(|| {
            thread::sleep(ms(200));

            w.wake();
            assert_eq!(POLL.load(Ordering::SeqCst), 2);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
            assert_eq!(chan.len(), 0);

            drop(handle);
            assert_eq!(POLL.load(Ordering::SeqCst), 2);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
            assert_eq!(chan.len(), 0);

            thread::sleep(ms(400));

            assert_eq!(POLL.load(Ordering::SeqCst), 2);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
            assert_eq!(chan.len(), 0);
        })
        .run();
}

#[test]
fn cancel_and_wake_during_run() {
    future!(f, waker, POLL, DROP_F);
    schedule!(s, chan, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    task.run();
    let w = waker();
    w.wake_by_ref();
    let task = chan.recv().unwrap();

    Parallel::new()
        .add(|| {
            assert!(catch_unwind(|| task.run()).is_err());
            drop(waker());
            assert_eq!(POLL.load(Ordering::SeqCst), 2);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
            assert_eq!(chan.len(), 0);
        })
        .add(|| {
            thread::sleep(ms(200));

            drop(handle);
            assert_eq!(POLL.load(Ordering::SeqCst), 2);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
            assert_eq!(chan.len(), 0);

            w.wake();
            assert_eq!(POLL.load(Ordering::SeqCst), 2);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 0);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 0);
            assert_eq!(chan.len(), 0);

            thread::sleep(ms(400));

            assert_eq!(POLL.load(Ordering::SeqCst), 2);
            assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
            assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
            assert_eq!(chan.len(), 0);
        })
        .run();
}

#[test]
fn panic_and_poll() {
    future!(f, waker, POLL, DROP_F);
    schedule!(s, chan, SCHEDULE, DROP_S);
    let (task, handle) = async_task::spawn(f, s);

    task.run();
    waker().wake();
    assert_eq!(POLL.load(Ordering::SeqCst), 1);
    assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 0);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 0);

    let mut handle = handle;
    assert!(try_await(&mut handle).is_none());

    let task = chan.recv().unwrap();
    assert!(catch_unwind(|| task.run()).is_err());
    assert_eq!(POLL.load(Ordering::SeqCst), 2);
    assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 0);

    assert!(try_await(&mut handle).is_some());
    assert_eq!(POLL.load(Ordering::SeqCst), 2);
    assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 0);

    drop(waker());
    drop(handle);
    assert_eq!(POLL.load(Ordering::SeqCst), 2);
    assert_eq!(SCHEDULE.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_F.load(Ordering::SeqCst), 1);
    assert_eq!(DROP_S.load(Ordering::SeqCst), 1);
}
