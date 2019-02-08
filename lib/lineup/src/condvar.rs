use core::cell::UnsafeCell;
use core::time::Duration;
use log::trace;

use crate::mutex::Mutex;
use crate::tls::Environment;
use crate::{ds, Scheduler, ThreadId, ThreadState};

#[derive(Debug)]
struct CondVar {
    inner: UnsafeCell<CondVarInner>,
}

unsafe impl Send for CondVar {}
unsafe impl Sync for CondVar {}

impl CondVar {
    pub fn new() -> CondVar {
        CondVar {
            inner: UnsafeCell::new(CondVarInner::new()),
        }
    }

    pub fn wait(&self, mtx: &Mutex) {
        let cv = unsafe { &mut *self.inner.get() };
        cv.wait(mtx);
    }

    pub fn wait_nowrap(&self, mtx: &Mutex) {
        let cv = unsafe { &mut *self.inner.get() };
        cv.wait_nowrap(mtx);
    }

    pub fn timed_wait(&self, mtx: &Mutex, d: Duration) {
        let cv = unsafe { &mut *self.inner.get() };
        cv.timed_wait(mtx, d);
    }

    pub fn signal(&self) {
        let cv = unsafe { &mut *self.inner.get() };
        cv.signal();
    }

    pub fn broadcast(&self) {
        let cv = unsafe { &mut *self.inner.get() };
        cv.broadcast();
    }

    pub fn has_waiters(&self) {
        let cv = unsafe { &mut *self.inner.get() };
        cv.has_waiters();
    }
}

#[derive(Debug)]
struct CondVarInner {
    waiters: ds::Vec<ThreadId>,
}

impl Drop for CondVarInner {
    fn drop(&mut self) {
        assert!(
            self.waiters.is_empty(),
            "Can't have outstanding waiters on CV"
        );
    }
}

impl CondVarInner {
    pub fn new() -> CondVarInner {
        CondVarInner {
            waiters: ds::Vec::with_capacity(Scheduler::MAX_THREADS),
        }
    }

    fn cv_unschedule(&mut self, mtx: &Mutex, rid: &mut u64) {
        let yielder: &mut ThreadState = Environment::thread();
        (yielder.upcalls.schedule)(&rid, Some(mtx));
        mtx.exit();
    }

    fn cv_reschedule(&mut self, mtx: &Mutex, rid: &u64) {
        let yielder: &mut ThreadState = Environment::thread();

        if mtx.is_spin() || mtx.is_kmutex() {
            (yielder.upcalls.schedule)(&rid, Some(mtx));
            mtx.enter_nowrap();
        } else {
            mtx.enter_nowrap();
            (yielder.upcalls.schedule)(&rid, Some(mtx));
        }
    }

    pub fn wait(&mut self, mtx: &Mutex) {
        let tid = Environment::tid();
        let yielder: &mut ThreadState = Environment::thread();

        let mut rid: u64 = 0;
        self.cv_unschedule(mtx, &mut rid);
        self.waiters.push(tid);
        trace!("waiting for {:?}", tid);
        yielder.make_unrunnable(tid);
        self.cv_reschedule(mtx, &rid);
    }

    pub fn wait_nowrap(&mut self, mtx: &Mutex) {
        let tid = Environment::tid();
        let yielder: &mut ThreadState = Environment::thread();

        mtx.exit();
        self.waiters.push(tid);
        yielder.make_unrunnable(tid);
        mtx.enter();
    }

    pub fn timed_wait(&mut self, _mutex: &Mutex, _d: Duration) {
        unreachable!("CV timedwaits")
    }

    pub fn signal(&mut self) {
        let waking_tid = self.waiters.pop();

        waking_tid.map(|tid| {
            let yielder: &mut ThreadState = Environment::thread();
            yielder.make_runnable(tid);
        });
    }

    pub fn broadcast(&mut self) {
        let waiters = self.waiters.clone();
        self.waiters.clear();
        let yielder: &mut ThreadState = Environment::thread();
        yielder.make_all_runnable(waiters);
    }

    pub fn has_waiters(&self) -> bool {
        !self.waiters.is_empty()
    }
}

#[test]
fn test_condvar() {
    use crate::DEFAULT_UPCALLS;
    let mut s = Scheduler::new(DEFAULT_UPCALLS);

    let cv = ds::Arc::new(CondVar::new());

    let cv1: ds::Arc<CondVar> = cv.clone();
    let cv2: ds::Arc<CondVar> = cv.clone();

    let mtx = ds::Arc::new(Mutex::new(false, false));
    let m2: ds::Arc<Mutex> = mtx.clone();

    s.spawn(4096, move |mut yielder| {
        for _i in 0..12 {
            cv1.signal();
        }
    });

    s.spawn(4096, move |mut yielder| {
        for _i in 0..5 {
            m2.enter();
            cv2.wait(&m2);
            m2.exit();
        }
    });

    for _run in 0..100 {
        s.run();
    }
}
