//! Cancelling long jobs (solves, optimisers, tuners).
//!
//! The app makes a [`Token`] per job and installs it on the worker thread
//! with [`Token::install`]; the long loops call [`check`] now and then,
//! which returns `Err` with [`MESSAGE`] once the token is cancelled.
//! Code that starts its own threads passes the token on with
//! [`current`] and [`Token::install`] in each.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// What a cancelled job returns as its error.
pub const MESSAGE: &str = "cancelled";

#[derive(Clone, Debug, Default)]
pub struct Token(Arc<AtomicBool>);

thread_local! {
    static CURRENT: RefCell<Option<Token>> = const { RefCell::new(None) };
}

impl Token {
    pub fn new() -> Token {
        Token::default()
    }

    /// Asks the job to stop at its next check.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Makes this the token the calling thread's checks look at, until
    /// the guard is dropped.
    pub fn install(&self) -> Guard {
        let old = CURRENT.with(|c| c.borrow_mut().replace(self.clone()));
        Guard(old)
    }
}

/// Puts back the thread's previous token when dropped.
pub struct Guard(Option<Token>);

impl Drop for Guard {
    fn drop(&mut self) {
        let old = self.0.take();
        CURRENT.with(|c| *c.borrow_mut() = old);
    }
}

/// The calling thread's token, to hand to threads it starts.
pub fn current() -> Option<Token> {
    CURRENT.with(|c| c.borrow().clone())
}

/// Installs `t` when there is one (for a thread started inside a job).
pub fn adopt(t: &Option<Token>) -> Option<Guard> {
    t.as_ref().map(Token::install)
}

/// True once the calling thread's job has been cancelled.
pub fn cancelled() -> bool {
    CURRENT.with(|c| c.borrow().as_ref().is_some_and(Token::is_cancelled))
}

/// `Err(MESSAGE)` once the calling thread's job has been cancelled.
pub fn check() -> Result<(), String> {
    if cancelled() {
        Err(MESSAGE.to_string())
    } else {
        Ok(())
    }
}

/// True when `e` is the error of a cancelled job.
pub fn is_cancel(e: &str) -> bool {
    e == MESSAGE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancelled_token_stops_checks_on_its_thread_only() {
        assert!(check().is_ok());
        let t = Token::new();
        {
            let _g = t.install();
            assert!(check().is_ok());
            t.cancel();
            assert!(check().is_err());
            let inner = current();
            std::thread::scope(|s| {
                s.spawn(|| {
                    assert!(check().is_ok(), "a new thread has no token");
                    let _g = adopt(&inner);
                    assert!(check().is_err());
                });
            });
        }
        assert!(check().is_ok(), "the guard put the old token back");
    }
}
