//! DeckFiles' webOS entropy bridge.
//!
//! getrandom 0.4's stock Linux backend discovers libc's getrandom() symbol with dlsym and falls
//! back to /dev/urandom when that symbol or kernel support is absent. Some webOS app sandboxes
//! expose the kernel syscall but not the libc wrapper or device node. Use the kernel CSPRNG
//! directly first, then the standard random device nodes as compatibility fallbacks. Never
//! synthesize predictable bytes: if every OS source fails, return an error to the caller.

#[cfg(all(target_os = "linux", target_arch = "arm"))]
#[unsafe(no_mangle)]
pub unsafe extern "Rust" fn __getrandom_v03_custom(
    dest: *mut u8,
    len: usize,
) -> Result<(), getrandom::Error> {
    use std::io::Read;

    if len == 0 {
        return Ok(());
    }

    // SAFETY: getrandom's contract provides a valid writable buffer of len bytes. The slice
    // never escapes this function and is filled completely before success is returned.
    let output = unsafe { std::slice::from_raw_parts_mut(dest, len) };
    let mut filled = 0usize;

    // Bypass libc's dlsym-based detection. This succeeds on kernels that implement getrandom(2)
    // even when the TV's libc does not export a getrandom() wrapper.
    while filled < output.len() {
        // SAFETY: the pointer addresses the still-unfilled tail of a valid writable buffer.
        let result = unsafe {
            libc::syscall(
                libc::SYS_getrandom,
                output[filled..].as_mut_ptr(),
                output.len() - filled,
                0 as libc::c_uint,
            )
        };
        if result > 0 {
            filled += result as usize;
            continue;
        }
        if result == 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        break;
    }

    if filled == output.len() {
        return Ok(());
    }

    // Older firmware may predate getrandom(2), or sandbox policy may block the syscall. Preserve
    // kernel CSPRNG guarantees by falling back only to kernel random devices.
    let mut source = std::fs::File::open("/dev/urandom")
        .or_else(|_| std::fs::File::open("/dev/random"))
        .map_err(|error| {
            let code = error.raw_os_error().unwrap_or(libc::EIO).unsigned_abs().min(u16::MAX as u32) as u16;
            getrandom::Error::new_custom(code)
        })?;
    source.read_exact(&mut output[filled..]).map_err(|error| {
        let code = error.raw_os_error().unwrap_or(libc::EIO).unsigned_abs().min(u16::MAX as u32) as u16;
        getrandom::Error::new_custom(code)
    })?;
    Ok(())
}
