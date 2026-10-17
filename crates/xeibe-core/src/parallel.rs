//! Worker pools for scans and reads (`docs/architecture.md`, "Parallel
//! pipeline").
//!
//! `threads` counts every thread that works. With 1 (or 0) everything runs on
//! the calling thread and nothing is spawned, which is also the only mode on
//! WebAssembly. With N there are N workers, which take their items from the
//! shared input one at a time: the input is not a stage with a thread of its own.
//!
//! The input is a run of streams ([`Streams`]), such as the sources of a
//! multi-file input, each split into chunks. Each open stream has a lock of
//! its own. A worker takes its next item from the oldest open stream that no
//! other worker holds, and opens the next stream when every open one is
//! held, up to `max_open`. So several sources are split (and decompressed)
//! side by side when one can't keep the workers busy.
//!
//! In order ([`MapOptions::preserve_order`]), only one stream is open at a
//! time: items are then taken in input order, which is what keeps a bounded
//! reorder buffer from waiting forever (see [`ParallelMap`]).
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

/// Streams open at once on `threads` workers: one stream (a source being
/// split) keeps several workers busy parsing, so half as many as there are
/// workers, and at most 8 (each holds buffers of a few MB).
pub fn max_open_streams(threads: usize) -> usize {
    (threads / 2).clamp(1, 8)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Work a worker does for the input before it takes an item, and while
/// every open stream is held by other workers, such as decoding the input
/// ahead ([`crate::decode::DecodeAhead::help`]); `false` when there is none.
pub type Helper = Arc<dyn Fn() -> bool + Send + Sync>;

fn spawn_error(error: std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), format!("starting a worker thread: {error}"))
}

/// An input made of streams, each a run of items in order: the sources of a
/// scan or read, split into chunks. The order of the input is the streams in
/// order, each with its items in order.
pub trait Streams: Send {
    type Item: Send;
    type Stream: Iterator<Item = Self::Item> + Send;

    /// The next stream; `None` at the end. Runs under a lock that all workers
    /// share, so it should be quick: the stream does its own opening on its
    /// first `next`, under its own lock.
    fn next_stream(&mut self) -> Option<Self::Stream>;

    /// An item after which nothing more is taken from the input, such as an
    /// error. In order, it comes after every item before it.
    fn ends_input(_item: &Self::Item) -> bool {
        false
    }

    /// A last item, once every stream has ended (and none ended the input).
    fn finish(&mut self) -> Option<Self::Item> {
        None
    }
}

/// An iterator as an input of one stream.
pub struct One<I>(Option<I>);

impl<I> One<I> {
    pub fn new(items: I) -> Self {
        One(Some(items))
    }
}

impl<I> Streams for One<I>
where
    I: Iterator + Send,
    I::Item: Send,
{
    type Item = I::Item;
    type Stream = I;

    fn next_stream(&mut self) -> Option<I> {
        self.0.take()
    }
}

/// The items of an input in order, on the calling thread. A panic in the
/// input becomes an `Err` and ends it.
///
/// [`Sequential::rest`] hands on what is left after some items were taken
/// this way, such as the sample a read takes before its workers start.
pub struct Sequential<S: Streams> {
    streams: S,
    current: Option<S::Stream>,
    done: bool,
}

impl<S: Streams> Sequential<S> {
    pub fn new(streams: S) -> Self {
        Sequential {
            streams,
            current: None,
            done: false,
        }
    }

    /// The rest of the input, with `taken` (items taken from it, kept for
    /// later) first.
    pub fn rest(self, taken: Vec<S::Item>) -> Rest<S> {
        let first = (!taken.is_empty() || self.current.is_some()).then(|| Resumed {
            taken: taken.into_iter(),
            stream: self.current,
        });
        Rest {
            streams: self.streams,
            first,
            done: self.done,
        }
    }
}

impl<S: Streams> Iterator for Sequential<S> {
    type Item = Result<S::Item, Panic>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.done {
            if let Some(stream) = &mut self.current {
                match catch_unwind(AssertUnwindSafe(|| stream.next())) {
                    Ok(Some(item)) => {
                        self.done = S::ends_input(&item);
                        return Some(Ok(item));
                    }
                    Ok(None) => self.current = None,
                    Err(panic) => {
                        self.done = true;
                        return Some(Err(panic));
                    }
                }
            }
            match catch_unwind(AssertUnwindSafe(|| self.streams.next_stream())) {
                Ok(Some(stream)) => self.current = Some(stream),
                Ok(None) => {
                    self.done = true;
                    return catch_unwind(AssertUnwindSafe(|| self.streams.finish())).transpose();
                }
                Err(panic) => {
                    self.done = true;
                    return Some(Err(panic));
                }
            }
        }
        None
    }
}

/// What is left of an input after [`Sequential`] took some of it.
pub struct Rest<S: Streams> {
    streams: S,
    /// The items taken, then the rest of the stream they were taken from.
    first: Option<Resumed<S::Stream>>,
    /// The input had ended (its last item was taken).
    done: bool,
}

/// A stream of [`Rest`]: items taken earlier, then the stream.
pub struct Resumed<St: Iterator> {
    taken: std::vec::IntoIter<St::Item>,
    stream: Option<St>,
}

impl<St: Iterator> Iterator for Resumed<St> {
    type Item = St::Item;

    fn next(&mut self) -> Option<St::Item> {
        self.taken.next().or_else(|| self.stream.as_mut()?.next())
    }
}

impl<S> Streams for Rest<S>
where
    S: Streams,
    S::Stream: Send,
{
    type Item = S::Item;
    type Stream = Resumed<S::Stream>;

    fn next_stream(&mut self) -> Option<Self::Stream> {
        if let Some(first) = self.first.take() {
            return Some(first);
        }
        if self.done {
            return None;
        }
        let stream = self.streams.next_stream()?;
        Some(Resumed {
            taken: Vec::new().into_iter(),
            stream: Some(stream),
        })
    }

    fn ends_input(item: &Self::Item) -> bool {
        S::ends_input(item)
    }

    fn finish(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        self.streams.finish()
    }
}

/// An item and its sequence number (the order in which items were taken),
/// or a panic in the input in its place.
type Taken<T> = (u64, Result<T, Panic>);

/// An input as the workers share it.
struct Input<S: Streams> {
    list: Mutex<StreamList<S>>,
    /// The streams opened and not ended, oldest first.
    open: Mutex<Vec<Arc<OpenStream<S::Stream>>>>,
    /// No stream is left to open.
    exhausted: AtomicBool,
    /// An item ended the input, or the input panicked: nothing more is
    /// taken. Set with [`Numbers::stopped`], read without its lock.
    stopped: AtomicBool,
    numbers: Mutex<Numbers>,
    max_open: usize,
}

/// The numbering of the items taken. An item and the stop get their numbers
/// under one lock, so no item taken after the stop is numbered: in order,
/// everything before an error comes out before it, and nothing after.
struct Numbers {
    next: u64,
    stopped: bool,
}

struct StreamList<S> {
    streams: S,
    started: usize,
    ended: usize,
    finished: bool,
}

struct OpenStream<St> {
    state: Mutex<StreamState<St>>,
}

struct StreamState<St> {
    stream: St,
    done: bool,
}

enum Pulled<T> {
    Item(Taken<T>),
    /// The stream had ended, with the input's last item if it was the last
    /// stream.
    Ended(Option<Taken<T>>),
}

// Locks are taken in this order, never the other way: a stream's state, the
// list of streams, the open streams, the numbers.
impl<S: Streams> Input<S> {
    fn new(streams: S, max_open: usize) -> Self {
        Input {
            list: Mutex::new(StreamList {
                streams,
                started: 0,
                ended: 0,
                finished: false,
            }),
            open: Mutex::new(Vec::new()),
            exhausted: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            numbers: Mutex::new(Numbers {
                next: 0,
                stopped: false,
            }),
            max_open: max_open.max(1),
        }
    }

    /// The next item for a worker; `None` once the input is used up or
    /// stopped.
    fn take(&self, helper: Option<&(dyn Fn() -> bool + Sync)>) -> Option<Taken<S::Item>> {
        // Decode ahead first, so that the worker that splits a source mostly
        // finds it decoded.
        loop {
            match help(helper) {
                Ok(true) => {}
                Ok(false) => break,
                Err(panic) => return self.fail(panic),
            }
        }
        loop {
            if self.stopped.load(Ordering::Acquire) {
                return None;
            }
            // Read before the open streams: once it is set, every stream
            // there will be is in the list.
            let exhausted = self.exhausted.load(Ordering::Acquire);
            let open: Vec<_> = lock(&self.open).clone();
            let Some(head) = open.first() else {
                if exhausted {
                    return None;
                }
                if let Some(last) = self.open_next() {
                    return Some(last);
                }
                continue;
            };
            let mut ended = false;
            for stream in &open {
                let state = match stream.state.try_lock() {
                    Ok(state) => state,
                    Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                    Err(TryLockError::WouldBlock) => continue,
                };
                match self.pull(stream, state) {
                    Pulled::Item(taken) | Pulled::Ended(Some(taken)) => return Some(taken),
                    Pulled::Ended(None) => {
                        ended = true;
                        break;
                    }
                }
            }
            if ended {
                continue;
            }
            // Every open stream is held by another worker.
            if !exhausted && open.len() < self.max_open {
                if let Some(last) = self.open_next() {
                    return Some(last);
                }
                continue;
            }
            match help(helper) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(panic) => return self.fail(panic),
            }
            match self.pull(head, lock(&head.state)) {
                Pulled::Item(taken) | Pulled::Ended(Some(taken)) => return Some(taken),
                Pulled::Ended(None) => {}
            }
        }
    }

    /// Add the next stream to the open ones. Returns the input's last item
    /// if there was no stream left and that ended the input, or a panic.
    fn open_next(&self) -> Option<Taken<S::Item>> {
        let mut list = lock(&self.list);
        // Another worker opened one meanwhile, or found the end.
        if self.exhausted.load(Ordering::Acquire) || lock(&self.open).len() >= self.max_open {
            return None;
        }
        match catch_unwind(AssertUnwindSafe(|| list.streams.next_stream())) {
            Ok(Some(stream)) => {
                list.started += 1;
                lock(&self.open).push(Arc::new(OpenStream {
                    state: Mutex::new(StreamState {
                        stream,
                        done: false,
                    }),
                }));
                None
            }
            Ok(None) => {
                self.exhausted.store(true, Ordering::Release);
                self.finish(&mut list)
            }
            Err(panic) => self.fail(panic),
        }
    }

    /// The input's last item, once every stream has ended.
    fn finish(&self, list: &mut StreamList<S>) -> Option<Taken<S::Item>> {
        if !self.exhausted.load(Ordering::Acquire)
            || list.ended < list.started
            || list.finished
            || self.stopped.load(Ordering::Acquire)
        {
            return None;
        }
        list.finished = true;
        let last = catch_unwind(AssertUnwindSafe(|| list.streams.finish())).transpose()?;
        Some((self.number(false)?, last))
    }

    /// The next item of a stream this worker holds.
    fn pull(
        &self,
        stream: &Arc<OpenStream<S::Stream>>,
        mut state: MutexGuard<'_, StreamState<S::Stream>>,
    ) -> Pulled<S::Item> {
        if state.done {
            return Pulled::Ended(None);
        }
        match catch_unwind(AssertUnwindSafe(|| state.stream.next())) {
            Ok(Some(item)) => {
                let ends = S::ends_input(&item);
                // Numbered while the stream is held: in order (one stream
                // open at a time), the numbers follow the input.
                let Some(seq) = self.number(ends) else {
                    // Taken after the input was stopped.
                    return Pulled::Ended(None);
                };
                state.done |= ends;
                Pulled::Item((seq, Ok(item)))
            }
            Ok(None) => {
                state.done = true;
                // Before the stream is let go, so that no worker finds it
                // open and ended.
                lock(&self.open).retain(|open| !Arc::ptr_eq(open, stream));
                drop(state);
                let mut list = lock(&self.list);
                list.ended += 1;
                Pulled::Ended(self.finish(&mut list))
            }
            Err(panic) => {
                state.done = true;
                match self.number(true) {
                    Some(seq) => Pulled::Item((seq, Err(panic))),
                    None => Pulled::Ended(None),
                }
            }
        }
    }

    /// A panic outside a stream (in the input's list, or in the helper):
    /// it ends the input, after the items taken so far.
    fn fail(&self, panic: Panic) -> Option<Taken<S::Item>> {
        Some((self.number(true)?, Err(panic)))
    }

    /// The next item's number; `ends`: nothing is taken after it. `None`
    /// once the input was stopped.
    fn number(&self, ends: bool) -> Option<u64> {
        let mut numbers = lock(&self.numbers);
        if numbers.stopped {
            return None;
        }
        let seq = numbers.next;
        numbers.next += 1;
        if ends {
            numbers.stopped = true;
            self.stopped.store(true, Ordering::Release);
            self.exhausted.store(true, Ordering::Release);
        }
        Some(seq)
    }
}

fn help(helper: Option<&(dyn Fn() -> bool + Sync)>) -> Result<bool, Panic> {
    match helper {
        Some(helper) => catch_unwind(AssertUnwindSafe(helper)),
        None => Ok(false),
    }
}

/// Fold the items of `input` on `threads` workers, each into an accumulator
/// of its own (`init`), and return the accumulators, one per worker. `step`
/// returning `Break` stops every worker after its current item.
///
/// The items are taken from up to `max_open` streams at once (see the module
/// docs), so taking an item (such as splitting the next chunk) runs on
/// whichever worker is free. Before it takes one, and while other workers
/// hold every open stream, a worker calls `helper` until it has nothing to
/// do. With `threads <= 1`, all of it runs on the calling thread, one stream
/// after another. A panic is resumed on the calling thread once every
/// worker has stopped.
pub fn fold<S, A>(
    threads: usize,
    max_open: usize,
    name: &str,
    input: S,
    helper: Option<&(dyn Fn() -> bool + Sync)>,
    init: impl Fn() -> A + Sync,
    step: impl Fn(&mut A, S::Item) -> ControlFlow<()> + Sync,
) -> std::io::Result<Vec<A>>
where
    S: Streams,
    A: Send,
{
    if threads <= 1 {
        let mut acc = init();
        for item in Sequential::new(input) {
            let item = item.unwrap_or_else(|panic| resume_unwind(panic));
            if step(&mut acc, item).is_break() {
                break;
            }
        }
        return Ok(vec![acc]);
    }

    let input = Input::new(input, max_open);
    let stop = AtomicBool::new(false);
    let work = || {
        let mut acc = init();
        while !stop.load(Ordering::Relaxed) {
            let Some((_, item)) = input.take(helper) else {
                break;
            };
            let item = item.unwrap_or_else(|panic| resume_unwind(panic));
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
    /// Streams of the input open at once ([`max_open_streams`]); 1 in order.
    pub max_open: usize,
    /// Return the results in input order (else in order of completion).
    pub preserve_order: bool,
    /// Worker thread names: `{name}-{index}`.
    pub name: String,
    /// Called by a worker before it takes an item, and while other workers
    /// hold every open stream.
    pub helper: Option<Helper>,
}

/// `f` applied to every item of an input, on [`MapOptions::threads`] workers,
/// as an iterator of results. A panic in `f` becomes an `Err` in its place;
/// one in the input (or the helper) an `Err` after which the input is not
/// read.
///
/// Dropping the map stops it: workers take no more items, and those waiting
/// for room are woken and end.
///
/// At most `in_flight` items are between the input and the consumer: a
/// worker takes a ticket before it takes an item, and the consumer returns it
/// when it hands that item's result on. In order this can't wait forever,
/// because only one stream is open at a time: items are taken in input
/// order, so the result the consumer waits for belongs to an item a worker
/// has already taken. Without order, the consumer returns each ticket as the
/// result comes in.
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

impl<R: Send + 'static> ParallelMap<R> {
    /// [`ParallelMap::streams`] over an input of one stream.
    pub fn new<T, F>(
        options: MapOptions,
        items: impl Iterator<Item = T> + Send + 'static,
        f: F,
    ) -> std::io::Result<Self>
    where
        T: Send + 'static,
        F: Fn(T) -> R + Send + Sync + 'static,
    {
        Self::streams(options, One::new(items), f)
    }

    /// Start the workers (none with `threads <= 1`). Fails only when a thread
    /// can't be started.
    pub fn streams<S, F>(options: MapOptions, input: S, f: F) -> std::io::Result<Self>
    where
        S: Streams + 'static,
        S::Item: 'static,
        S::Stream: 'static,
        F: Fn(S::Item) -> R + Send + Sync + 'static,
    {
        if options.threads <= 1 {
            let mut items = Sequential::new(input);
            let next = move || {
                let item = items.next()?;
                Some(item.and_then(|item| catch_unwind(AssertUnwindSafe(|| f(item)))))
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
        // In order, one stream at a time (see above).
        let max_open = if options.preserve_order {
            1
        } else {
            options.max_open
        };
        let input = Arc::new(Input::new(input, max_open));
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
fn work<S: Streams, R>(
    shared: &Shared,
    input: &Input<S>,
    f: &(impl Fn(S::Item) -> R + ?Sized),
    sender: &mpsc::Sender<(u64, Result<R, Panic>)>,
    helper: Option<&(dyn Fn() -> bool + Send + Sync)>,
) {
    let helper = helper.map(|helper| helper as &(dyn Fn() -> bool + Sync));
    loop {
        if !shared.take_ticket() {
            return;
        }
        let Some((seq, item)) = input.take(helper) else {
            shared.return_ticket();
            return;
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
            // in, without gaps up to where it ended) or the map was dropped.
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
    use std::time::Duration;

    fn options(threads: usize, preserve_order: bool) -> MapOptions {
        MapOptions {
            threads,
            in_flight: threads + 2,
            max_open: 3,
            preserve_order,
            name: "test".into(),
            helper: None,
        }
    }

    /// Streams of the given lengths; item `(stream, i)`, each split slowly.
    /// Records how many streams were being split at the same time.
    struct Lengths {
        lengths: Vec<u64>,
        next: usize,
        split: Duration,
        /// (streams being split now, the most at once)
        splitting: Arc<Mutex<(usize, usize)>>,
    }

    impl Lengths {
        fn new(lengths: &[u64]) -> Self {
            Lengths {
                lengths: lengths.to_vec(),
                next: 0,
                split: Duration::from_micros(100),
                splitting: Arc::default(),
            }
        }

        fn items(&self) -> Vec<(usize, u64)> {
            let mut items = Vec::new();
            for (stream, &len) in self.lengths.iter().enumerate() {
                items.extend((0..len).map(|i| (stream, i)));
            }
            items
        }
    }

    impl Streams for Lengths {
        type Item = (usize, u64);
        type Stream = Box<dyn Iterator<Item = (usize, u64)> + Send>;

        fn next_stream(&mut self) -> Option<Self::Stream> {
            let stream = self.next;
            let len = *self.lengths.get(stream)?;
            self.next += 1;
            let (split, splitting) = (self.split, self.splitting.clone());
            Some(Box::new((0..len).map(move |i| {
                {
                    let mut s = lock(&splitting);
                    s.0 += 1;
                    s.1 = s.1.max(s.0);
                }
                std::thread::sleep(split);
                lock(&splitting).0 -= 1;
                (stream, i)
            })))
        }
    }

    #[test]
    fn map_keeps_input_order_on_any_number_of_threads() {
        for threads in [0, 1, 2, 7] {
            let map = ParallelMap::new(options(threads, true), 0..200u64, |i| {
                // Later items finish first.
                std::thread::sleep(Duration::from_micros(200 - i));
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
    fn in_order_streams_are_split_one_at_a_time() {
        // Empty streams too, and a long one first.
        let lengths = [40, 0, 3, 1, 0, 25, 7];
        for threads in [1, 2, 6] {
            let input = Lengths::new(&lengths);
            let (expected, splitting) = (input.items(), input.splitting.clone());
            let map = ParallelMap::streams(
                MapOptions {
                    max_open: 4,
                    ..options(threads, true)
                },
                input,
                |item| item,
            )
            .unwrap();
            let out: Vec<_> = map.map(|r| r.unwrap()).collect();
            assert_eq!(out, expected, "{threads} threads");
            assert_eq!(lock(&splitting).1, 1, "{threads} threads");
        }
    }

    #[test]
    fn without_order_several_streams_are_split_at_once() {
        let mut input = Lengths::new(&[30; 6]);
        input.split = Duration::from_millis(2);
        let splitting = input.splitting.clone();
        let start = std::time::Instant::now();
        let map = ParallelMap::streams(
            MapOptions {
                max_open: 6,
                in_flight: 16,
                ..options(6, false)
            },
            input,
            |item| item,
        )
        .unwrap();
        let mut out: Vec<_> = map.map(|r| r.unwrap()).collect();
        out.sort();
        assert_eq!(out, Lengths::new(&[30; 6]).items());
        assert!(lock(&splitting).1 > 1);
        // One after another: 180 × 2 ms = 360 ms; six at once: 60 ms.
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_millis(200), "{elapsed:?}");
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
        for preserve_order in [true, false] {
            let taken = Arc::new(AtomicUsize::new(0));
            let mut input = Lengths::new(&[100; 5]);
            input.split = Duration::ZERO;
            let counter = taken.clone();
            let map = ParallelMap::streams(options(3, preserve_order), input, move |i| {
                counter.fetch_add(1, Ordering::SeqCst);
                i
            })
            .unwrap();
            std::thread::sleep(Duration::from_millis(50));
            assert_eq!(taken.load(Ordering::SeqCst), 5, "{preserve_order}");
            drop(map);
        }
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
    fn a_panic_in_the_helper_ends_the_map_after_the_items_before_it() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let helper: Helper = Arc::new(move || {
            if counter.fetch_add(1, Ordering::SeqCst) == 20 {
                panic!("helper");
            }
            false
        });
        let map = ParallelMap::new(
            MapOptions {
                helper: Some(helper),
                ..options(3, true)
            },
            0..1000u64,
            |i| i,
        )
        .unwrap();
        let out: Vec<Result<u64, String>> = map.map(|r| r.map_err(|p| panic_message(&p))).collect();
        let (last, before) = out.split_last().unwrap();
        assert_eq!(last, &Err("helper".to_string()));
        assert!(before.iter().enumerate().all(|(i, r)| r == &Ok(i as u64)));
    }

    /// Streams of 20 items; stream 2 fails at its third.
    struct Failing(usize);

    impl Streams for Failing {
        type Item = Result<(usize, u64), String>;
        type Stream = Box<dyn Iterator<Item = Self::Item> + Send>;

        fn next_stream(&mut self) -> Option<Self::Stream> {
            let stream = self.0;
            self.0 += 1;
            (stream < 6).then(|| {
                Box::new((0..20u64).map(move |i| {
                    std::thread::sleep(Duration::from_micros(300));
                    if stream == 2 && i == 2 {
                        Err("failed".to_string())
                    } else {
                        Ok((stream, i))
                    }
                })) as Self::Stream
            })
        }

        fn ends_input(item: &Self::Item) -> bool {
            item.is_err()
        }
    }

    #[test]
    fn an_item_that_ends_the_input_comes_after_everything_before_it() {
        for threads in [1, 4] {
            let map = ParallelMap::streams(options(threads, true), Failing(0), |i| i).unwrap();
            let out: Vec<_> = map.map(|r| r.unwrap()).collect();
            assert_eq!(out.len(), 43, "{threads}");
            assert_eq!(out[39], Ok((1, 19)));
            assert_eq!(out[41], Ok((2, 1)));
            assert_eq!(out[42], Err("failed".to_string()));
        }
    }

    #[test]
    fn without_order_an_item_that_ends_the_input_ends_the_map() {
        let map = ParallelMap::streams(options(4, false), Failing(0), |i| i).unwrap();
        let out: Vec<_> = map.map(|r| r.unwrap()).collect();
        assert_eq!(out.iter().filter(|r| r.is_err()).count(), 1);
        assert!(out.len() < 120);
    }

    #[test]
    fn the_last_item_comes_after_every_stream() {
        struct WithLast(Lengths);
        impl Streams for WithLast {
            type Item = (usize, u64);
            type Stream = <Lengths as Streams>::Stream;
            fn next_stream(&mut self) -> Option<Self::Stream> {
                self.0.next_stream()
            }
            fn finish(&mut self) -> Option<Self::Item> {
                Some((99, 0))
            }
        }
        for (threads, preserve_order) in [(1, true), (4, true), (4, false)] {
            let map = ParallelMap::streams(
                options(threads, preserve_order),
                WithLast(Lengths::new(&[5, 9, 2])),
                |i| i,
            )
            .unwrap();
            let out: Vec<_> = map.map(|r| r.unwrap()).collect();
            assert_eq!(out.len(), 17);
            assert_eq!(out.last(), Some(&(99, 0)));
        }
    }

    #[test]
    fn the_rest_of_an_input_starts_with_what_was_taken() {
        // Taken within a stream, at the end of one, and to the end.
        let lengths = [6, 4, 0, 5];
        for take in [3, 6, 15, 30] {
            for threads in [1, 3] {
                let mut sequential = Sequential::new(Lengths::new(&lengths));
                let taken: Vec<_> = sequential.by_ref().take(take).map(|r| r.unwrap()).collect();
                let map =
                    ParallelMap::streams(options(threads, true), sequential.rest(taken), |i| i)
                        .unwrap();
                let out: Vec<_> = map.map(|r| r.unwrap()).collect();
                assert_eq!(out, Lengths::new(&lengths).items(), "{take} {threads}");
            }
        }
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
        std::thread::sleep(Duration::from_millis(50));
        let after_drop = live.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(live.load(Ordering::SeqCst), after_drop);
    }

    #[test]
    fn fold_visits_every_item_once() {
        for threads in [1, 4] {
            let input = Lengths::new(&[30, 0, 12, 50, 1]);
            let expected = input.items();
            let seen = fold(threads, 3, "test", input, None, Vec::new, |seen, item| {
                seen.push(item);
                ControlFlow::Continue(())
            })
            .unwrap();
            assert_eq!(seen.len(), threads);
            let mut seen: Vec<_> = seen.into_iter().flatten().collect();
            seen.sort();
            assert_eq!(seen, expected);
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
            1,
            "test",
            One::new(items),
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
            1,
            "test",
            One::new(0..3),
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
