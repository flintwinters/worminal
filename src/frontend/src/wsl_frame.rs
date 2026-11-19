//! Under WSLg the Windows side draws every window's frame. Windows 11 lets DWM recolor a window's caption and
//! border, so a mapped window briefly carries a unique title, a hidden `powershell.exe` finds that window and
//! recolors it to the terminal background, and the real title returns. Anywhere else this does nothing.
use gtk::prelude::*;
use std::{cell::Cell, ffi::OsStr, rc::Rc, time::Duration};

const DWMWA_BORDER_COLOR: u32 = 34;
const DWMWA_CAPTION_COLOR: u32 = 35;
const DWMWA_TEXT_COLOR: u32 = 36;
const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
const BACKGROUND: usize = 259;
const FOREGROUND: usize = 258;

fn is_wsl(distro: Option<&OsStr>, osrelease: &str) -> bool {
    distro.is_some() || osrelease.to_ascii_lowercase().contains("microsoft")
}
fn detected() -> bool {
    let osrelease = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    is_wsl(std::env::var_os("WSL_DISTRO_NAME").as_deref(), &osrelease)
}
/// DWM takes 0x00BBGGRR; configuration colors are validated `#rrggbb`.
fn colorref(hex: &str) -> Option<u32> {
    let rgb = u32::from_str_radix(hex.strip_prefix('#')?, 16).ok()?;
    Some((rgb & 0xFF) << 16 | (rgb & 0xFF00) | rgb >> 16)
}
fn script(token: &str, caption: u32, text: u32) -> String {
    format!(
        r#"Add-Type -TypeDefinition @'
using System; using System.Text; using System.Runtime.InteropServices;
public static class WorminalFrame {{
  delegate bool Visit(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(Visit f, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("dwmapi.dll")] static extern int DwmSetWindowAttribute(IntPtr h, uint a, ref uint v, int s);
  static void Set(IntPtr h, uint a, uint v) {{ DwmSetWindowAttribute(h, a, ref v, 4); }}
  public static int Style(string token) {{
    int found = 0;
    EnumWindows((h, l) => {{
      var title = new StringBuilder(512); GetWindowText(h, title, 512);
      if (title.ToString().Contains(token)) {{
        Set(h, {DWMWA_CAPTION_COLOR}, {caption}); Set(h, {DWMWA_TEXT_COLOR}, {text}); Set(h, {DWMWA_BORDER_COLOR}, {DWMWA_COLOR_NONE}); found++;
      }}
      return true;
    }}, IntPtr.Zero);
    return found;
  }}
}}
'@
for ($i = 0; $i -lt 12; $i++) {{ if ([WorminalFrame]::Style('{token}') -gt 0) {{ break }}; Start-Sleep -Milliseconds 250 }}"#
    )
}
/// `powershell.exe -EncodedCommand` takes base64 of UTF-16LE, which spares the script all command-line quoting.
fn encoded(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    gtk::glib::base64_encode(&bytes).into()
}

/// Returns the window so construction can end with the call.
pub(crate) fn style(window: gtk::Window) -> gtk::Window {
    let settings = worminal_config::get();
    if !settings.wsl_window_frame || !detected() {
        return window;
    }
    let colors = |slot: usize| settings.colors[slot].as_deref().and_then(colorref);
    let (Some(caption), Some(text)) = (colors(BACKGROUND), colors(FOREGROUND)) else {
        return window;
    };
    let done = Cell::new(false);
    window.connect_map(move |window| {
        // One styling per window: DWM attributes last as long as the Windows window does.
        if done.replace(true) {
            return;
        }
        let token = format!(
            "worminal-frame-{}-{:x}",
            std::process::id(),
            window.as_ptr() as usize
        );
        let saved = window.title().map(String::from).unwrap_or_default();
        window.set_title(&token);
        let helper = Rc::new(crate::startup::powershell(&encoded(&script(
            &token, caption, text,
        ))));
        let weak = window.downgrade();
        gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
            let finished = match &*helper {
                Ok(exit) => !matches!(exit.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
                Err(_) => true,
            };
            if !finished {
                return gtk::glib::ControlFlow::Continue;
            }
            // The application may already have retitled the window; only undo our own marker.
            if let Some(window) = weak
                .upgrade()
                .filter(|w| w.title().as_deref() == Some(&token))
            {
                window.set_title(&saved);
            }
            gtk::glib::ControlFlow::Break
        });
    });
    window
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_is_the_distro_variable_or_a_microsoft_kernel() {
        assert!(is_wsl(Some(OsStr::new("Ubuntu")), "6.6.0-generic"));
        assert!(is_wsl(None, "5.15.153.1-microsoft-standard-WSL2"));
        assert!(!is_wsl(None, "7.3.0-10-generic"));
    }
    #[test]
    fn colors_become_dwm_bgr() {
        assert_eq!(colorref("#181818"), Some(0x0018_1818));
        assert_eq!(colorref("#112233"), Some(0x0033_2211));
        assert_eq!(colorref("0x112233"), None);
    }
    #[test]
    fn the_script_targets_only_the_token_and_encodes_as_utf16le() {
        let text = script("worminal-frame-1-a", 0x0033_2211, 0x00ff_ffff);
        assert!(text.contains("Style('worminal-frame-1-a')"));
        assert!(text.contains("35, 3351057") && text.contains("36, 16777215"));
        let decoded = gtk::glib::base64_decode(&encoded(&text));
        let units: Vec<u16> = decoded
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(String::from_utf16(&units).unwrap(), text);
    }
}
