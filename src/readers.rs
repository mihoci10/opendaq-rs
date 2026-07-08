//! Typed sample readers.
//!
//! openDAQ's readers exchange sample data through untyped buffers whose
//! element type is fixed when the reader is created.  The Rust readers carry
//! that type statically instead: `StreamReader<V, D>` reads values as `V` and
//! domain stamps as `D` (defaulting to `f64` values / `i64` ticks, like the
//! other bindings), so `read` returns plain typed vectors and no caller ever
//! touches a raw buffer.
//!
//! Dimensioned ("2-D") signals -- e.g. FFT amplitude spectra -- pack several
//! values into each sample.  Reads return [`Samples`], which carries the
//! sample width alongside the flat data; for plain scalar signals the width
//! is 1 and `Samples` behaves like a `Vec`.  The width must match the
//! descriptor the *reader* is currently decoding, which can lag the signal's
//! (the reader drains a packet queue), so stream readers keep `skip_events`
//! off by default and track the width from their own descriptor-changed
//! events, mirroring the cl-opendaq bindings.

use std::ffi::c_void;
use std::marker::PhantomData;
use std::sync::Mutex;

use crate::error::{check, Error, Result};
use crate::generated::{
    DataDescriptor, EventPacket, InputPortConfig, Packet, ReaderStatus, Signal,
};
use crate::marshal;
use crate::object::{Interface, Ref};
use crate::sys::{self, ReadMode, ReadStatus, ReadTimeoutType, SampleType};
use crate::value::Value;

/// A Rust element type a reader can decode samples into.
pub trait Sample: Copy + Default + Send + Sync + 'static + crate::sealed::Sealed {
    /// The openDAQ sample type this Rust type stores.
    const SAMPLE_TYPE: SampleType;
}

macro_rules! impl_sample {
    ($($t:ty => $st:ident),* $(,)?) => {$(
        impl crate::sealed::Sealed for $t {}
        impl Sample for $t {
            const SAMPLE_TYPE: SampleType = SampleType::$st;
        }
    )*};
}

impl_sample! {
    f32 => Float32,
    f64 => Float64,
    u8 => UInt8,
    i8 => Int8,
    u16 => UInt16,
    i16 => Int16,
    u32 => UInt32,
    i32 => Int32,
    u64 => UInt64,
    i64 => Int64,
}

/// Samples returned by a read: flat `values` plus the per-sample `width`
/// (1 for scalar signals; the dimension size for dimensioned signals; the
/// block size for block readers).  Dereferences to the flat value slice, so
/// for ordinary scalar signals it is used exactly like a `Vec`.
#[derive(Debug, Clone, PartialEq)]
pub struct Samples<V> {
    values: Vec<V>,
    width: usize,
}

impl<V> Samples<V> {
    fn new(values: Vec<V>, width: usize) -> Samples<V> {
        Samples {
            values,
            width: width.max(1),
        }
    }

    /// The flat values, `sample_count() * width()` of them.
    pub fn values(&self) -> &[V] {
        &self.values
    }

    /// Number of values in one sample (1 unless the signal is dimensioned).
    pub fn width(&self) -> usize {
        self.width
    }

    /// Number of samples read.
    pub fn sample_count(&self) -> usize {
        self.values.len() / self.width
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// One slice of `width()` values per sample.
    pub fn rows(&self) -> std::slice::ChunksExact<'_, V> {
        self.values.chunks_exact(self.width)
    }

    /// The flat values, consuming `self`.
    pub fn into_values(self) -> Vec<V> {
        self.values
    }
}

impl<V> std::ops::Deref for Samples<V> {
    type Target = [V];
    fn deref(&self) -> &[V] {
        &self.values
    }
}

impl<'a, V> IntoIterator for &'a Samples<V> {
    type Item = &'a V;
    type IntoIter = std::slice::Iter<'a, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}

impl<V> IntoIterator for Samples<V> {
    type Item = V;
    type IntoIter = std::vec::IntoIter<V>;
    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
    }
}

pub(crate) const READER_INVALIDATED_OPERATION: &str = "opendaq::reader::read";

fn reader_invalidated() -> Error {
    Error::new(
        sys::OPENDAQ_ERR_INVALIDSTATE,
        READER_INVALIDATED_OPERATION,
        Some(
            "the reader was invalidated by a descriptor change it cannot convert; recover by \
             building a from_existing reader with read types matching the new descriptor"
                .into(),
        ),
    )
}

/// Number of scalar values in one sample described by `descriptor`: the size
/// of its single dimension, or 1 -- mirroring the openDAQ readers, which
/// unpack exactly one dimension.
fn descriptor_sample_width(descriptor: &DataDescriptor) -> Result<usize> {
    let dimensions = descriptor.dimensions()?;
    if dimensions.len() == 1 {
        Ok(dimensions[0].size()?.max(1))
    } else {
        Ok(1)
    }
}

fn signal_sample_width(signal: Option<&Signal>) -> usize {
    match signal.and_then(|s| s.descriptor().ok().flatten()) {
        Some(descriptor) => descriptor_sample_width(&descriptor).unwrap_or(1),
        None => 1,
    }
}

/// The value sample width announced by a reader status' descriptor-changed
/// event, if it carries a value descriptor.
fn event_value_width(status: &ReaderStatus) -> Option<usize> {
    if status.read_status().ok()? != ReadStatus::Event {
        return None;
    }
    let packet: EventPacket = status.event_packet().ok()??;
    let parameters = packet.parameters().ok()?;
    let descriptor = match parameters.get("DataDescriptor")? {
        Value::Object(o) => o.try_cast::<DataDescriptor>()?,
        _ => return None,
    };
    descriptor_sample_width(&descriptor).ok()
}

struct RawRead {
    samples_read: usize,
    status: Option<ReaderStatus>,
}

/// The shared width-tracking read loop (see the module docs): re-reads past
/// zero-sample descriptor events, caches the width each event announces for
/// the next batch, and surfaces invalidation as an error.
fn read_tracking_width<T>(
    signal: Option<&Signal>,
    cached_width: &Mutex<Option<usize>>,
    mut read_into: impl FnMut(usize) -> Result<(T, RawRead)>,
) -> Result<T> {
    let mut width = cached_width
        .lock()
        .unwrap()
        .unwrap_or_else(|| signal_sample_width(signal));
    let mut last = None;
    for _ in 0..1024 {
        let (result, raw) = read_into(width)?;
        let event_width = raw.status.as_ref().and_then(event_value_width);
        if raw.samples_read > 0 {
            if let Some(w) = event_width {
                *cached_width.lock().unwrap() = Some(w);
            }
            return Ok(result);
        }
        // Checked before the event branch: an invalidating event also carries
        // a descriptor, but re-reading an invalid reader can never succeed.
        if let Some(status) = &raw.status {
            if !status.valid()? {
                return Err(reader_invalidated());
            }
            if status.read_status()? == ReadStatus::Event {
                // A zero-sample descriptor-changed event: adopt any new width
                // and re-read rather than surface an empty result.
                if let Some(w) = event_width {
                    width = w;
                    *cached_width.lock().unwrap() = Some(w);
                }
                last = Some(result);
                continue;
            }
        }
        // Genuine zero-sample read (e.g. a timeout with no data).
        return Ok(result);
    }
    last.map(Ok).unwrap_or_else(|| Err(reader_invalidated()))
}

fn check_status_valid(status: Option<&ReaderStatus>) -> Result<()> {
    if let Some(status) = status {
        if !status.valid()? {
            return Err(reader_invalidated());
        }
    }
    Ok(())
}

macro_rules! reader_common_methods {
    () => {
        /// The raw interface pointer (for use with [`crate::sys`]).
        pub fn as_raw(&self) -> *mut c_void {
            self.inner.as_ptr()
        }

        /// Number of samples currently available to read.
        pub fn available_count(&self) -> Result<usize> {
            let mut count: usize = 0;
            check(
                unsafe {
                    (sys::api().daqReader_getAvailableCount)(
                        self.inner.as_ptr() as *mut _,
                        &mut count,
                    )
                },
                "daqReader_getAvailableCount",
            )?;
            Ok(count)
        }

        /// True when no samples are available.
        pub fn empty(&self) -> Result<bool> {
            let mut empty: u8 = 0;
            check(
                unsafe {
                    (sys::api().daqReader_getEmpty)(self.inner.as_ptr() as *mut _, &mut empty)
                },
                "daqReader_getEmpty",
            )?;
            Ok(empty != 0)
        }

        /// The openDAQ sample type values are read as (`V::SAMPLE_TYPE`).
        pub fn value_read_type(&self) -> SampleType {
            V::SAMPLE_TYPE
        }

        /// The openDAQ sample type domain stamps are read as (`D::SAMPLE_TYPE`).
        pub fn domain_read_type(&self) -> SampleType {
            D::SAMPLE_TYPE
        }
    };
}

// ---------------------------------------------------------------------------
// Stream reader
// ---------------------------------------------------------------------------

/// Options for constructing a [`StreamReader`]; the defaults mirror the
/// other openDAQ bindings.
#[derive(Debug, Clone, Copy)]
pub struct StreamReaderOptions {
    pub read_mode: ReadMode,
    pub timeout_type: ReadTimeoutType,
    /// Off by default so `read` sees descriptor-changed events and can track
    /// the value sample width of dimensioned signals.
    pub skip_events: bool,
}

impl Default for StreamReaderOptions {
    fn default() -> Self {
        StreamReaderOptions {
            read_mode: ReadMode::Scaled,
            timeout_type: ReadTimeoutType::All,
            skip_events: false,
        }
    }
}

/// A signal data reader that keeps an internal read position and advances it
/// on every read.  `V` is the Rust type values are converted to, `D` the type
/// for domain stamps.
pub struct StreamReader<V: Sample = f64, D: Sample = i64> {
    inner: Ref,
    signal: Option<Signal>,
    width: Mutex<Option<usize>>,
    _marker: PhantomData<(V, D)>,
}

impl<V: Sample, D: Sample> StreamReader<V, D> {
    /// Create a reader over `signal` with default options.
    ///
    /// Built through `daqStreamReaderBuilder`, like the other bindings, so
    /// event skipping is configurable (the plain create call has no such
    /// parameter).
    pub fn new(signal: &Signal) -> Result<Self> {
        Self::with_options(signal, StreamReaderOptions::default())
    }

    /// Create a reader over `signal` with explicit [`StreamReaderOptions`].
    pub fn with_options(signal: &Signal, options: StreamReaderOptions) -> Result<Self> {
        let api = sys::api();
        let mut builder: *mut sys::daqStreamReaderBuilder = std::ptr::null_mut();
        check(
            unsafe { (api.daqStreamReaderBuilder_createStreamReaderBuilder)(&mut builder) },
            "daqStreamReaderBuilder_createStreamReaderBuilder",
        )?;
        let builder = unsafe { Ref::from_owned(builder as *mut c_void) }.ok_or_else(|| {
            Error::new(
                sys::OPENDAQ_ERR_GENERALERROR,
                "createStreamReaderBuilder",
                None,
            )
        })?;
        let b = builder.as_ptr() as *mut sys::daqStreamReaderBuilder;
        unsafe {
            check(
                (api.daqStreamReaderBuilder_setSignal)(b, signal.as_raw() as *mut _),
                "setSignal",
            )?;
            check(
                (api.daqStreamReaderBuilder_setValueReadType)(b, V::SAMPLE_TYPE as u32),
                "setValueReadType",
            )?;
            check(
                (api.daqStreamReaderBuilder_setDomainReadType)(b, D::SAMPLE_TYPE as u32),
                "setDomainReadType",
            )?;
            check(
                (api.daqStreamReaderBuilder_setReadMode)(b, options.read_mode as u32),
                "setReadMode",
            )?;
            check(
                (api.daqStreamReaderBuilder_setReadTimeoutType)(b, options.timeout_type as u32),
                "setReadTimeoutType",
            )?;
            check(
                (api.daqStreamReaderBuilder_setSkipEvents)(b, u8::from(options.skip_events)),
                "setSkipEvents",
            )?;
        }
        let mut reader: *mut sys::daqStreamReader = std::ptr::null_mut();
        check(
            unsafe { (api.daqStreamReaderBuilder_build)(b, &mut reader) },
            "daqStreamReaderBuilder_build",
        )?;
        let inner = unsafe { Ref::from_owned(reader as *mut c_void) }.ok_or_else(|| {
            Error::new(
                sys::OPENDAQ_ERR_GENERALERROR,
                "daqStreamReaderBuilder_build",
                None,
            )
        })?;
        Ok(StreamReader {
            inner,
            signal: Some(signal.clone()),
            width: Mutex::new(None),
            _marker: PhantomData,
        })
    }

    /// Create a reader fed by an input port instead of a signal.
    ///
    /// Calls the openDAQ C function `daqStreamReader_createStreamReaderFromPort()`.
    pub fn from_port(port: &InputPortConfig) -> Result<Self> {
        let op = "daqStreamReader_createStreamReaderFromPort";
        let mut reader: *mut sys::daqStreamReader = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqStreamReader_createStreamReaderFromPort)(
                    &mut reader,
                    port.as_raw() as *mut _,
                    V::SAMPLE_TYPE as u32,
                    D::SAMPLE_TYPE as u32,
                    ReadMode::Scaled as u32,
                    ReadTimeoutType::All as u32,
                )
            },
            op,
        )?;
        let inner = unsafe { Ref::from_owned(reader as *mut c_void) }
            .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_GENERALERROR, op, None))?;
        Ok(StreamReader {
            inner,
            signal: None,
            width: Mutex::new(None),
            _marker: PhantomData,
        })
    }

    /// Rebuild an invalidated reader with this type's read types, inheriting
    /// the unread packets (see the invalidation note on [`StreamReader::read`]).
    ///
    /// Calls the openDAQ C function `daqStreamReader_createStreamReaderFromExisting()`.
    pub fn from_existing<V2: Sample, D2: Sample>(
        invalidated: &StreamReader<V2, D2>,
    ) -> Result<Self> {
        let op = "daqStreamReader_createStreamReaderFromExisting";
        let mut reader: *mut sys::daqStreamReader = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqStreamReader_createStreamReaderFromExisting)(
                    &mut reader,
                    invalidated.inner.as_ptr() as *mut _,
                    V::SAMPLE_TYPE as u32,
                    D::SAMPLE_TYPE as u32,
                )
            },
            op,
        )?;
        let inner = unsafe { Ref::from_owned(reader as *mut c_void) }
            .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_GENERALERROR, op, None))?;
        Ok(StreamReader {
            inner,
            signal: invalidated.signal.clone(),
            width: Mutex::new(None),
            _marker: PhantomData,
        })
    }

    reader_common_methods!();

    /// Read up to `count` samples, waiting at most `timeout_ms`.
    ///
    /// Returns the samples actually read (possibly none on timeout).  When
    /// the signal's descriptor changes to a sample type the reader cannot
    /// convert to `V`/`D`, the reader is *invalidated*: `read` fails with an
    /// error whose [`Error::is_reader_invalidated`] is true, and reading can
    /// be resumed by building a [`StreamReader::from_existing`] reader with
    /// matching read types.
    pub fn read(&self, count: usize, timeout_ms: usize) -> Result<Samples<V>> {
        read_tracking_width(self.signal.as_ref(), &self.width, |width| {
            let mut values = vec![V::default(); count.max(1) * width];
            let mut read = count;
            let mut status: *mut sys::daqReaderStatus = std::ptr::null_mut();
            check(
                unsafe {
                    (sys::api().daqStreamReader_read)(
                        self.inner.as_ptr() as *mut _,
                        values.as_mut_ptr() as *mut c_void,
                        &mut read,
                        timeout_ms,
                        &mut status,
                    )
                },
                "daqStreamReader_read",
            )?;
            let status = unsafe { marshal::take_object::<ReaderStatus>(status as *mut _) };
            values.truncate(read * width);
            Ok((
                Samples::new(values, width),
                RawRead {
                    samples_read: read,
                    status,
                },
            ))
        })
    }

    /// Like [`StreamReader::read`], but also reads one domain stamp (e.g. a
    /// timestamp tick) per sample.
    pub fn read_with_domain(
        &self,
        count: usize,
        timeout_ms: usize,
    ) -> Result<(Samples<V>, Vec<D>)> {
        read_tracking_width(self.signal.as_ref(), &self.width, |width| {
            let mut values = vec![V::default(); count.max(1) * width];
            let mut domain = vec![D::default(); count.max(1)];
            let mut read = count;
            let mut status: *mut sys::daqReaderStatus = std::ptr::null_mut();
            check(
                unsafe {
                    (sys::api().daqStreamReader_readWithDomain)(
                        self.inner.as_ptr() as *mut _,
                        values.as_mut_ptr() as *mut c_void,
                        domain.as_mut_ptr() as *mut c_void,
                        &mut read,
                        timeout_ms,
                        &mut status,
                    )
                },
                "daqStreamReader_readWithDomain",
            )?;
            let status = unsafe { marshal::take_object::<ReaderStatus>(status as *mut _) };
            values.truncate(read * width);
            domain.truncate(read);
            Ok((
                (Samples::new(values, width), domain),
                RawRead {
                    samples_read: read,
                    status,
                },
            ))
        })
    }

    /// Skip up to `count` unread samples, returning how many were skipped.
    pub fn skip_samples(&self, count: usize) -> Result<usize> {
        let mut skipped = count;
        let mut status: *mut sys::daqReaderStatus = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqStreamReader_skipSamples)(
                    self.inner.as_ptr() as *mut _,
                    &mut skipped,
                    &mut status,
                )
            },
            "daqStreamReader_skipSamples",
        )?;
        let status = unsafe { marshal::take_object::<ReaderStatus>(status as *mut _) };
        check_status_valid(status.as_ref())?;
        Ok(skipped)
    }
}

// ---------------------------------------------------------------------------
// Tail reader
// ---------------------------------------------------------------------------

/// A reader that always returns the last `history_size` samples of a signal.
pub struct TailReader<V: Sample = f64, D: Sample = i64> {
    inner: Ref,
    signal: Option<Signal>,
    _marker: PhantomData<(V, D)>,
}

impl<V: Sample, D: Sample> TailReader<V, D> {
    /// Create a tail reader keeping the last `history_size` samples.
    ///
    /// Built through `daqTailReaderBuilder` with `skipEvents` on, like the
    /// other bindings, so the initial descriptor-changed event is skipped and
    /// the first read already returns data (the direct create call has no
    /// such parameter).
    pub fn new(signal: &Signal, history_size: usize) -> Result<Self> {
        let api = sys::api();
        let op = "daqTailReaderBuilder_createTailReaderBuilder";
        let mut builder: *mut sys::daqTailReaderBuilder = std::ptr::null_mut();
        check(
            unsafe { (api.daqTailReaderBuilder_createTailReaderBuilder)(&mut builder) },
            op,
        )?;
        let builder = unsafe { Ref::from_owned(builder as *mut c_void) }
            .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_GENERALERROR, op, None))?;
        let b = builder.as_ptr() as *mut sys::daqTailReaderBuilder;
        unsafe {
            check(
                (api.daqTailReaderBuilder_setSignal)(b, signal.as_raw() as *mut _),
                "setSignal",
            )?;
            check(
                (api.daqTailReaderBuilder_setHistorySize)(b, history_size),
                "setHistorySize",
            )?;
            check(
                (api.daqTailReaderBuilder_setValueReadType)(b, V::SAMPLE_TYPE as u32),
                "setValueReadType",
            )?;
            check(
                (api.daqTailReaderBuilder_setDomainReadType)(b, D::SAMPLE_TYPE as u32),
                "setDomainReadType",
            )?;
            check(
                (api.daqTailReaderBuilder_setReadMode)(b, ReadMode::Scaled as u32),
                "setReadMode",
            )?;
            check(
                (api.daqTailReaderBuilder_setSkipEvents)(b, 1),
                "setSkipEvents",
            )?;
        }
        let mut reader: *mut sys::daqTailReader = std::ptr::null_mut();
        check(
            unsafe { (api.daqTailReaderBuilder_build)(b, &mut reader) },
            "daqTailReaderBuilder_build",
        )?;
        let inner = unsafe { Ref::from_owned(reader as *mut c_void) }.ok_or_else(|| {
            Error::new(
                sys::OPENDAQ_ERR_GENERALERROR,
                "daqTailReaderBuilder_build",
                None,
            )
        })?;
        Ok(TailReader {
            inner,
            signal: Some(signal.clone()),
            _marker: PhantomData,
        })
    }

    reader_common_methods!();

    /// The configured history size.
    pub fn history_size(&self) -> Result<usize> {
        let mut size: usize = 0;
        check(
            unsafe {
                (sys::api().daqTailReader_getHistorySize)(self.inner.as_ptr() as *mut _, &mut size)
            },
            "daqTailReader_getHistorySize",
        )?;
        Ok(size)
    }

    /// Read up to `count` of the most recent samples.
    pub fn read(&self, count: usize) -> Result<Samples<V>> {
        let width = signal_sample_width(self.signal.as_ref());
        let mut values = vec![V::default(); count.max(1) * width];
        let mut read = count;
        let mut status: *mut sys::daqTailReaderStatus = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqTailReader_read)(
                    self.inner.as_ptr() as *mut _,
                    values.as_mut_ptr() as *mut c_void,
                    &mut read,
                    &mut status,
                )
            },
            "daqTailReader_read",
        )?;
        let status = unsafe { marshal::take_object::<ReaderStatus>(status as *mut _) };
        check_status_valid(status.as_ref())?;
        values.truncate(read * width);
        Ok(Samples::new(values, width))
    }

    /// Like [`TailReader::read`], but also reads one domain stamp per sample.
    pub fn read_with_domain(&self, count: usize) -> Result<(Samples<V>, Vec<D>)> {
        let width = signal_sample_width(self.signal.as_ref());
        let mut values = vec![V::default(); count.max(1) * width];
        let mut domain = vec![D::default(); count.max(1)];
        let mut read = count;
        let mut status: *mut sys::daqTailReaderStatus = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqTailReader_readWithDomain)(
                    self.inner.as_ptr() as *mut _,
                    values.as_mut_ptr() as *mut c_void,
                    domain.as_mut_ptr() as *mut c_void,
                    &mut read,
                    &mut status,
                )
            },
            "daqTailReader_readWithDomain",
        )?;
        let status = unsafe { marshal::take_object::<ReaderStatus>(status as *mut _) };
        check_status_valid(status.as_ref())?;
        values.truncate(read * width);
        domain.truncate(read);
        Ok((Samples::new(values, width), domain))
    }
}

// ---------------------------------------------------------------------------
// Block reader
// ---------------------------------------------------------------------------

/// A reader that returns samples in whole blocks of a fixed size.
pub struct BlockReader<V: Sample = f64, D: Sample = i64> {
    inner: Ref,
    _marker: PhantomData<(V, D)>,
}

impl<V: Sample, D: Sample> BlockReader<V, D> {
    /// Create a block reader returning blocks of `block_size` samples.
    ///
    /// Built through `daqBlockReaderBuilder` with `skipEvents` on, like the
    /// other bindings, so the initial descriptor-changed event is skipped and
    /// the first read already returns data (the direct create call has no
    /// such parameter).
    pub fn new(signal: &Signal, block_size: usize) -> Result<Self> {
        let api = sys::api();
        let op = "daqBlockReaderBuilder_createBlockReaderBuilder";
        let mut builder: *mut sys::daqBlockReaderBuilder = std::ptr::null_mut();
        check(
            unsafe { (api.daqBlockReaderBuilder_createBlockReaderBuilder)(&mut builder) },
            op,
        )?;
        let builder = unsafe { Ref::from_owned(builder as *mut c_void) }
            .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_GENERALERROR, op, None))?;
        let b = builder.as_ptr() as *mut sys::daqBlockReaderBuilder;
        unsafe {
            check(
                (api.daqBlockReaderBuilder_setSignal)(b, signal.as_raw() as *mut _),
                "setSignal",
            )?;
            check(
                (api.daqBlockReaderBuilder_setBlockSize)(b, block_size),
                "setBlockSize",
            )?;
            check(
                (api.daqBlockReaderBuilder_setValueReadType)(b, V::SAMPLE_TYPE as u32),
                "setValueReadType",
            )?;
            check(
                (api.daqBlockReaderBuilder_setDomainReadType)(b, D::SAMPLE_TYPE as u32),
                "setDomainReadType",
            )?;
            check(
                (api.daqBlockReaderBuilder_setReadMode)(b, ReadMode::Scaled as u32),
                "setReadMode",
            )?;
            check(
                (api.daqBlockReaderBuilder_setSkipEvents)(b, 1),
                "setSkipEvents",
            )?;
        }
        let mut reader: *mut sys::daqBlockReader = std::ptr::null_mut();
        check(
            unsafe { (api.daqBlockReaderBuilder_build)(b, &mut reader) },
            "daqBlockReaderBuilder_build",
        )?;
        let inner = unsafe { Ref::from_owned(reader as *mut c_void) }.ok_or_else(|| {
            Error::new(
                sys::OPENDAQ_ERR_GENERALERROR,
                "daqBlockReaderBuilder_build",
                None,
            )
        })?;
        Ok(BlockReader {
            inner,
            _marker: PhantomData,
        })
    }

    reader_common_methods!();

    /// The configured block size.
    pub fn block_size(&self) -> Result<usize> {
        let mut size: usize = 0;
        check(
            unsafe {
                (sys::api().daqBlockReader_getBlockSize)(self.inner.as_ptr() as *mut _, &mut size)
            },
            "daqBlockReader_getBlockSize",
        )?;
        Ok(size)
    }

    /// Read up to `count` whole blocks; the result's rows are the blocks.
    pub fn read(&self, count: usize, timeout_ms: usize) -> Result<Samples<V>> {
        let block_size = self.block_size()?;
        let mut values = vec![V::default(); count.max(1) * block_size];
        let mut read = count;
        let mut status: *mut sys::daqBlockReaderStatus = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqBlockReader_read)(
                    self.inner.as_ptr() as *mut _,
                    values.as_mut_ptr() as *mut c_void,
                    &mut read,
                    timeout_ms,
                    &mut status,
                )
            },
            "daqBlockReader_read",
        )?;
        let status = unsafe { marshal::take_object::<ReaderStatus>(status as *mut _) };
        check_status_valid(status.as_ref())?;
        values.truncate(read * block_size);
        Ok(Samples::new(values, block_size))
    }

    /// Like [`BlockReader::read`], with the domain stamps read in blocks too.
    pub fn read_with_domain(
        &self,
        count: usize,
        timeout_ms: usize,
    ) -> Result<(Samples<V>, Samples<D>)> {
        let block_size = self.block_size()?;
        let mut values = vec![V::default(); count.max(1) * block_size];
        let mut domain = vec![D::default(); count.max(1) * block_size];
        let mut read = count;
        let mut status: *mut sys::daqBlockReaderStatus = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqBlockReader_readWithDomain)(
                    self.inner.as_ptr() as *mut _,
                    values.as_mut_ptr() as *mut c_void,
                    domain.as_mut_ptr() as *mut c_void,
                    &mut read,
                    timeout_ms,
                    &mut status,
                )
            },
            "daqBlockReader_readWithDomain",
        )?;
        let status = unsafe { marshal::take_object::<ReaderStatus>(status as *mut _) };
        check_status_valid(status.as_ref())?;
        values.truncate(read * block_size);
        domain.truncate(read * block_size);
        Ok((
            Samples::new(values, block_size),
            Samples::new(domain, block_size),
        ))
    }
}

// ---------------------------------------------------------------------------
// Multi reader
// ---------------------------------------------------------------------------

/// A reader over several signals at once, aligned on a common domain.
/// `read` fills one buffer per signal and returns one `Vec` per signal.
pub struct MultiReader<V: Sample = f64, D: Sample = i64> {
    inner: Ref,
    signal_count: usize,
    _marker: PhantomData<(V, D)>,
}

impl<V: Sample, D: Sample> MultiReader<V, D> {
    /// Create a multi reader over `signals` with default options.
    ///
    /// Calls the openDAQ C function `daqMultiReader_createMultiReader()`.
    pub fn new(signals: &[Signal]) -> Result<Self> {
        let op = "daqMultiReader_createMultiReader";
        let list = marshal::list_from_interfaces(signals)?;
        let mut reader: *mut sys::daqMultiReader = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqMultiReader_createMultiReader)(
                    &mut reader,
                    list.as_ptr() as *mut _,
                    V::SAMPLE_TYPE as u32,
                    D::SAMPLE_TYPE as u32,
                    ReadMode::Scaled as u32,
                    ReadTimeoutType::All as u32,
                )
            },
            op,
        )?;
        let inner = unsafe { Ref::from_owned(reader as *mut c_void) }
            .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_GENERALERROR, op, None))?;
        Ok(MultiReader {
            inner,
            signal_count: signals.len(),
            _marker: PhantomData,
        })
    }

    reader_common_methods!();

    /// Read up to `count` aligned samples, returning one `Vec` per signal
    /// (all of the same length, the count actually read).
    pub fn read(&self, count: usize, timeout_ms: usize) -> Result<Vec<Vec<V>>> {
        let mut buffers: Vec<Vec<V>> = (0..self.signal_count)
            .map(|_| vec![V::default(); count.max(1)])
            .collect();
        let mut pointers: Vec<*mut c_void> = buffers
            .iter_mut()
            .map(|b| b.as_mut_ptr() as *mut c_void)
            .collect();
        let mut read = count;
        let mut status: *mut sys::daqMultiReaderStatus = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqMultiReader_read)(
                    self.inner.as_ptr() as *mut _,
                    pointers.as_mut_ptr() as *mut c_void,
                    &mut read,
                    timeout_ms,
                    &mut status,
                )
            },
            "daqMultiReader_read",
        )?;
        let status = unsafe { marshal::take_object::<ReaderStatus>(status as *mut _) };
        check_status_valid(status.as_ref())?;
        for buffer in &mut buffers {
            buffer.truncate(read);
        }
        Ok(buffers)
    }

    /// Like [`MultiReader::read`], but also reads the per-signal domain stamps.
    #[allow(clippy::type_complexity)]
    pub fn read_with_domain(
        &self,
        count: usize,
        timeout_ms: usize,
    ) -> Result<(Vec<Vec<V>>, Vec<Vec<D>>)> {
        let mut values: Vec<Vec<V>> = (0..self.signal_count)
            .map(|_| vec![V::default(); count.max(1)])
            .collect();
        let mut domain: Vec<Vec<D>> = (0..self.signal_count)
            .map(|_| vec![D::default(); count.max(1)])
            .collect();
        let mut value_ptrs: Vec<*mut c_void> = values
            .iter_mut()
            .map(|b| b.as_mut_ptr() as *mut c_void)
            .collect();
        let mut domain_ptrs: Vec<*mut c_void> = domain
            .iter_mut()
            .map(|b| b.as_mut_ptr() as *mut c_void)
            .collect();
        let mut read = count;
        let mut status: *mut sys::daqMultiReaderStatus = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqMultiReader_readWithDomain)(
                    self.inner.as_ptr() as *mut _,
                    value_ptrs.as_mut_ptr() as *mut c_void,
                    domain_ptrs.as_mut_ptr() as *mut c_void,
                    &mut read,
                    timeout_ms,
                    &mut status,
                )
            },
            "daqMultiReader_readWithDomain",
        )?;
        let status = unsafe { marshal::take_object::<ReaderStatus>(status as *mut _) };
        check_status_valid(status.as_ref())?;
        for buffer in &mut values {
            buffer.truncate(read);
        }
        for buffer in &mut domain {
            buffer.truncate(read);
        }
        Ok((values, domain))
    }

    /// The common domain's origin (an ISO-8601 epoch string).
    pub fn origin(&self) -> Result<String> {
        let mut out: *mut sys::daqString = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqMultiReader_getOrigin)(self.inner.as_ptr() as *mut _, &mut out)
            },
            "daqMultiReader_getOrigin",
        )?;
        Ok(unsafe { marshal::take_string(out) })
    }

    /// The common domain's tick resolution (seconds per tick).
    pub fn tick_resolution(&self) -> Result<Option<crate::Ratio>> {
        let mut out: *mut sys::daqRatio = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqMultiReader_getTickResolution)(
                    self.inner.as_ptr() as *mut _,
                    &mut out,
                )
            },
            "daqMultiReader_getTickResolution",
        )?;
        unsafe { crate::value::take_ratio(out, "daqMultiReader_getTickResolution") }
    }

    /// True once every signal is synchronized to the common domain.
    pub fn is_synchronized(&self) -> Result<bool> {
        let mut out: u8 = 0;
        check(
            unsafe {
                (sys::api().daqMultiReader_getIsSynchronized)(
                    self.inner.as_ptr() as *mut _,
                    &mut out,
                )
            },
            "daqMultiReader_getIsSynchronized",
        )?;
        Ok(out != 0)
    }

    /// The common sample rate of the aligned signals.
    pub fn common_sample_rate(&self) -> Result<i64> {
        let mut out: i64 = 0;
        check(
            unsafe {
                (sys::api().daqMultiReader_getCommonSampleRate)(
                    self.inner.as_ptr() as *mut _,
                    &mut out,
                )
            },
            "daqMultiReader_getCommonSampleRate",
        )?;
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Packet reader
// ---------------------------------------------------------------------------

/// A reader that hands out whole packets instead of copying samples.
pub struct PacketReader {
    inner: Ref,
}

impl PacketReader {
    /// Calls the openDAQ C function `daqPacketReader_createPacketReader()`.
    pub fn new(signal: &Signal) -> Result<PacketReader> {
        let op = "daqPacketReader_createPacketReader";
        let mut reader: *mut sys::daqPacketReader = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqPacketReader_createPacketReader)(
                    &mut reader,
                    signal.as_raw() as *mut _,
                )
            },
            op,
        )?;
        let inner = unsafe { Ref::from_owned(reader as *mut c_void) }
            .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_GENERALERROR, op, None))?;
        Ok(PacketReader { inner })
    }

    /// Calls the openDAQ C function `daqPacketReader_createPacketReaderFromPort()`.
    pub fn from_port(port: &InputPortConfig) -> Result<PacketReader> {
        let op = "daqPacketReader_createPacketReaderFromPort";
        let mut reader: *mut sys::daqPacketReader = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqPacketReader_createPacketReaderFromPort)(
                    &mut reader,
                    port.as_raw() as *mut _,
                )
            },
            op,
        )?;
        let inner = unsafe { Ref::from_owned(reader as *mut c_void) }
            .ok_or_else(|| Error::new(sys::OPENDAQ_ERR_GENERALERROR, op, None))?;
        Ok(PacketReader { inner })
    }

    /// The raw interface pointer (for use with [`crate::sys`]).
    pub fn as_raw(&self) -> *mut c_void {
        self.inner.as_ptr()
    }

    /// The next available packet, if any.
    pub fn read(&self) -> Result<Option<Packet>> {
        let mut packet: *mut sys::daqPacket = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqPacketReader_read)(self.inner.as_ptr() as *mut _, &mut packet)
            },
            "daqPacketReader_read",
        )?;
        Ok(unsafe { marshal::take_object::<Packet>(packet as *mut _) })
    }

    /// All currently queued packets.
    pub fn read_all(&self) -> Result<Vec<Packet>> {
        let mut packets: *mut sys::daqList = std::ptr::null_mut();
        check(
            unsafe {
                (sys::api().daqPacketReader_readAll)(self.inner.as_ptr() as *mut _, &mut packets)
            },
            "daqPacketReader_readAll",
        )?;
        unsafe { marshal::take_list::<Packet>(packets, "daqPacketReader_readAll") }
    }

    /// Number of packets currently available.
    pub fn available_count(&self) -> Result<usize> {
        let mut count: usize = 0;
        check(
            unsafe {
                (sys::api().daqReader_getAvailableCount)(self.inner.as_ptr() as *mut _, &mut count)
            },
            "daqReader_getAvailableCount",
        )?;
        Ok(count)
    }

    /// True when no packets are queued.
    pub fn empty(&self) -> Result<bool> {
        let mut empty: u8 = 0;
        check(
            unsafe { (sys::api().daqReader_getEmpty)(self.inner.as_ptr() as *mut _, &mut empty) },
            "daqReader_getEmpty",
        )?;
        Ok(empty != 0)
    }
}
