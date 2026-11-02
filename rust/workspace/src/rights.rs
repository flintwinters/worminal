//! Receive Unix file descriptors and close them if validation fails.
use std::{
    io, mem,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    ptr,
};

fn control(max_descriptors: usize) -> io::Result<(Vec<usize>, usize)> {
    // cmsghdr requires native alignment; a byte Vec would not provide that guarantee.
    const _: () = assert!(mem::align_of::<usize>() >= mem::align_of::<libc::cmsghdr>());
    let bytes = max_descriptors
        .checked_mul(mem::size_of::<libc::c_int>())
        .and_then(|bytes| u32::try_from(bytes).ok())
        .filter(|_| max_descriptors != 0)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid descriptor capacity")
        })?;
    let capacity = unsafe { libc::CMSG_SPACE(bytes) } as usize;
    if capacity < bytes as usize + mem::size_of::<libc::cmsghdr>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "descriptor capacity overflow",
        ));
    }
    Ok((
        vec![0usize; capacity.div_ceil(mem::size_of::<usize>())],
        capacity,
    ))
}

pub fn send_descriptor(
    socket: &UnixStream,
    descriptor: BorrowedFd<'_>,
    marker: u8,
) -> io::Result<()> {
    send_descriptors(socket.as_fd(), &[descriptor], marker)
}

pub fn send_descriptors(
    socket: BorrowedFd<'_>,
    descriptors: &[BorrowedFd<'_>],
    mut marker: u8,
) -> io::Result<()> {
    let (mut control, capacity) = control(descriptors.len())?;
    let mut iovec = libc::iovec {
        iov_base: ptr::from_mut(&mut marker).cast(),
        iov_len: 1,
    };
    // All pointers refer to live, aligned allocations; sendmsg duplicates borrowed descriptors.
    unsafe {
        let mut message: libc::msghdr = mem::zeroed();
        message.msg_iov = &mut iovec;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = capacity;
        let header = libc::CMSG_FIRSTHDR(&message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len =
            libc::CMSG_LEN((descriptors.len() * mem::size_of::<libc::c_int>()) as u32) as usize;
        for (index, descriptor) in descriptors.iter().enumerate() {
            ptr::write_unaligned(
                libc::CMSG_DATA(header).cast::<libc::c_int>().add(index),
                descriptor.as_raw_fd(),
            );
        }
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

pub fn receive_descriptors(
    socket: BorrowedFd<'_>,
    marker: u8,
    maximum: usize,
) -> io::Result<Vec<OwnedFd>> {
    let (mut control, capacity) = control(maximum)?;
    let mut byte = 0u8;
    let mut iovec = libc::iovec {
        iov_base: ptr::from_mut(&mut byte).cast(),
        iov_len: 1,
    };
    let mut received = Vec::with_capacity(maximum);
    unsafe {
        let mut message: libc::msghdr = mem::zeroed();
        message.msg_iov = &mut iovec;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = capacity;
        let count = loop {
            let count = libc::recvmsg(socket.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC);
            if count >= 0 {
                break count;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        };
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while !header.is_null() {
            let length = (*header).cmsg_len;
            let prefix = libc::CMSG_LEN(0) as usize;
            // Kernel-generated headers are aligned and bounded by msg_controllen. Check lengths
            // before reading their payload, and own every installed fd even on truncated messages.
            let offset = header
                .cast::<u8>()
                .offset_from(control.as_ptr().cast::<u8>()) as usize;
            if length < prefix || length > message.msg_controllen.saturating_sub(offset) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid descriptor control length",
                ));
            }
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let bytes = length - prefix;
                for index in 0..bytes / mem::size_of::<libc::c_int>() {
                    let fd = ptr::read_unaligned(
                        libc::CMSG_DATA(header).cast::<libc::c_int>().add(index),
                    );
                    received.push(OwnedFd::from_raw_fd(fd));
                }
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
        if count != 1
            || byte != marker
            || message.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) != 0
            || received.is_empty()
            || received.len() > maximum
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid descriptor bundle",
            ));
        }
    }
    Ok(received)
}
