use core::sync::atomic::{AtomicUsize, Ordering};

use tock_registers::interfaces::{ReadWriteable, Readable, Writeable};

use crate::{isb, mrs, msr};

use super::regs::{PMCCFILTR_EL0, PMCR_EL0, PMUSERENR_EL0};

/// See ARM PMU Events
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
#[repr(u32)]
enum PmuEvent {
    MemAccess = 0x13,      // Data memory access
    L2dCache = 0x16,       // Level 2 data cache access
    L2dCacheRefill = 0x17, // Level 2 data cache refill
}

#[derive(Clone, Debug)]
pub struct PmuEventCounter {
    event: PmuEvent,
    index: u32,
}

impl PmuEventCounter {
    fn enable(&self, initial_value: u32) {
        // Disable counter
        msr!(PMCNTENCLR_EL0, 1u64 << self.index);

        // Clear overflows
        msr!(PMOVSCLR_EL0, 1u64 << self.index);

        self.set_counter(initial_value);

        // Enable interrupt for this counter
        msr!(PMINTENSET_EL1, 1u64 << self.index);

        // Enable counter
        msr!(PMCNTENSET_EL0, 1u64 << self.index);
        isb!();
    }

    fn disable(&self) {
        // Disable counter
        msr!(PMCNTENCLR_EL0, 1u64 << self.index);

        // Disbale interrupt for this counter
        msr!(PMINTENCLR_EL1, 1u64 << self.index);

        // Reset the event conuter to 0
        self.set_counter(0);
    }

    fn set_counter(&self, initial_value: u32) {
        // Select event counter
        msr!(PMSELR_EL0, self.index, "x");
        // Set event type
        msr!(PMXEVTYPER_EL0, self.event as u32, "x");
        // Set event conuter initial value
        msr!(PMXEVCNTR_EL0, initial_value, "x");
    }

    fn read_counter(&self) -> u32 {
        // Select event counter
        msr!(PMSELR_EL0, self.index, "x");
        mrs!(PMXEVCNTR_EL0) as u32
    }
}

#[allow(dead_code)]
pub fn cpu_cycle_count() -> u64 {
    mrs!(PMCCNTR_EL0)
}

pub struct PmuManager {
    event_counters_num: AtomicUsize,
    // event_counters_list: Mutex<Vec<PmuEventCounter>>,
}

impl PmuManager {
    pub fn new() -> Self {
        let counter_list = PmuManager {
            event_counters_num: AtomicUsize::new(0),
            // event_counters_list: Mutex::new(Vec::new()),
        };
        // enable Long cycle count, disable Clock divider
        PMCR_EL0.modify(PMCR_EL0::LC::Enable + PMCR_EL0::D::Disable);

        // reset Clock counter and event counter
        PMCR_EL0.modify(PMCR_EL0::P::Reset + PMCR_EL0::C::Reset);

        // enable PMU
        PMCR_EL0.modify(PMCR_EL0::E::Enable);

        // enables the cycle counter
        msr!(PMCNTENSET_EL0, 1u64 << 31);

        // only count EL0 and EL1, don't count EL2
        PMCCFILTR_EL0.write(
            PMCCFILTR_EL0::P::Count + PMCCFILTR_EL0::U::Count + PMCCFILTR_EL0::NSH::DontCount,
        );

        // software can access PMCCNTR_EL0
        PMUSERENR_EL0.write(PMUSERENR_EL0::EN::Trap + PMUSERENR_EL0::CR::Trap);

        counter_list
    }
}
