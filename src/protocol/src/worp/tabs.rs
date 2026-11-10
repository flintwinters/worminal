//! Transport-independent options for explicit terminal creation.
use std::io;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalOptions {
    pub directory: String,
    pub title: String,
    pub columns: u32,
    pub rows: u32,
    pub command: Vec<String>,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            directory: String::new(),
            title: String::new(),
            columns: 80,
            rows: 24,
            command: Vec::new(),
        }
    }
}

impl TerminalOptions {
    pub fn validate(&self) -> io::Result<()> {
        let fields = std::iter::once(&self.directory)
            .chain(std::iter::once(&self.title))
            .chain(&self.command);
        if self.columns == 0
            || self.columns > crate::WORKSPACE_MAX_COLS
            || self.rows == 0
            || self.rows > crate::WORKSPACE_MAX_ROWS
            || self.command.len() > crate::WORKSPACE_MAX_ARGS as usize
            || (!self.directory.is_empty() && !self.directory.starts_with('/'))
            || self.command.first().is_some_and(String::is_empty)
            || fields.clone().any(|value| value.contains('\0'))
            || fields.map(String::len).sum::<usize>() > super::MAX_INPUT_BYTES
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                "Invalid terminal options: use an absolute target directory, bounded size/argv and no NUL bytes."));
        }
        Ok(())
    }
}

pub fn validate_url(url: &str) -> io::Result<()> {
    let scheme = url
        .split_once(':')
        .filter(|(_, target)| !target.is_empty())
        .map(|(scheme, _)| scheme);
    if url.len() > super::MAX_INPUT_BYTES
        || url.chars().any(char::is_control)
        || scheme.is_none_or(|scheme| {
            scheme.is_empty()
                || !scheme.as_bytes()[0].is_ascii_alphabetic()
                || !scheme
                    .bytes()
                    .all(|value| value.is_ascii_alphanumeric() || b"+-.".contains(&value))
        })
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput,
            "Browser target must be an explicit URL (for example https://host/ or file:///absolute/path)."));
    }
    Ok(())
}
