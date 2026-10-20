//! Polled hardware drivers (I²C, GPIO, 1-Wire, CAN, BLE). Each produces
//! chunks on `read` and accepts writes; `link::polled` turns one into a Link.
// Only Linux drivers and BLE use the polled path.
#![cfg_attr(not(any(target_os = "linux", feature = "bluetooth")), allow(dead_code))]

pub mod bluetooth;
pub mod canbus;
pub mod gpio;
pub mod i2c;
pub mod onewire;

use std::future::Future;

use crate::error::Result;

pub trait IoDriver: Send {
    fn open(&mut self) -> impl Future<Output = Result<()>> + Send;
    /// One chunk of data, `Ok(0)` when nothing arrived within the driver's
    /// own wait (poll interval or 100 ms).
    fn read(&mut self, buf: &mut [u8]) -> impl Future<Output = Result<usize>> + Send;
    fn write(&mut self, data: &[u8]) -> impl Future<Output = Result<usize>> + Send;
    fn description(&self) -> String;
}
