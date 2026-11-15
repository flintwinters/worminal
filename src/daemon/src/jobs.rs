//! Shell job observation uses the existing directory poll, without walking the host process table.
use std::fs;

pub fn background(shell: libc::pid_t, foreground: libc::pid_t) -> bool {
    if shell <= 0 {
        return false;
    }
    let Ok(children) = fs::read_to_string(format!("/proc/{shell}/task/{shell}/children")) else {
        return false;
    };
    children.split_whitespace().any(|pid| {
        fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .is_some_and(|stat| background_stat(&stat, shell, foreground))
    })
}

fn background_stat(stat: &str, shell: libc::pid_t, foreground: libc::pid_t) -> bool {
    // comm can contain spaces and parentheses. Fields after its final ')' begin with state.
    let Some((_, fields)) = stat.rsplit_once(')') else {
        return false;
    };
    let mut fields = fields.split_whitespace();
    let state = fields.next().unwrap_or_default();
    let _parent = fields.next();
    let group = fields
        .next()
        .and_then(|value| value.parse::<libc::pid_t>().ok());
    let session = fields
        .next()
        .and_then(|value| value.parse::<libc::pid_t>().ok());
    // Stopped jobs count too; zombies and detached sessions do not. The shell's own group
    // includes helper processes that are not independently controlled shell jobs.
    !matches!(state, "Z" | "X" | "")
        && session == Some(shell)
        && group.is_some_and(|group| group > 0 && group != shell && group != foreground)
}

#[cfg(test)]
mod tests {
    use super::background_stat;

    #[test]
    fn distinguishes_background_stopped_foreground_and_exited_jobs() {
        for state in ["R", "S", "T", "t"] {
            let stat = format!("20 (job (name)) {state} 10 20 10 1");
            assert!(background_stat(&stat, 10, 10));
            assert!(!background_stat(&stat, 10, 20));
        }
        for fields in ["Z 10 20 10", "X 10 20 10", "S 10 10 10", "S 10 20 30"] {
            assert!(!background_stat(&format!("20 (job) {fields}"), 10, 10));
        }
    }
}
