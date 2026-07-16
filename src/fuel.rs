//! Hierarchical fuel meters: deterministic step budgets, carved out from
//! parent budgets at fork. See docs/superpowers/specs/2026-07-16-fuel-preemption-design.md.

use crate::error::{JSError, Result};

pub const DEFAULT_QUANTUM: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeterId(pub u32);

#[derive(Debug, Clone)]
pub struct Meter {
    /// None = unlimited.
    pub remaining: Option<u64>,
    pub parent: Option<MeterId>,
}

#[derive(Debug, Clone)]
pub struct Meters {
    slots: Vec<Meter>,
}

impl Meters {
    pub const ROOT: MeterId = MeterId(0);

    pub fn new() -> Self {
        Self {
            slots: vec![Meter {
                remaining: None,
                parent: None,
            }],
        }
    }

    pub fn from_slots(slots: Vec<Meter>) -> Self {
        debug_assert!(!slots.is_empty(), "meters must contain the root at slot 0");
        Self { slots }
    }

    pub fn slots(&self) -> &[Meter] {
        &self.slots
    }

    pub fn get(&self, id: MeterId) -> &Meter {
        &self.slots[id.0 as usize]
    }

    pub fn remaining(&self, id: MeterId) -> Option<u64> {
        self.get(id).remaining
    }

    pub fn set_root_budget(&mut self, budget: Option<u64>) {
        self.slots[0].remaining = budget;
    }

    /// Deduct `spent` from a finite meter; unlimited meters ignore charges.
    pub fn charge(&mut self, id: MeterId, spent: u64) {
        if let Some(rem) = &mut self.slots[id.0 as usize].remaining {
            *rem = rem.saturating_sub(spent);
        }
    }

    /// Top up a finite meter; unlimited meters need no fuel.
    pub fn add(&mut self, id: MeterId, amount: u64) {
        if let Some(rem) = &mut self.slots[id.0 as usize].remaining {
            *rem = rem.saturating_add(amount);
        }
    }

    /// Carve `amount` out of `from` into a fresh child meter. Fuel moves,
    /// it is never created: the deduction happens up front, so total
    /// spending stays bounded by the root budget at any fork depth.
    pub fn carve(&mut self, from: MeterId, amount: u64) -> Result<MeterId> {
        if let Some(rem) = &mut self.slots[from.0 as usize].remaining {
            if *rem < amount {
                return Err(JSError::Message(format!(
                    "Fork: insufficient fuel (requested {amount}, remaining {rem})"
                )));
            }
            *rem -= amount;
        }
        let id = MeterId(self.slots.len() as u32);
        self.slots.push(Meter {
            remaining: Some(amount),
            parent: Some(from),
        });
        Ok(id)
    }

    /// Return a consumed fiber's leftover to its parent meter. Idempotent:
    /// the leftover is taken (meter drops to 0), so a second call moves
    /// nothing. Refunding into an unlimited parent discards the leftover.
    pub fn refund_into_parent(&mut self, id: MeterId) {
        let Some(parent) = self.slots[id.0 as usize].parent else {
            return;
        };
        let leftover = self.slots[id.0 as usize].remaining.take().unwrap_or(0);
        self.slots[id.0 as usize].remaining = Some(0);
        self.add(parent, leftover);
    }
}

#[cfg(test)]
mod fuel_tests {
    use super::*;

    #[test]
    fn root_is_unlimited_by_default() {
        let m = Meters::new();
        assert_eq!(m.remaining(Meters::ROOT), None);
    }

    #[test]
    fn carve_deducts_from_parent_immediately() {
        let mut m = Meters::new();
        m.set_root_budget(Some(100));
        let child = m.carve(Meters::ROOT, 30).unwrap();
        assert_eq!(m.remaining(Meters::ROOT), Some(70));
        assert_eq!(m.remaining(child), Some(30));
        assert_eq!(m.get(child).parent, Some(Meters::ROOT));
    }

    #[test]
    fn carve_from_unlimited_parent_creates_finite_child() {
        let mut m = Meters::new();
        let child = m.carve(Meters::ROOT, 30).unwrap();
        assert_eq!(m.remaining(Meters::ROOT), None);
        assert_eq!(m.remaining(child), Some(30));
    }

    #[test]
    fn carve_more_than_remaining_is_an_error() {
        let mut m = Meters::new();
        m.set_root_budget(Some(10));
        let err = m.carve(Meters::ROOT, 11).unwrap_err();
        assert!(err.to_string().contains("insufficient fuel"), "{err}");
        // Failed carve must not deduct.
        assert_eq!(m.remaining(Meters::ROOT), Some(10));
    }

    #[test]
    fn charge_saturates_at_zero() {
        let mut m = Meters::new();
        m.set_root_budget(Some(5));
        m.charge(Meters::ROOT, 7);
        assert_eq!(m.remaining(Meters::ROOT), Some(0));
    }

    #[test]
    fn charge_on_unlimited_is_noop() {
        let mut m = Meters::new();
        m.charge(Meters::ROOT, 1000);
        assert_eq!(m.remaining(Meters::ROOT), None);
    }

    #[test]
    fn refund_moves_leftover_to_parent_and_empties_child() {
        let mut m = Meters::new();
        m.set_root_budget(Some(100));
        let child = m.carve(Meters::ROOT, 30).unwrap();
        m.charge(child, 12);
        m.refund_into_parent(child);
        assert_eq!(m.remaining(Meters::ROOT), Some(88)); // 70 + 18
        assert_eq!(m.remaining(child), Some(0));
        // Double refund must not double-credit.
        m.refund_into_parent(child);
        assert_eq!(m.remaining(Meters::ROOT), Some(88));
    }

    #[test]
    fn refund_into_unlimited_parent_discards_leftover() {
        let mut m = Meters::new();
        let child = m.carve(Meters::ROOT, 30).unwrap();
        m.refund_into_parent(child);
        assert_eq!(m.remaining(Meters::ROOT), None);
        assert_eq!(m.remaining(child), Some(0));
    }

    #[test]
    fn add_tops_up_a_finite_meter() {
        let mut m = Meters::new();
        m.set_root_budget(Some(0));
        m.add(Meters::ROOT, 50);
        assert_eq!(m.remaining(Meters::ROOT), Some(50));
    }
}
