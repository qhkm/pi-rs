use crate::keyboard::kitty::{matches_event, parse_input};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EditorAction {
    CursorUp,
    CursorDown,
    CursorLeft,
    CursorRight,
    CursorWordLeft,
    CursorWordRight,
    CursorLineStart,
    CursorLineEnd,
    PageUp,
    PageDown,
    DeleteCharBackward,
    DeleteCharForward,
    DeleteWordBackward,
    DeleteWordForward,
    DeleteToLineStart,
    DeleteToLineEnd,
    NewLine,
    Submit,
    Tab,
    Copy,
    Yank,
    YankPop,
    Undo,
    SelectUp,
    SelectDown,
    SelectConfirm,
    SelectCancel,
}

pub struct KeybindingsManager {
    bindings: HashMap<EditorAction, Vec<String>>,
}

impl KeybindingsManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: HashMap<EditorAction, Vec<String>>) -> Self {
        let mut manager = Self::default();
        for (action, keys) in config {
            manager.bindings.insert(action, keys);
        }
        manager
    }

    /// Returns true if `data` (raw terminal input) matches any bound key for `action`.
    pub fn matches(&self, data: &str, action: EditorAction) -> bool {
        let keys = match self.bindings.get(&action) {
            Some(k) => k,
            None => return false,
        };

        let events = parse_input(data);
        if events.is_empty() {
            return false;
        }

        let event = &events[0];
        for key_desc in keys {
            if matches_event(event, key_desc) {
                return true;
            }
        }
        false
    }

    /// Returns the list of key descriptors bound to an action.
    pub fn get_keys(&self, action: EditorAction) -> &[String] {
        self.bindings
            .get(&action)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

impl Default for KeybindingsManager {
    fn default() -> Self {
        let mut bindings: HashMap<EditorAction, Vec<String>> = HashMap::new();

        bindings.insert(EditorAction::CursorUp, vec!["up".into()]);
        bindings.insert(EditorAction::CursorDown, vec!["down".into()]);
        bindings.insert(
            EditorAction::CursorLeft,
            vec!["left".into(), "ctrl+b".into()],
        );
        bindings.insert(
            EditorAction::CursorRight,
            vec!["right".into(), "ctrl+f".into()],
        );
        bindings.insert(
            EditorAction::CursorWordLeft,
            vec!["alt+left".into(), "ctrl+left".into(), "alt+b".into()],
        );
        bindings.insert(
            EditorAction::CursorWordRight,
            vec!["alt+right".into(), "ctrl+right".into(), "alt+f".into()],
        );
        bindings.insert(
            EditorAction::CursorLineStart,
            vec!["home".into(), "ctrl+a".into()],
        );
        bindings.insert(
            EditorAction::CursorLineEnd,
            vec!["end".into(), "ctrl+e".into()],
        );
        bindings.insert(EditorAction::DeleteCharBackward, vec!["backspace".into()]);
        bindings.insert(
            EditorAction::DeleteCharForward,
            vec!["delete".into(), "ctrl+d".into()],
        );
        bindings.insert(
            EditorAction::DeleteWordBackward,
            vec!["ctrl+w".into(), "alt+backspace".into()],
        );
        bindings.insert(
            EditorAction::DeleteWordForward,
            vec!["alt+d".into(), "alt+delete".into()],
        );
        bindings.insert(EditorAction::DeleteToLineStart, vec!["ctrl+u".into()]);
        bindings.insert(EditorAction::DeleteToLineEnd, vec!["ctrl+k".into()]);
        bindings.insert(EditorAction::Yank, vec!["ctrl+y".into()]);
        bindings.insert(EditorAction::YankPop, vec!["alt+y".into()]);
        bindings.insert(EditorAction::Undo, vec!["ctrl+z".into()]);
        bindings.insert(EditorAction::Submit, vec!["enter".into()]);
        bindings.insert(EditorAction::NewLine, vec!["shift+enter".into()]);
        bindings.insert(EditorAction::Tab, vec!["tab".into()]);
        bindings.insert(EditorAction::SelectConfirm, vec!["enter".into()]);
        bindings.insert(EditorAction::SelectCancel, vec!["escape".into()]);
        bindings.insert(EditorAction::PageUp, vec!["pageup".into()]);
        bindings.insert(EditorAction::PageDown, vec!["pagedown".into()]);
        bindings.insert(EditorAction::SelectUp, vec!["up".into()]);
        bindings.insert(EditorAction::SelectDown, vec!["down".into()]);
        bindings.insert(EditorAction::Copy, vec!["ctrl+c".into()]);

        KeybindingsManager { bindings }
    }
}
