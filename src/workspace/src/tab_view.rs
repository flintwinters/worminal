//! Shared tab chrome. Adapters map palette indices, text metrics, and pointer coordinates.
use worminal_protocol::{
    cell::{COLOR_DEFAULT_BACKGROUND as BG, COLOR_DEFAULT_FOREGROUND as FG, COLOR_TRUECOLOR},
    cell::{MODE_BACKGROUND_JOB, MODE_BUSY},
    worp::events::{split_directory_field, tab_label, TabRecord},
    ATTENTION_DONE, ATTENTION_HELP,
};

pub const SSH: u32 = 5;
pub const BUSY: u32 = 8;

#[derive(Clone, Default)]
pub struct Tab {
    pub id: u32,
    pub label: String,
    pub browser: bool,
    pub proxy: bool,
    /// Shown by a daemon on another host.
    pub remote: bool,
    /// An SSH client is in the terminal's foreground.
    pub ssh: bool,
    /// A task in the terminal is reporting progress.
    pub busy: bool,
    pub background_job: bool,
    /// A bookmark can be made of this tab: a terminal with a known directory, or a web page.
    pub starrable: bool,
    /// The tab is bookmarked.
    pub starred: bool,
}
#[derive(Clone, Copy)]
pub struct Style {
    pub foreground: u32,
    pub background: Option<u32>,
}

pub fn terminal_label(directory: &[u8], ssh_user: Option<&[u8]>) -> String {
    String::from_utf8_lossy(&tab_label(directory, ssh_user))
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}

impl Tab {
    pub fn terminal(record: &TabRecord) -> Self {
        let (directory, ssh_user) = split_directory_field(&record.directory);
        Self {
            id: record.id,
            label: terminal_label(directory, ssh_user),
            remote: !record.host.is_empty(),
            ssh: ssh_user.is_some(),
            busy: record.mode & MODE_BUSY != 0,
            background_job: record.mode & MODE_BACKGROUND_JOB != 0,
            ..Self::default()
        }
    }

    pub fn tinted(&self, severity: u32) -> bool {
        self.style(false, severity).background.is_some()
    }

    pub fn style(&self, selected: bool, severity: u32) -> Style {
        let proxy = self.proxy.then_some(COLOR_TRUECOLOR | 0xfe8019);
        if selected {
            return Style {
                background: Some(FG),
                foreground: proxy.unwrap_or(if self.ssh { SSH } else { BG }),
            };
        }
        if severity > 0 {
            return Style {
                background: Some(severity_color(severity)),
                foreground: BG,
            };
        }
        let background = if self.background_job {
            Some(6)
        } else {
            self.busy.then_some(BUSY)
        };
        let foreground = proxy.unwrap_or(if self.background_job {
            BG
        } else if self.ssh {
            SSH
        } else if self.browser {
            1
        } else if self.remote {
            2
        } else {
            7
        });
        Style {
            foreground,
            background,
        }
    }
}

/// Palette color of an alert, green when finished, yellow when help is needed, or red on failure.
pub fn severity_color(severity: u32) -> u32 {
    match severity {
        ATTENTION_DONE => 2,
        ATTENTION_HELP => 3,
        _ => 1,
    }
}

extern "C" {
    fn wcwidth(character: libc::wchar_t) -> libc::c_int;
}

fn cell_width(character: char) -> usize {
    // This also supports GTK processes with a different LC_CTYPE from their SSH shell.
    let width = unsafe { wcwidth(character as libc::wchar_t) };
    usize::try_from(width).unwrap_or(character.len_utf8())
}

pub const STAR_CELLS: usize = 2;

impl Tab {
    pub fn caption(&self) -> &str {
        if self.label.is_empty() {
            "Shell"
        } else {
            &self.label
        }
    }

    pub fn star(&self) -> Option<char> {
        self.starrable
            .then_some(if self.starred { '★' } else { '☆' })
    }

    pub fn label_column(&self) -> usize {
        1 + usize::from(self.starrable) * STAR_CELLS
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub index: usize,
    pub row: usize,
    pub column: usize,
    pub width: usize,
    pub label_bytes: usize,
}

pub struct Strip {
    pub tabs: Vec<Placement>,
    pub rows: usize,
}

impl Strip {
    pub fn hit(&self, column: usize, row: usize, entries: &[Tab]) -> Option<(u32, bool)> {
        let place = self.tabs.iter().find(|place| {
            row == place.row && (place.column..place.column + place.width).contains(&column)
        })?;
        let tab = &entries[place.index];
        Some((
            tab.id,
            tab.starrable && column - place.column < tab.label_column(),
        ))
    }
}

/// Wrap in catalog order and show the selected row when the display can only show part of the strip.
pub fn layout(tabs: &[Tab], active: u32, columns: usize, max_rows: usize) -> Strip {
    let columns = columns.max(1);
    let max_rows = max_rows.max(1);
    let (mut row, mut column) = (0, 0);
    let mut placements = Vec::with_capacity(tabs.len());
    for (index, tab) in tabs.iter().enumerate() {
        let mut width = tab.label_column();
        let mut label_bytes = 0;
        for character in tab.caption().chars() {
            let next = width + cell_width(character);
            if next >= columns {
                break;
            }
            width = next;
            label_bytes += character.len_utf8();
        }
        let width = (width + 1).min(columns);
        if column != 0 && column + width > columns {
            row += 1;
            column = 0;
        }
        placements.push(Placement {
            index,
            row,
            column,
            width,
            label_bytes,
        });
        column += width;
    }
    let rows = (row + 1).min(max_rows);
    let selected_row = placements
        .iter()
        .find(|place| tabs[place.index].id == active)
        .map_or(0, |place| place.row);
    let first_row = selected_row.saturating_add(1).saturating_sub(rows);
    placements.retain_mut(|place| {
        if (first_row..first_row + rows).contains(&place.row) {
            place.row -= first_row;
            true
        } else {
            false
        }
    });
    Strip {
        tabs: placements,
        rows,
    }
}
