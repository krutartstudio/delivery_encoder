#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    K2,
    K4,
    K6,
}

impl Resolution {
    pub fn as_str(&self) -> &'static str {
        match self {
            Resolution::K2 => "2K (2048x2048)",
            Resolution::K4 => "4K (4096x4096)",
            Resolution::K6 => "6K (Original)",
        }
    }

    pub fn target_size(&self) -> Option<(u32, u32)> {
        match self {
            Resolution::K2 => Some((2048, 2048)),
            Resolution::K4 => Some((4096, 4096)),
            Resolution::K6 => None,
        }
    }

    // NEW: Get filter flags for scaling
    pub fn filter_flags(&self) -> &'static str {
        "lanczos+full_chroma_inp+full_chroma_int"
    }
}
