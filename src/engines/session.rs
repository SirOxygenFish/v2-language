//! Engine sessions and the watchdog thread (INTERNALS 1.3, ch. 8).
//!
//! Sessions arrive in roadmap 1.2. The watchdog thread already runs: it will own the call and
//! startup deadlines (8.5) and the interrupt path (6.5) as those milestones land.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// How long the watchdog sleeps between checks at most (INTERNALS 1.3).
const TICK: Duration = Duration::from_millis(100);

#[derive(Default)]
struct WatchState {
    stop: bool,
}

/// Handle to the running watchdog thread.
pub struct Watchdog {
    shared: Arc<(Mutex<WatchState>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Watchdog {
    pub fn start() -> Watchdog {
        let shared = Arc::new((Mutex::new(WatchState::default()), Condvar::new()));
        let theirs = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("v2-watchdog".into())
            .spawn(move || watch(&theirs))
            .ok();
        Watchdog { shared, thread }
    }

    /// Ask the thread to finish and wait for it.
    pub fn stop(mut self) {
        {
            let (lock, cvar) = &*self.shared;
            lock.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
            cvar.notify_all();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn watch(shared: &(Mutex<WatchState>, Condvar)) {
    let (lock, cvar) = shared;
    let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
    while !state.stop {
        state = match cvar.wait_timeout(state, TICK) {
            Ok((guard, _)) => guard,
            Err(poisoned) => poisoned.into_inner().0,
        };
    }
}
