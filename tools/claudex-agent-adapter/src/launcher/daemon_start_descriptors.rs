#[cfg(unix)]
pub(super) type Io<T> = std::io::Result<T>;

#[cfg(unix)]
// Keep the pre-exec entrypoint as a single delegation; range selection and
// iteration are exercised through injected descriptor operations.
// coverage-exception: pre-exec-syscall; symbol=fn close_inherited_descriptors; evidence=launcher::daemon_start::tests::closes_the_inherited_descriptor_range_via_the_injected_operation
#[rustfmt::skip]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn close_inherited_descriptors() -> Io<()> { close_system(|fd| unsafe { libc::close(fd); }) }

#[cfg(unix)]
// `Command::pre_exec` runs this after fork, where instrumented runtime state is
// not safe to exercise in-process. Its sequencing and errors use the helper.
// coverage-exception: pre-exec-syscall; symbol=fn detach_session_and_close_inherited_descriptors; evidence=launcher::daemon_start::tests::detaches_before_closing_descriptors_with_injected_operations
#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn detach_session_and_close_inherited_descriptors() -> Io<()> {
    detach_session_and_close_with(
        || {
            if unsafe { libc::setsid() } == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        },
        close_inherited_descriptors,
    )
}

#[cfg(unix)]
pub(super) fn detach_session_and_close_with(
    mut detach: impl FnMut() -> Io<()>,
    close: impl FnOnce() -> Io<()>,
) -> Io<()> {
    detach()?;
    close()
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

#[cfg(all(test, unix))]
pub(super) fn close_file_descriptor(fd: i32) {
    unsafe {
        libc::close(fd);
    }
}
