use std::{
    env,
    fs::OpenOptions,
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use crossterm::event::KeyCode;
use tokio::sync::mpsc;

const TUI_INPUT_TRACE_ENV: &str = "CLASH_TUI_TUI_INPUT_TRACE";
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TuiInputEvent {
    Key(KeyCode),
    Paste(String),
}

pub(crate) struct TuiInputReaderGuard {
    stop: Arc<AtomicBool>,
}

impl Drop for TuiInputReaderGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn spawn_tui_input_reader() -> (mpsc::UnboundedReceiver<TuiInputEvent>, TuiInputReaderGuard) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let trace_path = tui_input_trace_path();
    thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut buffer = [0_u8; 1024];
        let mut pending = Vec::new();
        while !thread_stop.load(Ordering::Relaxed) {
            match stdin.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    for input_event in tui_input_events_from_bytes(&mut pending, &buffer[..read]) {
                        trace_tui_input_event_record(trace_path.as_deref(), &input_event);
                        if sender.send(input_event).is_err() {
                            return;
                        }
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    });
    (receiver, TuiInputReaderGuard { stop })
}

pub(crate) fn tui_input_events_from_bytes(pending: &mut Vec<u8>, bytes: &[u8]) -> Vec<TuiInputEvent> {
    pending.extend_from_slice(bytes);
    let mut events = Vec::new();
    loop {
        if pending.is_empty() {
            break;
        }
        if pending.starts_with(BRACKETED_PASTE_START) {
            let content_start = BRACKETED_PASTE_START.len();
            let Some(end_start) =
                find_subslice(&pending[content_start..], BRACKETED_PASTE_END).map(|offset| offset + content_start)
            else {
                break;
            };
            let paste = String::from_utf8_lossy(&pending[content_start..end_start]).into_owned();
            pending.drain(..end_start + BRACKETED_PASTE_END.len());
            events.push(TuiInputEvent::Paste(paste));
            continue;
        }

        if let Some((code, len)) = key_code_from_escape_sequence(pending) {
            pending.drain(..len);
            events.push(TuiInputEvent::Key(code));
            continue;
        }

        match pending[0] {
            0x03 => {
                pending.drain(..1);
                events.push(TuiInputEvent::Key(KeyCode::Esc));
            }
            b'\r' | b'\n' => {
                pending.drain(..1);
                events.push(TuiInputEvent::Key(KeyCode::Enter));
            }
            b'\t' => {
                pending.drain(..1);
                events.push(TuiInputEvent::Key(KeyCode::Tab));
            }
            0x7f | 0x08 => {
                pending.drain(..1);
                events.push(TuiInputEvent::Key(KeyCode::Backspace));
            }
            0x1b => {
                pending.drain(..1);
                events.push(TuiInputEvent::Key(KeyCode::Esc));
            }
            byte if byte.is_ascii_control() => {
                pending.drain(..1);
            }
            _ => {
                let Some((ch, len)) = first_utf8_char(pending) else {
                    break;
                };
                pending.drain(..len);
                events.push(TuiInputEvent::Key(KeyCode::Char(ch)));
            }
        }
    }
    events
}

fn key_code_from_escape_sequence(bytes: &[u8]) -> Option<(KeyCode, usize)> {
    for (sequence, code) in [
        (b"\x1b[A".as_slice(), KeyCode::Up),
        (b"\x1b[B".as_slice(), KeyCode::Down),
        (b"\x1b[C".as_slice(), KeyCode::Right),
        (b"\x1b[D".as_slice(), KeyCode::Left),
        (b"\x1b[H".as_slice(), KeyCode::Home),
        (b"\x1b[F".as_slice(), KeyCode::End),
        (b"\x1b[Z".as_slice(), KeyCode::BackTab),
        (b"\x1b[2~".as_slice(), KeyCode::Insert),
        (b"\x1b[3~".as_slice(), KeyCode::Delete),
        (b"\x1b[5~".as_slice(), KeyCode::PageUp),
        (b"\x1b[6~".as_slice(), KeyCode::PageDown),
        (b"\x1bOH".as_slice(), KeyCode::Home),
        (b"\x1bOF".as_slice(), KeyCode::End),
        (b"\x1bOA".as_slice(), KeyCode::Up),
        (b"\x1bOB".as_slice(), KeyCode::Down),
        (b"\x1bOC".as_slice(), KeyCode::Right),
        (b"\x1bOD".as_slice(), KeyCode::Left),
    ] {
        if bytes.starts_with(sequence) {
            return Some((code, sequence.len()));
        }
    }
    None
}

fn first_utf8_char(bytes: &[u8]) -> Option<(char, usize)> {
    let max_len = bytes.len().min(4);
    for len in 1..=max_len {
        let Ok(text) = std::str::from_utf8(&bytes[..len]) else {
            continue;
        };
        let Some(ch) = text.chars().next() else {
            continue;
        };
        if ch.len_utf8() == len {
            return Some((ch, len));
        }
    }
    if bytes.len() >= 4 || bytes[0].is_ascii() {
        return Some((char::REPLACEMENT_CHARACTER, 1));
    }
    None
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn tui_input_trace_path() -> Option<PathBuf> {
    env::var_os(TUI_INPUT_TRACE_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn trace_tui_input_event_record(path: Option<&Path>, event: &TuiInputEvent) {
    let Some(path) = path else {
        return;
    };
    let line = tui_input_event_trace_line(event);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

pub(crate) fn tui_input_event_trace_line(event: &TuiInputEvent) -> String {
    match event {
        TuiInputEvent::Key(code) => format!("key code={} source=raw", key_code_trace_label(*code)),
        TuiInputEvent::Paste(value) => format!("paste len={}", value.len()),
    }
}

const fn key_code_trace_label(code: KeyCode) -> &'static str {
    match code {
        KeyCode::Backspace => "backspace",
        KeyCode::Enter => "enter",
        KeyCode::Left => "left",
        KeyCode::Right => "right",
        KeyCode::Up => "up",
        KeyCode::Down => "down",
        KeyCode::Home => "home",
        KeyCode::End => "end",
        KeyCode::PageUp => "page-up",
        KeyCode::PageDown => "page-down",
        KeyCode::Tab => "tab",
        KeyCode::BackTab => "back-tab",
        KeyCode::Delete => "delete",
        KeyCode::Insert => "insert",
        KeyCode::F(_) => "function",
        KeyCode::Char(_) => "char",
        KeyCode::Null => "null",
        KeyCode::Esc => "esc",
        KeyCode::CapsLock => "caps-lock",
        KeyCode::ScrollLock => "scroll-lock",
        KeyCode::NumLock => "num-lock",
        KeyCode::PrintScreen => "print-screen",
        KeyCode::Pause => "pause",
        KeyCode::Menu => "menu",
        KeyCode::KeypadBegin => "keypad-begin",
        KeyCode::Media(_) => "media",
        KeyCode::Modifier(_) => "modifier",
    }
}
