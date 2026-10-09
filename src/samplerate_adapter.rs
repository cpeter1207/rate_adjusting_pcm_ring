//! Narrow runtime binding to the separately versioned sample-rate adapter.
//!
//! ADR 0022 deliberately keeps direct resampler FFI out of the ring. The
//! adapter descriptor is resolved once during ring construction and its
//! persistent converter is used only by the sole consumer thereafter.

use core::ffi::{CStr, c_char, c_int, c_void};
use core::mem::size_of;
use core::ptr::{self, NonNull};

/// ABI implemented by the separately versioned sample-rate adapter.
const ADAPTER_ABI_VERSION: u32 = 2;
/// Successful sample-rate adapter operation result.
const ADAPTER_OK: c_int = 0;
/// Capability required from the selected dynamic adapter.
const REQUIRED_CAPABILITY: &[u8] = b"rptadv.samplerate\0";

/// Adapter callback that creates one mono persistent converter.
type ConverterCreate = unsafe extern "C" fn(u32, u32, u32, u32, *mut *mut c_void) -> c_int;
/// Adapter callback that resets one stopped converter.
type ConverterReset = unsafe extern "C" fn(*mut c_void) -> c_int;
/// Adapter callback that processes bounded canonical F32 PCM.
type ConverterProcess = unsafe extern "C" fn(
    *mut c_void,
    *const f32,
    u32,
    *mut f32,
    u32,
    f64,
    *mut u32,
    *mut u32,
) -> c_int;
/// Adapter callback that destroys one stopped converter.
type ConverterDestroy = unsafe extern "C" fn(*mut c_void);
/// Adapter callback reporting queued input beyond the fixed filter delay.
type ConverterQueuedInput = unsafe extern "C" fn(*mut c_void, *mut u32) -> c_int;
/// Adapter callback reporting the fixed FIR delay in output frames.
type ConverterOutputDelay = unsafe extern "C" fn(*mut c_void, *mut u32) -> c_int;

/// C-compatible descriptor exported by `librptadv_samplerate_adapter.so.2`.
#[repr(C)]
pub(crate) struct AdapterDescriptor {
    pub(crate) struct_size: u32,
    pub(crate) abi_version: u32,
    pub(crate) capability_name: *const c_char,
    pub(crate) converter_create: Option<ConverterCreate>,
    pub(crate) converter_reset: Option<ConverterReset>,
    pub(crate) converter_process: Option<ConverterProcess>,
    pub(crate) converter_destroy: Option<ConverterDestroy>,
    pub(crate) converter_queued_input: Option<ConverterQueuedInput>,
    pub(crate) converter_output_delay: Option<ConverterOutputDelay>,
}

// Descriptors are immutable function tables supplied by a loaded shared
// object.  They contain no mutable state and remain valid for that object's
// process lifetime.
unsafe impl Sync for AdapterDescriptor {}

/// Validated adapter callbacks retained by one ring instance.
#[derive(Clone, Copy)]
pub(crate) struct AdapterFunctions {
    create: ConverterCreate,
    reset: ConverterReset,
    process: ConverterProcess,
    destroy: ConverterDestroy,
    queued_input: ConverterQueuedInput,
    output_delay: ConverterOutputDelay,
}

/// One persistent converter owned exclusively by a ring consumer.
pub(crate) struct Converter {
    handle: NonNull<c_void>,
    functions: AdapterFunctions,
    pub(crate) output_delay_samples: u32,
}

impl AdapterFunctions {
    /// Validate an adapter descriptor before any real-time operation begins.
    pub(crate) unsafe fn from_descriptor(descriptor: *const AdapterDescriptor) -> Result<Self, ()> {
        let descriptor = unsafe { descriptor.as_ref() }.ok_or(())?;
        if descriptor.struct_size < size_of::<AdapterDescriptor>() as u32
            || descriptor.abi_version != ADAPTER_ABI_VERSION
            || descriptor.capability_name.is_null()
        {
            return Err(());
        }
        let capability = unsafe { CStr::from_ptr(descriptor.capability_name) };
        if capability.to_bytes_with_nul() != REQUIRED_CAPABILITY {
            return Err(());
        }
        let create = descriptor.converter_create.ok_or(())?;
        let reset = descriptor.converter_reset.ok_or(())?;
        let process = descriptor.converter_process.ok_or(())?;
        let destroy = descriptor.converter_destroy.ok_or(())?;
        let queued_input = descriptor.converter_queued_input.ok_or(())?;
        let output_delay = descriptor.converter_output_delay.ok_or(())?;
        Ok(Self {
            create,
            reset,
            process,
            destroy,
            queued_input,
            output_delay,
        })
    }

    /// Create the preallocated converter outside the real-time path.
    pub(crate) fn create_converter(
        self,
        input_rate: u32,
        output_rate: u32,
        max_input: u32,
        max_output: u32,
    ) -> Result<Converter, ()> {
        let mut handle = ptr::null_mut();
        let result =
            unsafe { (self.create)(input_rate, output_rate, max_input, max_output, &mut handle) };
        if result != ADAPTER_OK {
            return Err(());
        }
        let Some(handle) = NonNull::new(handle) else {
            return Err(());
        };
        let mut converter = Converter {
            handle,
            functions: self,
            output_delay_samples: 0,
        };
        if converter.reset().is_err() {
            return Err(());
        }
        let result =
            unsafe { (self.output_delay)(handle.as_ptr(), &mut converter.output_delay_samples) };
        if result != ADAPTER_OK {
            return Err(());
        }
        Ok(converter)
    }
}

impl Converter {
    /// Discard prior-burst history on the sole consumer endpoint.
    pub(crate) fn reset(&mut self) -> Result<(), ()> {
        let result = unsafe { (self.functions.reset)(self.handle.as_ptr()) };
        if result == ADAPTER_OK {
            Ok(())
        } else {
            Err(())
        }
    }

    /// Report accepted input still queued beyond fixed filter lookahead.
    pub(crate) fn queued_input(&mut self) -> Result<u32, ()> {
        let mut frames = 0;
        let result = unsafe { (self.functions.queued_input)(self.handle.as_ptr(), &mut frames) };
        if result == ADAPTER_OK {
            Ok(frames)
        } else {
            Err(())
        }
    }

    /// Convert one bounded preallocated input/output chunk without allocation.
    pub(crate) fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        ratio: f64,
    ) -> Result<(usize, usize), ()> {
        if output.is_empty() || !ratio.is_finite() || ratio <= 0.0 {
            return Err(());
        }
        // Ring creation bounds the input workspace to the adapter's u32 ABI;
        // the output cache never exceeds the fixed conversion quantum.
        let input_length = input.len() as u32;
        let output_length = output.len() as u32;
        let mut input_used = 0_u32;
        let mut output_generated = 0_u32;
        let result = unsafe {
            (self.functions.process)(
                self.handle.as_ptr(),
                input.as_ptr(),
                input_length,
                output.as_mut_ptr(),
                output_length,
                ratio,
                &mut input_used,
                &mut output_generated,
            )
        };
        if result != ADAPTER_OK || input_used > input_length || output_generated > output_length {
            return Err(());
        }
        Ok((input_used as usize, output_generated as usize))
    }
}

impl Drop for Converter {
    /// Release the adapter-owned persistent converter at control-plane teardown.
    fn drop(&mut self) {
        unsafe {
            (self.functions.destroy)(self.handle.as_ptr());
        }
    }
}

#[link(name = "rptadv_samplerate_adapter")]
unsafe extern "C" {
    /// Return the process-lifetime descriptor from the selected dynamic adapter.
    fn rptadv_samplerate_adapter_descriptor() -> *const AdapterDescriptor;
}

/// Resolve the mandatory dynamic sample-rate adapter during ring creation.
pub(crate) fn load_functions() -> Result<AdapterFunctions, ()> {
    let descriptor = unsafe { rptadv_samplerate_adapter_descriptor() };
    unsafe { AdapterFunctions::from_descriptor(descriptor) }
}
