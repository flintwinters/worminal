//! Presentation is refreshed at the GUI commit boundary, independently of descriptor ownership.
use super::{invalid, MAX_WINDOWS, PRIMARY_DETACHED};
use std::{collections::BTreeSet, io};
use worminal_protocol::{Buffer, Payload};

/// A close received while committing is replayed by the committed owner, never silently discarded.
pub const CLOSE_REQUESTED: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Presentation {
    pub id: u32,
    pub selected: u32,
    pub geometry: [i32; 4],
    pub flags: u32,
    pub zoom: f64,
}
impl Presentation {
    pub(super) fn write(self, body: &mut Buffer) -> io::Result<()> {
        if self.id == 0
            || self.geometry[2] <= 0
            || self.geometry[3] <= 0
            || self.flags & !(7 | PRIMARY_DETACHED | CLOSE_REQUESTED) != 0
            || !self.zoom.is_finite()
            || !(-10.0..=40.0).contains(&self.zoom)
        {
            return Err(invalid());
        }
        body.put_u32(self.id)?;
        body.put_u32(self.selected)?;
        for value in self.geometry {
            body.put_u32(value as u32)?;
        }
        body.put_u32(self.flags)?;
        body.put_u32((self.zoom.to_bits() >> 32) as u32)?;
        body.put_u32(self.zoom.to_bits() as u32)
    }
    pub(super) fn read(body: &mut Payload<'_>, legacy_id: Option<u32>) -> io::Result<Self> {
        Ok(Self {
            id: match legacy_id {
                Some(id) => id,
                None => body.u32()?,
            },
            selected: body.u32()?,
            geometry: [
                body.u32()? as i32,
                body.u32()? as i32,
                body.u32()? as i32,
                body.u32()? as i32,
            ],
            flags: body.u32()?,
            zoom: f64::from_bits((u64::from(body.u32()?) << 32) | u64::from(body.u32()?)),
        })
    }
    pub(super) fn configuration(self) -> Self {
        Self {
            selected: 0,
            geometry: [0, 0, 1, 1],
            flags: self.flags & PRIMARY_DETACHED,
            zoom: 0.0,
            ..self
        }
    }
}
pub fn encode_presentations(views: &[Presentation]) -> io::Result<Vec<u8>> {
    if views.is_empty() || views.len() > MAX_WINDOWS {
        return Err(invalid());
    }
    let mut body = Buffer::default();
    body.put_u32(views.len() as u32)?;
    let mut ids = BTreeSet::new();
    for view in views {
        if !ids.insert(view.id) {
            return Err(invalid());
        }
        view.write(&mut body)?;
    }
    Ok(body.into_bytes())
}
pub fn decode_presentations(bytes: &[u8]) -> io::Result<Vec<Presentation>> {
    let mut body = Payload::new(bytes);
    let count = body.u32()? as usize;
    if count == 0 || count > MAX_WINDOWS {
        return Err(invalid());
    }
    let mut views = Vec::with_capacity(count);
    for _ in 0..count {
        views.push(Presentation::read(&mut body, None)?);
    }
    body.finish()?;
    encode_presentations(&views)?;
    Ok(views)
}
