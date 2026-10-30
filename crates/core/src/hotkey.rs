/*
 * Author: Jamius Siam
 * Since: 31/10/2026
 */

//! Hotkey strings such as `"Alt+Shift+R"` ↔ Win32 `RegisterHotKey` modifiers and virtual-key codes.

/// `MOD_*` values from `RegisterHotKey`.
pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    /// Combination of `MOD_*` flags. At least one modifier is required, and Shift alone only for
    /// F1–F24, PrintScreen and Pause (Shift+A would hijack typing).
    pub modifiers: u32,
    /// Win32 virtual-key code.
    pub vk: u32,
}

/// Named keys besides letters, digits and F1–F24.
const NAMED_KEYS: [(&str, u32); 14] = [
    ("Space", 0x20),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("PrintScreen", 0x2C),
    ("Pause", 0x13),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("Backquote", 0xC0),
];

impl Hotkey {
    /// Parses `"Ctrl+Shift+R"`-style strings (case-insensitive, `Control`/`Win`/`Windows` accepted).
    /// Returns `None` if the modifiers are not enough (see [`Hotkey::modifiers`]) or the key is unknown.
    pub fn parse(s: &str) -> Option<Hotkey> {
        let mut modifiers = 0;
        let mut vk = None;
        for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers |= MOD_CONTROL,
                "alt" => modifiers |= MOD_ALT,
                "shift" => modifiers |= MOD_SHIFT,
                "win" | "windows" => modifiers |= MOD_WIN,
                _ if vk.is_none() => vk = Some(key_from_name(part)?),
                _ => return None,
            }
        }
        let vk = vk?;
        is_allowed(modifiers, vk).then_some(Hotkey { modifiers, vk })
    }

    /// Returns `None` if `vk` has no name in [`Hotkey::parse`]'s vocabulary.
    pub fn from_parts(modifiers: u32, vk: u32) -> Option<Hotkey> {
        (is_allowed(modifiers, vk) && key_name(vk).is_some()).then_some(Hotkey { modifiers, vk })
    }

    /// Canonical display string, e.g. `"Ctrl+Alt+Shift+Win+R"` order.
    pub fn to_display_string(&self) -> String {
        let mut parts = Vec::new();
        for (flag, name) in [(MOD_CONTROL, "Ctrl"), (MOD_ALT, "Alt"), (MOD_SHIFT, "Shift"), (MOD_WIN, "Win")] {
            if self.modifiers & flag != 0 {
                parts.push(name.to_string());
            }
        }
        parts.push(key_name(self.vk).unwrap_or_else(|| format!("0x{:02X}", self.vk)));
        parts.join("+")
    }
}

/// A modifier is required; Shift is not enough unless the key is not a typing key.
fn is_allowed(modifiers: u32, vk: u32) -> bool {
    let non_typing = matches!(vk, 0x70..=0x87 | 0x2C | 0x13);
    modifiers & (MOD_CONTROL | MOD_ALT | MOD_WIN) != 0 || (modifiers != 0 && non_typing)
}

fn key_from_name(name: &str) -> Option<u32> {
    let upper = name.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    if bytes.len() == 1 && bytes[0].is_ascii_alphanumeric() {
        return Some(bytes[0] as u32);
    }
    if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u32>().ok())
        && (1..=24).contains(&n)
    {
        return Some(0x70 + n - 1);
    }
    NAMED_KEYS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, vk)| *vk)
}

fn key_name(vk: u32) -> Option<String> {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => Some(char::from(vk as u8).to_string()),
        0x70..=0x87 => Some(format!("F{}", vk - 0x70 + 1)),
        _ => NAMED_KEYS.iter().find(|(_, v)| *v == vk).map(|(n, _)| n.to_string()),
    }
}
