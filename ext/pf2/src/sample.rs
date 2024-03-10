use std::time::Instant;

use rb_sys::*;

use crate::backtrace::{Backtrace, BacktraceState};

const MAX_STACK_DEPTH: usize = 500;
const MAX_C_STACK_DEPTH: usize = 1000;

#[derive(Debug, PartialEq)]
pub struct Sample {
    pub ruby_thread: VALUE,
    pub timestamp: Instant,
    pub line_count: i32,
    pub frames: [VALUE; MAX_STACK_DEPTH],
    pub linenos: [i32; MAX_STACK_DEPTH],
    pub c_backtrace_pcs: [usize; MAX_C_STACK_DEPTH + 1],
}

impl Sample {
    /// Captures the stack trace of a Ruby Thread.
    /// The native stack trace is captured as well if `backtrace_state` is provided.
    ///
    /// # Notes
    ///
    /// This functions is NOT strictly async-signal-safe as it calls
    /// rb_profile_thread_frames(), which is not async-signal-safe. However, it
    /// seems to work in practice.
    pub fn capture(ruby_thread: VALUE, backtrace_state: Option<&BacktraceState>) -> Self {
        // Capture the native stack trace
        // c_backtrace_pcs[0] = Depth of native backtrace
        let mut c_backtrace_pcs = [0; MAX_C_STACK_DEPTH + 1];
        if let Some(backtrace_state) = backtrace_state {
            Backtrace::backtrace_simple(
                backtrace_state,
                0,
                |pc: usize| -> i32 {
                    if c_backtrace_pcs[0] >= MAX_C_STACK_DEPTH {
                        return 1;
                    }
                    c_backtrace_pcs[0] += 1;
                    c_backtrace_pcs[c_backtrace_pcs[0]] = pc;
                    0
                },
                Some(Backtrace::backtrace_error_callback),
            );
        }

        let mut sample = Sample {
            ruby_thread,
            timestamp: Instant::now(),
            line_count: 0,
            frames: [0; MAX_STACK_DEPTH],
            linenos: [0; MAX_STACK_DEPTH],
            c_backtrace_pcs,
        };

        // Capture the Ruby stack trace
        unsafe {
            sample.line_count = rb_profile_thread_frames(
                ruby_thread,
                0,
                2000,
                sample.frames.as_mut_ptr(),
                sample.linenos.as_mut_ptr(),
            );
        };
        sample
    }

    pub unsafe fn dmark(&self) {
        rb_gc_mark(self.ruby_thread);
        for frame in self.frames.iter() {
            rb_gc_mark(*frame);
        }
    }
}
