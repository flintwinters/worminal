//! Native snapshot staging has no PTY authority. Commit consumes staged ownership without fallible setup.
use crate::{
    handoff_io::{self, Channel, CommitReceived, CommitSent, Mapping},
    process::{self, ChildReaper, PinnedProcess, PreparedPty},
    queue::Queue,
    state::{core, Printer, Tab, Terminal, INPUT_QUEUE},
};
use std::{
    fs::File,
    io::{self, Write},
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
};
use worminal_engine::Engine;
use worminal_protocol::{Buffer, Payload, MAX_PAYLOAD};
use worminal_workspace::{handoff::*, rights};

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid handoff snapshot")
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> io::Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(invalid)?;
        let result = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(result)
    }
    fn number(&mut self) -> io::Result<u32> {
        Ok(u32::from_ne_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn remaining(&self) -> &'a [u8] {
        &self.bytes[self.offset..]
    }
}

fn string(payload: &mut Payload<'_>) -> io::Result<Vec<u8>> {
    Ok(payload
        .string()?
        .split(|byte| *byte == 0)
        .next()
        .unwrap()
        .to_vec())
}

struct PreparedTerminal {
    engine: Engine,
    pty: PreparedPty,
    input: Queue,
    pending: Vec<u8>,
    remote_scroll: i32,
    printer: Printer,
}
struct StagedTab {
    tab: Tab,
    terminal: Option<PreparedTerminal>,
}
impl StagedTab {
    fn commit(mut self, reaper: ChildReaper, commit: &CommitReceived) -> Tab {
        if let Some(terminal) = self.terminal {
            self.tab.terminal = Some(Terminal {
                engine: terminal.engine,
                pty: terminal.pty.commit(reaper, commit),
                pending: terminal.pending,
                remote_scroll: terminal.remote_scroll,
                input: terminal.input,
                printer: terminal.printer,
            });
        }
        self.tab
    }
}

fn decode_tab(
    cursor: &mut Cursor<'_>,
    id: u32,
    descriptors: &mut impl Iterator<Item = OwnedFd>,
) -> io::Result<StagedTab> {
    let length = cursor.number()? as usize;
    if length > MAX_PAYLOAD {
        return Err(invalid());
    }
    let mut payload = Payload::new(cursor.take(length)?);
    let browser = payload.u32()?;
    let mode = payload.u32()?;
    let style = payload.u32()?;
    let child = payload.u32()?;
    if browser > 1 || style > 7 || child > i32::MAX as u32 {
        return Err(invalid());
    }
    let title = string(&mut payload)?;
    let mut tab = Tab::record(id, &title);
    tab.initial_title = string(&mut payload)?;
    tab.directory = string(&mut payload)?;
    tab.clipboard = string(&mut payload)?;
    let url = string(&mut payload)?;
    tab.url = (browser != 0).then_some(url);
    tab.mode = mode;
    tab.cursor = style;
    if payload.u32()? as usize != worminal_config::COLOR_NAMES.len() {
        return Err(invalid());
    }
    let mut colors = Vec::with_capacity(worminal_config::COLOR_NAMES.len());
    for _ in worminal_config::COLOR_NAMES {
        let color = string(&mut payload)?;
        colors.push((!color.is_empty()).then_some(color));
    }
    if colors.iter().any(Option::is_some) {
        tab.colors = Some(colors);
    }
    let input = payload.u32()? as usize;
    if input > INPUT_QUEUE {
        return Err(invalid());
    }
    let input = Queue::from_bytes(payload.take(input)?.to_vec(), INPUT_QUEUE)?;
    payload.finish()?;
    let terminal = if browser == 0 {
        let (engine, transport, used) = core(Engine::import_prefix(cursor.remaining()))?;
        cursor.take(used)?;
        let process = PinnedProcess::prepare(child as i32)?;
        let master = File::from(descriptors.next().ok_or_else(invalid)?);
        let output = File::from(descriptors.next().ok_or_else(invalid)?);
        let pty = PreparedPty::validate(master, process)?;
        let printer = match transport.output_kind {
            0 => Printer::File(output),
            1 => Printer::Client,
            -1 => Printer::Disabled,
            _ => return Err(invalid()),
        };
        Some(PreparedTerminal {
            engine,
            pty,
            input,
            pending: transport.pending[..transport.pending_bytes as usize].to_vec(),
            remote_scroll: transport.remote_scroll,
            printer,
        })
    } else {
        None
    };
    Ok(StagedTab { tab, terminal })
}

fn encode_tab(tab: &mut Tab, file: &mut File) -> io::Result<()> {
    // Keep the legacy export's conservative extra-u32 allowance in its metadata size limit.
    let mut body = Buffer::with_limit(MAX_PAYLOAD - 4);
    body.put_u32(u32::from(tab.url.is_some()))?;
    body.put_u32(tab.mode)?;
    body.put_u32(tab.cursor)?;
    body.put_u32(
        tab.terminal
            .as_mut()
            .map_or(0, |terminal| terminal.pty.live_pid()) as u32,
    )?;
    for field in [
        &tab.title[..],
        &tab.initial_title,
        &tab.directory,
        &tab.clipboard,
        tab.url.as_deref().unwrap_or_default(),
    ] {
        body.put_string(field)?;
    }
    body.put_u32(worminal_config::COLOR_NAMES.len() as u32)?;
    for index in 0..worminal_config::COLOR_NAMES.len() {
        body.put_string(
            tab.colors
                .as_ref()
                .and_then(|colors| colors[index].as_deref())
                .unwrap_or_default(),
        )?;
    }
    let input = tab
        .terminal
        .as_ref()
        .map_or(&[][..], |terminal| terminal.input.pending());
    body.put_u32(input.len() as u32)?;
    body.put_bytes(input)?;
    file.write_all(&(body.as_bytes().len() as u32).to_ne_bytes())?;
    file.write_all(body.as_bytes())?;
    let transport = tab.terminal.as_ref().map(|_| tab.transport());
    if let Some(terminal) = &mut tab.terminal {
        file.write_all(&core(terminal.engine.export(&transport.unwrap()))?)?;
    }
    Ok(())
}

/// After C is delivered this function empties donor ownership before attempting A reception.
pub(crate) fn send(
    name: &[u8],
    tabs: &mut Vec<Tab>,
    reaper: &ChildReaper,
    next_id: Option<u32>,
) -> io::Result<CommitSent> {
    if tabs.len() > HANDOFF_MAX_TABS as usize || (tabs.is_empty() && next_id.is_none()) {
        return Err(invalid());
    }
    reaper.reserve(tabs.len())?;
    let channel = Channel::connect(name)?;
    let mut file = handoff_io::snapshot()?;
    for field in [
        HANDOFF_MAGIC,
        if next_id.is_some() {
            HANDOFF_UPGRADE_VERSION
        } else {
            HANDOFF_MANUAL_VERSION
        },
        tabs.len() as u32,
    ] {
        file.write_all(&field.to_ne_bytes())?;
    }
    if let Some(next_id) = next_id {
        file.write_all(&next_id.to_ne_bytes())?;
    }
    for tab in tabs.iter_mut() {
        if next_id.is_some() {
            file.write_all(&tab.id.to_ne_bytes())?;
        }
        encode_tab(tab, &mut file)?;
        if file.metadata()?.len() > u64::from(HANDOFF_SNAPSHOT_LIMIT) {
            return Err(invalid());
        }
    }
    handoff_io::seal(&mut file)?;
    let output = std::io::stdout();
    let mut descriptors: Vec<BorrowedFd<'_>> = vec![file.as_fd()];
    for tab in tabs.iter() {
        if let Some(terminal) = &tab.terminal {
            descriptors.push(terminal.pty.descriptor().ok_or_else(invalid)?);
            descriptors.push(match &terminal.printer {
                Printer::File(file) => file.as_fd(),
                _ => output.as_fd(),
            });
        }
    }
    rights::send_descriptors(channel.as_fd(), &descriptors, b'H')?;
    drop(descriptors);
    let commit = channel.commit()?;
    // C is irrevocable even if A is lost. Disown before any further fallible operation or PTY read.
    for tab in tabs.iter_mut() {
        if let Some(terminal) = &mut tab.terminal {
            terminal.pty.disown(&commit);
        }
    }
    tabs.clear();
    if let Err(error) = channel.acknowledgement() {
        process::diagnostic(&format!(
            "Handoff committed; receiver acknowledgement missing: {error}"
        ));
    }
    Ok(commit)
}

#[derive(Clone, Copy)]
pub(crate) enum Incoming<'a> {
    Append,
    Upgrade,
    Mapped {
        ids: &'a [u32],
        next_id: u32,
        retiring_pid: i32,
    },
}

pub(crate) fn receive(
    channel: Channel,
    tabs: &mut Vec<Tab>,
    next_id: &mut u32,
    reaper: &ChildReaper,
    incoming: Incoming<'_>,
    signals: &crate::reactor::Signals,
) -> io::Result<()> {
    process::ensure_receiver_session()?;
    let upgrading = matches!(incoming, Incoming::Upgrade);
    let preserving = !matches!(incoming, Incoming::Append);
    let mut descriptors =
        rights::receive_descriptors(channel.as_fd(), b'H', HANDOFF_MAX_FDS as usize)?.into_iter();
    let mapping = Mapping::new(descriptors.next().ok_or_else(invalid)?)?;
    let mut cursor = Cursor {
        bytes: mapping.bytes(),
        offset: 0,
    };
    let magic = cursor.number()?;
    let version = cursor.number()?;
    let count = cursor.number()?;
    if magic != HANDOFF_MAGIC
        || version
            != if upgrading {
                HANDOFF_UPGRADE_VERSION
            } else {
                HANDOFF_MANUAL_VERSION
            }
        || count > HANDOFF_MAX_TABS
        || (!upgrading && count == 0)
        || next_id.checked_add(count).is_none()
        || (preserving && !tabs.is_empty())
    {
        return Err(invalid());
    }
    let saved_next = match incoming {
        Incoming::Upgrade => cursor.number()?,
        Incoming::Append => *next_id + count,
        Incoming::Mapped { ids, next_id, .. } => {
            if ids.len() != count as usize {
                return Err(invalid());
            }
            next_id
        }
    };
    if saved_next == 0 {
        return Err(invalid());
    }
    let mut staged = Vec::with_capacity(count as usize);
    let retiring = if let Incoming::Mapped { retiring_pid, .. } = incoming {
        if retiring_pid != 0 {
            if process::peer_pid(channel.as_raw_fd())? != retiring_pid {
                return Err(invalid());
            }
            let pin = PinnedProcess::prepare(retiring_pid)?;
            if pin.live_pid() != retiring_pid {
                return Err(invalid());
            }
            pin.signal(0)?;
            Some(pin)
        } else {
            None
        }
    } else {
        None
    };
    let mut previous = 0;
    for index in 0..count {
        let id = match incoming {
            Incoming::Upgrade => cursor.number()?,
            Incoming::Append => *next_id + index,
            Incoming::Mapped { ids, .. } => ids[index as usize],
        };
        if preserving && (id <= previous || id >= saved_next) {
            return Err(invalid());
        }
        previous = id;
        staged.push(decode_tab(&mut cursor, id, &mut descriptors)?);
    }
    if !cursor.remaining().is_empty() || descriptors.next().is_some() {
        return Err(invalid());
    }
    // Reserve publication space before R. Adoption after C performs no allocation, validation, or OS setup.
    tabs.try_reserve(staged.len()).map_err(io::Error::other)?;
    reaper.reserve(staged.len())?;
    // Reject a pending stop before R; accepting it afterward would kill the newly adopted shells.
    if signals.shutdown_requested()? {
        return Err(io::ErrorKind::Interrupted.into());
    }
    channel.ready()?;
    let commit = channel.accept_commit()?;
    // Legacy donors publish an empty catalog after A and destroy reconnecting views. C already
    // transferred authority: pause the pinned donor before A; the coordinator verifies this
    // receiver and retires that donor without running its old shutdown hooks.
    if let Some(retiring) = retiring {
        if let Err(error) = retiring.signal(libc::SIGSTOP) {
            process::error(&format!(
                "Committed receiver could not pause legacy donor: {error}"
            ));
        }
    }
    for tab in staged {
        tabs.push(tab.commit(reaper.clone(), &commit));
    }
    *next_id = saved_next;
    let _ = channel.acknowledge();
    Ok(())
}
