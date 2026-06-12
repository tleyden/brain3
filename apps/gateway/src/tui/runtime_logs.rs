use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

const DEFAULT_MAX_LINES: usize = 2_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeLogsState {
    Loading,
    Ready,
    Empty,
    Unavailable(String),
}

pub struct RuntimeLogs {
    path: PathBuf,
    byte_offset: u64,
    partial_line: String,
    lines: VecDeque<String>,
    max_lines: usize,
    follow: bool,
    scroll_from_bottom: usize,
    state: RuntimeLogsState,
}

impl RuntimeLogs {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            byte_offset: 0,
            partial_line: String::new(),
            lines: VecDeque::new(),
            max_lines: DEFAULT_MAX_LINES,
            follow: true,
            scroll_from_bottom: 0,
            state: RuntimeLogsState::Loading,
        }
    }

    pub fn refresh(&mut self) {
        let metadata = match std::fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) => {
                self.state = RuntimeLogsState::Unavailable(format!(
                    "Unable to read {}: {error}",
                    self.path.display()
                ));
                return;
            }
        };

        if metadata.len() < self.byte_offset {
            self.reset_file_state();
        }

        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) => {
                self.state = RuntimeLogsState::Unavailable(format!(
                    "Unable to open {}: {error}",
                    self.path.display()
                ));
                return;
            }
        };

        if let Err(error) = file.seek(SeekFrom::Start(self.byte_offset)) {
            self.state = RuntimeLogsState::Unavailable(format!(
                "Unable to seek {}: {error}",
                self.path.display()
            ));
            return;
        }

        let mut buffer = Vec::new();
        if let Err(error) = file.read_to_end(&mut buffer) {
            self.state = RuntimeLogsState::Unavailable(format!(
                "Unable to read {}: {error}",
                self.path.display()
            ));
            return;
        }

        self.byte_offset = self.byte_offset.saturating_add(buffer.len() as u64);
        self.ingest_bytes(&buffer);
        self.update_state();
    }

    pub fn lines(&self) -> &VecDeque<String> {
        &self.lines
    }

    pub fn state(&self) -> &RuntimeLogsState {
        &self.state
    }

    pub fn is_following(&self) -> bool {
        self.follow
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(lines);
        self.normalize_scroll();
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(lines);
        self.normalize_scroll();
    }

    pub fn jump_to_end(&mut self) {
        self.scroll_from_bottom = 0;
        self.follow = true;
    }

    pub fn scroll_offset_for_height(&self, height: usize) -> u16 {
        if height == 0 {
            return 0;
        }

        let total_lines = self.lines.len();
        if total_lines <= height {
            return 0;
        }

        let max_hidden_below = total_lines.saturating_sub(height);
        let hidden_below = self.scroll_from_bottom.min(max_hidden_below);
        let offset = total_lines
            .saturating_sub(height)
            .saturating_sub(hidden_below);

        offset.min(u16::MAX as usize) as u16
    }

    fn ingest_bytes(&mut self, buffer: &[u8]) {
        if buffer.is_empty() {
            return;
        }

        let text = String::from_utf8_lossy(buffer);
        let mut combined = String::with_capacity(self.partial_line.len() + text.len());
        combined.push_str(&self.partial_line);
        combined.push_str(&text);
        self.partial_line.clear();

        for chunk in combined.split_inclusive('\n') {
            if let Some(line) = chunk.strip_suffix('\n') {
                self.push_line(line.trim_end_matches('\r').to_string());
            } else {
                self.partial_line.push_str(chunk);
            }
        }
    }

    fn push_line(&mut self, line: String) {
        if self.lines.len() == self.max_lines {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    fn reset_file_state(&mut self) {
        self.byte_offset = 0;
        self.partial_line.clear();
        self.lines.clear();
        self.scroll_from_bottom = 0;
        self.follow = true;
    }

    fn update_state(&mut self) {
        self.normalize_scroll();
        self.state = if self.lines.is_empty() {
            RuntimeLogsState::Empty
        } else {
            RuntimeLogsState::Ready
        };
    }

    fn normalize_scroll(&mut self) {
        self.scroll_from_bottom = self.scroll_from_bottom.min(self.lines.len());
        self.follow = self.scroll_from_bottom == 0;
    }
}
