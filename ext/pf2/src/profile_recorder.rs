use std::collections::HashSet;
use std::ptr::null_mut;
use std::time::Instant;

use backtrace_sys2::backtrace_create_state;
use rb_sys::*;

use crate::backtrace::{Backtrace, BacktraceState};
use crate::profile::Profile;
use crate::ringbuffer::Ringbuffer;

// Capacity large enough to hold 1 second worth of samples for 16 threads
// 16 threads * 20 samples per second * 1 second = 320
const DEFAULT_RINGBUFFER_CAPACITY: usize = 320;

#[derive(Debug)]
pub struct ProfileRecorder {
    pub profile: Profile,
    pub temporary_sample_buffer: Ringbuffer,
    pub backtrace_state: BacktraceState,
    known_values: HashSet<VALUE>,
}

impl ProfileRecorder {
    pub fn new() -> Self {
        let profile = Profile {
            start_timestamp: Instant::now(),
            samples: vec![],
        };

        let backtrace_state = unsafe {
            let ptr = backtrace_create_state(
                null_mut(),
                1,
                Some(Backtrace::backtrace_error_callback),
                null_mut(),
            );
            BacktraceState::new(ptr)
        };

        Self {
            profile,
            temporary_sample_buffer: Ringbuffer::new(DEFAULT_RINGBUFFER_CAPACITY),
            backtrace_state,
            known_values: HashSet::new(),
        }
    }

    pub fn flush_temporary_sample_buffer(&mut self) {
        while let Some(sample) = self.temporary_sample_buffer.pop() {
            self.known_values.insert(sample.ruby_thread);
            for frame in sample.frames.iter() {
                if frame == &0 {
                    break;
                }
                self.known_values.insert(*frame);
            }
            self.profile.samples.push(sample);
        }
    }

    pub unsafe fn dmark(&self) {
        for value in self.known_values.iter() {
            rb_gc_mark(*value);
        }
        self.temporary_sample_buffer.dmark();
    }
}
