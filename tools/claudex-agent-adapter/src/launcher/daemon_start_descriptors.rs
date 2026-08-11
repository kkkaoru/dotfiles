#[cfg(unix)]
pub(super) type Io<T> = std::io::Result<T>;

#[cfg(unix)]
// Keep the pre-exec entrypoint as a single delegation; the range logic is tested below.
#[rustfmt::skip]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn close_inherited_descriptors() -> Io<()> { close_system(close_file_descriptor) }

#[cfg(unix)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn detach_session_and_close_inherited_descriptors() -> Io<()> {
    if unsafe { libc::setsid() } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    close_inherited_descriptors()
}

#[cfg(unix)]
pub(super) fn close_system(close: impl FnMut(i32)) -> Io<()> {
    close_inherited_descriptors_with(unsafe { libc::sysconf(libc::_SC_OPEN_MAX) }, close)
}

#[cfg(unix)]
pub(super) fn bounded_descriptor_limit(max_fd: libc::c_long) -> i32 {
    if max_fd > 3 && max_fd < 1_048_576 {
        max_fd as i32
    } else {
        1024
    }
}

#[cfg(unix)]
pub(super) fn close_inherited_descriptors_with(
    max_fd: libc::c_long,
    close: impl FnMut(i32),
) -> std::io::Result<()> {
    close_descriptors_up_to(bounded_descriptor_limit(max_fd), close);
    Ok(())
}

#[cfg(unix)]
pub(super) fn close_descriptors_up_to(max_fd: i32, mut close: impl FnMut(i32)) {
    for fd in 3..max_fd {
        close(fd);
    }
}

#[cfg(unix)]
pub(super) fn close_file_descriptor(fd: i32) {
    unsafe {
        libc::close(fd);
    }
}
