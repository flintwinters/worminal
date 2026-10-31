//! Audited Unix descriptor boundary. Borrowed descriptors stay owned by the caller.
use std::{
    io, mem,
    os::{
        fd::{AsRawFd, BorrowedFd},
        unix::net::UnixStream,
    },
    ptr,
};

pub fn send_descriptor(
    socket: &UnixStream,
    descriptor: BorrowedFd<'_>,
    mut marker: u8,
) -> io::Result<()> {
    // cmsghdr requires native alignment; a byte Vec would not provide that guarantee.
    const _: () = assert!(mem::align_of::<usize>() >= mem::align_of::<libc::cmsghdr>());
    let capacity = unsafe { libc::CMSG_SPACE(mem::size_of::<libc::c_int>() as u32) } as usize;
    let mut control = vec![0usize; capacity.div_ceil(mem::size_of::<usize>())];
    let mut iovec = libc::iovec {
        iov_base: ptr::from_mut(&mut marker).cast(),
        iov_len: 1,
    };
    // All pointers refer to live, aligned allocations through sendmsg; the kernel duplicates the fd.
    unsafe {
        let mut message: libc::msghdr = mem::zeroed();
        message.msg_iov = &mut iovec;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = capacity;
        let header = libc::CMSG_FIRSTHDR(&message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(mem::size_of::<libc::c_int>() as u32) as usize;
        ptr::write_unaligned(
            libc::CMSG_DATA(header).cast::<libc::c_int>(),
            descriptor.as_raw_fd(),
        );
        loop {
            let sent = libc::sendmsg(socket.as_raw_fd(), &message, libc::MSG_NOSIGNAL);
            if sent == 1 {
                return Ok(());
            }
            if sent == -1 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "descriptor marker was not delivered",
            ));
        }
    }
}
