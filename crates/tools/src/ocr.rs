#![allow(clippy::pedantic, missing_debug_implementations)] // stand-in: mirrors the original signatures
//! Stand-in for the original `agent::tools::desktop::ocr`. The original reads text
//! from images with a hosted model or a local Python OCR; neither is ported yet, so
//! this reports "unavailable", which the `read` tool already handles (it then tells
//! the model it can still see the image itself).

#![allow(dead_code)]

#[derive(Debug, Clone)]
pub struct OcrResult {
    pub text: String,
    pub method: OcrMethod,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrMethod {
    HuggingFaceAPI,
    LocalRapidOcr,
    Unavailable,
}

impl OcrMethod {
    pub fn method_str(&self) -> &'static str {
        match self {
            OcrMethod::HuggingFaceAPI => "Baidu Unlimited-OCR via HF API",
            OcrMethod::LocalRapidOcr => "RapidOCR (PP-OCRv6) via local Python",
            OcrMethod::Unavailable => "unavailable",
        }
    }
}

pub async fn ocr_image(_path: &std::path::Path) -> OcrResult {
    OcrResult {
        text: String::new(),
        method: OcrMethod::Unavailable,
        duration_ms: 0,
    }
}
