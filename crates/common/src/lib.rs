//! Code every realXmarket program carries: the XCAV vault transfer helpers
//! and the payment-mint guard. Errors stay in each program; the guard
//! reports what it found and the caller maps that onto its own enum.

pub mod election;
pub mod mint_guard;
pub mod vault;
