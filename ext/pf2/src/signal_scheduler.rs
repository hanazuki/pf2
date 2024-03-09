#![deny(unsafe_op_in_unsafe_fn)]

mod configuration;
mod timer_installer;

use self::configuration::{Configuration, TimeMode};
use self::timer_installer::TimerInstaller;
use crate::profile_recorder::ProfileRecorder;
use crate::profile_serializer::ProfileSerializer;
use crate::sample::Sample;

use core::panic;
use std::collections::HashSet;
use std::ffi::{c_int, c_void, CStr, CString};
use std::mem::ManuallyDrop;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;
use std::{mem, ptr::null_mut};

use rb_sys::*;

use crate::util::*;

#[derive(Debug)]
pub struct SignalScheduler {
    configuration: Option<configuration::Configuration>,
    profile_recorder: Option<Arc<RwLock<ProfileRecorder>>>,
}

pub struct SignalHandlerArgs {
    profile_recorder: Arc<RwLock<ProfileRecorder>>,
    context_ruby_thread: VALUE,
}

impl SignalScheduler {
    fn new() -> Self {
        Self {
            configuration: None,
            profile_recorder: None,
        }
    }

    fn initialize(&mut self, argc: c_int, argv: *const VALUE, _rbself: VALUE) -> VALUE {
        // Parse arguments
        let kwargs: VALUE = Qnil.into();
        unsafe {
            rb_scan_args(argc, argv, cstr!(":"), &kwargs);
        };
        let mut kwargs_values: [VALUE; 4] = [Qnil.into(); 4];
        unsafe {
            rb_get_kwargs(
                kwargs,
                [
                    rb_intern(cstr!("interval_ms")),
                    rb_intern(cstr!("threads")),
                    rb_intern(cstr!("time_mode")),
                    rb_intern(cstr!("track_new_threads")),
                ]
                .as_mut_ptr(),
                0,
                4,
                kwargs_values.as_mut_ptr(),
            );
        };
        let interval: Duration = if kwargs_values[0] != Qundef as VALUE {
            let interval_ms = unsafe { rb_num2long(kwargs_values[0]) };
            Duration::from_millis(interval_ms.try_into().unwrap_or_else(|_| {
                eprintln!(
                    "[Pf2] Warning: Specified interval ({}) is not valid. Using default value (49ms).",
                    interval_ms
                );
                49
            }))
        } else {
            Duration::from_millis(49)
        };
        let threads: VALUE = if kwargs_values[1] != Qundef as VALUE {
            kwargs_values[1]
        } else {
            unsafe { rb_funcall(rb_cThread, rb_intern(cstr!("list")), 0) }
        };
        let time_mode: configuration::TimeMode = if kwargs_values[2] != Qundef as VALUE {
            let specified_mode = unsafe {
                let mut str = rb_funcall(kwargs_values[2], rb_intern(cstr!("to_s")), 0);
                let ptr = rb_string_value_ptr(&mut str);
                CStr::from_ptr(ptr).to_str().unwrap()
            };
            configuration::TimeMode::from_str(specified_mode).unwrap_or_else(|_| {
                // Raise an ArgumentError
                unsafe {
                    rb_raise(
                        rb_eArgError,
                        cstr!("Invalid time mode. Valid values are 'cpu' and 'wall'."),
                    )
                }
            })
        } else {
            configuration::TimeMode::CpuTime
        };
        let track_new_threads: bool = if kwargs_values[3] != Qundef as VALUE {
            RTEST(kwargs_values[3])
        } else {
            false
        };

        let mut target_ruby_threads = HashSet::new();
        unsafe {
            for i in 0..RARRAY_LEN(threads) {
                let ruby_thread: VALUE = rb_ary_entry(threads, i);
                target_ruby_threads.insert(ruby_thread);
            }
        }

        self.configuration = Some(Configuration {
            interval,
            target_ruby_threads,
            time_mode,
            track_new_threads,
        });

        Qnil.into()
    }

    fn start(&mut self, _rbself: VALUE) -> VALUE {
        let profile_recorder = Arc::new(RwLock::new(ProfileRecorder::new()));
        self.start_profile_buffer_flusher_thread(&profile_recorder);
        self.install_signal_handler();

        TimerInstaller::install_timer_to_ruby_threads(
            self.configuration.as_ref().unwrap().clone(), // FIXME: don't clone
            Arc::clone(&profile_recorder),
        );

        self.profile_recorder = Some(profile_recorder);

        Qtrue.into()
    }

    fn stop(&mut self, _rbself: VALUE) -> VALUE {
        if let Some(profile_recorder) = &self.profile_recorder {
            // Finalize
            match profile_recorder.try_write() {
                Ok(mut profile_recorder) => {
                    profile_recorder.flush_temporary_sample_buffer();
                }
                Err(_) => {
                    println!("[pf2 ERROR] stop: Failed to acquire profile lock.");
                    return Qfalse.into();
                }
            }

            let profile_recorder = profile_recorder.try_read().unwrap();
            log::debug!(
                "Number of samples: {}",
                profile_recorder.profile.samples.len()
            );

            let serialized = ProfileSerializer::serialize(&profile_recorder);
            let serialized = CString::new(serialized).unwrap();
            unsafe { rb_str_new_cstr(serialized.as_ptr()) }
        } else {
            panic!("stop() called before start()");
        }
    }

    // Install signal handler for profiling events to the current process.
    fn install_signal_handler(&self) {
        let mut sa: libc::sigaction = unsafe { mem::zeroed() };
        sa.sa_sigaction = Self::signal_handler as usize;
        sa.sa_flags = libc::SA_SIGINFO;
        let err = unsafe { libc::sigaction(libc::SIGALRM, &sa, null_mut()) };
        if err != 0 {
            panic!("sigaction failed: {}", err);
        }
    }

    // Respond to the signal and collect a sample.
    // This function is called when a timer fires.
    //
    // Expected to be async-signal-safe, but the current implementation is not.
    extern "C" fn signal_handler(
        _sig: c_int,
        info: *mut libc::siginfo_t,
        _ucontext: *mut libc::ucontext_t,
    ) {
        let args = unsafe {
            let ptr = extract_si_value_sival_ptr(info) as *mut SignalHandlerArgs;
            ManuallyDrop::new(Box::from_raw(ptr))
        };

        let mut profile_recorder = match args.profile_recorder.try_write() {
            Ok(profile_recorder) => profile_recorder,
            Err(_) => {
                // FIXME: Do we want to properly collect GC samples? I don't know yet.
                log::trace!("Failed to acquire profile lock (garbage collection possibly in progress). Dropping sample.");
                return;
            }
        };

        let sample = Sample::capture(args.context_ruby_thread, &profile_recorder.backtrace_state); // NOT async-signal-safe
        if profile_recorder
            .temporary_sample_buffer
            .push(sample)
            .is_err()
        {
            log::debug!("Temporary sample buffer full. Dropping sample.");
        }
    }

    fn start_profile_buffer_flusher_thread(&self, profile_recorder: &Arc<RwLock<ProfileRecorder>>) {
        let profile_recorder = Arc::clone(profile_recorder);
        thread::spawn(move || loop {
            log::trace!("Flushing temporary sample buffer");
            match profile_recorder.try_write() {
                Ok(mut profile_recorder) => {
                    profile_recorder.flush_temporary_sample_buffer();
                }
                Err(_) => {
                    log::debug!("flusher: Failed to acquire profile lock");
                }
            }
            thread::sleep(Duration::from_millis(500));
        });
    }

    // Ruby Methods

    pub unsafe extern "C" fn rb_initialize(
        argc: c_int,
        argv: *const VALUE,
        rbself: VALUE,
    ) -> VALUE {
        let mut collector = unsafe { Self::get_struct_from(rbself) };
        collector.initialize(argc, argv, rbself)
    }

    pub unsafe extern "C" fn rb_start(rbself: VALUE) -> VALUE {
        let mut collector = unsafe { Self::get_struct_from(rbself) };
        collector.start(rbself)
    }

    pub unsafe extern "C" fn rb_stop(rbself: VALUE) -> VALUE {
        let mut collector = unsafe { Self::get_struct_from(rbself) };
        collector.stop(rbself)
    }

    // Functions for TypedData

    // Extract the SignalScheduler struct from a Ruby object
    unsafe fn get_struct_from(obj: VALUE) -> ManuallyDrop<Box<Self>> {
        unsafe {
            let ptr = rb_check_typeddata(obj, &RBDATA);
            ManuallyDrop::new(Box::from_raw(ptr as *mut SignalScheduler))
        }
    }

    #[allow(non_snake_case)]
    pub unsafe extern "C" fn rb_alloc(_rbself: VALUE) -> VALUE {
        let collector = Box::new(SignalScheduler::new());
        unsafe { Arc::increment_strong_count(&collector) };

        unsafe {
            let rb_mPf2: VALUE = rb_define_module(cstr!("Pf2"));
            let rb_cSignalScheduler =
                rb_define_class_under(rb_mPf2, cstr!("SignalScheduler"), rb_cObject);

            // "Wrap" the SignalScheduler struct into a Ruby object
            rb_data_typed_object_wrap(
                rb_cSignalScheduler,
                Box::into_raw(collector) as *mut c_void,
                &RBDATA,
            )
        }
    }

    unsafe extern "C" fn dmark(ptr: *mut c_void) {
        unsafe {
            let collector = ManuallyDrop::new(Box::from_raw(ptr as *mut SignalScheduler));
            if let Some(profile_recorder) = &collector.profile_recorder {
                match profile_recorder.read() {
                    Ok(profile_recorder) => {
                        profile_recorder.dmark();
                    }
                    Err(_) => {
                        panic!("[pf2 FATAL] dmark: Failed to acquire profile lock.");
                    }
                }
            }
        }
    }

    unsafe extern "C" fn dfree(ptr: *mut c_void) {
        unsafe {
            drop(Box::from_raw(ptr as *mut SignalScheduler));
        }
    }

    unsafe extern "C" fn dsize(_: *const c_void) -> size_t {
        // FIXME: Report something better
        mem::size_of::<SignalScheduler>() as size_t
    }
}

static mut RBDATA: rb_data_type_t = rb_data_type_t {
    wrap_struct_name: cstr!("SignalScheduler"),
    function: rb_data_type_struct__bindgen_ty_1 {
        dmark: Some(SignalScheduler::dmark),
        dfree: Some(SignalScheduler::dfree),
        dsize: Some(SignalScheduler::dsize),
        dcompact: None,
        reserved: [null_mut(); 1],
    },
    parent: null_mut(),
    data: null_mut(),
    flags: 0,
};
