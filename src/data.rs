//! Typed access to data packet sample buffers.
//!
//! `daqDataPacket_getData` / `daqDataPacket_getRawData` return their sample
//! buffer through a `void**` whose element type is only known at runtime from
//! the packet's data descriptor, so they are excluded from generation and
//! wrapped here with the [`Sample`] types instead.

use std::ffi::c_void;

use crate::error::{check, Error, Result};
use crate::generated::DataPacket;
use crate::object::Interface;
use crate::readers::Sample;
use crate::sys;

impl DataPacket {
    fn buffer(&self, op: &'static str) -> Result<*mut c_void> {
        let mut address: *mut c_void = std::ptr::null_mut();
        check(
            unsafe { (sys::api().daqDataPacket_getData)(self.as_raw() as *mut _, &mut address) },
            op,
        )?;
        Ok(address)
    }

    fn expect_sample_type(&self, expected: sys::SampleType, op: &'static str) -> Result<()> {
        let descriptor = self.data_descriptor()?.ok_or_else(|| {
            Error::new(
                sys::OPENDAQ_ERR_INVALIDSTATE,
                op,
                Some("packet has no data descriptor".into()),
            )
        })?;
        let actual = descriptor.sample_type()?;
        if actual != expected {
            return Err(Error::new(
                sys::OPENDAQ_ERR_INVALIDTYPE,
                op,
                Some(format!("packet holds {actual:?} samples, not {expected:?}")),
            ));
        }
        Ok(())
    }

    /// The packet's samples as a typed `Vec`; `V` must match the packet's
    /// data descriptor sample type exactly.
    ///
    /// Calls the openDAQ C function `daqDataPacket_getData()`.
    pub fn data<V: Sample>(&self) -> Result<Vec<V>> {
        let op = "daqDataPacket_getData";
        self.expect_sample_type(V::SAMPLE_TYPE, op)?;
        let count = self.sample_count()?;
        let buffer = self.buffer(op)?;
        if buffer.is_null() {
            return Ok(Vec::new());
        }
        let mut values = vec![V::default(); count];
        unsafe {
            std::ptr::copy_nonoverlapping(buffer as *const V, values.as_mut_ptr(), count);
        }
        Ok(values)
    }

    /// Write `samples` into the packet's buffer in place.  The packet owns
    /// the buffer and must already be sized to hold them (its sample count);
    /// `V` must match the descriptor's sample type exactly.
    pub fn set_data<V: Sample>(&self, samples: &[V]) -> Result<()> {
        let op = "daqDataPacket_getData";
        self.expect_sample_type(V::SAMPLE_TYPE, op)?;
        let count = self.sample_count()?;
        if samples.len() > count {
            return Err(Error::new(
                sys::OPENDAQ_ERR_OUTOFRANGE,
                op,
                Some(format!(
                    "{} samples do not fit a packet of {count}",
                    samples.len()
                )),
            ));
        }
        let buffer = self.buffer(op)?;
        if buffer.is_null() {
            return Err(Error::new(
                sys::OPENDAQ_ERR_INVALIDSTATE,
                op,
                Some("packet has no buffer".into()),
            ));
        }
        unsafe {
            std::ptr::copy_nonoverlapping(samples.as_ptr(), buffer as *mut V, samples.len());
        }
        Ok(())
    }

    /// The packet's raw (pre-scaling) memory as bytes.  Reinterpret it via
    /// the data descriptor when a typed view is needed; for decoded values
    /// use [`DataPacket::data`].
    ///
    /// Calls the openDAQ C function `daqDataPacket_getRawData()`.
    pub fn raw_data(&self) -> Result<Vec<u8>> {
        let op = "daqDataPacket_getRawData";
        let mut address: *mut c_void = std::ptr::null_mut();
        check(
            unsafe { (sys::api().daqDataPacket_getRawData)(self.as_raw() as *mut _, &mut address) },
            op,
        )?;
        let size = self.raw_data_size()?;
        if address.is_null() || size == 0 {
            return Ok(Vec::new());
        }
        let mut bytes = vec![0u8; size];
        unsafe {
            std::ptr::copy_nonoverlapping(address as *const u8, bytes.as_mut_ptr(), size);
        }
        Ok(bytes)
    }
}
