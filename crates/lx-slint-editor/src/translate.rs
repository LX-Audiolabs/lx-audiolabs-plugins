//! baseview key codes → Slint named-key text (from slint-baseview, ISC).

use slint::platform::Key;
use slint::SharedString;

pub fn key_text_from_code(code: keyboard_types::Code) -> Option<SharedString> {
    use keyboard_types::Code;

    let key = match code {
        Code::Enter | Code::NumpadEnter => Key::Return,
        Code::Tab => Key::Tab,
        Code::Space => Key::Space,
        Code::Backspace => Key::Backspace,
        Code::Delete => Key::Delete,
        Code::Escape => Key::Escape,
        Code::Insert => Key::Insert,
        Code::ArrowUp => Key::UpArrow,
        Code::ArrowDown => Key::DownArrow,
        Code::ArrowLeft => Key::LeftArrow,
        Code::ArrowRight => Key::RightArrow,
        Code::Home => Key::Home,
        Code::End => Key::End,
        Code::PageUp => Key::PageUp,
        Code::PageDown => Key::PageDown,
        Code::ShiftLeft => Key::Shift,
        Code::ShiftRight => Key::ShiftR,
        Code::ControlLeft => Key::Control,
        Code::ControlRight => Key::ControlR,
        Code::AltLeft => Key::Alt,
        Code::AltRight => Key::AltGr,
        Code::MetaLeft => Key::Meta,
        Code::MetaRight => Key::MetaR,
        Code::CapsLock => Key::CapsLock,
        Code::F1 => Key::F1,
        Code::F2 => Key::F2,
        Code::F3 => Key::F3,
        Code::F4 => Key::F4,
        Code::F5 => Key::F5,
        Code::F6 => Key::F6,
        Code::F7 => Key::F7,
        Code::F8 => Key::F8,
        Code::F9 => Key::F9,
        Code::F10 => Key::F10,
        Code::F11 => Key::F11,
        Code::F12 => Key::F12,
        _ => return None,
    };
    Some(key.into())
}
