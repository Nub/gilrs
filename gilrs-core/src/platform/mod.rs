// Copyright 2016-2018 Mateusz Sieczko and other GilRs Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Module which exports the platform-specific types.
//!
//! Each backend has to provide:
//!
//! * A `FfDevice` (a struct which handles force feedback)
//! * A `Gilrs` context
//! * A `Gamepad` struct
//! * A static `str` which specifies the name of the SDL input mapping
//! * A constant which define whether Y axis of sticks points upwards or downwards
//! * A module with the platform-specific constants for common gamepad buttons
//!   called `native_ev_codes`
//!

pub use self::platform::*;

#[cfg(target_os = "linux")]
#[path = "linux/mod.rs"]
mod platform;

#[cfg(target_os = "windows")]
#[path = "windows/mod.rs"]
mod platform;

#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
#[path = "default/mod.rs"]
mod platform;