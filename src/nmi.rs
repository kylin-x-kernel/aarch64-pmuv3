//! PMU as NMI trigger source.
//!
//! This module provides functionality to use PMU counter overflow as an NMI-like
//! interrupt source. When the counter overflows, it triggers an interrupt that
//! can be used for watchdog or profiling purposes.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::{isb, mrs, msr};

/// PMU overflow interrupt number (typically PPI 23, so INTID 23).
pub const PMU_OVERFLOW_IRQ: u32 = 23;

/// Error type for PMU NMI operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmuNmiError {
    /// PMU not available on this platform.
    NotAvailable,
    /// Invalid counter index.
    InvalidCounter,
    /// Counter already in use.
    CounterInUse,
    /// Operation failed.
    Failed,
}

/// PMU NMI trigger configuration.
pub struct PmuNmi {
    /// Counter index to use (0-30 for event counters, 31 for cycle counter).
    counter_index: u32,
    /// Threshold value - interrupt triggers when counter reaches this.
    threshold: u64,
    /// Whether the NMI source is enabled.
    enabled: AtomicBool,
    /// Current period in nanoseconds (for informational purposes).
    period_ns: AtomicU64,
}

impl PmuNmi {
    /// Create a new PMU NMI source using the cycle counter (counter 31).
    ///
    /// # Arguments
    /// * `threshold` - Counter value at which to trigger interrupt.
    ///                 For cycle counter, this is CPU cycles.
    pub const fn new_cycle_counter(threshold: u64) -> Self {
        Self {
            counter_index: 31,
            threshold,
            enabled: AtomicBool::new(false),
            period_ns: AtomicU64::new(0),
        }
    }

    /// Create a new PMU NMI source using an event counter.
    ///
    /// # Arguments
    /// * `counter_index` - Event counter index (0-30)
    /// * `event_type` - PMU event type to count
    /// * `threshold` - Counter value at which to trigger interrupt
    pub const fn new_event_counter(counter_index: u32, threshold: u64) -> Self {
        Self {
            counter_index,
            threshold,
            enabled: AtomicBool::new(false),
            period_ns: AtomicU64::new(0),
        }
    }

    /// Get the counter index.
    pub fn counter_index(&self) -> u32 {
        self.counter_index
    }

    /// Get the threshold value.
    pub fn threshold(&self) -> u64 {
        self.threshold
    }

    /// Set a new threshold value.
    pub fn set_threshold(&mut self, threshold: u64) {
        self.threshold = threshold;
    }

    /// Initialize the PMU for NMI generation.
    ///
    /// This configures the PMU but does not enable the counter.
    pub fn init(&self) -> Result<(), PmuNmiError> {
        // Read PMU version from ID_AA64DFR0_EL1
        let aa64dfr0: u64 = mrs!(ID_AA64DFR0_EL1);
        let pmu_ver = (aa64dfr0 >> 8) & 0xF;

        // PMUv3 is version >= 1
        if pmu_ver == 0 || pmu_ver == 0xF {
            return Err(PmuNmiError::NotAvailable);
        }

        // Read number of counters from PMCR_EL0
        let pmcr: u64 = mrs!(PMCR_EL0);
        let num_counters = ((pmcr >> 11) & 0x1F) as u32;

        // Validate counter index
        if self.counter_index < 31 && self.counter_index >= num_counters {
            return Err(PmuNmiError::InvalidCounter);
        }

        Ok(())
    }

    /// Enable the PMU NMI trigger.
    ///
    /// This starts the counter and enables overflow interrupt.
    pub fn enable(&self) -> Result<(), PmuNmiError> {
        if self.counter_index == 31 {
            // Cycle counter
            self.enable_cycle_counter()?;
        } else {
            // Event counter
            self.enable_event_counter()?;
        }

        self.enabled.store(true, Ordering::Release);
        Ok(())
    }

    /// Disable the PMU NMI trigger.
    pub fn disable(&self) -> Result<(), PmuNmiError> {
        self.enabled.store(false, Ordering::Release);

        if self.counter_index == 31 {
            // Disable cycle counter
            msr!(PMCNTENCLR_EL0, 1u64 << 31);
            // Disable overflow interrupt
            msr!(PMINTENCLR_EL1, 1u64 << 31);
        } else {
            // Disable event counter
            msr!(PMCNTENCLR_EL0, 1u64 << self.counter_index);
            // Disable overflow interrupt
            msr!(PMINTENCLR_EL1, 1u64 << self.counter_index);
        }

        isb!();
        Ok(())
    }

    /// Check if overflow occurred and clear the flag.
    ///
    /// Returns true if overflow was detected (and cleared).
    /// This should be called from the interrupt handler.
    pub fn check_and_clear_overflow(&self) -> bool {
        let mask = if self.counter_index == 31 {
            1u64 << 31
        } else {
            1u64 << self.counter_index
        };

        // Read overflow status
        let overflow: u64 = mrs!(PMOVSSET_EL0);

        if (overflow & mask) != 0 {
            // Clear overflow flag
            msr!(PMOVSCLR_EL0, mask);
            isb!();
            true
        } else {
            false
        }
    }

    /// Reset the counter to prepare for next trigger.
    ///
    /// This should be called after handling the overflow.
    pub fn reset_counter(&self) {
        if self.counter_index == 31 {
            // Set cycle counter to (MAX - threshold) so it overflows after `threshold` cycles
            let initial_value = u64::MAX - self.threshold;
            msr!(PMCCNTR_EL0, initial_value);
        } else {
            // For event counters, set to (MAX_U32 - threshold)
            let initial_value = (u32::MAX as u64) - (self.threshold & 0xFFFFFFFF);
            msr!(PMSELR_EL0, self.counter_index, "x");
            msr!(PMXEVCNTR_EL0, initial_value as u32, "x");
        }
        isb!();
    }

    /// Handle PMU overflow interrupt.
    ///
    /// Call this from your interrupt handler. Returns true if this was
    /// a PMU overflow that was handled.
    pub fn handle_overflow(&self) -> bool {
        if !self.enabled.load(Ordering::Acquire) {
            return false;
        }

        if self.check_and_clear_overflow() {
            // Reset counter for next period
            self.reset_counter();
            true
        } else {
            false
        }
    }

    /// Check if the NMI source is currently enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    // Private helper methods

    fn enable_cycle_counter(&self) -> Result<(), PmuNmiError> {
        // Disable counter first
        msr!(PMCNTENCLR_EL0, 1u64 << 31);

        // Clear any pending overflow
        msr!(PMOVSCLR_EL0, 1u64 << 31);

        // Configure cycle counter filter (count all cycles in EL1 and EL0)
        // NSH=0 (count EL2), P=1 (count EL1), U=1 (count EL0), NSK=0, M=0
        let filter: u64 = (1 << 31) | (1 << 30); // P and U bits
        msr!(PMCCFILTR_EL0, filter);

        // Set initial counter value
        let initial_value = u64::MAX - self.threshold;
        msr!(PMCCNTR_EL0, initial_value);

        // Enable overflow interrupt for cycle counter
        msr!(PMINTENSET_EL1, 1u64 << 31);

        // Enable the cycle counter
        msr!(PMCNTENSET_EL0, 1u64 << 31);

        // Ensure PMU is enabled
        let pmcr: u64 = mrs!(PMCR_EL0);
        msr!(PMCR_EL0, pmcr | (1 << 0)); // Set E bit

        isb!();
        Ok(())
    }

    fn enable_event_counter(&self) -> Result<(), PmuNmiError> {
        let counter_mask = 1u64 << self.counter_index;

        // Disable counter first
        msr!(PMCNTENCLR_EL0, counter_mask);

        // Clear any pending overflow
        msr!(PMOVSCLR_EL0, counter_mask);

        // Select the counter
        msr!(PMSELR_EL0, self.counter_index, "x");

        // Set initial counter value
        let initial_value = (u32::MAX as u64) - (self.threshold & 0xFFFFFFFF);
        msr!(PMXEVCNTR_EL0, initial_value as u32, "x");

        // Enable overflow interrupt
        msr!(PMINTENSET_EL1, counter_mask);

        // Enable the counter
        msr!(PMCNTENSET_EL0, counter_mask);

        // Ensure PMU is enabled
        let pmcr: u64 = mrs!(PMCR_EL0);
        msr!(PMCR_EL0, pmcr | (1 << 0)); // Set E bit

        isb!();
        Ok(())
    }
}

/// Global PMU NMI instance for convenience.
///
/// Configured with cycle counter and a default threshold of 100M cycles.
pub static PMU_NMI: PmuNmi = PmuNmi::new_cycle_counter(100_000_000);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pmu_nmi_creation() {
        let nmi = PmuNmi::new_cycle_counter(1000);
        assert_eq!(nmi.counter_index(), 31);
        assert_eq!(nmi.threshold(), 1000);
        assert!(!nmi.is_enabled());
    }
}
