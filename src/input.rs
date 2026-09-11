//! A one-line text field, shared by the filter and the prompt.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The only place typing happens.
#[derive(Default, Clone)]
pub struct Input {
    pub text: String,
    pub cursor: usize,
}

impl Input {
    pub fn new(text: &str) -> Input {
        Input { text: text.to_string(), cursor: text.chars().count() }
    }

    /// The editing keys every field shares. Returns false for anything it does not
    /// handle, so the caller can decide what the key means.
    pub fn edit(&mut self, k: KeyEvent) -> bool {
        match k.code {
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::Home => self.home(),
            KeyCode::End => self.end(),
            KeyCode::Char(c) if k.modifiers.contains(KeyModifiers::CONTROL) => match c {
                'u' => self.clear_before(),
                'w' => self.delete_word(),
                'a' => self.home(),
                'e' => self.end(),
                _ => return false,
            },
            KeyCode::Char(c) => self.insert(c),
            _ => return false,
        }
        true
    }

    fn byte(&self, at: usize) -> usize {
        self.text.char_indices().nth(at).map(|(i, _)| i).unwrap_or(self.text.len())
    }

    pub fn insert(&mut self, c: char) {
        let at = self.byte(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let at = self.byte(self.cursor - 1);
        self.text.remove(at);
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.cursor < self.text.chars().count() {
            let at = self.byte(self.cursor);
            self.text.remove(at);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.chars().count());
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.chars().count();
    }

    /// ctrl+w: rub out the word behind the cursor.
    pub fn delete_word(&mut self) {
        let mut i = self.cursor;
        let chars: Vec<char> = self.text.chars().collect();
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let from = self.byte(i);
        let to = self.byte(self.cursor);
        self.text.replace_range(from..to, "");
        self.cursor = i;
    }

    /// ctrl+u: clear back to the start.
    pub fn clear_before(&mut self) {
        let to = self.byte(self.cursor);
        self.text.replace_range(..to, "");
        self.cursor = 0;
    }
}
