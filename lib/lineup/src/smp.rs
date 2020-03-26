use core::fmt;
use rawtime::Instant;

#[cfg(test)]
extern crate env_logger;

#[cfg(test)]
mod ds {
    pub use hashbrown::HashMap;
    pub use std::sync::Arc;
    pub use std::vec::Vec;
}

#[cfg(not(test))]
mod ds {
    pub use alloc::boxed::Box;
    pub use alloc::sync::Arc;
    pub use alloc::vec::Vec;
    pub use hashbrown::HashMap;
}

use core::hash::{Hash, Hasher};
use core::ops::Add;
use core::ptr;
use core::time::Duration;
use log::*;

use super::mutex;
use super::stack::LineupStack;
use super::tls;
use super::*;

use fringe::generator::{Generator, Yielder};
use fringe::Stack;

pub struct Scheduler<'a> {
    threads: ds::HashMap<
        ThreadId,
        (
            Thread,
            Generator<'a, YieldResume, YieldRequest, LineupStack>,
        ),
    >,
    runnable: ds::Vec<ThreadId>,
    waiting: ds::Vec<(ThreadId, Instant)>,
    run_idx: usize,
    tid_counter: usize,
    upcalls: Upcalls,
    tls: tls::ThreadLocalStorage<'static>,
    state: tls::SchedulerState,
}

impl<'a> Scheduler<'a> {
    pub const MAX_THREADS: usize = 64;

    pub fn new(upcalls: Upcalls) -> Scheduler<'a> {
        Scheduler {
            threads: ds::HashMap::with_capacity(Scheduler::MAX_THREADS),
            runnable: ds::Vec::with_capacity(Scheduler::MAX_THREADS),
            waiting: ds::Vec::with_capacity(Scheduler::MAX_THREADS),
            run_idx: 0,
            tid_counter: 0,
            upcalls: upcalls,
            tls: tls::ThreadLocalStorage::new(),
            state: tls::SchedulerState::new(),
        }
    }

    #[cfg(test)]
    fn spawn_core(&mut self, core_id: usize) {
        use std::thread;
        use std::time::Duration;

        thread::spawn(move || {
            for i in 1..10 {
                println!("hi number {} from the spawned thread!", core_id);
                thread::sleep(Duration::from_millis(1));
            }
        });
    }

    fn add_thread(
        &mut self,
        handle: Thread,
        generator: Generator<'a, YieldResume, YieldRequest, LineupStack>,
    ) -> Option<ThreadId> {
        let tid = handle.id.clone();
        assert!(
            !self.threads.contains_key(&tid),
            "Thread {} already exists?",
            tid
        );

        if self.threads.len() <= Scheduler::MAX_THREADS {
            self.threads.insert(tid, (handle, generator));
            Some(tid)
        } else {
            error!("too many threads");
            return None;
        }
    }

    fn mark_runnable(&mut self, tid: ThreadId) {
        assert!(
            self.threads.contains_key(&tid),
            "Thread {} does not exist? Can't mark_runnable.",
            tid
        );
        assert!(
            !self.runnable.contains(&tid),
            "Thread {} is already runnable?",
            tid
        );

        // CondVars can wake up before time-out is done
        self.waiting.drain_filter(|(wtid, _)| *wtid == tid);

        if !self.runnable.contains(&tid) {
            self.runnable.push(tid);
        }
    }

    fn mark_unrunnable(&mut self, tid: ThreadId) {
        trace!("Removing Thread {} from run-list.", tid);

        assert!(
            self.threads.contains_key(&tid),
            "Thread {} does not exist? Can't mark_unrunnable.",
            tid
        );
        assert!(
            self.runnable.contains(&tid),
            "Thread {} is not runnable?",
            tid
        );

        while let Some(_) = self.runnable.remove_item(&tid) {}
    }

    fn reset_run_index(&mut self) {
        self.run_idx = 0;
    }

    pub fn spawn<F>(&mut self, stack_size: usize, f: F, arg: *mut u8) -> Option<ThreadId>
    where
        F: 'static + FnOnce(*mut u8) + Send,
    {
        let tid = ThreadId(self.tid_counter);
        let stack = LineupStack::from_size(stack_size);
        let (handle, generator) = unsafe { Thread::new(tid, stack, f, arg, self.upcalls) };

        self.add_thread(handle, generator).map(|tid| {
            self.mark_runnable(tid);
            self.tid_counter += 1;
            tid
        })
    }

    pub fn spawn_with_stack<F>(
        &mut self,
        stack: LineupStack,
        f: F,
        arg: *mut u8,
    ) -> Option<ThreadId>
    where
        F: 'static + FnOnce(*mut u8) + Send,
    {
        let tid = ThreadId(self.tid_counter);
        let (handle, generator) = unsafe { Thread::new(tid, stack, f, arg, self.upcalls) };

        self.add_thread(handle, generator).map(|tid| {
            self.mark_runnable(tid);
            self.tid_counter += 1;
            tid
        })
    }

    pub fn run(&mut self) {
        unsafe {
            tls::arch::set_tls((&mut self.tls) as *mut tls::ThreadLocalStorage);
            tls::set_scheduler_state((&mut self.state) as *mut tls::SchedulerState);
        }

        loop {
            // Get previous IRQ state and reset it
            let is_irq_pending = self
                .state
                .signal_irq
                .swap(false, core::sync::atomic::Ordering::AcqRel);
            // TODO(correctness): Hard-coded assumption that threadId 1 is IRQ handler
            if is_irq_pending {
                self.runnable.insert(0, ThreadId(1));
            }

            // Try to add any threads in SchedulerState to runlist
            for tid in self.state.make_runnable.iter() {
                trace!("making {:?} from mark_runnable runnable!", tid);
                if !self.runnable.contains(&tid) {
                    self.runnable.push(*tid);
                }
            }
            self.state.make_runnable.clear();

            // Try to find anything waiting threads that have timeouts
            let now = Instant::now();

            // TODO: Don't have to pay 2n for this
            for (tid, timeout) in self.waiting.iter() {
                if *timeout <= now {
                    self.runnable.push(*tid);
                }
            }
            self.waiting.drain_filter(|(tid, timeout)| *timeout <= now);

            // If there is nothing to run anymore, we are done.
            if self.runnable.is_empty() {
                return;
            }

            // Start off where we left off last
            let tid = self.runnable[self.run_idx];

            trace!(
                "dispatching {:?}, self.runnable({}) = {:?}",
                tid,
                self.runnable.len(),
                self.runnable,
            );

            let action: YieldResume = {
                let thread: &mut Thread = &mut self
                    .threads
                    .get_mut(&tid)
                    .expect("Can't find thread state?")
                    .0;

                trace!("thread = {:?}", thread);
                thread.return_with.unwrap_or(YieldResume::Completed)
            };

            unsafe {
                let thread: &mut Thread = &mut self
                    .threads
                    .get_mut(&tid)
                    .expect("Can't find thread state?")
                    .0;

                // if this is the first time we run this,
                // we should not overwrite thread state
                // the thread will do it for us.
                if !thread.state.is_null() {
                    tls::set_thread_state(thread.state);
                }
            }

            let result = {
                let generator = &mut self.threads.get_mut(&tid).unwrap().1;
                generator.resume(action)
            };

            let (is_done, retresult) = match result {
                None => {
                    trace!("Thread {} has terminated.", tid);
                    trace!(
                        "self.runnable({}) = {:?} ",
                        self.runnable.len(),
                        self.runnable,
                    );

                    self.mark_unrunnable(tid);
                    self.threads.remove(&tid);

                    unsafe {
                        tls::set_thread_state(ptr::null_mut());
                    }
                    (true, YieldResume::Completed)
                }
                Some(YieldRequest::None) => {
                    trace!("Thread {} has YieldRequest::None.", tid);
                    // Put at end of the queue
                    self.mark_unrunnable(tid);
                    self.mark_runnable(tid);
                    (false, YieldResume::Completed)
                }
                Some(YieldRequest::Runnable(rtid)) => {
                    trace!("YieldRequest::Runnable {:?}", rtid);
                    self.mark_runnable(rtid);
                    (false, YieldResume::Completed)
                }
                Some(YieldRequest::Unrunnable(rtid)) => {
                    trace!("YieldRequest::Unrunnable {:?}", rtid);
                    self.mark_unrunnable(rtid);
                    (false, YieldResume::Completed)
                }
                Some(YieldRequest::RunnableList(rtids)) => {
                    trace!("YieldRequest::RunnableList {:?}", rtids);
                    for rtid in rtids.iter() {
                        self.mark_runnable(*rtid);
                    }
                    (false, YieldResume::Completed)
                }
                Some(YieldRequest::Timeout(until)) => {
                    trace!(
                        "The thread #{:?} has suspended itself until {:?}.",
                        tid,
                        until.duration_since(Instant::now()),
                    );

                    self.waiting.push((tid, until));
                    self.mark_unrunnable(tid);
                    (false, YieldResume::Completed)
                }
                Some(YieldRequest::Spawn(function, arg)) => {
                    trace!("self.spawn {:?} {:p}", function, arg);
                    let tid = self
                        .spawn(
                            64 * 4096,
                            move |arg| unsafe {
                                (function.unwrap())(arg);
                            },
                            arg,
                        )
                        .expect("Can't spawn the thread");
                    (false, YieldResume::Spawned(tid))
                }
                Some(YieldRequest::SpawnWithStack(stack, function, arg)) => {
                    trace!("self.spawn {:?} {:p}", function, arg);
                    let tid = self
                        .spawn_with_stack(
                            stack,
                            move |arg| unsafe {
                                (function.unwrap())(arg);
                            },
                            arg,
                        )
                        .expect("Can't spawn the thread");
                    (false, YieldResume::Spawned(tid))
                }
            };

            // If thread is not done we need to preserve TLS
            // TODO: I modified libfringe to do this, but not
            // sure if llvm actually does it, check assembly!
            if !is_done {
                trace!("tid {:?} not done, getting thread state", tid);
                let thread: &mut Thread = &mut self
                    .threads
                    .get_mut(&tid)
                    .expect("Can't find thread state for tid?")
                    .0;
                thread.return_with = Some(retresult);

                unsafe {
                    thread.state = tls::get_thread_state();
                    tls::set_thread_state(ptr::null_mut());
                }
            }
        }

        unsafe {
            tls::set_thread_state(ptr::null_mut());
            tls::set_scheduler_state(ptr::null_mut());
        }
    }
}

#[derive(Debug, Clone)]
pub struct ThreadState<'a> {
    yielder: &'a Yielder<YieldResume, YieldRequest>,
    tid: ThreadId,
    pub upcalls: Upcalls,
    pub rump_lwp: *const u64,
    pub rumprun_lwp: *const u64,
}

impl<'a> ThreadState<'a> {
    fn yielder(&self) -> &'a Yielder<YieldResume, YieldRequest> {
        self.yielder
    }

    pub fn set_lwp(&mut self, lwp_ptr: *const u64) {
        self.rump_lwp = lwp_ptr;
    }

    pub fn spawn_with_stack(
        &self,
        s: LineupStack,
        f: Option<unsafe extern "C" fn(arg1: *mut u8) -> *mut u8>,
        arg: *mut u8,
    ) -> Option<ThreadId> {
        let request = YieldRequest::SpawnWithStack(s, f, arg);
        match self.yielder().suspend(request) {
            YieldResume::Spawned(tid) => Some(tid),
            _ => None,
        }
    }

    pub fn spawn(
        &self,
        f: Option<unsafe extern "C" fn(arg1: *mut u8) -> *mut u8>,
        arg: *mut u8,
    ) -> Option<ThreadId> {
        let request = YieldRequest::Spawn(f, arg);
        match self.yielder().suspend(request) {
            YieldResume::Spawned(tid) => Some(tid),
            _ => None,
        }
    }

    pub fn sleep(&self, d: Duration) {
        let request = YieldRequest::Timeout(Instant::now().add(d));
        self.yielder().suspend(request);
    }

    pub fn block(&self) {
        let request = YieldRequest::Unrunnable(tls::Environment::tid());
        self.yielder().suspend(request);
    }

    pub fn make_runnable(&self, tid: ThreadId) {
        let request = YieldRequest::Runnable(tid);
        self.yielder().suspend(request);
    }

    fn make_all_runnable(&self, tids: ds::Vec<ThreadId>) {
        let request = YieldRequest::RunnableList(tids);
        self.yielder().suspend(request);
    }

    fn make_unrunnable(&self, tid: ThreadId) {
        let request = YieldRequest::Unrunnable(tid);
        self.yielder().suspend(request);
    }

    fn suspend(&self, request: YieldRequest) {
        self.yielder().suspend(request);
    }

    pub fn relinquish(&self) {
        self.suspend(YieldRequest::None);
    }
}

#[test]
fn smp_sched() {
    use crate::ds;
    use crate::mutex::Mutex;

    let mut s = Scheduler::new(DEFAULT_UPCALLS);
    let mtx = ds::Arc::new(Mutex::new(false, true));
    let m1: ds::Arc<Mutex> = mtx.clone();
    let m2: ds::Arc<Mutex> = mtx.clone();

    s.spawn_core(0);
    s.spawn_core(1);
    s.spawn_core(2);
    s.spawn_core(3);
}
