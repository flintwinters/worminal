use std::{
    ffi::{CStr, OsString},
    io,
    os::unix::ffi::OsStringExt,
};
use worminal_protocol::{
    Payload, WORKSPACE_MAX_ARGS, WORKSPACE_MAX_COLS, WORKSPACE_MAX_ENV, WORKSPACE_MAX_ROWS,
};

#[derive(Debug)]
pub struct Launch {
    pub columns: u32,
    pub rows: u32,
    pub allow_alternate: bool,
    pub title: Vec<u8>,
    pub directory: Vec<u8>,
    pub line: Vec<u8>,
    pub output: Vec<u8>,
    pub window_id: Vec<u8>,
    pub arguments: Vec<OsString>,
    pub environment: Option<Vec<(OsString, OsString)>>,
}

fn c_string(payload: &mut Payload<'_>) -> io::Result<Vec<u8>> {
    // C launch fields are length-delimited C strings. Preserve their historical prefix interpretation.
    Ok(payload
        .string()?
        .split(|byte| *byte == 0)
        .next()
        .unwrap()
        .to_vec())
}

impl Launch {
    pub fn environment_for_restore(&self) -> Vec<(OsString, OsString)> {
        let marker = CStr::from_bytes_with_nul(crate::recovery::RESTORE_ENV_NAME).unwrap();
        let marker = OsString::from_vec(marker.to_bytes().to_vec());
        let mut environment = self
            .environment
            .clone()
            .unwrap_or_else(|| std::env::vars_os().collect());
        environment.retain(|(key, _)| *key != marker);
        environment.push((marker, OsString::from("1")));
        environment
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        let mut payload = Payload::new(bytes);
        let columns = payload.u32()?;
        let rows = payload.u32()?;
        let allow_alternate = payload.u32()? != 0;
        let argument_count = payload.u32()?;
        let environment_count = payload.u32()?;
        if columns == 0
            || columns > WORKSPACE_MAX_COLS
            || rows == 0
            || rows > WORKSPACE_MAX_ROWS
            || argument_count > WORKSPACE_MAX_ARGS
            || environment_count > WORKSPACE_MAX_ENV
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid workspace launch bounds",
            ));
        }
        let title = c_string(&mut payload)?;
        let directory = c_string(&mut payload)?;
        let line = c_string(&mut payload)?;
        let output = c_string(&mut payload)?;
        let window_id = c_string(&mut payload)?;
        let mut arguments = Vec::with_capacity(argument_count as usize);
        for _ in 0..argument_count {
            arguments.push(OsString::from_vec(c_string(&mut payload)?));
        }
        // No supplied environment means inherit the daemon, rather than launch with an empty environment.
        let mut environment = (environment_count != 0).then(Vec::new);
        for _ in 0..environment_count {
            let entry = c_string(&mut payload)?;
            if let Some(separator) = entry.iter().position(|byte| *byte == b'=') {
                if separator != 0 {
                    environment.as_mut().unwrap().push((
                        OsString::from_vec(entry[..separator].to_vec()),
                        OsString::from_vec(entry[separator + 1..].to_vec()),
                    ));
                }
            }
        }
        payload.finish()?;
        Ok(Self {
            columns,
            rows,
            allow_alternate,
            title,
            directory,
            line,
            output,
            window_id,
            arguments,
            environment,
        })
    }
}
