//! Worker pools for scans and reads (`docs/architecture.md`, "Parallel
//! pipeline").
//!
//! `threads` counts every thread that works. With 1 (or 0) everything runs on
//! the calling thread and nothing is spawned, which is also the only mode on
//! WebAssembly. With N there are N workers, which take their items from the
//! shared input one at a time: the input is not a stage with a thread of its own.
//!
//! Where threads can't be started (WebAssembly), asking for more than one is
//! an error, not a panic.

use std::any::Any;
use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError, mpsc};

/// A caught panic's payload.
pub type Panic = Box<dyn Any + Send + 'static>;

/// The message of a caught panic, if it has one.
pub fn panic_message(panic: &Panic) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

/// One thread per core, or 1 where that can't be known (WebAssembly).
pub fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Work a worker does for the input before it takes an item, and while
/// another worker takes one, such as decoding the input ahead
/// ([`crate::decode::DecodeAhead::help`]); `false` when there is none.
pub type Helper = Arc<dyn Fn() -> bool + Send + Sync>;

/// Lock the input. With a helper, help first as long as there is something
/// to do (so that a source is decoded ahead before the worker that splits
/// it needs it, not by that worker), and again while another thread holds
/// the input.
fn lock_or_help<'a, T>(
    mutex: &'a Mutex<T>,
    helper: Option<&(dyn Fn() -> bool + Sync)>,
) -> MutexGuard<'a, T> {
    if let Some(helper) = helper {
        while helper() {}
        loop {
            match mutex.try_lock() {
                Ok(guard) => return guard,
                Err(TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => {
                    if !helper() {
                        break;
                    }
                }
            }
        }
    }
    lock(mutex)
}

fn spawn_error(error: std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), format!("starting a worker thread: {error}"))
}

/// Fold `items` on `threads` workers, each into an accumulator of its own
/// (`init`), and return the accumulators, one per worker. `step` returning
/// `Break` stops every worker after its current item.
///
/// The items are taken one at a time under a lock, so `items.next()` (such as
/// splitting the next chunk) runs on whichever worker is free. Before it
/// takes one, and while another worker holds the lock, a worker calls
/// `helper` until it has nothing to do. With
/// `threads <= 1`, all of it runs on the calling thread. A panic in a worker
/// is resumed on the calling thread once every worker has stopped.
pub fn fold<T, A>(
    threads: usize,
    name: &str,
    items: impl Iterator<Item = T> + Send,
    helper: Option<&(dyn Fn() -> bool + Sync)>,
    init: impl Fn() -> A + Sync,
    step: impl Fn(&mut A, T) -> ControlFlow<()> + Sync,
) -> std::io::Result<Vec<A>>
where
    T: Send,
    A: Send,
{
    if threads <= 1 {
        let mut acc = init();
        for item in items {
            if step(&mut acc, item).is_break() {
                break;
            }
        }
        return Ok(vec![acc]);
    }

    let items = Mutex::new(items);
    let stop = AtomicBool::new(false);
    let work = || {
        let mut acc = init();
        loop {
            let item = {
                let mut items = lock_or_help(&items, helper);
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                items.next()
            };
            let Some(item) = item else { break };
            if step(&mut acc, item).is_break() {
                stop.store(true, Ordering::Relaxed);
                break;
            }
        }
        acc
    };
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(threads);
        for index in 0..threads {
            let spawned = std::thread::Builder::new()
                .name(format!("{name}-{index}"))
                .spawn_scoped(scope, || {
                    catch_unwind(AssertUnwindSafe(work)).inspect_err(|_| {
                        stop.store(true, Ordering::Relaxed);
                    })
                });
            match spawned {
                Ok(worker) => workers.push(worker),
                Err(error) => {
                    // The workers already started stop at their next item.
                    stop.store(true, Ordering::Relaxed);
                    return Err(spawn_error(error));
                }
            }
        }
        let mut accs = Vec::with_capacity(workers.len());
        let mut panic = None;
        for worker in workers {
            match worker.join().unwrap_or_else(|panic| Err(panic)) {
                Ok(acc) => accs.push(acc),
                Err(caught) => {
                    panic.get_or_insert(caught);
                }
            }
        }
        if let Some(panic) = panic {
            resume_unwind(panic);
        }
        Ok(accs)
    })
}

/// How a [`ParallelMap`] runs.
#[derive(Clone)]
pub struct MapOptions {
    /// Threads that work; `<= 1` runs every item on the consumer's thread.
    pub threads: usize,
    /// Items taken from the input but not yet returned by
    /// [`ParallelMap::next`] (backpressure). At least `threads`.
    pub in_flight: usize,
    /// Return the results in input order (else in order of completion).
    pub preserve_order: bool,
    /// Worker thread names: `{name}-{index}`.
    pub name: String,
    /// Called by a worker before it takes an item, and while the input is
    /// taken by another.
    pub helper: Option<Helper>,
}

/// `f` applied to every item of an input, on [`MapOptions::threads`] workers,
/// as an iterator of results. A panic in `f` or in the input becomes an
/// `Err` in its place; the input is not read after one.
///
/// Dropping the map stops it: workers take no more items, and those waiting
/// for room are woken and end.
pub struct ParallelMap<R> {
    inner: Inner<R>,
}

enum Inner<R> {
    /// Each `next` takes one item and maps it on the consumer's thread.
    Inline(Box<dyn FnMut() -> Option<Result<R, Panic>> + Send>),
    Pool(Pool<R>),
    Done,
}

struct Pool<R> {
    shared: Arc<Shared>,
    results: mpsc::Receiver<(u64, Result<R, Panic>)>,
    /// Results that came ahead of `next` (in order only).
    pending: BTreeMap<u64, Result<R, Panic>>,
    next: u64,
    preserve_order: bool,
}

/// Tickets bound the items in flight: a worker takes one before it takes an
/// item, and the consumer returns it when it hands that item's result on.
struct Shared {
    tickets: Mutex<usize>,
    wake: Condvar,
    cancelled: AtomicBool,
}

impl Shared {
    /// `false` once the map is dropped.
    fn take_ticket(&self) -> bool {
        let mut free = lock(&self.tickets);
        loop {
            if self.cancelled.load(Ordering::Relaxed) {
                return false;
            }
            if *free > 0 {
                *free -= 1;
                return true;
            }
            free = self
                .wake
                .wait(free)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn return_ticket(&self) {
        *lock(&self.tickets) += 1;
        self.wake.notify_one();
    }

    fn cancel(&self) {
        // Under the lock, so that no worker misses the wake-up between its
        // check and its wait.
        let _tickets = lock(&self.tickets);
        self.cancelled.store(true, Ordering::Relaxed);
        self.wake.notify_all();
    }
}

/// The input as the workers share it.
struct Input<T> {
    items: Box<dyn Iterator<Item = T> + Send>,
    next_seq: u64,
    /// Ended, or panicked.
    done: bool,
}

impl<R: Send + 'static> ParallelMap<R> {
    /// Start the workers (none with `threads <= 1`). Fails only when a thread
    /// can't be started.
    pub fn new<T, F>(
        options: MapOptions,
        items: impl Iterator<Item = T> + Send + 'static,
        f: F,
    ) -> std::io::Result<Self>
    where
        T: Send + 'static,
        F: Fn(T) -> R + Send + Sync + 'static,
    {
        if options.threads <= 1 {
            let mut items = items;
            let mut done = false;
            let next = move || {
                if done {
                    return None;
                }
                match catch_unwind(AssertUnwindSafe(|| items.next())) {
                    Ok(Some(item)) => Some(catch_unwind(AssertUnwindSafe(|| f(item)))),
                    Ok(None) => {
                        done = true;
                        None
                    }
                    Err(panic) => {
                        done = true;
                        Some(Err(panic))
                    }
                }
            };
            return Ok(ParallelMap {
                inner: Inner::Inline(Box::new(next)),
            });
        }

        let shared = Arc::new(Shared {
            tickets: Mutex::new(options.in_flight.max(options.threads)),
            wake: Condvar::new(),
            cancelled: AtomicBool::new(false),
        });
        let input = Arc::new(Mutex::new(Input {
            items: Box::new(items),
            next_seq: 0,
            done: false,
        }));
        let f = Arc::new(f);
        let (sender, results) = mpsc::channel();
        // Built first, so that a failed spawn drops it and so cancels the
        // workers already started.
        let pool = Pool {
            shared: shared.clone(),
            results,
            pending: BTreeMap::new(),
            next: 0,
            preserve_order: options.preserve_order,
        };
        for index in 0..options.threads {
            let (shared, input, f, sender, helper) = (
                shared.clone(),
                input.clone(),
                f.clone(),
                sender.clone(),
                options.helper.clone(),
            );
            std::thread::Builder::new()
                .name(format!("{}-{index}", options.name))
                .spawn(move || work(&shared, &input, &*f, &sender, helper.as_deref()))
                .map_err(spawn_error)?;
        }
        Ok(ParallelMap {
            inner: Inner::Pool(pool),
        })
    }
}

/// A worker: take a ticket, then an item, map it, send the result.
fn work<T, R>(
    shared: &Shared,
    input: &Mutex<Input<T>>,
    f: &(impl Fn(T) -> R + ?Sized),
    sender: &mpsc::Sender<(u64, Result<R, Panic>)>,
    helper: Option<&(dyn Fn() -> bool + Send + Sync)>,
) {
    loop {
        if !shared.take_ticket() {
            return;
        }
        let (seq, item) = {
            let mut input = match helper {
                Some(helper) => lock_or_help(input, Some(helper)),
                None => lock(input),
            };
            if input.done {
                drop(input);
                shared.return_ticket();
                return;
            }
            match catch_unwind(AssertUnwindSafe(|| input.items.next())) {
                Ok(Some(item)) => {
                    let seq = input.next_seq;
                    input.next_seq += 1;
                    (seq, Ok(item))
                }
                Ok(None) => {
                    input.done = true;
                    drop(input);
                    shared.return_ticket();
                    return;
                }
                Err(panic) => {
                    input.done = true;
                    let seq = input.next_seq;
                    input.next_seq += 1;
                    (seq, Err(panic))
                }
            }
        };
        let result = item.and_then(|item| catch_unwind(AssertUnwindSafe(|| f(item))));
        // The consumer is gone.
        if sender.send((seq, result)).is_err() {
            return;
        }
    }
}

impl<R> Iterator for ParallelMap<R> {
    type Item = Result<R, Panic>;

    fn next(&mut self) -> Option<Self::Item> {
        let next = match &mut self.inner {
            Inner::Inline(next) => next(),
            Inner::Pool(pool) => pool.next(),
            Inner::Done => None,
        };
        if next.is_none() {
            self.inner = Inner::Done;
        }
        next
    }
}

impl<R> Pool<R> {
    fn next(&mut self) -> Option<Result<R, Panic>> {
        loop {
            if self.preserve_order
                && let Some(result) = self.pending.remove(&self.next)
            {
                self.next += 1;
                self.shared.return_ticket();
                return Some(result);
            }
            // Every worker has ended: the input is used up (all results are
            // in, without gaps) or the map was dropped.
            let (seq, result) = self.results.recv().ok()?;
            if self.preserve_order {
                self.pending.insert(seq, result);
            } else {
                self.shared.return_ticket();
                return Some(result);
            }
        }
    }
}

impl<R> Drop for Pool<R> {
    fn drop(&mut self) {
        self.shared.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn options(threads: usize, preserve_order: bool) -> MapOptions {
        MapOptions {
            threads,
            in_flight: threads + 2,
            preserve_order,
            name: "test".into(),
            helper: None,
        }
    }

    #[test]
    fn map_keeps_input_order_on_any_number_of_threads() {
        for threads in [0, 1, 2, 7] {
            let map = ParallelMap::new(options(threads, true), 0..200u64, |i| {
                // Later items finish first.
                std::thread::sleep(std::time::Duration::from_micros(200 - i));
                i * 2
            })
            .unwrap();
            let out: Vec<u64> = map.map(|r| r.unwrap()).collect();
            assert_eq!(
                out,
                (0..200).map(|i| i * 2).collect::<Vec<_>>(),
                "{threads}"
            );
        }
    }

    #[test]
    fn map_without_order_returns_every_result() {
        let map = ParallelMap::new(options(4, false), 0..100u64, |i| i).unwrap();
        let mut out: Vec<u64> = map.map(|r| r.unwrap()).collect();
        out.sort();
        assert_eq!(out, (0..100).collect::<Vec<_>>());
    }

    #[test]
    fn one_thread_maps_on_the_calling_thread() {
        let caller = std::thread::current().id();
        let map = ParallelMap::new(options(1, true), 0..3, move |_| {
            std::thread::current().id() == caller
        })
        .unwrap();
        assert!(map.into_iter().all(|on_caller| on_caller.unwrap()));
    }

    #[test]
    fn map_bounds_the_items_in_flight() {
        // A consumer that takes nothing: the workers stop after `in_flight` items.
        let taken = Arc::new(AtomicUsize::new(0));
        let counter = taken.clone();
        let items = (0..1000).inspect(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let map = ParallelMap::new(options(3, true), items, |i| i).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(taken.load(Ordering::SeqCst), 5);
        drop(map);
    }

    #[test]
    fn a_panic_becomes_an_error_in_its_place() {
        for threads in [1, 3] {
            let map = ParallelMap::new(options(threads, true), 0..10, |i| {
                if i == 4 {
                    panic!("item four");
                }
                i
            })
            .unwrap();
            let out: Vec<Result<i32, String>> =
                map.map(|r| r.map_err(|p| panic_message(&p))).collect();
            assert_eq!(out[3], Ok(3));
            assert_eq!(out[4], Err("item four".to_string()));
            assert_eq!(out.len(), 10);
        }
    }

    #[test]
    fn a_panic_in_the_input_ends_the_map() {
        let items = (0..10).map(|i| if i == 6 { panic!("input six") } else { i });
        let map = ParallelMap::new(options(3, true), items, |i| i).unwrap();
        let out: Vec<Result<i32, String>> = map.map(|r| r.map_err(|p| panic_message(&p))).collect();
        assert_eq!(out.len(), 7);
        assert_eq!(out[6], Err("input six".to_string()));
    }

    #[test]
    fn dropping_the_map_ends_its_workers() {
        let live = Arc::new(AtomicUsize::new(0));
        let mut map = {
            let live = live.clone();
            ParallelMap::new(options(4, true), 0.., move |i: u64| {
                live.fetch_add(1, Ordering::SeqCst);
                i
            })
            .unwrap()
        };
        assert_eq!(map.next().map(|r| r.unwrap()), Some(0));
        drop(map);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let after_drop = live.load(Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(live.load(Ordering::SeqCst), after_drop);
    }

    #[test]
    fn fold_visits_every_item_once() {
        for threads in [1, 4] {
            let sums = fold(
                threads,
                "test",
                1..=1000u64,
                None,
                || 0u64,
                |sum, i| {
                    *sum += i;
                    ControlFlow::Continue(())
                },
            )
            .unwrap();
            assert_eq!(sums.len(), threads);
            assert_eq!(sums.iter().sum::<u64>(), 500_500);
        }
    }

    #[test]
    fn fold_stops_after_a_break() {
        let taken = AtomicUsize::new(0);
        let items = (0..10_000).inspect(|_| {
            taken.fetch_add(1, Ordering::SeqCst);
        });
        fold(
            4,
            "test",
            items,
            None,
            || (),
            |_, i| {
                if i == 10 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            },
        )
        .unwrap();
        assert!(taken.load(Ordering::SeqCst) < 100);
    }

    #[test]
    fn one_thread_folds_on_the_calling_thread() {
        let caller = std::thread::current().id();
        let on_caller = fold(
            1,
            "test",
            0..3,
            None,
            || true,
            |all, _| {
                *all &= std::thread::current().id() == caller;
                ControlFlow::Continue(())
            },
        )
        .unwrap();
        assert_eq!(on_caller, [true]);
    }
}
