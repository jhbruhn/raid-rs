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

use std::{
    fmt::Display, io, process::Command
};
use nix::unistd::{getuid,ROOT};

/// Log a command invocation in a shell-paste-friendly form.
pub fn log_cmd(cmd: &std::process::Command) {
    let prog = cmd.get_program().to_string_lossy();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    println!("+ {} {}", prog, args.join(" "));
}

pub fn check_dev<S: AsRef<str> + Display>(dev: S) -> io::Result<()> {
    if !dev.as_ref().starts_with("/dev/") {
        return Err(io::Error::new(io::ErrorKind::InvalidFilename, format!("Incorrect device path: {}",dev)))
    }
    Ok(())
}

/// Macro to check if multiple device names are correct.
#[macro_export] 
macro_rules! check_devs {
    ($devs:expr) => {
        for dev in $devs.iter() {
            check_dev(dev)?;
        }
    }
}

fn is_root() -> bool {
    return getuid() == ROOT;
}

pub fn run_cmd(mut command: Command) -> io::Result<()> {
    log_cmd(&command);
    let output = command.output()?;
    if output.status.success() {
        return Ok(());
    }
    let code = output
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".to_string());
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(io::Error::other(format!(
        "command exited with status {}: {}",
        code,
        stderr.trim()
    )))
}

/// Returns io::Error with String "Action requires root privileges." if not running as root.
pub fn root_check() -> io::Result<()> {
    if !is_root() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Action requires root privileges."))
    }
    
    Ok(())
}
