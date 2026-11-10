//! Compatibility binding from canonical Worp commands to installed v2 peers.
use super::{commands::*, launch::Launch, v2, Request};
use crate::*;
use std::io;

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid Worp command")
}
fn pair(bytes: &[u8]) -> io::Result<(u32, u32)> {
    let mut fields = Payload::new(bytes);
    let a = fields.u32()?;
    let b = fields.u32()?;
    fields.finish()?;
    Ok((a, b))
}
fn integer(bytes: &[u8]) -> io::Result<u32> {
    let mut fields = Payload::new(bytes);
    let value = fields.u32()?;
    fields.finish()?;
    Ok(value)
}
fn string(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut fields = Payload::new(bytes);
    let value = fields.string()?.split(|b| *b == 0).next().unwrap().to_vec();
    fields.finish()?;
    Ok(value)
}

#[expect(
    clippy::too_many_lines,
    reason = "exhaustive legacy wire binding table"
)]
pub fn decode(packet: Packet) -> io::Result<Option<Command>> {
    let tab = packet.tab;
    if packet.kind == WIRE_WORP
        && packet.tab == 0
        && packet.body.starts_with(&WORP_OPEN_DETACHED.to_be_bytes())
    {
        return Ok(Some(Command::new(
            0,
            CommandKind::Open {
                launch: Launch::decode(&packet.body[4..])?,
                mode: OpenMode::Detached,
            },
        )));
    }
    let kind = match packet.kind {
        WIRE_WORP => match v2::decode_request(&packet)? {
            Request::OpenTerminal { options } => CommandKind::Open {
                launch: Launch::from_options(&options),
                mode: OpenMode::Detached,
            },
            Request::OpenBrowser { url } => CommandKind::Browser {
                url: url.into_bytes(),
                subscribe: false,
            },
            Request::Input { bytes, .. } => CommandKind::Input {
                bytes,
                control: Control::Preserve,
            },
            Request::Close { .. } => CommandKind::Close { receipt: true },
            request => CommandKind::Call(request),
        },
        WIRE_HELLO | WIRE_NEW => CommandKind::Open {
            launch: Launch::decode(&packet.body)?,
            mode: if packet.kind == WIRE_HELLO {
                OpenMode::ReuseRestored
            } else {
                OpenMode::Subscribe
            },
        },
        WIRE_INPUT => CommandKind::Input {
            bytes: packet.body,
            control: Control::Acquire,
        },
        WIRE_FOCUS | WIRE_RESIZE => {
            let (columns, rows) = pair(&packet.body)?;
            if packet.kind == WIRE_FOCUS {
                CommandKind::Focus { columns, rows }
            } else {
                CommandKind::Resize { columns, rows }
            }
        }
        WIRE_CLOSE => CommandKind::Close { receipt: false },
        WIRE_SCROLL => {
            let (direction, amount) = pair(&packet.body)?;
            if direction > 1 {
                return Err(invalid());
            }
            CommandKind::Scroll {
                up: direction != 0,
                amount,
            }
        }
        WIRE_CONTROL => CommandKind::Control(match integer(&packet.body)? {
            0 => ControlAction::TogglePrinter,
            1 => ControlAction::PrintScreen,
            2 => ControlAction::Break,
            _ => return Err(invalid()),
        }),
        WIRE_FOCUS_STATE => CommandKind::FocusState(match integer(&packet.body)? {
            0 => false,
            1 => true,
            _ => return Err(invalid()),
        }),
        WIRE_SELECT_ALL => {
            let (generation, operation) = pair(&packet.body)?;
            CommandKind::Selection {
                generation,
                operation: match operation {
                    0 => SelectionOperation::Start,
                    1 => SelectionOperation::Next,
                    2 => SelectionOperation::Cancel,
                    _ => return Err(invalid()),
                },
            }
        }
        WIRE_BROWSER => CommandKind::Browser {
            url: string(&packet.body)?,
            subscribe: true,
        },
        WIRE_BROWSER_NAVIGATE => CommandKind::Navigate(string(&packet.body)?),
        WIRE_BROWSER_REPLACE => CommandKind::ReplaceBrowser(string(&packet.body)?),
        WIRE_ATTACH => CommandKind::Subscribe,
        WIRE_RELEASE => CommandKind::Release,
        WIRE_CAPABILITIES => CommandKind::Capabilities(integer(&packet.body)?),
        WIRE_SERVICE_INFO => CommandKind::ServiceInfo,
        WIRE_STOP => CommandKind::Stop,
        WIRE_HANDIN | WIRE_HANDOFF => CommandKind::Transfer {
            outbound: packet.kind == WIRE_HANDOFF,
            token: string(&packet.body)?,
        },
        _ => return Ok(None),
    };
    Ok(Some(Command { tab, kind }))
}

#[expect(
    clippy::too_many_lines,
    reason = "exhaustive legacy wire binding table"
)]
pub fn encode(command: &Command) -> io::Result<Packet> {
    command.validate()?;
    let mut body = Buffer::default();
    let kind = match &command.kind {
        CommandKind::Call(request) => return v2::encode_request(request),
        CommandKind::Input {
            bytes,
            control: Control::Preserve,
        } => {
            return v2::encode_request(&Request::Input {
                tab: command.tab,
                bytes: bytes.clone(),
            })
        }
        CommandKind::Close { receipt: true } => {
            return v2::encode_request(&Request::Close { tab: command.tab })
        }
        CommandKind::Open { launch, mode } => {
            if *mode == OpenMode::Detached {
                body.put_u32(WORP_OPEN_DETACHED)?;
                body.put_bytes(&launch.encode()?)?;
                return Ok(Packet {
                    kind: WIRE_WORP,
                    tab: 0,
                    body: body.into_bytes(),
                });
            }
            body.put_bytes(&launch.encode()?)?;
            if *mode == OpenMode::ReuseRestored {
                WIRE_HELLO
            } else {
                WIRE_NEW
            }
        }
        CommandKind::Input { bytes, .. } => {
            body.put_bytes(bytes)?;
            WIRE_INPUT
        }
        CommandKind::Focus { columns, rows } | CommandKind::Resize { columns, rows } => {
            body.put_u32(*columns)?;
            body.put_u32(*rows)?;
            if matches!(command.kind, CommandKind::Focus { .. }) {
                WIRE_FOCUS
            } else {
                WIRE_RESIZE
            }
        }
        CommandKind::Close { .. } => WIRE_CLOSE,
        CommandKind::Scroll { up, amount } => {
            body.put_u32(u32::from(*up))?;
            body.put_u32(*amount)?;
            WIRE_SCROLL
        }
        CommandKind::Control(action) => {
            body.put_u32(match action {
                ControlAction::TogglePrinter => 0,
                ControlAction::PrintScreen => 1,
                ControlAction::Break => 2,
            })?;
            WIRE_CONTROL
        }
        CommandKind::FocusState(focused) => {
            body.put_u32(u32::from(*focused))?;
            WIRE_FOCUS_STATE
        }
        CommandKind::Selection {
            generation,
            operation,
        } => {
            body.put_u32(*generation)?;
            body.put_u32(match operation {
                SelectionOperation::Start => 0,
                SelectionOperation::Next => 1,
                SelectionOperation::Cancel => 2,
            })?;
            WIRE_SELECT_ALL
        }
        CommandKind::Browser {
            url,
            subscribe: true,
        } => {
            body.put_string(url)?;
            WIRE_BROWSER
        }
        CommandKind::Browser {
            url,
            subscribe: false,
        } => {
            return v2::encode_request(&Request::OpenBrowser {
                url: String::from_utf8(url.clone()).map_err(|_| invalid())?,
            })
        }
        CommandKind::Navigate(url) => {
            body.put_string(url)?;
            WIRE_BROWSER_NAVIGATE
        }
        CommandKind::ReplaceBrowser(url) => {
            body.put_string(url)?;
            WIRE_BROWSER_REPLACE
        }
        CommandKind::Subscribe => WIRE_ATTACH,
        CommandKind::Release => WIRE_RELEASE,
        CommandKind::Capabilities(features) => {
            body.put_u32(*features)?;
            WIRE_CAPABILITIES
        }
        CommandKind::ServiceInfo => WIRE_SERVICE_INFO,
        CommandKind::Stop => WIRE_STOP,
        CommandKind::Transfer { outbound, token } => {
            body.put_string(token)?;
            if *outbound {
                WIRE_HANDOFF
            } else {
                WIRE_HANDIN
            }
        }
    };
    Ok(Packet {
        kind,
        tab: command.tab,
        body: body.into_bytes(),
    })
}
