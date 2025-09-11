/*!
    Utility functions
    
    Copyright (C) 2025 LIZARD-OFFICIAL-77
    This program is free software; you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation; either version 2 of the License, or
    (at your option) any later version.
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.

    You should have received a copy of the GNU General Public License along
    with this program; if not, write to the Free Software Foundation, Inc.,
    51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA.
*/

use nix::unistd::{getuid,ROOT};

pub fn is_root() -> bool {
    return getuid() == ROOT;
}

/// Macro to run command and return result.
#[macro_export] 
macro_rules! run_cmd {
    ($cmd:expr) => {
        match $cmd.output() {
            Ok(_) => {
                return Ok(())
            }
            Err(err) => {
                return Err(err.to_string())
            }
        }
    };
}

/// Macro to check if device name is correct.
#[macro_export]  
macro_rules! check_dev {
    ($dev:expr) => {
        if !$dev.starts_with("/dev/") {
            return Err(format!("Incorrect device path: {}",$dev))
        }
    }
}

/// Macro to check if multiple device names are correct.
#[macro_export] 
macro_rules! check_devs {
    ($devs:expr) => {
        for dev in $devs.iter() {
            check_dev!(dev);
        }
    }
}

/// Returns Result::Err with String "Action requires root." if not running as root.
#[macro_export] 
macro_rules! root_check {
    () => {
        if !is_root() {
            return Err("Action requires root.".to_string())
        }
    };
}
