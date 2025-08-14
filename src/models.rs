#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    K2,
    K4,
    K6,
    K8,
}

impl Resolution {
    pub fn as_str(&self) -> &'static str {
        match self {
            Resolution::K2 => "2K (2048x2048)",
            Resolution::K4 => "4K (4096x4096)",
            Resolution::K6 => "6K (Original)",
            Resolution::K8 => "8K (8192x8192)",
        }
    }

    pub fn target_size(&self) -> Option<(u32, u32)> {
        match self {
            Resolution::K2 => Some((2048, 2048)),
            Resolution::K4 => Some((4096, 4096)),
            Resolution::K6 => None,
            Resolution::K8 => Some((8192, 8192)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRateOption {
    Original,
    Fps60,
}

impl FrameRateOption {
    pub fn as_str(&self) -> &'static str {
        match self {
            FrameRateOption::Original => "Original",
            FrameRateOption::Fps60 => "60 FPS",
        }
    }
}

