#![no_std]

pub mod pmuv3;
pub mod nmi;

#[macro_use]
pub mod regs;

pub use pmuv3::PmuEventCounter;
pub use nmi::{PmuNmi, PmuNmiError, PMU_NMI, PMU_OVERFLOW_IRQ};

/// Handle PMU overflow interrupt.
///
/// Call this from your IRQ handler to check and handle PMU overflow.
/// Returns true if a PMU overflow was handled.
pub fn handle() -> bool {
    PMU_NMI.handle_overflow()
}
