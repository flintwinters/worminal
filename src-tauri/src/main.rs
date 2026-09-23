#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod theme;

use base64::{engine::general_purpose::STANDARD, Engine};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use std::{collections::VecDeque, io::Write, sync::{mpsc, Arc, Mutex}};
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
    commands: mpsc::Sender<PtyCommand>,
    output: Arc<Mutex<OutputState>>,
}

enum PtyCommand {
    Write(String),
    Resize { cols: u16, rows: u16 },
}

#[derive(Serialize)]
struct Snapshot {
    chunks: Vec<OutputChunk>,
    next_sequence: u64,
}

fn publish_output(app: &tauri::AppHandle, output: &Mutex<OutputState>, bytes: &[u8]) {
    let (chunk, attached) = {
        let mut state = output.lock().unwrap();
        let chunk = OutputChunk {
            sequence: state.next_sequence,
            data: STANDARD.encode(bytes),
            bytes: bytes.len(),
        };
        state.next_sequence += 1;
        state.bytes += bytes.len();
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

fn run_terminal(app: tauri::AppHandle, output: Arc<Mutex<OutputState>>, commands: mpsc::Receiver<PtyCommand>) -> Result<(), String> {
    let pair = native_pty_system().openpty(PtySize {
        rows: 24, cols: 80, pixel_width: 0, pixel_height: 0,
    }).map_err(|e| e.to_string())?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }.into()
    });
    let mut command = CommandBuilder::new(shell);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    let _child = pair.slave.spawn_command(command).map_err(|e| e.to_string())?;
    drop(pair.slave);
    let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let output_for_reader = output.clone();
    let app_for_reader = app.clone();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buffer = [0; 8192];
        while let Ok(count) = std::io::Read::read(&mut reader, &mut buffer) {
            if count == 0 { break; }
            publish_output(&app_for_reader, &output_for_reader, &buffer[..count]);
        }
    });

    // The channel retains keystrokes and resize requests sent while the shell starts.
    // One worker owns the PTY so those requests reach it in their original order.
    for command in commands {
        match command {
            PtyCommand::Write(data) => writer.write_all(data.as_bytes()).map_err(|e| e.to_string())?,
            PtyCommand::Resize { cols, rows } if cols > 0 && rows > 0 => {
                pair.master.resize(PtySize { cols, rows, pixel_width: 0, pixel_height: 0 })
                    .map_err(|e| e.to_string())?;
            }
            PtyCommand::Resize { .. } => {}
        }
    }
    Ok(())
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
    state.commands.send(PtyCommand::Write(data)).map_err(|e| e.to_string())
}

#[tauri::command]
fn resize_pty(state: tauri::State<'_, PtyState>, cols: u16, rows: u16) -> Result<(), String> {
    state.commands.send(PtyCommand::Resize { cols, rows }).map_err(|e| e.to_string())
}

#[tauri::command]
fn load_theme() -> serde_json::Value {
    theme::load_theme()
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let (commands, receiver) = mpsc::channel();
            let output = Arc::new(Mutex::new(OutputState {
                chunks: VecDeque::new(), bytes: 0, next_sequence: 0, attached: false,
            }));
            app.manage(PtyState { commands, output: output.clone() });
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                if let Err(error) = run_terminal(handle.clone(), output.clone(), receiver) {
                    publish_output(&handle, &output, format!("\r\n\x1b[31mWorminal: {error}\x1b[0m\r\n").as_bytes());
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![attach, write_pty, resize_pty, load_theme])
        .run(tauri::generate_context!())
        .expect("failed to run Worminal");
}
