//! One admission gate for every local accelerator action. A switch first closes
//! admission, then waits for already dispatched requests before committing.
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct State {
    blocked: bool,
    in_flight: usize,
    generation: u64,
}

#[derive(Clone, Default)]
pub struct Gate(Arc<Mutex<State>>);

pub struct Permit(Gate);

impl Gate {
    pub fn block(&self) {
        if let Ok(mut s) = self.0.lock() {
            s.blocked = true;
        }
    }
    pub fn ready(&self, generation: u64) -> bool {
        self.0
            .lock()
            .is_ok_and(|s| !s.blocked && s.generation == generation)
    }
    pub fn cancel_block(&self, generation: u64) {
        if let Ok(mut s) = self.0.lock() {
            if s.generation == generation {
                s.blocked = false;
            }
        }
    }
    pub fn drained(&self) -> bool {
        self.0.lock().is_ok_and(|s| s.in_flight == 0)
    }
    pub fn resume(&self, generation: u64) -> bool {
        self.0.lock().is_ok_and(|mut s| {
            if s.in_flight != 0 {
                return false;
            }
            s.generation = generation;
            s.blocked = false;
            true
        })
    }
    pub fn enter(&self, generation: u64) -> Option<Permit> {
        let mut s = self.0.lock().ok()?;
        if s.blocked || s.generation != generation {
            return None;
        }
        s.in_flight += 1;
        Some(Permit(self.clone()))
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if let Ok(mut s) = self.0 .0.lock() {
            s.in_flight -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switch_waits_for_sent_requests_and_refuses_new_work() {
        let gate = Gate::default();
        let request = gate.enter(0).unwrap();
        gate.block();
        assert!(gate.enter(0).is_none());
        assert!(!gate.resume(0));
        drop(request);
        assert!(gate.drained());
        assert!(gate.resume(0));
        assert!(gate.enter(0).is_some());
    }
}
