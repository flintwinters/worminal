//! Recognizes an SSH client in a terminal's foreground and the remote user it connects as.
use std::os::fd::RawFd;

/// ssh options that consume a value (OpenSSH `ssh(1)` synopsis).
const VALUE_OPTIONS: &[u8] = b"BbcDEeFIiJLlmOoPpQRSWw";

/// The user an `ssh` command line logs in as: `user@host` beats `-l`, which beats `-o User=`; with none of
/// them the local user applies (host-specific `~/.ssh/config` users are not resolved).
pub fn user(args: &[Vec<u8>], local_user: &[u8]) -> Option<Vec<u8>> {
    let name = args.first()?.rsplit(|byte| *byte == b'/').next()?;
    if name != b"ssh" {
        return None;
    }
    let (mut login, mut option, mut destination) = (None, None, None);
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        if arg == b"--" {
            destination = rest.next();
            break;
        }
        if arg.len() < 2 || arg[0] != b'-' {
            destination = Some(arg);
            break;
        }
        for (position, flag) in arg.iter().enumerate().skip(1) {
            if !VALUE_OPTIONS.contains(flag) {
                continue;
            }
            let attached = &arg[position + 1..];
            let value = if attached.is_empty() {
                rest.next().map(Vec::as_slice).unwrap_or_default()
            } else {
                attached
            };
            match flag {
                b'l' => login = Some(value),
                b'o' => option = option.or_else(|| user_option(value)),
                _ => {}
            }
            break;
        }
    }
    let from_destination = destination.and_then(|destination| {
        let destination = destination.strip_prefix(b"ssh://").unwrap_or(destination);
        let at = destination.iter().rposition(|byte| *byte == b'@')?;
        Some(&destination[..at])
    });
    let user = from_destination.or(login).or(option).unwrap_or(local_user);
    (!user.is_empty()).then(|| user.to_vec())
}

/// `User=name` or `User name`, key case-insensitive.
fn user_option(option: &[u8]) -> Option<&[u8]> {
    let split = option
        .iter()
        .position(|byte| *byte == b'=' || *byte == b' ')?;
    let (key, value) = option.split_at(split);
    key.eq_ignore_ascii_case(b"User")
        .then(|| value[1..].trim_ascii())
}

/// The daemon user's login name, for ssh commands that name no user.
pub fn local_user() -> Vec<u8> {
    // SAFETY: getpwuid returns null or a pointer valid until the next passwd call; it is copied at once.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() {
            return Vec::new();
        }
        std::ffi::CStr::from_ptr((*entry).pw_name)
            .to_bytes()
            .to_vec()
    }
}

/// The foreground job of the terminal on `master`, when its leader is an ssh client.
pub fn foreground_user(master: RawFd, local_user: &[u8]) -> Option<Vec<u8>> {
    // SAFETY: tcgetpgrp only reads the descriptor's terminal state.
    let group = unsafe { libc::tcgetpgrp(master) };
    if group <= 0 {
        return None;
    }
    let cmdline = std::fs::read(format!("/proc/{group}/cmdline")).ok()?;
    let args: Vec<Vec<u8>> = cmdline
        .split(|byte| *byte == 0)
        .take_while(|arg| !arg.is_empty())
        .map(<[u8]>::to_vec)
        .collect();
    user(&args, local_user)
}

#[cfg(test)]
mod tests {
    use super::user;

    fn parse(line: &str) -> Option<String> {
        let args: Vec<Vec<u8>> = line.split(' ').map(|a| a.as_bytes().to_vec()).collect();
        user(&args, b"me").map(|u| String::from_utf8(u).unwrap())
    }

    #[test]
    fn extracts_the_remote_user_from_common_invocations() {
        assert_eq!(parse("ssh alice@host").as_deref(), Some("alice"));
        assert_eq!(
            parse("/usr/bin/ssh -p 2222 -tt bob@host uptime").as_deref(),
            Some("bob")
        );
        assert_eq!(parse("ssh -l carol host").as_deref(), Some("carol"));
        assert_eq!(parse("ssh -lcarol host").as_deref(), Some("carol"));
        assert_eq!(parse("ssh -o User=dave host").as_deref(), Some("dave"));
        assert_eq!(
            parse("ssh -oUser=dave -i key host").as_deref(),
            Some("dave")
        );
        assert_eq!(parse("ssh -l carol erin@host").as_deref(), Some("erin"));
        assert_eq!(parse("ssh ssh://frank@host").as_deref(), Some("frank"));
        assert_eq!(parse("ssh -- gina@host").as_deref(), Some("gina"));
        assert_eq!(parse("ssh -J jump@hop host").as_deref(), Some("me"));
        assert_eq!(parse("ssh host").as_deref(), Some("me"));
    }

    #[test]
    fn other_programs_are_not_ssh() {
        assert_eq!(parse("sshd -D"), None);
        assert_eq!(parse("vim ssh"), None);
        assert_eq!(parse("scp a b@c:d"), None);
    }
}
