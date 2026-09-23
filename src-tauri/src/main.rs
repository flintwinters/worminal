#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod theme;

use base64::{engine::general_purpose::STANDARD, Engine};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize, PtySystem};
use serde::Serialize;
use std::{collections::VecDeque, io::Write, sync::{Arc, Mutex}};
use tauri::{Emitter, Manager};

#[derive(Clone, Serialize)]
struct OutputChunk {
    sequence: u64,
    data: String,
    #[serde(skip)]
    bytes: usize,
}

struct OutputState {
    chunks: VecDeque<OutputChunk>,
    bytes: usize,
    next_sequence: u64,
    attached: bool,
}

struct PtyState {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    output: Arc<Mutex<OutputState>>,
    _child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
}

#[derive(Serialize)]
struct Snapshot {
    chunks: Vec<OutputChunk>,
    next_sequence: u64,
}

fn spawn_terminal(app: tauri::AppHandle) -> Result<PtyState, String> {
    let pair = native_pty_system().openpty(PtySize {
        rows: 24, cols: 80, pixel_width: 0, pixel_height: 0,
    }).map_err(|e| e.to_string())?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }.into()
    });
    let mut command = CommandBuilder::new(shell);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    let child = pair.slave.spawn_command(command).map_err(|e| e.to_string())?;
    drop(pair.slave);
    let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let output = Arc::new(Mutex::new(OutputState {
        chunks: VecDeque::new(), bytes: 0, next_sequence: 0, attached: false,
    }));
    let output_for_reader = output.clone();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buffer = [0; 8192];
        while let Ok(count) = std::io::Read::read(&mut reader, &mut buffer) {
            if count == 0 { break; }
            let (chunk, attached) = {
                let mut state = output_for_reader.lock().unwrap();
                let chunk = OutputChunk {
                    sequence: state.next_sequence,
                    data: STANDARD.encode(&buffer[..count]),
                    bytes: count,
                };
                state.next_sequence += 1;
                state.bytes += count;
                state.chunks.push_back(chunk.clone());
                while state.bytes > 1024 * 1024 {
                    if let Some(old) = state.chunks.pop_front() {
                        state.bytes -= old.bytes;
                    }
                }
                (chunk, state.attached)
            };
            if attached { let _ = app.emit("pty-output", chunk); }
        }
    });
    Ok(PtyState {
        master: Mutex::new(pair.master), writer: Mutex::new(writer),
        output, _child: Mutex::new(child),
    })
}

#[tauri::command]
fn attach(state: tauri::State<'_, PtyState>) -> Snapshot {
    let mut output = state.output.lock().unwrap();
    output.attached = true;
    Snapshot {
        chunks: output.chunks.iter().cloned().collect(),
        next_sequence: output.next_sequence,
    }
}

#[tauri::command]
fn write_pty(state: tauri::State<'_, PtyState>, data: String) -> Result<(), String> {
    state.writer.lock().map_err(|e| e.to_string())?
        .write_all(data.as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
fn resize_pty(state: tauri::State<'_, PtyState>, cols: u16, rows: u16) -> Result<(), String> {
    if cols == 0 || rows == 0 { return Ok(()); }
    state.master.lock().map_err(|e| e.to_string())?
        .resize(PtySize { cols, rows, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn load_theme() -> serde_json::Value {
    theme::load_theme()
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let terminal = spawn_terminal(app.handle().clone())
                .map_err(std::io::Error::other)?;
            app.manage(terminal);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![attach, write_pty, resize_pty, load_theme])
        .run(tauri::generate_context!())
        .expect("failed to run Worminal");
}
