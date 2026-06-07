use rdev::Key;
use serde::{Deserialize, Serialize};

// --- KEY MAPPING MACRO ---
macro_rules! define_keys {
    ($name:ident { $($variant:ident => $rdev:ident),* $(,)? }) => {
        #[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
        pub enum $name {
            $($variant),*
        }

        impl $name {
            pub fn to_rdev(&self) -> Key {
                match self {
                    $(Self::$variant => Key::$rdev),*
                }
            }

            pub fn from_rdev(key: Key) -> Option<Self> {
                match key {
                    $(Key::$rdev => Some(Self::$variant),)*
                    _ => None,
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{:?}", self)
            }
        }

        impl std::str::FromStr for $name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $(stringify!($variant) => Ok(Self::$variant),)*
                    _ => Err(format!("Unknown key: {}", s)),
                }
            }
        }
    };
}

define_keys!(ConfigKey {
    KeyA => KeyA, KeyB => KeyB, KeyC => KeyC, KeyD => KeyD, KeyE => KeyE,
    KeyF => KeyF, KeyG => KeyG, KeyH => KeyH, KeyI => KeyI, KeyJ => KeyJ,
    KeyK => KeyK, KeyL => KeyL, KeyM => KeyM, KeyN => KeyN, KeyO => KeyO,
    KeyP => KeyP, KeyQ => KeyQ, KeyR => KeyR, KeyS => KeyS, KeyT => KeyT,
    KeyU => KeyU, KeyV => KeyV, KeyW => KeyW, KeyX => KeyX, KeyY => KeyY,
    KeyZ => KeyZ, Num0 => Num0, Num1 => Num1, Num2 => Num2, Num3 => Num3,
    Num4 => Num4, Num5 => Num5, Num6 => Num6, Num7 => Num7, Num8 => Num8,
    Num9 => Num9, F1 => F1, F2 => F2, F3 => F3, F4 => F4, F5 => F5,
    F6 => F6, F7 => F7, F8 => F8, F9 => F9, F10 => F10, F11 => F11,
    F12 => F12, Space => Space, Return => Return, Escape => Escape,
    Tab => Tab, Backspace => Backspace, Insert => Insert, Delete => Delete,
    Home => Home, End => End, PageUp => PageUp, PageDown => PageDown,
    UpArrow => UpArrow, DownArrow => DownArrow, LeftArrow => LeftArrow,
    RightArrow => RightArrow, Alt => Alt, ControlLeft => ControlLeft,
    ControlRight => ControlRight, ShiftLeft => ShiftLeft, ShiftRight => ShiftRight,
    MetaLeft => MetaLeft, MetaRight => MetaRight, CapsLock => CapsLock,
    NumLock => NumLock, ScrollLock => ScrollLock,
    Comma => Comma, Dot => Dot, Slash => Slash, SemiColon => SemiColon,
    Quote => Quote, LeftBracket => LeftBracket, RightBracket => RightBracket,
    BackSlash => BackSlash, Minus => Minus, Equal => Equal, Backquote => BackQuote,
});

// Convert rdev::Key to ConfigKey then to string
pub fn key_to_string(key: Key) -> String {
    let s = ConfigKey::from_rdev(key)
        .map(|k| k.to_string())
        .unwrap_or_else(|| format!("{:?}", key));
    if s.starts_with("Key") && s.len() == 4 {
        s[3..].to_uppercase()
    } else {
        s
    }
}

// Convert string to ConfigKey then to rdev::Key
pub fn string_to_key(s: &str) -> Option<Key> {
    let normalized = if s.len() == 1 && s.chars().next().map_or(false, |c| c.is_ascii_alphabetic())
    {
        format!("Key{}", s.to_uppercase())
    } else {
        s.to_string()
    };
    normalized.parse::<ConfigKey>().ok().map(|k| k.to_rdev())
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BindKey {
    Keyboard(Key),
    MouseMiddle,
    Mouse4,
    Mouse5,
}

impl BindKey {
    pub fn to_string(&self) -> String {
        match self {
            BindKey::Keyboard(k) => key_to_string(*k),
            BindKey::MouseMiddle => "Mouse Middle".to_string(),
            BindKey::Mouse4 => "Mouse 4".to_string(),
            BindKey::Mouse5 => "Mouse 5".to_string(),
        }
    }

    pub fn from_string(s: &str) -> Option<Self> {
        match s {
            "Mouse Middle" => Some(BindKey::MouseMiddle),
            "Mouse 4" => Some(BindKey::Mouse4),
            "Mouse 5" => Some(BindKey::Mouse5),
            _ => string_to_key(s).map(BindKey::Keyboard),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub enum FeatureId {
    #[serde(alias = "HackingPostMessage")]
    HackingClickMtd,
    #[serde(alias = "HackingPostMessage2")]
    HackingJumpMtd,
    #[serde(alias = "HackingEsc")]
    HackingEscMtd,
    TipsSkip,
    Restart,
    NoFallDamage,
    ShiftToggle,
    AutoClicker,
    #[serde(alias = "GrabNoGun")]
    FastLoadout,
    Bhop,
    HoldItemBug,
    #[serde(alias = "LMBHoldToggle")]
    KeepItemClicker,
    #[serde(alias = "GunAndTool")]
    GangstaGrip,
    QuickExit,
    ToggleAllMacros,
    DoubleClick,
}

impl std::fmt::Display for FeatureId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::str::FromStr for FeatureId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "HackingClickMtd" | "HackingPostMessage" => Ok(Self::HackingClickMtd),
            "HackingJumpMtd" | "HackingPostMessage2" => Ok(Self::HackingJumpMtd),
            "HackingEscMtd" | "HackingEsc" => Ok(Self::HackingEscMtd),
            "TipsSkip" => Ok(Self::TipsSkip),
            "Restart" => Ok(Self::Restart),
            "NoFallDamage" => Ok(Self::NoFallDamage),
            "ShiftToggle" => Ok(Self::ShiftToggle),
            "AutoClicker" => Ok(Self::AutoClicker),
            "FastLoadout" | "GrabNoGun" => Ok(Self::FastLoadout),
            "Bhop" => Ok(Self::Bhop),
            "HoldItemBug" => Ok(Self::HoldItemBug),
            "KeepItemClicker" | "LMBHoldToggle" => Ok(Self::KeepItemClicker),
            "GangstaGrip" | "GunAndTool" => Ok(Self::GangstaGrip),
            "QuickExit" => Ok(Self::QuickExit),
            "ToggleAllMacros" => Ok(Self::ToggleAllMacros),
            "DoubleClick" => Ok(Self::DoubleClick),
            _ => Err(format!("Unknown variant: {}", s)),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub enum DoubleClickButton {
    #[default]
    Left,
    Right,
}

impl std::fmt::Display for DoubleClickButton {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DoubleClickButton::Left => write!(f, "LMB"),
            DoubleClickButton::Right => write!(f, "RMB"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SerializableFeature {
    pub id: FeatureId,
    pub bind_key: Option<String>,
    pub enabled: bool,
}

pub struct Feature {
    pub id: FeatureId,
    pub name: String,
    pub bind_key: Option<BindKey>,
    pub enabled: bool,
    pub selecting: bool,
}
